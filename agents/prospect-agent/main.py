import json,os,click,uvicorn,httpx
from openai import AsyncOpenAI
from a2a.helpers import new_task_from_user_message,new_text_artifact_update_event,new_text_status_update_event
from a2a.server.agent_execution import AgentExecutor,RequestContext
from a2a.server.events import EventQueue
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes,create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities,AgentCard,AgentInterface,AgentSkill,TaskState
from starlette.applications import Starlette
async def search(prompt):
 key=os.getenv("ANAKIN_API_KEY")
 if not key: raise RuntimeError("ANAKIN_API_KEY is required")
 async with httpx.AsyncClient(timeout=60) as c:
  r=await c.post("https://api.anakin.io/v1/search",headers={"X-API-Key":key,"Content-Type":"application/json"},json={"prompt":prompt,"limit":10}); r.raise_for_status(); return r.json()
def obj(t):
 t=t.strip(); fence=chr(96)*3
 if t.startswith(fence): t=t.split("\n",1)[1].rsplit(fence,1)[0].strip()
 return json.loads(t)
class Executor(AgentExecutor):
 def __init__(self,llm,model): self.llm,self.model=llm,model
 async def execute(self,context,q):
  raw=context.get_user_input(); task=context.current_task or new_task_from_user_message(context.message); await q.enqueue_event(task)
  await q.enqueue_event(new_text_status_update_event(task_id=task.id,context_id=task.context_id,state=TaskState.TASK_STATE_WORKING,text="Finding high-signal prospects..."))
  try:
   results=(await search(f"""Find 5-10 real companies matching this ICP. Look for observable public signals such as hiring, expansion, funding, launches or technology adoption. Include evidence and source URLs. Never claim purchase intent.
ICP:
{raw[:25000]}""")).get("results",[])
   prompt=f"""Select up to 3 relevant companies using ONLY these results. Return ONLY JSON:
{{"prospects":[{{"company":"","website":"","score":0,"intent":"high|medium","signal":"","reason":"","source":""}}]}}
Score is a heuristic 0-100 for ICP fit + signal strength, not purchase probability.
RESULTS:
{json.dumps(results,ensure_ascii=False)[:30000]}"""
   r=await self.llm.chat.completions.create(model=self.model,messages=[{"role":"system","content":prompt},{"role":"user","content":"Return JSON."}]); out=json.dumps(obj(r.choices[0].message.content),ensure_ascii=False)
  except Exception as e: out=json.dumps({"error":str(e)})
  await q.enqueue_event(new_text_artifact_update_event(task_id=task.id,context_id=task.context_id,name="prospects",text=out)); await q.enqueue_event(new_text_status_update_event(task_id=task.id,context_id=task.context_id,state=TaskState.TASK_STATE_COMPLETED,text=out))
 async def cancel(self,context,q): pass
@click.command()
@click.option("--host",default="0.0.0.0")
@click.option("--port",default=int(os.getenv("PORT","8003")),type=int)
def main(host,port):
 llm=AsyncOpenAI(api_key=os.getenv("OPENAI_API_KEY"),base_url=os.getenv("OPENAI_BASE_URL")); model=os.getenv("MODEL","deepseek-v4-flash")
 card=AgentCard(name="grower-prospect-agent",description="Finds companies using ICP fit and observable public GTM signals.",supported_interfaces=[AgentInterface(protocol_binding="JSONRPC",url="http://"+host+":"+str(port)+"/")],version="1.0.0",default_input_modes=["text/plain"],default_output_modes=["text/plain"],capabilities=AgentCapabilities(streaming=True),skills=[AgentSkill(id="prospecting",name="Prospect Discovery",description="Finds relevant companies from live web research.",tags=["grower","gtm","prospects","anakin"],examples=["Find prospects"])])
 h=DefaultRequestHandler(agent_executor=Executor(llm,model),task_store=InMemoryTaskStore(),agent_card=card); routes=[]; routes.extend(create_agent_card_routes(card)); routes.extend(create_jsonrpc_routes(h,rpc_url="/")); uvicorn.run(Starlette(routes=routes),host=host,port=port)
if __name__=="__main__": main()

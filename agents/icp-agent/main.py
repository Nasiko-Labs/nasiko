import json,os,click,uvicorn
from openai import AsyncOpenAI
from a2a.helpers import new_task_from_user_message,new_text_artifact_update_event,new_text_status_update_event
from a2a.server.agent_execution import AgentExecutor,RequestContext
from a2a.server.events import EventQueue
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.routes import create_agent_card_routes,create_jsonrpc_routes
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import AgentCapabilities,AgentCard,AgentInterface,AgentSkill,TaskState
from starlette.applications import Starlette
def obj(t):
 t=t.strip(); fence=chr(96)*3
 if t.startswith(fence): t=t.split("\n",1)[1].rsplit(fence,1)[0].strip()
 return json.loads(t)
class Executor(AgentExecutor):
 def __init__(self,llm,model): self.llm,self.model=llm,model
 async def execute(self,context,q):
  raw=context.get_user_input(); task=context.current_task or new_task_from_user_message(context.message); await q.enqueue_event(task)
  await q.enqueue_event(new_text_status_update_event(task_id=task.id,context_id=task.context_id,state=TaskState.TASK_STATE_WORKING,text="Sharpening ICP..."))
  try:
   prompt=f"""Return ONLY JSON: {{"ideal_customer":{{"industries":[],"company_size":"","geography":[],"buyer_roles":[],"pain_points":[],"technologies":[]}},"buying_signals":[],"why_now":[]}}
Use only evidence in the research. Signals must be observable events, never claimed purchase intent.
RESEARCH:
{raw[:30000]}"""
   r=await self.llm.chat.completions.create(model=self.model,messages=[{"role":"system","content":prompt},{"role":"user","content":"Return JSON."}]); out=json.dumps(obj(r.choices[0].message.content),ensure_ascii=False)
  except Exception as e: out=json.dumps({"error":str(e)})
  await q.enqueue_event(new_text_artifact_update_event(task_id=task.id,context_id=task.context_id,name="icp",text=out)); await q.enqueue_event(new_text_status_update_event(task_id=task.id,context_id=task.context_id,state=TaskState.TASK_STATE_COMPLETED,text=out))
 async def cancel(self,context,q): pass
@click.command()
@click.option("--host",default="0.0.0.0")
@click.option("--port",default=int(os.getenv("PORT","8002")),type=int)
def main(host,port):
 llm=AsyncOpenAI(api_key=os.getenv("OPENAI_API_KEY"),base_url=os.getenv("OPENAI_BASE_URL")); model=os.getenv("MODEL","deepseek-v4-flash")
 card=AgentCard(name="grower-icp-agent",description="Sharpens Grower ideal customer profile and observable buying signals.",supported_interfaces=[AgentInterface(protocol_binding="JSONRPC",url="http://"+host+":"+str(port)+"/")],version="1.0.0",default_input_modes=["text/plain"],default_output_modes=["text/plain"],capabilities=AgentCapabilities(streaming=True),skills=[AgentSkill(id="icp",name="ICP Strategy",description="Defines industries, size, geography, buyer roles and pains.",tags=["grower","gtm","icp"],examples=["Define the ICP"])])
 h=DefaultRequestHandler(agent_executor=Executor(llm,model),task_store=InMemoryTaskStore(),agent_card=card); routes=[]; routes.extend(create_agent_card_routes(card)); routes.extend(create_jsonrpc_routes(h,rpc_url="/")); uvicorn.run(Starlette(routes=routes),host=host,port=port)
if __name__=="__main__": main()

"""Grower Research Agent."""
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
BASE="https://api.anakin.io/v1"
async def call(path,payload,timeout=90):
    h={"Content-Type":"application/json"}
    if os.getenv("ANAKIN_API_KEY"): h["X-API-Key"]=os.environ["ANAKIN_API_KEY"]
    async with httpx.AsyncClient(timeout=timeout) as c:
        r=await c.post(BASE+path,headers=h,json=payload); r.raise_for_status(); return r.json()
def obj(t):
    t=t.strip()
    fence=chr(96)*3
    if t.startswith(fence): t=t.split("\n",1)[1].rsplit(fence,1)[0].strip()
    return json.loads(t)
class Executor(AgentExecutor):
    def __init__(self,llm,model): self.llm,self.model=llm,model
    async def execute(self,context,q):
        raw=context.get_user_input(); task=context.current_task or new_task_from_user_message(context.message); await q.enqueue_event(task)
        await q.enqueue_event(new_text_status_update_event(task_id=task.id,context_id=task.context_id,state=TaskState.TASK_STATE_WORKING,text="Researching market with Anakin..."))
        try:
            try: inp=json.loads(raw)
            except: inp={"business_description":raw}
            website=inp.get("website",""); product=inp.get("product") or inp.get("business_description",""); geo=inp.get("geography") or inp.get("target_market","")
            sources=[]
            if website:
                page=await call("/url-scraper/scrape",{"url":website,"generateJson":False})
                sources.append({"title":"Company website","url":website,"content":page.get("markdown","")[:12000]})
            search=await call("/search",{"prompt":f"Research this business for B2B GTM. Product: {product}. Geography: {geo}. Website: {website}. Find positioning, customer types, pains, competitors and observable buying signals. Prefer current sources.","limit":5})
            sources+=search.get("results",[])
            prompt=f"""Return ONLY JSON with keys company, product, value_proposition, market, competitors, customer_types, pain_points, buying_signals, sources.
Use only the evidence below. Unknown values must be empty.
EVIDENCE:
{json.dumps(sources,ensure_ascii=False)[:30000]}"""
            r=await self.llm.chat.completions.create(model=self.model,messages=[{"role":"system","content":prompt},{"role":"user","content":"Return JSON."}])
            result=obj(r.choices[0].message.content); result["sources"]=[{"title":s.get("title",""),"url":s.get("url","")} for s in sources if s.get("url")][:8]; out=json.dumps(result,ensure_ascii=False)
        except Exception as e: out=json.dumps({"error":str(e)})
        await q.enqueue_event(new_text_artifact_update_event(task_id=task.id,context_id=task.context_id,name="research",text=out))
        await q.enqueue_event(new_text_status_update_event(task_id=task.id,context_id=task.context_id,state=TaskState.TASK_STATE_COMPLETED,text=out))
    async def cancel(self,context,q): pass
@click.command()
@click.option("--host",default="0.0.0.0")
@click.option("--port",default=int(os.getenv("PORT","8001")),type=int)
def main(host,port):
    llm=AsyncOpenAI(api_key=os.getenv("OPENAI_API_KEY"),base_url=os.getenv("OPENAI_BASE_URL")); model=os.getenv("MODEL","deepseek-v4-flash")
    card=AgentCard(name="grower-research-agent",description="Researches a business, market, competitors, customers and observable buying signals with Anakin.",supported_interfaces=[AgentInterface(protocol_binding="JSONRPC",url="http://"+host+":"+str(port)+"/")],version="1.0.0",default_input_modes=["text/plain"],default_output_modes=["text/plain"],capabilities=AgentCapabilities(streaming=True),skills=[AgentSkill(id="market-research",name="Market Research",description="Grounded GTM research from a website or business idea.",tags=["grower","gtm","research","anakin"],examples=["Research my product"])])
    h=DefaultRequestHandler(agent_executor=Executor(llm,model),task_store=InMemoryTaskStore(),agent_card=card); routes=[]; routes.extend(create_agent_card_routes(card)); routes.extend(create_jsonrpc_routes(h,rpc_url="/")); uvicorn.run(Starlette(routes=routes),host=host,port=port)
if __name__=="__main__": main()

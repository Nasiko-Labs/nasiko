"""Runs on the CPU VM: Nasiko owns agent deployment, routing and invocation."""
import asyncio
import io
import hashlib
import json
import os
from pathlib import Path
import random
import subprocess
import time
import uuid
import zipfile

import httpx
import nasiko_host
from agentkv.prompting import compile_prompt
from agentkv.control import Event, JevEstimator
from agentkv.measurements import delta
from agentkv.policy import check_support, engine_idle, retention_score, should_warm


def package():
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for path in Path("/app/agent-package").iterdir():
            if path.is_file(): archive.writestr(path.name, path.read_bytes())
    return buffer.getvalue()


async def deploy_roles(client, roles, harness="direct"):
    agents = {}
    for role in roles:
        env={"AGENTKV_ROLE":role,"AGENTKV_MODEL":"agentkv-qwen"}
        if role=="coder":
            env["AGENTKV_HARNESS"]=harness
        response = await client.post("/api/agents/upload", data={"name":"agentkv-"+role,
            "version_tag":"0.1.0", "env":json.dumps(env)},
            files={"source":("agent.zip",package(),"application/zip")})
        if response.status_code >= 400:
            raise RuntimeError(f"Agent upload {role} failed: HTTP {response.status_code}: {response.text[:400]}")
        data=response.json()["data"]
        agents[role]=data["agent_id"]
        for _ in range(180):
            status=(await client.get("/api/agents/uploads/"+data["build_id"])).json()
            if status.get("status")=="completed": break
            if status.get("status")=="failed":
                logs=subprocess.run(['docker','logs','--tail','80','server'],capture_output=True,text=True)
                detail=logs.stdout+logs.stderr
                for secret in nasiko_host.REDACTIONS:
                    detail=detail.replace(secret,'[redacted]')
                print(detail[-5000:],flush=True)
                raise RuntimeError(f"Agent build failed: {status.get('error_details')}")
            await asyncio.sleep(2)
        else: raise RuntimeError("Agent deployment deadline exceeded")
        print(json.dumps({"deployed_agent":role,"agent_id":agents[role],"harness":env.get("AGENTKV_HARNESS","direct")}),flush=True)
    return agents


async def run():
    start = time.time()
    nasiko_host.main()
    settings = dict(line.split("=",1) for line in Path("/tmp/server.env").read_text().splitlines())
    workflow=os.environ.get("AGENTKV_WORKFLOW","coding")
    warming_enabled=os.environ.get("AGENTKV_WARMING","1")=="1"
    harness=os.environ.get("AGENTKV_HARNESS","pi" if workflow=="coding" else "direct")
    if workflow not in {"coding","support"}:
        raise RuntimeError("AGENTKV_WORKFLOW must be coding or support")
    if harness not in {"pi","direct"}:
        raise RuntimeError("AGENTKV_HARNESS must be pi or direct")
    task_file="/app/benchmarks/tasks.json" if workflow=="coding" else "/app/benchmarks/support.json"
    calibration_file="/app/benchmarks/calibration.json" if workflow=="coding" else "/app/benchmarks/support_calibration.json"
    roles=["coder","tester","reviewer"] if workflow=="coding" else ["triage","drafter"]
    root_agent="agentkv-coder" if workflow=="coding" else "agentkv-triage"
    def coder_body(task, method, request_id, source=None, extra=None):
        messages=compile_prompt(task, method)
        if extra: messages=list(messages)+list(extra)
        return {"messages":messages,"source":task.get("source","") if source is None else source,"request_id":request_id}
    async with httpx.AsyncClient(base_url="http://127.0.0.1:8080", timeout=420) as client:
        login = await client.post("/api/auth/login", json={"username":"agentkv","password":settings["ADMIN_PASSWORD"]})
        login.raise_for_status()
        client.headers["Authorization"] = "Bearer " + login.json()["token"]
        routing=await client.post('/api/llm-configs',json={
            'name':'AgentKV pinned Qwen','provider':'openai','model':'agentkv-qwen',
            'pinned':True,'pinned_model':'agentkv-qwen','temperature':0,'is_default':True})
        routing.raise_for_status()
        agents = await deploy_roles(client, roles, harness if workflow=="coding" else "direct")
        print(json.dumps({"control_plane":"nasiko","coding_harness":harness if workflow=="coding" else "direct","policy":"agentkv"}),flush=True)
        provider = httpx.AsyncClient(base_url=os.environ["AGENTKV_PROVIDER_URL"], timeout=900, follow_redirects=True,
            headers={"Authorization":"Bearer "+os.environ["AGENTKV_PROVIDER_KEY"]})
        async def invoke(role,payload,flow):
            rpc={"jsonrpc":"2.0","id":str(uuid.uuid4()),"method":"message/send","params":{"message":{
                "kind":"message","role":"user","messageId":str(uuid.uuid4()),
                "contextId":flow+"-"+role,
                "parts":[{"kind":"data","data":payload}]}}}
            headers={"traceparent":"00-"+flow+"-"+uuid.uuid4().hex[:16]+"-01"}
            response=await client.post("/api/agents/"+agents[role],json=rpc,headers=headers)
            response.raise_for_status()
            data=response.json()
            if "error" in data: raise RuntimeError("Agent RPC error: "+str(data["error"]))
            result=next(part["data"] for part in data["result"]["parts"] if part.get("kind")=="data")
            if role!='tester' and result.get('request_id'):
                observed=await provider.get('/request-usage/'+result['request_id'])
                if observed.status_code==200:
                    snapshot=observed.json()['usage']
                    if result.get('harness')=='pi' and result.get('usage'):
                        result['last_request_usage']=snapshot
                        result['usage_source']='pi_capture_sum'
                    else:
                        result['nasiko_usage']=result.get('usage')
                        result['usage']=snapshot
                        result['usage_source']='vllm_response'
            return result
        if workflow=="coding":
            preflight=await invoke('tester',{'code':'def add(a,b): return a+b','tests':'assert add(2,3)==5'},uuid.uuid4().hex)
            if not preflight.get('passed'): raise RuntimeError('Nasiko tester preflight failed')
        print('Nasiko authenticated agent invocation passed',flush=True)
        try:
            ready = await provider.get("/health")
            ready.raise_for_status()
            if not ready.json()["ready"]: raise RuntimeError("GPU unavailable")
            gpu_started_at = ready.json().get("gpu_started_at")
            print("GPU ready; beginning measured workflows",flush=True)
            tasks=json.loads(Path(task_file).read_text())[:int(os.environ["AGENTKV_TASK_LIMIT"])]
            methods=os.environ["AGENTKV_METHODS"].split(",")
            concurrency=int(os.environ["AGENTKV_CONCURRENCY"])
            repeats=int(os.environ["AGENTKV_REPEATS"])
            estimator=JevEstimator(os.environ["TYPESAFE_API_KEY"])
            records=[]
            trials=[]
            warmup_start=time.perf_counter()
            warmup_task=max(tasks,key=lambda t:len(t['context']))
            warmup_role="coder" if workflow=="coding" else "triage"
            warmup=await invoke(warmup_role,{'messages':compile_prompt(warmup_task,'B',warmup_role),'max_tokens':1,
                'request_id':'warmup-'+uuid.uuid4().hex},uuid.uuid4().hex)
            inventory=(await provider.get('/engine-state')).json()
            resident=[g for g in inventory.get('groups',[]) if warmup['request_id'] in g['request_id']]
            adapter_gate={'disabled':inventory.get('disabled'), 'resident_groups':len(resident)}
            if any(m in methods for m in ('C','D','E')):
                if inventory.get('disabled') or not resident:
                    raise RuntimeError('Retention gate: no complete resident hybrid prefix')
                request_id=resident[-1]['request_id']
                await provider.post('/cache-hints',json={'enabled':True,'hints':[
                    {'request_id':request_id,'score':1,'lease_seconds':15}]})
                repeated=await invoke(warmup_role,{'messages':compile_prompt(warmup_task,'B',warmup_role),'max_tokens':1,
                    'request_id':'warmup-'+uuid.uuid4().hex},uuid.uuid4().hex)
                inventory=(await provider.get('/engine-state')).json()
                adapter_gate.update(acknowledged=any(a['request_id']==request_id and a['status']=='prioritized'
                    for a in inventory.get('action_history',[])),
                    cached_tokens=(repeated.get('usage',{}).get('prompt_tokens_details') or {}).get('cached_tokens'),
                    identical_first_token=repeated['content']==warmup['content'])
                await provider.post('/cache-hints',json={'enabled':False,'hints':[]})
                if not adapter_gate['acknowledged'] or not adapter_gate['cached_tokens'] or not adapter_gate['identical_first_token']:
                    raise RuntimeError('Retention gate: action, reuse or output check failed: '+str(adapter_gate))
            print(json.dumps({'adapter_gate':adapter_gate}),flush=True)
            warmup_seconds=time.perf_counter()-warmup_start
            await asyncio.sleep(6)
            calibration=[]
            calibration_metrics=(await provider.get('/engine-metrics')).text
            calibration_prompt_tokens=0
            for task in json.loads(Path(calibration_file).read_text()):
                flow=uuid.uuid4().hex
                if workflow=="coding":
                    patch=await invoke("coder",coder_body(task,"B","calibration-"+uuid.uuid4().hex),flow)
                    calibration_prompt_tokens+=(patch.get("usage") or {}).get("prompt_tokens",0)
                    tested=await invoke("tester",{"code":patch["code"],"tests":task["tests"]},flow)
                    repair_needed=not tested["passed"]
                    if repair_needed:
                        calibration_repair=await invoke("coder",coder_body(task,"B","calibration-"+uuid.uuid4().hex,
                            source=patch.get("code") or task.get("source",""), extra=[
                            {"role":"assistant","content":patch["content"]},
                            {"role":"user","content":"Repair the function: "+str(tested)}]),flow)
                        calibration_prompt_tokens+=(calibration_repair.get("usage") or {}).get("prompt_tokens",0)
                    calibration.append({"task_id":task["id"],"repository":task["repository"],"repair":repair_needed})
                else:
                    labelled=await invoke("triage",{"messages":compile_prompt(task,"B","triage"),"request_id":"calibration-"+uuid.uuid4().hex},flow)
                    calibration_prompt_tokens+=(labelled.get("usage") or {}).get("prompt_tokens",0)
                    calibration.append({"task_id":task["id"],"repository":task["repository"],"repair":False})
            await asyncio.sleep(6)
            calibration_after=(await provider.get('/engine-metrics')).text
            prefill_seconds=delta(calibration_metrics,calibration_after,'vllm:request_prefill_time_seconds_sum')
            prefill_ms_per_token=1000*prefill_seconds/calibration_prompt_tokens if prefill_seconds and calibration_prompt_tokens else None
            repair_frequency=(1+sum(r["repair"] for r in calibration))/(2+len(calibration))
            print(json.dumps({"calibration_tasks":len(calibration),"repair_frequency":repair_frequency,"workflow":workflow}),flush=True)

            async def warm_exact(messages):
                body={"model":"agentkv-qwen","messages":messages,"temperature":0,"max_tokens":1,"seed":7,
                      "chat_template_kwargs":{"enable_thinking":False},"request_id":"warm-"+uuid.uuid4().hex}
                started=time.perf_counter()
                response=await provider.post("/v1/chat/completions",json=body)
                payload=response.json() if response.status_code==200 else {}
                usage=(payload.get("usage") or {})
                return {"status":"warmed" if response.status_code==200 else "http_"+str(response.status_code),
                        "seconds":time.perf_counter()-started,"usage":usage,
                        "cached_tokens":(usage.get("prompt_tokens_details") or {}).get("cached_tokens")}

            async def retain(record, request_id, repair_probability):
                state=(await provider.get('/engine-state')).json()
                matches=[g for g in state.get('groups',[]) if request_id and request_id in g['request_id']]
                if not matches or not prefill_ms_per_token or not state.get('bytes_per_block'):
                    record['policy_status']='no_measured_resident_candidate'
                    return None
                group=matches[-1]
                bytes_used=group['blocks']*state['bytes_per_block']
                prompt_tokens=group.get('reusable_tokens',0)
                avoided_ms=prefill_ms_per_token*prompt_tokens
                if not avoided_ms: return None
                hint={'request_id':group['request_id'],'score':retention_score(repair_probability,avoided_ms,bytes_used),'lease_seconds':15}
                record.update(hints=[hint],policy_status='proposed',
                    policy_cost={'estimated_prefill_ms':avoided_ms,'resident_bytes':bytes_used,
                                 'basis':'calibration_linear_prefill_estimate'})
                return hint

            for repetition in range(repeats):
                order=methods.copy(); random.Random(37+repetition).shuffle(order)
                for method in order:
                    await provider.post("/cache-hints",json={"enabled":False,"hints":[]})
                    reset=await provider.post("/reset-cache"); reset.raise_for_status()
                    before_metrics=(await provider.get("/engine-metrics")).text
                    trial_start=time.perf_counter()
                    semaphore=asyncio.Semaphore(concurrency)
                    hint_lock=asyncio.Lock()
                    active_hints={}
                    async def publish(flow, hint=None):
                        async with hint_lock:
                            if hint is None: active_hints.pop(flow,None)
                            else: active_hints[flow]=(time.time()+hint['lease_seconds'],hint)
                            now=time.time()
                            live=[{**h,'lease_seconds':expiry-now} for expiry,h in active_hints.values() if expiry>now]
                            selected=sorted(live,key=lambda h:h['score'],reverse=True)[:3]
                            response=await provider.post('/cache-hints',json={'enabled':bool(selected),'hints':selected})
                            response.raise_for_status()
                    async def coding_workflow(task):
                        offered = time.perf_counter()
                        async with semaphore:
                            flow=uuid.uuid4().hex
                            started=time.perf_counter()
                            record={"task_id":task["id"],"repository":task["repository"],"method":method,
                                "repetition":repetition,"concurrency":concurrency,"flow_id":flow,"events":[],"success":False,
                                "jev":None,"hints":[],"engine_acknowledgements":[],"warm":None,"workflow":"coding","harness":harness}
                            policy_task=None
                            try:
                                response=await client.post("/api/flows",json={"flow_id":flow,"title":task["id"],"root_agent_name":root_agent,
                                    "metadata":{"agentkv_method":method,"task_id":task["id"]}})
                                response.raise_for_status()
                                messages=compile_prompt(task,method)
                                record["events"].append({"agent":"coder","status":"started","time":time.time()})
                                async def plan_flow():
                                    probability=.5 if method!='D' else repair_frequency
                                    reuse=1.0
                                    if method=='E':
                                        event=Event(event_id=flow,flow_id=flow,revision=1,kind='started',agent='coder',
                                            evidence=('Workflow: coder -> tester -> coder on failure, otherwise reviewer. '
                                                +task['goal']+'\nSource excerpt:\n'+task.get('source',''))[:2000])
                                        jev_start=time.perf_counter()
                                        try:
                                            probs,usage=await asyncio.wait_for(estimator.plan(event),timeout=1.5)
                                            probability,reuse=probs['repair'],probs['reuse_prefix']
                                            record['jev']={'probability':probability,'reuse_prefix':reuse,'usage':usage,
                                                           'seconds':time.perf_counter()-jev_start}
                                        except Exception:
                                            record['jev']={'status':'fallback','seconds':time.perf_counter()-jev_start,'usage':'unknown'}
                                            probability,reuse=repair_frequency,1.0
                                    record['repair_probability']=probability
                                    record['reuse_prefix_probability']=reuse
                                    record['decision_after_test']=False
                                    return probability, reuse
                                if method in {'C','D','E'}:
                                    policy_task=asyncio.create_task(plan_flow())
                                patch=await invoke("coder",coder_body(task,method,"agentkv-"+uuid.uuid4().hex),flow)
                                record["events"].append({"agent":"coder","status":"completed","time":time.time(),"usage":patch.get("usage"),"request_id":patch.get("request_id")})
                                if patch.get("harness"):
                                    record["coder_harness"]={"returncode":patch.get("returncode"),
                                        "stderr":(patch.get("stderr") or "")[-800:],
                                        "usage_source":patch.get("usage_source")}
                                repair_probability, reuse_probability = .5 if method!='D' else repair_frequency, 1.0
                                if policy_task:
                                    planned=await asyncio.gather(policy_task,return_exceptions=True)
                                    policy_task=None
                                    if planned and not isinstance(planned[0], Exception) and planned[0]:
                                        repair_probability, reuse_probability = planned[0]
                                if method in {'C','D','E'}:
                                    hint=await retain(record, patch.get("request_id"), repair_probability)
                                    if hint: await publish(flow, hint)
                                record["events"].append({"agent":"tester","status":"started","time":time.time()})
                                tester_task=asyncio.create_task(invoke("tester",{"code":patch["code"],"tests":task["tests"]},flow))
                                if should_warm(method=method, warming_enabled=warming_enabled,
                                               idle=engine_idle((await provider.get('/engine-metrics')).text),
                                               repair_probability=repair_probability,
                                               reuse_prefix_probability=reuse_probability):
                                    review_messages=compile_prompt(task,method,"reviewer",patch["code"])
                                    record["warm"]=await warm_exact(review_messages)
                                else:
                                    record["warm"]={"status":"skipped"}
                                tested=await tester_task
                                record["events"].append({"agent":"tester","status":"completed","time":time.time(),"passed":tested["passed"]})
                                if not tested["passed"]:
                                    record["events"].append({"agent":"coder","status":"started","time":time.time(),"repair":True})
                                    patch=await invoke("coder",coder_body(task,method,"agentkv-"+uuid.uuid4().hex,
                                        source=patch.get("code") or task.get("source",""), extra=[
                                        {"role":"assistant","content":patch["content"]},
                                        {"role":"user","content":"Repair the function. Tests failed:\n"+str(tested.get("output",tested.get("error"))) }]),flow)
                                    record["events"].append({"agent":"coder","status":"completed","time":time.time(),"repair":True,"usage":patch.get("usage")})
                                    tested=await invoke("tester",{"code":patch["code"],"tests":task["tests"]},flow)
                                    record["events"].append({"agent":"tester","status":"completed","time":time.time(),"passed":tested["passed"],"repair":True})
                                    record["repair_attempted"]=True
                                record["events"].append({"agent":"reviewer","status":"started","time":time.time()})
                                review=await invoke("reviewer",{"messages":compile_prompt(task,method,"reviewer",patch["code"]),
                                    "request_id":"agentkv-"+uuid.uuid4().hex},flow)
                                record["events"].append({"agent":"reviewer","status":"completed","time":time.time(),"usage":review.get("usage")})
                                record.update(success=tested["passed"],test_result=tested,review=review["content"],code=patch["code"])
                                state=(await provider.get("/engine-state")).json()
                                record["engine_acknowledgements"]=[a for a in state.get("action_history",state.get("acknowledgements",[])) if a["request_id"] in {h["request_id"] for h in record["hints"]}]
                                completion=await client.post("/api/flows/"+flow+"/complete",json={"status":"completed" if record["success"] else "failed"})
                                completion.raise_for_status()
                            except Exception as exc:
                                record["error"]=type(exc).__name__+": "+str(exc)[:250]
                            finally:
                                if policy_task:
                                    if not policy_task.done():
                                        policy_task.cancel()
                                        record['policy_status']='cancelled_at_completion'
                                        if method=='E' and record['jev'] is None:
                                            record['jev']={'status':'cancelled','usage':'unknown'}
                                    await asyncio.gather(policy_task,return_exceptions=True)
                                    await publish(flow)
                                elif method in {'C','D','E'}:
                                    await publish(flow)
                            record["service_seconds"]=time.perf_counter()-started
                            record["queue_seconds"]=started-offered
                            record["seconds"]=time.perf_counter()-offered
                            records.append(record)
                            print(json.dumps({"task":task["id"],"method":method,"success":record["success"],"seconds":record["seconds"],"error":record.get("error")}),flush=True)

                    async def support_workflow(task):
                        offered = time.perf_counter()
                        async with semaphore:
                            flow=uuid.uuid4().hex
                            started=time.perf_counter()
                            record={"task_id":task["id"],"repository":task["repository"],"method":method,
                                "repetition":repetition,"concurrency":concurrency,"flow_id":flow,"events":[],"success":False,
                                "jev":None,"hints":[],"engine_acknowledgements":[],"warm":None,"workflow":"support"}
                            try:
                                response=await client.post("/api/flows",json={"flow_id":flow,"title":task["id"],"root_agent_name":root_agent,
                                    "metadata":{"agentkv_method":method,"task_id":task["id"]}})
                                response.raise_for_status()
                                record["events"].append({"agent":"triage","status":"started","time":time.time()})
                                labelled=await invoke("triage",{"messages":compile_prompt(task,method,"triage"),
                                    "request_id":"agentkv-"+uuid.uuid4().hex},flow)
                                record["events"].append({"agent":"triage","status":"completed","time":time.time(),
                                    "usage":labelled.get("usage"),"request_id":labelled.get("request_id")})
                                label=(labelled.get("content") or "").strip()
                                if method in {'C','D','E'}:
                                    hint=await retain(record, labelled.get("request_id"), 0.0)
                                    if hint: await publish(flow, hint)
                                draft_messages=compile_prompt(task,method,"drafter",label)
                                if should_warm(method=method, warming_enabled=warming_enabled,
                                               idle=engine_idle((await provider.get('/engine-metrics')).text),
                                               repair_probability=0.0, reuse_prefix_probability=1.0):
                                    record["warm"]=await warm_exact(draft_messages)
                                else:
                                    record["warm"]={"status":"skipped"}
                                record["events"].append({"agent":"drafter","status":"started","time":time.time()})
                                drafted=await invoke("drafter",{"messages":draft_messages,
                                    "request_id":"agentkv-"+uuid.uuid4().hex},flow)
                                record["events"].append({"agent":"drafter","status":"completed","time":time.time(),
                                    "usage":drafted.get("usage")})
                                checked=check_support(task,label,drafted.get("content") or "")
                                record["events"].append({"agent":"checker","status":"completed","time":time.time(),"passed":checked["passed"]})
                                record.update(success=checked["passed"],test_result=checked,review=label,
                                              code=drafted.get("content") or "")
                                state=(await provider.get("/engine-state")).json()
                                record["engine_acknowledgements"]=[a for a in state.get("action_history",state.get("acknowledgements",[])) if a["request_id"] in {h["request_id"] for h in record["hints"]}]
                                completion=await client.post("/api/flows/"+flow+"/complete",json={"status":"completed" if record["success"] else "failed"})
                                completion.raise_for_status()
                            except Exception as exc:
                                record["error"]=type(exc).__name__+": "+str(exc)[:250]
                            finally:
                                if method in {'C','D','E'}:
                                    await publish(flow)
                            record["service_seconds"]=time.perf_counter()-started
                            record["queue_seconds"]=started-offered
                            record["seconds"]=time.perf_counter()-offered
                            records.append(record)
                            print(json.dumps({"task":task["id"],"method":method,"success":record["success"],"seconds":record["seconds"],"error":record.get("error")}),flush=True)

                    runner=coding_workflow if workflow=="coding" else support_workflow
                    await asyncio.gather(*(runner(task) for task in tasks))
                    trial_seconds=time.perf_counter()-trial_start
                    await asyncio.sleep(6)
                    trace_checks=[]
                    for sample in [r for r in records if r['method']==method and r['repetition']==repetition][-2:]:
                        trace=await client.get('/api/observability/trace/'+sample['flow_id'])
                        detail=trace.json().get('data',{}).get('trace',{}) if trace.status_code==200 else {}
                        trace_checks.append({'flow_id':sample['flow_id'],'http_status':trace.status_code,'spans':detail.get('num_spans',0)})
                    trials.append({"method":method,"repetition":repetition,"concurrency":concurrency,
                        "seconds":trial_seconds,"nasiko_traces":trace_checks,"metrics_before":before_metrics,
                        "metrics_after":(await provider.get("/engine-metrics")).text,
                        "engine_state":(await provider.get("/engine-state")).json()})
                    Path("/tmp/agentkv-report.json").write_text(json.dumps({"records":records,"trials":trials,"agents":agents,
                        "nasiko_environment":json.loads(Path("/tmp/nasiko-evidence.json").read_text()),
                        "calibration":calibration,"repair_frequency":repair_frequency,"adapter_gate":adapter_gate,
                        "prefill_ms_per_token":prefill_ms_per_token,"warmup_seconds":warmup_seconds,
                        "manifest":{"tasks_sha256":hashlib.sha256(Path(task_file).read_bytes()).hexdigest(),
                            "task_ids":[t['id'] for t in tasks],"methods":methods,"repeats":repeats,"concurrency":concurrency,
                            "workflow":workflow,"warming":warming_enabled,"harness":harness,
                            "model":"Qwen/Qwen3.8-27B","model_revision":"1d4bf0f2ff6012fd82039f2fa52739d0dd7c60c0",
                            "kv_cache_bytes_override":int(os.getenv("AGENTKV_CACHE_BYTES","0")),
                            "max_model_len":4096 if int(os.getenv("AGENTKV_CACHE_BYTES","0")) else 8192,
                            "vllm":"0.29.0","dtype":"bfloat16","temperature":0,"seed":7,"max_tokens":512,
                            "policy":"async-eviction-priority-v1","max_candidates":3,"lease_seconds":15,"quota_fraction":.25,
                            "jev_plan":"repair_and_reuse_prefix"},
                        "total_guest_seconds":time.time()-start,"gpu_started_at":gpu_started_at,
                        "completed_at":time.time(),"nasiko_commit":nasiko_host.NASIKO_COMMIT},indent=2))
            probes=[]
            probe_role="coder" if workflow=="coding" else "triage"
            for repetition in range(repeats):
                probe_methods=['A','B']; random.Random(73+repetition).shuffle(probe_methods)
                for method in probe_methods:
                    await provider.post('/cache-hints',json={'enabled':False,'hints':[]})
                    reset=await provider.post('/reset-cache'); reset.raise_for_status()
                    for task in tasks:
                        messages=compile_prompt(task,method,probe_role)
                        start_probe=time.perf_counter()
                        response=await invoke(probe_role,{'messages':messages,'max_tokens':1,
                            'request_id':'probe-'+uuid.uuid4().hex},uuid.uuid4().hex)
                        probes.append({'method':method,'repetition':repetition,'task_id':task['id'],
                            'seconds':time.perf_counter()-start_probe,'usage':response.get('usage'),
                            'messages_sha256':hashlib.sha256(json.dumps(messages,sort_keys=True).encode()).hexdigest()})
            report=json.loads(Path('/tmp/agentkv-report.json').read_text())
            report.update(prefill_probe=probes,completed_at=time.time(),total_guest_seconds=time.time()-start)
            Path('/tmp/agentkv-report.json').write_text(json.dumps(report,indent=2))
            print(json.dumps({'one_token_probe_requests':len(probes)}),flush=True)
        finally:
            await provider.aclose()


if __name__=="__main__": asyncio.run(run())

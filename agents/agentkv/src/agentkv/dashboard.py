"""Read-only dashboard for recorded, sanitized benchmark evidence."""
import json
import math
from pathlib import Path
from agentkv.measurements import usage_totals
from fastapi import FastAPI
from fastapi.responses import FileResponse


def percentile(values, p):
    ordered=sorted(values)
    return ordered[max(0, math.ceil(len(ordered)*p)-1)] if ordered else None


def summarize(report):
    if 'cohorts' in report:
        cohorts=[{'label':item['label'],'report':summarize(item['report'])} for item in report['cohorts']]
        return {**(cohorts[0]['report'] if cohorts else summarize({})), 'cohorts':cohorts}
    records=report.get("records",[])
    rows=[]
    for trial in report.get("trials",[]):
        selected=[r for r in records if r["method"]==trial["method"] and r["repetition"]==trial["repetition"] and r.get("concurrency",trial["concurrency"])==trial["concurrency"]]
        successes=sum(r["success"] for r in selected)
        latencies=[r["seconds"] for r in selected]
        rows.append({"method":trial["method"],"repetition":trial["repetition"],"concurrency":trial["concurrency"],
            "tasks":len(selected),"successes":successes,"failed":len(selected)-successes,
            "p50_seconds":percentile(latencies,.5),"p95_seconds":percentile(latencies,.95),
            "successes_per_hour":successes*3600/trial["seconds"] if trial["seconds"] else None,
            "cache_usage":usage_totals(selected),
            "acknowledged_workflows":trial.get("acknowledged_workflows"),
            "cost_per_thousand":trial.get("cost_per_thousand"),"cost_basis":trial.get("cost_basis","unavailable")})
    safe_records=[{k:r.get(k) for k in ("task_id","repository","method","repetition","concurrency","flow_id","events","success","seconds","jev","hints","engine_acknowledgements","repair_attempted","repair_probability","policy_status","policy_cost","decision_after_test","error")} for r in records]
    return {"status":"recorded" if records else "awaiting_benchmark","model":"Qwen3.8-27B", "gpu":"A100 80 GB",
            "rows":rows,"records":safe_records,"notes":report.get("notes",[]),"comparisons":report.get("comparisons",[]),
            "cache_budget_bytes":report.get('manifest',{}).get('kv_cache_bytes_override',0),"nasiko_url":None}


def create_dashboard(report_path: str, refresh=None):
    app=FastAPI(docs_url=None,redoc_url=None)
    @app.get("/")
    def index():
        return FileResponse(Path(__file__).parent/"static"/"index.html")
    @app.get("/api/report")
    def data():
        if refresh is not None:
            refresh()
        path=Path(report_path)
        return summarize(json.loads(path.read_text())) if path.exists() else summarize({})
    return app

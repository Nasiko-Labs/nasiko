"""CPU-only recorded evidence dashboard; no GPU or control endpoint is deployed."""
import json
from pathlib import Path
import modal

app=modal.App("agentkv-dashboard")
evidence=modal.Volume.from_name("agentkv-results",create_if_missing=True)
image=(modal.Image.debian_slim(python_version="3.12").pip_install("fastapi==0.141.1")
       .add_local_python_source("agentkv"))


@app.function(image=image,volumes={"/evidence":evidence},cpu=.25,memory=256,
              timeout=60,max_containers=1,scaledown_window=60)
@modal.asgi_app()
def web():
    from agentkv.dashboard import create_dashboard
    return create_dashboard("/evidence/integrated-latest.json",evidence.reload)


@app.function(image=image,volumes={"/evidence":evidence},cpu=.25,memory=256,
              timeout=60,retries=0,max_containers=1)
def save(report: dict):
    Path("/evidence/integrated-latest.json").write_text(json.dumps(report,indent=2))
    evidence.commit()


@app.local_entrypoint()
def publish(path: str="results/integrated-latest.json"):
    save.remote(json.loads(Path(path).read_text()))
    print("Recorded evidence saved to Modal Volume")

"""One real bounded Jev decision through AgentKV on Modal; no public service."""
import modal

app = modal.App("agentkv-decision-check")
image = (modal.Image.debian_slim(python_version="3.12")
         .pip_install("typesafe-sdk==0.7.0", "pydantic==2.13.5")
         .add_local_python_source("agentkv"))


@app.function(image=image, secrets=[modal.Secret.from_name("agentkv-jev")],
              cpu=.25, memory=256, timeout=30, retries=0, max_containers=1)
async def check():
    import os
    from agentkv.control import Candidate, Controller, Event, JevEstimator, Store
    store = Store()
    try:
        store.ingest("smoke", Event(event_id="one", flow_id="check", revision=1,
            kind="started", agent="coder", evidence="The patch is ready. Run the unit tests next.",
            candidates=[Candidate(agent="tester", prefix_id="a" * 64,
                                  incremental_bytes=1024, avoided_recompute_ms=100)]))
        decision = await Controller(store, JevEstimator(os.environ["TYPESAFE_API_KEY"])).decide("smoke", "check")
        return {"ok": decision["estimator"] == "jev", "estimator": decision["estimator"],
                "usage": decision["usage"], "engine_applied": decision["engine_applied"]}
    finally:
        store.close()


@app.local_entrypoint()
def main():
    result = check.remote()
    print(result)
    if not result["ok"]:
        raise SystemExit(1)

"""Local or private-network controller entrypoint."""
import json
import os
import uvicorn
from agentkv.api import create_app
from agentkv.control import JevEstimator, Store
from agentkv.nasiko import NasikoBridge


def main():
    tokens = json.loads(os.environ["AGENTKV_AUTH_TOKENS"])
    estimator = JevEstimator(os.environ["TYPESAFE_API_KEY"]) if os.getenv("TYPESAFE_API_KEY") else None
    store = Store(os.getenv("AGENTKV_DATABASE", "agentkv.sqlite3"))
    nasiko = None
    if os.getenv("AGENTKV_NASIKO_URL"):
        nasiko = NasikoBridge(store, owner=os.environ["AGENTKV_NASIKO_OWNER"],
            base_url=os.environ["AGENTKV_NASIKO_URL"], token=os.environ["AGENTKV_NASIKO_TOKEN"])
    app = create_app(tokens, store, estimator, nasiko)
    uvicorn.run(app, host=os.getenv("AGENTKV_HOST", "127.0.0.1"), port=8081,
                workers=1, access_log=False, limit_concurrency=32)


if __name__ == "__main__":
    main()

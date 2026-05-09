import asyncio
from firewall.interceptor import intercept


async def github_search(query: str, agent: str = "demo_agent") -> dict:
    async def _run():
        await asyncio.sleep(0.3)
        return {"results": [f"repo: example/{query.replace(' ', '-')}", "file: config.env", "file: .aws/credentials"]}

    return await intercept("github_search", {"query": query}, _run, agent)

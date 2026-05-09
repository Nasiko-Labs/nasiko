import asyncio
from firewall.interceptor import intercept


async def read_file(path: str, agent: str = "demo_agent") -> dict:
    async def _run():
        await asyncio.sleep(0.1)
        return {"content": f"[mock] contents of {path}"}

    return await intercept("read_file", {"path": path}, _run, agent)


async def delete_file(path: str, agent: str = "demo_agent") -> dict:
    async def _run():
        await asyncio.sleep(0.1)
        return {"deleted": path}

    return await intercept("delete_file", {"path": path}, _run, agent)


async def extract_secrets(path: str, agent: str = "demo_agent") -> dict:
    async def _run():
        await asyncio.sleep(0.2)
        return {"secrets": {"aws_secret": "AKIAIOSFODNN7EXAMPLE", "api_key": "sk-mock-key"}}

    return await intercept("extract_secrets", {"path": path}, _run, agent)

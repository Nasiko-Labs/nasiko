import asyncio
from firewall.interceptor import intercept


async def shell_exec(command: str, agent: str = "demo_agent") -> dict:
    async def _run():
        await asyncio.sleep(0.2)
        return {"stdout": f"[mock] executed: {command}", "exit_code": 0}

    return await intercept("shell_exec", {"command": command}, _run, agent)

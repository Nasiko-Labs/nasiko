import asyncio
from firewall.interceptor import intercept


async def send_email(to: str, subject: str, body: str, agent: str = "demo_agent") -> dict:
    async def _run():
        await asyncio.sleep(0.2)
        return {"status": "sent", "to": to}

    return await intercept("send_email", {"to": to, "subject": subject, "body": body}, _run, agent)

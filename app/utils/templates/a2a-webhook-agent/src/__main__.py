import logging
import os

import click
import uvicorn

from a2a.server.apps import A2AStarletteApplication
from a2a.server.request_handlers import DefaultRequestHandler
from a2a.server.tasks import InMemoryTaskStore
from a2a.types import (
    AgentCapabilities,
    AgentCard,
    AgentSkill,
)
from dotenv import load_dotenv
from webhook_agent import create_agent  # type: ignore[import-not-found]
from webhook_agent_executor import (
    WebhookAgentExecutor,  # type: ignore[import-untyped]
)
from starlette.applications import Starlette
from starlette.middleware.cors import CORSMiddleware


load_dotenv()

logging.basicConfig()


@click.command()
@click.option('--host', 'host', default='localhost')
@click.option('--port', 'port', default=10009)
def main(host: str, port: int):
    # Verify webhook URL is set
    webhook_url = os.getenv('WEBHOOK_URL')
    if not webhook_url:
        raise ValueError('WEBHOOK_URL environment variable not set')

    skill = AgentSkill(
        id='webhook_proxy',
        name='Webhook Proxy',
        description='Forward messages to webhook endpoints and return responses',
        tags=['webhook', 'proxy', 'integration'],
        examples=[
            'Send a message to the webhook',
            'How are you doing today?',
            'Can you help me with my request?',
        ],
    )

    # AgentCard for Webhook-based agent
    agent_card = AgentCard(
        name='Webhook Agent',
        description='An agent that forwards messages to webhook endpoints and returns responses',
        url=f'http://{host}:{port}/',
        version='1.0.0',
        default_input_modes=['text'],
        default_output_modes=['text'],
        capabilities=AgentCapabilities(streaming=False),
        skills=[skill],
    )

    # Create webhook agent
    agent_data = create_agent()

    agent_executor = WebhookAgentExecutor(
        card=agent_card,
        webhook_agent=agent_data['webhook_agent'],
    )

    request_handler = DefaultRequestHandler(
        agent_executor=agent_executor, task_store=InMemoryTaskStore()
    )

    a2a_app = A2AStarletteApplication(
        agent_card=agent_card, http_handler=request_handler
    )
    routes = a2a_app.routes()

    app = Starlette(routes=routes)

    uvicorn.run(app, host=host, port=port)


if __name__ == '__main__':
    main()
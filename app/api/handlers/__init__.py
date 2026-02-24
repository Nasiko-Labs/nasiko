"""
Handler Factory for main app
Provides a single point of import for all handlers with proper dependency injection
"""

from .base_handler import BaseHandler
from .chat_history_handler import ChatHistoryHandler
from .agent_upload_handler import AgentUploadHandler
from .agent_operations_handler import AgentOperationsHandler
from .agent_update_handler import AgentUpdateHandler
from .github_handler import GitHubHandler
from .health_handler import HealthHandler
from .n8n_handler import N8nHandler
from .registry_handler import RegistryHandler
from .traces_handler import TracesHandler
from .search_handler import SearchHandler
from .observability_handler import ObservabilityHandler
from .nanda_handler import NANDAHandler

class HandlerFactory:
    """
    Factory class that initializes and manages all handlers
    Provides a single point of access for all handler functionality
    """
    
    def __init__(self, service, logger, auth_states: dict = None):
        self.service = service
        self.logger = logger
        self.auth_states = auth_states or {}
        
        # Initialize all handlers with shared dependencies
        self.registry = RegistryHandler(service, logger)
        self.agent_upload = AgentUploadHandler(service, logger)
        self.agent_operations = AgentOperationsHandler(service, logger)
        self.agent_update = AgentUpdateHandler(service, logger)
        self.github = GitHubHandler(service, logger)
        self.health = HealthHandler()
        self.n8n = N8nHandler(service, logger)
        self.search = SearchHandler(service, logger)
        self.chat_history = ChatHistoryHandler(service, logger)
        self.observability = ObservabilityHandler(service, logger)
        self.nanda = NANDAHandler(service, logger)



__all__ = [
    'HandlerFactory', 'BaseHandler',
    'RegistryHandler', 'AgentUploadHandler', 'AgentOperationsHandler',
    'GitHubHandler', 'HealthHandler',
    'TracesHandler', 'N8nHandler', 'NANDAHandler'
]
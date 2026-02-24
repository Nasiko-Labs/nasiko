"""
Registry Handler - Manages agent registry operations
"""

from fastapi import HTTPException, status, Request

from app.pkg.auth import AuthClient
from .base_handler import BaseHandler
from ..types import (
    RegistryCreateRequest, RegistryResponse, RegistrySingleResponse, RegistryItemResponse,
    VersionStatusUpdateRequest, VersionStatusUpdateResponse,
    RegistryItemDetailResponse, UserAgentsResponse, UserAgentItemResponse,
    SimpleUserAgentsResponse, SimpleUserAgentResponse, RegistryUpsertRequest
)

class RegistryHandler(BaseHandler):
    """Handler for agent registry operations"""

    def __init__(self, service, logger):
        super().__init__(service, logger)
        self._search_handler = None

    @property
    def search_handler(self):
        """Lazy-load search handler to avoid circular imports"""
        if self._search_handler is None:
            from .search_handler import SearchHandler
            self._search_handler = SearchHandler(self.service, self.logger)
        return self._search_handler

    async def _index_agent_in_search(self, registry):
        """Helper method to index an agent in search"""
        try:
            # Format agent data for search indexing
            agent_data = {
                "agent_id": registry.id,
                "name": registry.name,
                "description": registry.description,
                "tags": getattr(registry, 'tags', []) or [],
                "icon_url": getattr(registry, 'iconUrl', None),
                "owner_id": getattr(registry, 'owner_id', None),
                "version": registry.version,
                "url": registry.url,
                "created_at": getattr(registry, 'created_at', None),
                "updated_at": getattr(registry, 'updated_at', None)
            }

            await self.search_handler.index_agent(agent_data)
            self.log_debug(f"Indexed agent in search: {registry.id}")
        except Exception as e:
            # Don't fail the main operation if search indexing fails
            self.log_warning(f"Failed to index agent in search: {e}")

    async def _remove_agent_from_search(self, agent_id: str):
        """Helper method to remove an agent from search"""
        try:
            await self.search_handler.delete_agent_from_search(agent_id)
            self.log_debug(f"Removed agent from search: {agent_id}")
        except Exception as e:
            # Don't fail the main operation if search removal fails
            self.log_warning(f"Failed to remove agent from search: {e}")

    def _transform_registry_to_item_response(self, registry) -> RegistryItemDetailResponse:
        """Transform registry entity to item response format"""
        # New transformation using AgentCard format
        capabilities_dict = registry.capabilities.model_dump() if hasattr(registry.capabilities,
                                                                          'model_dump') else registry.capabilities if isinstance(
            registry.capabilities, dict) else {}
        skills_list = [skill.model_dump() if hasattr(skill, 'model_dump') else skill for skill in
                       registry.skills] if registry.skills else []

        # Handle provider - convert to dict if it's a Pydantic model, exclude if None
        provider_dict = None
        if hasattr(registry, 'provider') and registry.provider is not None:
            provider_dict = registry.provider.model_dump() if hasattr(registry.provider,
                                                                      'model_dump') else registry.provider

        # Handle timestamps
        created_at_str = None
        updated_at_str = None
        if hasattr(registry, 'created_at') and registry.created_at:
            created_at_str = registry.created_at.isoformat() if hasattr(registry.created_at, 'isoformat') else str(
                registry.created_at)
        if hasattr(registry, 'updated_at') and registry.updated_at:
            updated_at_str = registry.updated_at.isoformat() if hasattr(registry.updated_at, 'isoformat') else str(
                registry.updated_at)

        return RegistryItemDetailResponse(
            id=registry.id,  # Agent ID from AgentCard
            name=registry.name,
            version=registry.version,
            description=registry.description,
            url=registry.url,
            preferredTransport=registry.preferredTransport if hasattr(registry, 'preferredTransport') else "JSONRPC",
            protocolVersion=registry.protocolVersion if hasattr(registry, 'protocolVersion') else "0.2.9",
            provider=provider_dict,
            iconUrl=registry.iconUrl if hasattr(registry, 'iconUrl') and registry.iconUrl else None,
            documentationUrl=registry.documentationUrl if hasattr(registry,
                                                                  'documentationUrl') and registry.documentationUrl else None,
            capabilities=capabilities_dict,
            securitySchemes=registry.securitySchemes if hasattr(registry, 'securitySchemes') else {},
            security=registry.security if hasattr(registry, 'security') else [],
            skills=skills_list,
            tags=registry.tags if hasattr(registry, 'tags') and registry.tags else [],  # Combined tags from skills
            defaultInputModes=registry.defaultInputModes if hasattr(registry, 'defaultInputModes') else [],
            defaultOutputModes=registry.defaultOutputModes if hasattr(registry, 'defaultOutputModes') else [],
            supportsAuthenticatedExtendedCard=registry.supportsAuthenticatedExtendedCard if hasattr(registry,
                                                                                                    'supportsAuthenticatedExtendedCard') else False,
            signatures=registry.signatures if hasattr(registry, 'signatures') else [],
            additionalInterfaces=registry.additionalInterfaces if hasattr(registry, 'additionalInterfaces') else None,
            created_at=created_at_str,
            updated_at=updated_at_str
        )

    async def create_registry(self, registry_data: RegistryCreateRequest) -> RegistrySingleResponse:
        """Create a new agent registry entry"""
        try:
            self.log_info("Creating registry", agent_name=registry_data.name)
            registry = await self.service.create_registry(registry_data)
            if registry:
                data = self._transform_registry_to_item_response(registry)

                # Index the new agent in search
                await self._index_agent_in_search(registry)

                self.log_info("Registry created successfully", registry_id=registry.id)
                return RegistrySingleResponse(
                    data=data,
                    status_code=201,
                    message="Registry created successfully"
                )
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Failed to create registry"
            )
        except ValueError as e:
            self.log_error("Registry creation failed - validation error", e)
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=str(e)
            )
        except Exception as e:
            await self.handle_service_error("create_registry", e)

    async def get_all_registries(self) -> RegistryResponse:
        """Get all agent registries"""
        try:
            self.log_debug("Fetching all registries")
            registries = await self.service.get_all_registries()
            registry_items = []

            for registry in registries:
                capabilities_dict = registry.capabilities.model_dump() if hasattr(registry.capabilities,
                                                                                  'model_dump') else registry.capabilities if isinstance(
                    registry.capabilities, dict) else {}
                skills_list = [skill.model_dump() if hasattr(skill, 'model_dump') else skill for skill in
                               registry.skills] if registry.skills else []

                item = RegistryItemResponse(
                    id=registry.id,  # Agent ID from AgentCard
                    db_id=str(registry._id) if hasattr(registry, '_id') else None,
                    name=registry.name,
                    version=registry.version,
                    description=registry.description,
                    url=registry.url,
                    preferredTransport=registry.preferredTransport if hasattr(registry,
                                                                              'preferredTransport') else "JSONRPC",
                    capabilities=capabilities_dict,
                    skills=skills_list,
                    defaultInputModes=registry.defaultInputModes if hasattr(registry, 'defaultInputModes') else [],
                    defaultOutputModes=registry.defaultOutputModes if hasattr(registry, 'defaultOutputModes') else []
                )
                registry_items.append(item)

            self.log_info("Registries retrieved successfully", count=len(registry_items))
            return RegistryResponse(
                data=registry_items,
                status_code=200,
                message="Registries retrieved successfully"
            )
        except Exception as e:
            await self.handle_service_error("get_all_registries", e)

    async def get_registry_by_name(self, agent_name: str) -> RegistrySingleResponse:
        """Get registry by agent name"""
        try:
            self.log_debug("Fetching registry by name", agent_name=agent_name)
            registry = await self.service.get_registry_by_name(agent_name)
            if registry:
                data = self._transform_registry_to_item_response(registry)
                return RegistrySingleResponse(
                    data=data,
                    status_code=200,
                    message="Registry retrieved successfully"
                )
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail=f"Registry with name {agent_name} not found"
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("get_registry_by_name", e)

    async def get_registry_by_agent_id(self, agent_id: str) -> RegistrySingleResponse:
        """Get registry by agent ID"""
        try:
            self.log_debug("Fetching registry by agent ID", agent_id=agent_id)
            registry = await self.service.get_registry_by_agent_id(agent_id)
            if registry:
                data = self._transform_registry_to_item_response(registry)
                return RegistrySingleResponse(
                    data=data,
                    status_code=200,
                    message="Registry retrieved successfully"
                )
            raise HTTPException(
                status_code=status.HTTP_404_NOT_FOUND,
                detail=f"Registry with agent_id {agent_id} not found"
            )
        except HTTPException:
            raise
        except Exception as e:
            await self.handle_service_error("get_registry_by_agent_id", e)

    async def get_user_agents(
        self, user_id: str, request: Request
    ) -> UserAgentsResponse | None:
        """Get all agents available to a user (uploaded + accessible via permissions)"""
        try:
            # Get authorization token from request
            auth_header = request.headers.get("authorization")
            if not auth_header:
                raise HTTPException(
                    status_code=401, detail="Authorization header required"
                )

            # Get agents the user has access to via auth service
            auth_client = AuthClient()
            accessible_agent_ids = await auth_client.get_user_accessible_agents(
                auth_header
            )

            self.log_info(
                "User accessible agents from auth",
                user_id=user_id,
                accessible_agents=accessible_agent_ids,
            )

            user_agents = []
            processed_agent_ids = set()

            # Get detailed information for accessible agents from registry
            for agent_id in accessible_agent_ids:
                if agent_id in processed_agent_ids:
                    continue

                try:
                    registry = await self.service.get_registry_by_agent_id(agent_id)
                    if registry:
                        # Extract description
                        description = None
                        if hasattr(registry, "description"):
                            description = registry.description
                        elif hasattr(registry, "agent") and hasattr(
                            registry.agent, "description"
                        ):
                            description = registry.agent.description
                        elif isinstance(registry, dict):
                            agent_info = registry.get("agent", {})
                            description = agent_info.get("description")

                        # Extract url
                        url = registry.url if hasattr(registry, "url") else None

                        capabilities_dict = {}
                        if hasattr(registry, "capabilities"):
                            if hasattr(registry.capabilities, "model_dump"):
                                capabilities_dict = registry.capabilities.model_dump()
                            elif isinstance(registry.capabilities, dict):
                                capabilities_dict = registry.capabilities

                        skills_list = []
                        if hasattr(registry, "skills") and registry.skills:
                            skills_list = [
                                (
                                    skill.model_dump()
                                    if hasattr(skill, "model_dump")
                                    else skill
                                )
                                for skill in registry.skills
                            ]

                        # Convert provider to dict if it's a Pydantic model
                        provider_dict = None
                        if hasattr(registry, "provider") and registry.provider:
                            if hasattr(registry.provider, "model_dump"):
                                provider_dict = registry.provider.model_dump()
                            elif isinstance(registry.provider, dict):
                                provider_dict = registry.provider

                        user_agent = UserAgentItemResponse(
                            id=registry.id if hasattr(registry, "id") else agent_id,
                            name=(
                                registry.name if hasattr(registry, "name") else agent_id
                            ),
                            version=getattr(registry, "version", "1.0.0"),
                            description=description
                            or "Agent accessible via permissions",
                            url=url or "",
                            protocolVersion=getattr(
                                registry, "protocolVersion", "0.2.9"
                            ),
                            preferredTransport=getattr(
                                registry, "preferredTransport", "JSONRPC"
                            ),
                            provider=provider_dict,
                            iconUrl=getattr(registry, "iconUrl", None),
                            documentationUrl=getattr(
                                registry, "documentationUrl", None
                            ),
                            capabilities=capabilities_dict,
                            securitySchemes=getattr(registry, "securitySchemes", {}),
                            security=getattr(registry, "security", []),
                            defaultInputModes=getattr(
                                registry, "defaultInputModes", []
                            ),
                            defaultOutputModes=getattr(
                                registry, "defaultOutputModes", []
                            ),
                            skills=skills_list,
                            supportsAuthenticatedExtendedCard=getattr(
                                registry, "supportsAuthenticatedExtendedCard", False
                            ),
                            signatures=getattr(registry, "signatures", []),
                            additionalInterfaces=getattr(
                                registry, "additionalInterfaces", None
                            ),
                            created_at=(
                                str(registry.created_at)
                                if hasattr(registry, "created_at")
                                else None
                            ),
                            updated_at=(
                                str(registry.updated_at)
                                if hasattr(registry, "updated_at")
                                else None
                            ),
                        )
                        user_agents.append(user_agent)
                        processed_agent_ids.add(agent_id)

                except Exception as e:
                    self.log_error(
                        f"Could not fetch registry details for agent {agent_id}: {e}"
                    )
                    user_agent = UserAgentItemResponse(
                        id=agent_id,
                        name=agent_id,
                        version="1.0.0",
                        description="Agent accessible via permissions (details unavailable)",
                        url="",
                        protocolVersion="0.2.9",
                        preferredTransport="JSONRPC",
                        provider=None,
                        iconUrl=None,
                        documentationUrl=None,
                        capabilities={},
                        securitySchemes={},
                        security=[],
                        defaultInputModes=[],
                        defaultOutputModes=[],
                        skills=[],
                        supportsAuthenticatedExtendedCard=False,
                        signatures=[],
                        additionalInterfaces=None,
                    )
                    user_agents.append(user_agent)
                    processed_agent_ids.add(agent_id)

            # Sort by name for better UX
            user_agents.sort(key=lambda x: x.name.lower())

            return UserAgentsResponse(
                data=user_agents,
                status_code=200,
                message=f"Retrieved {len(user_agents)} accessible agents for authenticated user",
            )

        except Exception as e:
            await self.handle_service_error("get_user_agents", e)

    async def get_my_agents(self, user_id: str, request: Request) -> SimpleUserAgentsResponse:
        """Get all agents available to the authenticated user from auth service"""
        try:
            self.log_info("Fetching agents for authenticated user", user_id=user_id)

            # Get authorization token from request
            auth_header = request.headers.get("authorization")
            if not auth_header:
                raise HTTPException(status_code=401, detail="Authorization header required")

            # Get agents the user has access to via auth service
            auth_client = AuthClient()
            accessible_agent_ids = await auth_client.get_user_accessible_agents(auth_header)

            self.log_info("User accessible agents from auth", user_id=user_id,
                          accessible_agents=accessible_agent_ids)

            user_agents = []
            processed_agent_ids = set()

            # Get detailed information for accessible agents from registry
            for agent_id in accessible_agent_ids:
                if agent_id in processed_agent_ids:
                    continue

                try:
                    # Try to get registry entry by agent_id (which could be agent name)
                    registry = await self.service.get_registry_by_agent_id(agent_id)
                    if registry:
                        # Get description from registry
                        description = None
                        if hasattr(registry, 'description'):
                            description = registry.description
                        elif hasattr(registry, 'agent') and hasattr(registry.agent, 'description'):
                            description = registry.agent.description
                        elif isinstance(registry, dict):
                            agent_info = registry.get("agent", {})
                            description = agent_info.get("description")

                        # Get icon_url from registry
                        icon_url = None
                        if hasattr(registry, 'iconUrl'):
                            icon_url = registry.iconUrl
                        elif hasattr(registry, 'icon_url'):
                            icon_url = registry.icon_url

                        # Get tags from registry (if available)
                        tags = []
                        if hasattr(registry, 'tags') and registry.tags:
                            tags = registry.tags

                        user_agent = SimpleUserAgentResponse(
                            agent_id=registry.id if hasattr(registry, 'id') else agent_id,
                            name=registry.name if hasattr(registry, 'name') else agent_id,
                            icon_url=icon_url,
                            tags=tags,
                            description=description
                        )
                        user_agents.append(user_agent)
                        processed_agent_ids.add(agent_id)

                except Exception as e:
                    self.log_debug(f"Could not fetch registry details for agent {agent_id}: {e}")
                    # Still include the agent with minimal info
                    user_agent = SimpleUserAgentResponse(
                        agent_id=agent_id,
                        name=agent_id,
                        icon_url=None,
                        tags=[],
                        description="Agent accessible via permissions (details unavailable)"
                    )
                    user_agents.append(user_agent)
                    processed_agent_ids.add(agent_id)

            # Sort by name for better UX
            user_agents.sort(key=lambda x: x.name.lower())

            return SimpleUserAgentsResponse(
                data=user_agents,
                status_code=200,
                message=f"Retrieved {len(user_agents)} accessible agents for authenticated user"
            )

        except Exception as e:
            await self.handle_service_error("get_my_agents", e)

    async def upsert_registry_by_name(self, registry_name: str,
                                      upsert_data: RegistryUpsertRequest) -> RegistrySingleResponse:
        """Upsert registry by name"""
        try:
            self.log_info("Upserting registry", registry_name=registry_name)
            registry = await self.service.upsert_registry_by_name(registry_name, upsert_data)
            if registry:
                data = self._transform_registry_to_item_response(registry)
                # Update the agent in search
                await self._index_agent_in_search(registry)

                self.log_info("Registry upserted successfully", registry_name=registry_name)
                return RegistrySingleResponse(
                    data=data,
                    status_code=200,
                    message="Registry upserted successfully"
                )
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail="Failed to upsert registry"
            )
        except ValueError as e:
            self.log_error("Registry upsert failed - validation error", e)
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail=str(e)
            )
        except Exception as e:
            await self.handle_service_error("upsert_registry_by_name", e)

    async def delete_agent_completely(self, agent_id: str, user_id: str):
        """Delete an agent and all related resources (K8s deployments, permissions, registry, database records)"""
        try:
            self.log_info("Starting complete agent deletion", agent_id=agent_id, user_id=user_id)
            
            result = await self.service.delete_agent_completely(agent_id, user_id)
            
            if result.get("success"):
                # Remove from search index
                await self._remove_agent_from_search(agent_id)
                
                self.log_info("Agent deleted completely", agent_id=agent_id, details=result.get("details"))
                return {
                    "message": f"Agent {agent_id} and all related resources deleted successfully",
                    "deleted_resources": result.get("details", {}),
                    "status_code": 200
                }
            else:
                raise HTTPException(
                    status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                    detail=f"Failed to delete agent: {result.get('error', 'Unknown error')}"
                )
                
        except HTTPException:
            raise
        except Exception as e:
            self.log_error("Agent deletion failed", agent_id=agent_id, error=str(e))
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to delete agent: {str(e)}"
            )

    async def update_agent_version_status(self, agent_name: str, status_update: VersionStatusUpdateRequest) -> VersionStatusUpdateResponse:
        """Update the status of an agent version"""
        try:
            self.log_info(f"Updating agent version status for {agent_name} to {status_update.status}")
            result = await self.service.update_agent_version_status(agent_name, status_update.status)
            
            if result:
                return VersionStatusUpdateResponse(
                    agent_name=agent_name,
                    status=status_update.status,
                    status_code=200,
                    message="Agent version status updated successfully"
                )
            else:
                raise HTTPException(
                    status_code=status.HTTP_404_NOT_FOUND,
                    detail=f"Agent {agent_name} not found"
                )
        except HTTPException:
            raise
        except Exception as e:
            self.log_error(f"Failed to update agent version status: {str(e)}")
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=f"Failed to update agent version status: {str(e)}"
            )

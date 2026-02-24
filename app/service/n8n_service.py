import httpx
import shutil
import json
import base64
from typing import Dict, List, Optional, Any
from datetime import datetime, timezone
from pathlib import Path
from app.entity.n8n_entity import N8nRegisterRequest

class N8nService:
    """N8n API service for interacting with n8n instances"""
    
    def __init__(self, base_url: str, api_key: str, logger):
        self.base_url = base_url.rstrip('/')
        self.api_key = api_key
        self.logger = logger
        self.headers = {
            'X-N8N-API-KEY': api_key,
            'Content-Type': 'application/json',
            'Accept': 'application/json'
        }
    
    async def test_connection(self) -> Dict[str, Any]:
        """Test the n8n connection and return instance info"""
        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                # Try to get instance info (this endpoint varies by n8n version)
                response = await client.get(
                    f"{self.base_url}/api/v1/workflows",
                    headers=self.headers,
                    params={"limit": 1}  # Just get 1 workflow to test connection
                )
                
                if response.status_code == 200:
                    data = response.json()
                    return {
                        "success": True,
                        "message": "Connection successful",
                        "instance_info": {
                            "url": self.base_url,
                            "status": "connected",
                            "workflows_accessible": True,
                            "total_workflows": data.get("count", "unknown") if isinstance(data, dict) else len(data) if isinstance(data, list) else "unknown"
                        }
                    }
                elif response.status_code == 401:
                    return {
                        "success": False,
                        "message": "Invalid API key",
                        "instance_info": None
                    }
                elif response.status_code == 403:
                    return {
                        "success": False,
                        "message": "API access forbidden - check if you have a paid plan for cloud instances",
                        "instance_info": None
                    }
                else:
                    return {
                        "success": False,
                        "message": f"Connection failed with status {response.status_code}",
                        "instance_info": None
                    }
                    
        except httpx.ConnectError:
            return {
                "success": False,
                "message": "Cannot connect to n8n instance - check URL",
                "instance_info": None
            }
        except httpx.TimeoutException:
            return {
                "success": False,
                "message": "Connection timeout - instance may be slow or unreachable",
                "instance_info": None
            }
        except Exception as e:
            self.logger.error(f"N8n connection test failed: {str(e)}")
            return {
                "success": False,
                "message": f"Connection test failed: {str(e)}",
                "instance_info": None
            }
    
    async def get_workflows(self) -> List[Dict[str, Any]]:
        """Fetch all workflows from n8n instance"""
        try:
            async with httpx.AsyncClient(timeout=60.0) as client:
                response = await client.get(
                    f"{self.base_url}/api/v1/workflows",
                    headers=self.headers
                )
                
                if response.status_code == 200:
                    data = response.json()
                    
                    # Handle different response formats
                    if isinstance(data, dict) and "data" in data:
                        workflows = data["data"]
                    elif isinstance(data, list):
                        workflows = data
                    else:
                        workflows = []
                    
                    # Normalize workflow data
                    normalized_workflows = []
                    for workflow in workflows:
                        normalized = self._normalize_workflow_data(workflow)
                        normalized_workflows.append(normalized)
                    
                    return normalized_workflows
                else:
                    self.logger.error(f"Failed to fetch workflows: {response.status_code} - {response.text}")
                    return []
                    
        except Exception as e:
            self.logger.error(f"Error fetching workflows: {str(e)}")
            return []
    
    async def get_workflow_by_id(self, workflow_id: str) -> Optional[Dict[str, Any]]:
        """Fetch a specific workflow by ID"""
        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                response = await client.get(
                    f"{self.base_url}/api/v1/workflows/{workflow_id}",
                    headers=self.headers
                )
                
                if response.status_code == 200:
                    workflow = response.json()
                    return self._normalize_workflow_data(workflow)
                else:
                    self.logger.error(f"Failed to fetch workflow {workflow_id}: {response.status_code}")
                    return None
                    
        except Exception as e:
            self.logger.error(f"Error fetching workflow {workflow_id}: {str(e)}")
            return None

    def extract_webhook_id(self, workflow_data: Dict[str, Any]) -> Optional[str]:
        """Extract webhook ID from Chat Trigger nodes"""
        nodes = workflow_data.get("nodes", [])
        for node in nodes:
            if node.get("type") == "@n8n/n8n-nodes-langchain.chatTrigger":
                return node.get("webhookId")
        return None
    
    def is_chat_workflow(self, workflow_data: Dict[str, Any]) -> bool:
        """Check if workflow is a chat workflow (has Chat Trigger node)"""
        nodes = workflow_data.get("nodes", [])
        for node in nodes:
            if node.get("type") == "@n8n/n8n-nodes-langchain.chatTrigger":
                return True
        return False
    
    async def get_executions(self, workflow_id: Optional[str] = None, limit: int = 20) -> List[Dict[str, Any]]:
        """Fetch workflow executions"""
        try:
            async with httpx.AsyncClient(timeout=60.0) as client:
                params = {"limit": limit}
                if workflow_id:
                    params["workflowId"] = workflow_id
                
                response = await client.get(
                    f"{self.base_url}/api/v1/executions",
                    headers=self.headers,
                    params=params
                )
                
                if response.status_code == 200:
                    data = response.json()
                    
                    # Handle different response formats
                    if isinstance(data, dict) and "data" in data:
                        executions = data["data"]
                    elif isinstance(data, list):
                        executions = data
                    else:
                        executions = []
                    
                    # Normalize execution data
                    normalized_executions = []
                    for execution in executions:
                        normalized = self._normalize_execution_data(execution)
                        normalized_executions.append(normalized)
                    
                    return normalized_executions
                else:
                    self.logger.error(f"Failed to fetch executions: {response.status_code}")
                    return []
                    
        except Exception as e:
            self.logger.error(f"Error fetching executions: {str(e)}")
            return []
    
    async def get_execution_by_id(self, execution_id: str, include_data: bool = True) -> Optional[Dict[str, Any]]:
        """Fetch a specific execution by ID"""
        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                params = {"includeData": "true"} if include_data else {}
                
                response = await client.get(
                    f"{self.base_url}/api/v1/executions/{execution_id}",
                    headers=self.headers,
                    params=params
                )
                
                if response.status_code == 200:
                    execution = response.json()
                    return self._normalize_execution_data(execution)
                else:
                    self.logger.error(f"Failed to fetch execution {execution_id}: {response.status_code}")
                    return None
                    
        except Exception as e:
            self.logger.error(f"Error fetching execution {execution_id}: {str(e)}")
            return None
    
    def _normalize_workflow_data(self, workflow: Dict[str, Any]) -> Dict[str, Any]:
        """Normalize workflow data from n8n API"""
        is_chat = self.is_chat_workflow(workflow)
        webhook_id = self.extract_webhook_id(workflow) if is_chat else None
        
        return {
            "id": str(workflow.get("id", "")),
            "name": workflow.get("name", "Untitled Workflow"),
            "active": workflow.get("active", False),
            "tags": workflow.get("tags", []),
            "nodes": workflow.get("nodes", []),
            "connections": workflow.get("connections", {}),
            "settings": workflow.get("settings", {}),
            "staticData": workflow.get("staticData", {}),
            "createdAt": workflow.get("createdAt"),
            "updatedAt": workflow.get("updatedAt"),
            "versionId": workflow.get("versionId"),
            "meta": workflow.get("meta", {}),
            "nodes_count": len(workflow.get("nodes", [])),
            "is_chat_workflow": is_chat,
            "webhook_id": webhook_id,
            "chat_url": f"{self.base_url}/webhook/{webhook_id}/chat" if webhook_id else None,
            "raw_data": workflow  # Keep original data for platform-specific fields
        }
    
    def _normalize_execution_data(self, execution: Dict[str, Any]) -> Dict[str, Any]:
        """Normalize execution data from n8n API"""
        started_at = execution.get("startedAt")
        finished_at = execution.get("stoppedAt") or execution.get("finishedAt")
        
        # Parse timestamps
        started_timestamp = None
        finished_timestamp = None
        duration_ms = None
        
        try:
            if started_at:
                if isinstance(started_at, str):
                    started_timestamp = datetime.fromisoformat(started_at.replace('Z', '+00:00'))
                else:
                    started_timestamp = datetime.fromtimestamp(started_at / 1000, tz=timezone.utc)
            
            if finished_at:
                if isinstance(finished_at, str):
                    finished_timestamp = datetime.fromisoformat(finished_at.replace('Z', '+00:00'))
                else:
                    finished_timestamp = datetime.fromtimestamp(finished_at / 1000, tz=timezone.utc)
            
            if started_timestamp and finished_timestamp:
                duration_ms = int((finished_timestamp - started_timestamp).total_seconds() * 1000)
                
        except Exception as e:
            self.logger.warning(f"Error parsing execution timestamps: {str(e)}")
        
        # Determine status
        finished = execution.get("finished", False)
        error = execution.get("data", {}).get("resultData", {}).get("error")
        
        if error:
            status = "error"
            success = False
        elif finished:
            status = "success" 
            success = True
        else:
            status = "running"
            success = None
        
        return {
            "id": str(execution.get("id", "")),
            "workflowId": str(execution.get("workflowId", "")),
            "mode": execution.get("mode", "unknown"),
            "status": status,
            "success": success,
            "started_at": started_timestamp,
            "finished_at": finished_timestamp,
            "duration_ms": duration_ms,
            "error": str(error) if error else None,
            "data": execution.get("data", {}),
            "retryOf": execution.get("retryOf"),
            "retrySuccessId": execution.get("retrySuccessId"),
            "raw_data": execution  # Keep original data
        }

    async def register_workflow_as_agent(self, request: N8nRegisterRequest, user_id: str, repo) -> Dict[str, Any]:
        """Register n8n workflow as a2a agent with user ownership and full agent deployment flow"""
        try:
            # Fetch workflow details directly from n8n (since handler provides direct access)
            workflow_data = await self.get_workflow_by_id(request.workflow_id)
            if not workflow_data:
                return {"success": False, "message": f"Workflow {request.workflow_id} not found in n8n instance. Please check the workflow ID."}
            
            # Extract webhook ID from workflow
            webhook_id = workflow_data.get("webhook_id") or request.workflow_id
            
            # Use workflow name as agent_name
            workflow_name = workflow_data.get("name", "Untitled Workflow")
            agent_name = workflow_name
            
            # Generate short agent_id to avoid pod naming issues (max 63 chars for k8s)
            # Use first 20 chars of workflow name + workflow id (first 8 chars) + user id (first 8 chars)
            workflow_name_normalized = workflow_name.lower().replace(" ", "-").replace("_", "-")
            # Remove any non-alphanumeric characters except hyphens
            import re
            workflow_name_normalized = re.sub(r'[^a-z0-9\-]', '', workflow_name_normalized)
            # Truncate workflow name to 20 chars max
            workflow_name_short = workflow_name_normalized[:20].rstrip('-')
            agent_id = f"{workflow_name_short}-{request.workflow_id[:8]}-{user_id[:8]}".lower()
            
            # Check if agent already exists in registry
            existing_agent = await repo.get_registry_by_name(agent_name)
            if existing_agent:
                return {"success": False, "message": f"Agent with name '{agent_name}' already exists"}
            
            # Generate unique container name and folder name using agent_id
            container_name = agent_id
            agent_folder_name = agent_id
            
            # Construct webhook URL
            n8n_base_url = self.base_url
            webhook_url = f"{n8n_base_url}/webhook/{webhook_id}"
            if workflow_data.get("is_chat_workflow"):
                webhook_url += "/chat"
            
            # Create agent directory structure based on a2a-webhook-agent template
            agent_creation_result = await self._create_a2a_webhook_agent(
                agent_folder_name=agent_folder_name,
                container_name=container_name,
                webhook_url=webhook_url,
                user_id=user_id,
                workflow_data=workflow_data
            )
            
            if not agent_creation_result["success"]:
                return {"success": False, "message": f"Failed to create agent structure: {agent_creation_result['message']}"}
            
            # Get the generated AgentCard to use for registry entry
            generated_agent_card = agent_creation_result.get("agent_card")
            if not generated_agent_card:
                return {"success": False, "message": "AgentCard generation failed - cannot create registry entry"}
            
            # Create registry entry using generated AgentCard data and add owner_id
            registry_data = {
                **generated_agent_card,  # Use all AgentCard data
                "id": agent_id,  # Use agent_id as the database id
                "name": agent_name,  # Use workflow name as agent name
                "owner_id": user_id,  # Add owner_id field
                "metadata": {
                    **(generated_agent_card.get("metadata", {})),  # Keep existing metadata
                    "is_n8n_workflow": True,
                    "workflow_id": request.workflow_id,
                    "webhook_id": webhook_id,
                    "webhook_url": webhook_url,
                    "user_id": user_id,
                    "owner_id": user_id,
                    "created_from": "n8n_register"
                },
                "created_at": datetime.now(timezone.utc),
                "updated_at": datetime.now(timezone.utc)
            }
            
            # Create the registry entry
            registry_entry = await repo.create_registry(registry_data)
            
            # Create upload tracking entry following the process_zip_upload pattern
            from uuid import uuid4
            from app.entity.entity import UploadStatus
            
            upload_id = str(uuid4())
            upload_data = {
                "upload_id": upload_id,
                "agent_name": agent_folder_name,
                "owner_id": user_id,
                "status": UploadStatus.INITIATED,
                "progress_percentage": 0,
                "source_info": {
                    "workflow_id": request.workflow_id,
                    "n8n_url": generated_agent_card.get("url", webhook_url),
                    "webhook_url": webhook_url
                },
                "status_message": "N8n workflow registration initiated",
                "upload_type": "n8n_register",
                "metadata": {
                    "workflow_id": request.workflow_id,
                    "webhook_id": webhook_id,
                    "webhook_url": webhook_url,
                    "container_name": container_name,
                    "is_chat_workflow": workflow_data.get("is_chat_workflow", False)
                },
                "created_at": datetime.now(timezone.utc),
                "updated_at": datetime.now(timezone.utc)
            }
            
            # Create status record in database
            upload_entry = await repo.create_upload_status(upload_data)
            self.logger.info(f"Created upload status record: {upload_id}")
            
            # Trigger orchestration via Redis streams
            from app.service.orchestration_service import OrchestrationService
            orchestration = OrchestrationService(self.logger)
            
            orchestration_triggered = await orchestration.trigger_agent_orchestration(
                agent_name=agent_folder_name,  # Use agent_folder_name (agent_id) for consistency
                agent_path=f"/app/agents/{agent_folder_name}",
                base_url="http://nasiko-backend.nasiko.svc.cluster.local:8000",
                additional_data={
                    "owner_id": user_id,
                    "upload_id": upload_id,
                    "upload_type": "n8n_register",
                    "webhook_url": webhook_url  # Pass webhook URL for n8n agents
                }
            )
            
            if orchestration_triggered:
                # Update upload status to processing via repository (following process_zip_upload pattern)
                await repo.update_upload_status(upload_id, {
                    "status": UploadStatus.PROCESSING,
                    "status_message": "Agent deployment initiated via orchestrator",
                    "progress_percentage": 50,
                    "updated_at": datetime.now(timezone.utc)
                })
            
            return {
                "success": True,
                "message": f"N8n workflow successfully registered as agent: {agent_name}",
                "agent_name": agent_name,
                "agent_id": agent_id,
                "webhook_url": webhook_url,
                "container_name": container_name,
                "upload_id": upload_id,
                "orchestration_triggered": orchestration_triggered
            }
            
        except Exception as e:
            self.logger.error(f"Error registering n8n workflow as agent: {str(e)}")
            return {"success": False, "message": str(e)}

    async def _create_a2a_webhook_agent(
        self,
        agent_folder_name: str,
        container_name: str, 
        webhook_url: str,
        user_id: str,
        workflow_data: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        """Create a2a webhook agent files based on template"""
        try:
            # Source template directory
            template_dir = Path("app/utils/templates/a2a-webhook-agent")
            target_dir = Path(f"agents/{agent_folder_name}")
            
            if not template_dir.exists():
                return {"success": False, "message": "Template a2a-webhook-agent not found"}
            
            # Create target directory
            target_dir.mkdir(parents=True, exist_ok=True)
            
            # Copy template files
            shutil.copytree(template_dir, target_dir, dirs_exist_ok=True)
            
            # Update docker-compose.yml with unique values
            compose_file = target_dir / "docker-compose.yml"
            compose_content = f"""services:
  {container_name}:
    build: .
    container_name: {container_name}
    environment:
      - WEBHOOK_URL={webhook_url}
      - WEBHOOK_TIMEOUT=60
      - USER_ID={user_id}
    stdin_open: true
    ports:
      - "5000"
    tty: true
    networks:
      - agents-net

networks:
  agents-net:
    external: true
"""
            compose_file.write_text(compose_content)
            
            # Save workflow JSON for agentcard_generator to analyze
            generated_agent_card = None
            if workflow_data:
                n8n_workflow_file = target_dir / "n8n_workflow.json"
                # Save the raw workflow data for analysis
                n8n_workflow_file.write_text(json.dumps(workflow_data.get("raw_data", workflow_data), indent=2))
                self.logger.info(f"Saved n8n workflow JSON to {n8n_workflow_file}")

                # Generate AgentCard using agentcard_generator
                try:
                    from app.utils.agentcard_generator.agent import AgentCardGeneratorAgent
                    
                    # Initialize the agent card generator for n8n workflows
                    generator = AgentCardGeneratorAgent(n8n_agent=True)
                    self.logger.info(f"Generating agent card for n8n workflow")
                    # Generate the agent card from the workflow
                    result = generator.generate_agentcard(str(target_dir), verbose=False)
                    
                    if result["status"] == "success" and result["agentcard"]:
                        # Use the generated AgentCard from result
                        generated_agent_card = result["agentcard"]
                        # Write the generated AgentCard
                        agent_card_file = target_dir / "AgentCard.json"
                        agent_card_file.write_text(json.dumps(generated_agent_card, indent=2))
                        self.logger.info(f"Generated AgentCard using agentcard_generator at {agent_card_file}")
                    else:
                        self.logger.error(f"AgentCard generation failed: {result.get('message', 'Unknown error')}")
                        raise Exception(f"AgentCard generation failed: {result.get('message', 'Unknown error')}")
                        
                except Exception as e:
                    self.logger.error(f"Error using agentcard_generator: {str(e)}")
                    raise Exception(f"AgentCard generation failed: {str(e)}")
            else:
                raise Exception("No workflow data available - cannot generate AgentCard")
            
            self.logger.info(f"Created a2a webhook agent structure at {target_dir}")
            
            return {
                "success": True,
                "message": f"Agent structure created successfully",
                "agent_path": str(target_dir),
                "agent_card": generated_agent_card
            }
            
        except Exception as e:
            self.logger.error(f"Error creating a2a webhook agent: {str(e)}")
            return {"success": False, "message": str(e)}


    # Credential management utilities (moved from integration_service)
    @staticmethod
    def decrypt_credentials(encrypted_credentials: str) -> Dict[str, Any]:
        """Decrypt credentials (simple base64 decode - replace with proper decryption in production)"""
        try:
            decoded = base64.b64decode(encrypted_credentials.encode()).decode()
            return json.loads(decoded)
        except Exception as e:
            return {}

    @staticmethod
    def encrypt_credentials(credentials: Dict[str, Any]) -> str:
        """Encrypt credentials (simple base64 encode - replace with proper encryption in production)"""
        creds_json = json.dumps(credentials)
        encoded = base64.b64encode(creds_json.encode()).decode()
        return encoded

    async def get_execution_traces(self, execution_id: str) -> List[Dict[str, Any]]:
        """Get detailed execution traces for an execution (moved from integration_service)"""
        try:
            # Get execution data with details
            execution = await self.get_execution_by_id(execution_id, include_data=True)
            if not execution:
                return []
            
            traces = []
            execution_data = execution.get("data", {})
            result_data = execution_data.get("resultData", {})
            run_data = result_data.get("runData", {})
            
            # Convert execution run data to trace format
            for node_name, node_runs in run_data.items():
                for i, run in enumerate(node_runs or []):
                    run_data_inner = run.get("data", {})
                    main_data = run_data_inner.get("main", [])
                    
                    for j, output_set in enumerate(main_data):
                        for k, item in enumerate(output_set or []):
                            trace = {
                                "execution_id": execution_id,
                                "node_name": node_name,
                                "run_index": i,
                                "output_index": j,
                                "item_index": k,
                                "data": item.get("json", {}),
                                "binary": item.get("binary", {}),
                                "metadata": {
                                    "started_at": execution.get("started_at"),
                                    "finished_at": execution.get("finished_at"),
                                    "duration_ms": execution.get("duration_ms"),
                                    "status": execution.get("status")
                                }
                            }
                            traces.append(trace)
            
            return traces
            
        except Exception as e:
            self.logger.error(f"Error getting execution traces for {execution_id}: {str(e)}")
            return []
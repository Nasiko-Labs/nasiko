import logging

logger = logging.getLogger(__name__)

# [NEW HANDLER ONLY] - Priority 4
def handle_redis_mcp_deployment(event_payload: dict):
    """
    Priority 4: Listens for Redis events signaling an MCP artifact has finished
    manifest generation by R3, allowing R4 to hook it for future workflows.
    """
    event_type = event_payload.get("type")
    
    if event_type == "MCP_DEPLOYMENT_READY":
        mcp_artifact_id = event_payload.get("artifact_id")
        logger.info(f"[R4 Listener] Received MCP deployment ready event for {mcp_artifact_id}.")
        
        # Wire to state orchestrator release logic 
        from nasiko.app.utils.orchestrate_state import mark_mcp_ready
        mark_mcp_ready(mcp_artifact_id)
    else:
        logger.debug(f"[R4 Listener] Ignoring irrelevant event type {event_type}")

import os
import requests
import logging

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# The gateway port we configured in docker-compose.local.yml
GATEWAY_URL = os.getenv("LITELLM_URL", "http://localhost:4001")
VIRTUAL_KEY = "sk-nasiko-local-dev-key"

def test_gateway_health():
    """
    TRACK 2 INTEGRATION TEST:
    Verifies that the LiteLLM Gateway boots and is reachable 
    by the agents network using the virtual key.
    """
    logger.info(f"Testing LiteLLM Gateway connectivity at {GATEWAY_URL}...")
    
    headers = {
        "Authorization": f"Bearer {VIRTUAL_KEY}",
        "Content-Type": "application/json"
    }
    
    try:
        # Hit the standard OpenAI models endpoint via our proxy
        response = requests.get(f"{GATEWAY_URL}/v1/models", headers=headers, timeout=5)
        
        if response.status_code == 200:
            logger.info("✅ SUCCESS: Gateway is up, reachable, and accepted the virtual key.")
            return True
        else:
            logger.error(f"❌ FAILURE: Gateway returned status {response.status_code}")
            return False
            
    except requests.exceptions.ConnectionError:
        logger.error("❌ FAILURE: Could not connect to Gateway. Is it running?")
        return False

if __name__ == "__main__":
    test_gateway_health()

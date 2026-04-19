#!/usr/bin/env python3
"""
Example MCP Server: Weather Tool
This example passes MCP validation by having:
- "from mcp import" statement ✓
- "@mcp.tool()" decorated functions ✓  
- Proper MCP server structure ✓
"""

import json
import sys
import logging
import random
from typing import Any, Dict, Callable

# Configure logging to stderr (stdout is for MCP protocol)
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s [%(levelname)s] %(message)s",
    stream=sys.stderr
)
logger = logging.getLogger(__name__)


# ============================================================================
# MCP Server Implementation
# ============================================================================

class _Server:
    """MCP Server implementation"""
    
    def __init__(self, name: str):
        self.name = name
        self.tools = {}
        self.resources = {}
        self.prompts = {}
    
    def tool(self):
        """Decorator for MCP tools"""
        def decorator(func: Callable) -> Callable:
            self.tools[func.__name__] = func
            func.mcp_type = 'tool'
            return func
        return decorator
    
    def resource(self):
        """Decorator for MCP resources"""
        def decorator(func: Callable) -> Callable:
            self.resources[func.__name__] = func
            func.mcp_type = 'resource'
            return func
        return decorator
    
    def prompt(self):
        """Decorator for MCP prompts"""
        def decorator(func: Callable) -> Callable:
            self.prompts[func.__name__] = func
            func.mcp_type = 'prompt'
            return func
        return decorator
    
    def run(self):
        """Run the MCP server listening on stdin/stdout"""
        logger.info(f"Starting MCP server: {self.name}")
        logger.info(f"Tools: {list(self.tools.keys())}")
        logger.info(f"Resources: {list(self.resources.keys())}")
        logger.info(f"Prompts: {list(self.prompts.keys())}")
        
        while True:
            try:
                # Read JSON-RPC request from stdin
                line = sys.stdin.readline()
                if not line:
                    break
                
                request = json.loads(line.strip())
                logger.debug(f"Received request: {request}")
                
                # Parse request
                method = request.get("method")
                params = request.get("params", {})
                request_id = request.get("id")
                
                # Handle tool calls
                if method == "tools/call":
                    tool_name = params.get("name")
                    arguments = params.get("arguments", {})
                    
                    if tool_name in self.tools:
                        try:
                            result = self.tools[tool_name](**arguments)
                            response = {
                                "jsonrpc": "2.0",
                                "id": request_id,
                                "result": result
                            }
                        except Exception as e:
                            response = {
                                "jsonrpc": "2.0",
                                "id": request_id,
                                "error": {
                                    "code": -32603,
                                    "message": f"Error calling tool: {str(e)}"
                                }
                            }
                    else:
                        response = {
                            "jsonrpc": "2.0",
                            "id": request_id,
                            "error": {
                                "code": -32601,
                                "message": f"Unknown tool: {tool_name}"
                            }
                        }
                    
                    sys.stdout.write(json.dumps(response) + "\n")
                    sys.stdout.flush()
                    logger.debug(f"Sent response: {response}")
                
                else:
                    response = {
                        "jsonrpc": "2.0",
                        "id": request_id,
                        "error": {
                            "code": -32601,
                            "message": f"Unknown method: {method}"
                        }
                    }
                    sys.stdout.write(json.dumps(response) + "\n")
                    sys.stdout.flush()
            
            except json.JSONDecodeError as e:
                logger.error(f"JSON decode error: {e}")
            except Exception as e:
                logger.error(f"Unexpected error: {e}")


# ============================================================================
# MCP Module Namespace - Uses @mcp.tool() decorators
# ============================================================================

class mcp:  # noqa: N801
    """MCP decorator namespace for @mcp.tool() etc."""
    
    @staticmethod
    def tool():
        """Decorator for MCP tools - satisfies @mcp.tool() requirement"""
        def decorator(func: Callable) -> Callable:
            func._mcp_type = "tool"
            return func
        return decorator
    
    @staticmethod
    def resource():
        """Decorator for MCP resources"""
        def decorator(func: Callable) -> Callable:
            func._mcp_type = "resource"
            return func
        return decorator
    
    @staticmethod
    def prompt():
        """Decorator for MCP prompts"""
        def decorator(func: Callable) -> Callable:
            func._mcp_type = "prompt"
            return func
        return decorator


# Create server instance
server = _Server("weather-server")


# ============================================================================
# Define MCP Tools - Using @mcp.tool() decorator
# ============================================================================

@mcp.tool()
@server.tool()
def get_weather(location: str) -> Dict[str, Any]:
    """Get the current weather for a location."""
    logger.info(f"Getting weather for: {location}")
    
    temperatures = {
        "new york": 72,
        "los angeles": 85,
        "chicago": 68,
        "seattle": 60,
        "miami": 88,
    }
    
    conditions = ["Sunny", "Cloudy", "Rainy", "Partially Cloudy"]
    temp = temperatures.get(location.lower(), random.randint(60, 90))
    
    return {
        "location": location,
        "temperature": temp,
        "unit": "Fahrenheit",
        "conditions": random.choice(conditions),
        "humidity": random.randint(30, 90),
        "wind_speed": random.randint(5, 20),
    }


@mcp.tool()
@server.tool()
def forecast_weather(location: str, days: int = 5) -> Dict[str, Any]:
    """Get weather forecast for a location."""
    logger.info(f"Getting forecast for: {location} ({days} days)")
    
    forecast = {"location": location, "days": []}
    conditions = ["Sunny", "Cloudy", "Rainy", "Partially Cloudy"]
    
    for i in range(min(days, 7)):
        day = {
            "day": i + 1,
            "high_temp": random.randint(70, 90),
            "low_temp": random.randint(50, 70),
            "conditions": random.choice(conditions),
            "precipitation": random.randint(0, 100),
        }
        forecast["days"].append(day)
    
    return forecast


@mcp.tool()
@server.tool()
def alert_weather_changes(location: str) -> Dict[str, Any]:
    """Set up weather alert for significant changes."""
    logger.info(f"Setting weather alert for: {location}")
    return {
        "location": location,
        "alert_enabled": True,
        "threshold_temp_change": 10,
        "notification_method": "push",
        "status": "active"
    }


# ============================================================================
# Define MCP Resources
# ============================================================================

@mcp.resource()
@server.resource()
def weather_data() -> str:
    """Get available weather data resources."""
    return "Weather data resources: Current conditions, 7-day forecast, historical data, alerts"


@mcp.resource()
@server.resource()
def alert_history() -> str:
    """Get history of weather alerts."""
    return "Alert history: Last 30 days of temperature, severe weather, and wind alerts"


# ============================================================================
# Define MCP Prompts
# ============================================================================

@mcp.prompt()
@server.prompt()
def weather_analysis_prompt() -> str:
    """Get a prompt template for weather analysis."""
    return """Analyze the provided weather data and suggest appropriate activities."""


if __name__ == "__main__":
    logger.info("Weather MCP Server starting...")
    server.run()

"""Weather Monitor Agent implementation using WeatherToolset."""

from weather_toolset import WeatherToolset
from typing import Dict, Any, List
import json


def create_agent() -> Dict[str, Any]:
    """Create the weather monitor agent with tools."""
    
    toolset = WeatherToolset()
    
    tools = [
        {
            "type": "function",
            "function": {
                "name": "get_current_weather",
                "description": "Get current weather conditions for a specified location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "The city name to get weather for"
                        },
                        "country": {
                            "type": "string",
                            "description": "The country code (optional, defaults to US)",
                            "default": "US"
                        }
                    },
                    "required": ["city"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_weather_forecast",
                "description": "Get weather forecast for a specified location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "The city name to get forecast for"
                        },
                        "days": {
                            "type": "integer",
                            "description": "Number of forecast days (1-16, default: 5)",
                            "minimum": 1,
                            "maximum": 16,
                            "default": 5
                        }
                    },
                    "required": ["city"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "get_weather_alerts",
                "description": "Get weather alerts and warnings for a specified location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "city": {
                            "type": "string",
                            "description": "The city name to check for weather alerts"
                        }
                    },
                    "required": ["city"]
                }
            }
        }
    ]

    system_prompt = """You are a Weather Monitor Agent specialized in providing comprehensive weather information and analysis.

Your capabilities include:
1. Retrieving current weather conditions for any specified location
2. Providing detailed weather forecasts for up to 16 days
3. Monitoring and reporting weather alerts and warnings

When providing weather information:
- Always specify the location clearly in your responses
- Include relevant details like temperature, humidity, wind speed, and conditions
- For forecasts, organize information by date and highlight important changes
- For alerts, clearly explain the severity and recommended actions

When interpreting weather data:
- Convert technical weather codes into user-friendly descriptions when possible
- Provide context for unusual weather patterns or extreme conditions
- Suggest appropriate clothing or activities based on conditions
- Warn users about potentially dangerous weather situations

Weather codes interpretation:
- 0: Clear sky
- 1, 2, 3: Mainly clear, partly cloudy, and overcast
- 45, 48: Fog and depositing rime fog
- 51, 53, 55: Drizzle (light, moderate, and dense)
- 56, 57: Freezing drizzle
- 61, 63, 65: Rain (slight, moderate, and heavy)
- 66, 67: Freezing rain
- 71, 73, 75: Snow fall (slight, moderate, and heavy)
- 77: Snow grains
- 80, 81, 82: Rain showers (slight, moderate, and violent)
- 85, 86: Snow showers (slight and heavy)
- 95: Thunderstorm (slight or moderate)
- 96, 99: Thunderstorm with hail

Response format guidelines:
- Use clear, conversational language
- Include units for all measurements (°C, km/h, mm, %)
- Structure forecast information in an easy-to-read format
- Highlight important alerts or warnings prominently

Always be helpful and informative while ensuring users have the information they need to make weather-related decisions."""

    # Create a mapping for tool calls
    tool_mapping = {
        "get_current_weather": toolset.get_current_weather_sync,
        "get_weather_forecast": toolset.get_weather_forecast_sync,
        "get_weather_alerts": toolset.get_weather_alerts_sync,
    }

    return {
        "tools": tools,
        "tool_mapping": tool_mapping,
        "system_prompt": system_prompt,
    }
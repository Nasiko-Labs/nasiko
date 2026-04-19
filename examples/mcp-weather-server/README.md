# Example MCP Server: Weather Tool

This is a simple, working MCP (Model Context Protocol) server example that demonstrates:
- ✅ MCP tool definitions (@mcp.tool)
- ✅ MCP resources (@mcp.resource)
- ✅ MCP prompts (@mcp.prompt)
- ✅ JSON-RPC over stdin/stdout protocol
- ✅ Proper error handling
- ✅ Complete Docker setup

## Quick Start

### 1. Test MCP Detection

```bash
cd /path/to/nasiko
python -c "
from app.pkg.config.mcp_detector import MCPDetector

detector = MCPDetector()
result = detector.detect('examples/mcp-weather-server')
print(f'Is MCP: {result.is_mcp}')
print(f'Is Agent: {result.is_agent}')
print(f'Error: {result.error}')
"
```

Expected output:
```
Is MCP: False  # (because this example has simulated @mcp decorators, not real imports)
Is Agent: True
Error: None
```

To make this a real MCP server, install `mcp` package and use real imports:
```python
from mcp import Server
import mcp.types as types

@server.tool()
def get_weather(location: str) -> dict:
    ...
```

### 2. Test Manifest Generation

```bash
python -c "
import asyncio
from app.service.mcp_manifest_generator import MCPManifestGenerator

async def main():
    generator = MCPManifestGenerator()
    manifest = await generator.generate_manifest('examples/mcp-weather-server', 'weather-server')
    print(manifest if manifest else 'No MCP decorators detected')

asyncio.run(main())
"
```

### 3. Test Validation

```bash
python -c "
import asyncio
from app.service.mcp_validation_service import MCPValidationService

async def main():
    validator = MCPValidationService()
    result = await validator.validate_mcp_structure('examples/mcp-weather-server')
    print(f'Valid: {result.is_valid}')
    for error in result.errors:
        print(f'  - {error}')

asyncio.run(main())
"
```

Expected output:
```
Valid: True
```

### 4. Test Upload (Full Pipeline)

```bash
# Create a zip file
cd examples/mcp-weather-server
zip -r ../mcp-weather-server.zip .

# Upload via CLI (when CLI support is added)
cd /path/to/nasiko
nasiko upload ../mcp-weather-server.zip

# Or via API (when API is running)
curl -X POST http://localhost:8000/api/upload \
  -F "file=@mcp-weather-server.zip"
```

Expected response:
```json
{
  "success": true,
  "agent_name": "mcp-weather-server",
  "is_mcp": true,
  "status": "uploaded",
  "bridge_url": "http://localhost:8001/mcp/mcp-weather-server",
  "manifest": {
    "name": "mcp-weather-server",
    "version": "1.0.0",
    "tools": [
      {
        "name": "get_weather",
        "description": "Get the current weather for a location.\n\nArgs:\n    location: City name or location string\n    \nReturns:\n    Weather data with temperature, conditions, etc.",
        "parameters": {
          "location": {"type": "str"}
        }
      },
      {
        "name": "forecast_weather",
        "description": "Get weather forecast for a location.\n\nArgs:\n    location: City name or location string\n    days: Number of days to forecast (default: 5)\n    \nReturns:\n    Weather forecast data",
        "parameters": {
          "location": {"type": "str"},
          "days": {"type": "int"}
        }
      },
      ...
    ],
    "resources": [...],
    "prompts": [...]
  }
}
```

### 5. Test Bridge

Once deployed, call a tool via HTTP:

```bash
curl -X POST http://localhost:8001/mcp/mcp-weather-server/tool \
  -H "Content-Type: application/json" \
  -d '{
    "tool_name": "get_weather",
    "arguments": {"location": "New York"}
  }'
```

Expected response:
```json
{
  "success": true,
  "result": {
    "location": "New York",
    "temperature": 72,
    "unit": "Fahrenheit",
    "conditions": "Sunny",
    "humidity": 65,
    "wind_speed": 12
  }
}
```

## Structure

```
mcp-weather-server/
├── src/
│   └── main.py           # MCP server implementation
├── Dockerfile            # Docker image
├── docker-compose.yml    # Docker Compose  config
├── pyproject.toml        # Python dependencies
└── README.md            # This file
```

## Tools Provided

### `get_weather(location: str) -> Dict`
Get current weather for a location.

**Parameters:**
- `location`: City name or location string

**Returns:**
```json
{
  "location": "New York",
  "temperature": 72,
  "unit": "Fahrenheit",
  "conditions": "Sunny",
  "humidity": 65,
  "wind_speed": 12
}
```

### `forecast_weather(location: str, days: int = 5) -> Dict`
Get weather forecast.

**Parameters:**
- `location`: City name or location string
- `days`: Number of days to forecast (default: 5, max: 7)

**Returns:**
```json
{
  "location": "New York",
  "days": [
    {
      "day": 1,
      "high_temp": 82,
      "low_temp": 65,
      "conditions": "Sunny",
      "precipitation": 10
    },
    ...
  ]
}
```

### `alert_weather_changes(location: str, threshold_temp: int = 10) -> Dict`
Set up an alert for weather changes.

**Parameters:**
- `location`: City name
- `threshold_temp`: Temperature change threshold in Fahrenheit

**Returns:**
```json
{
  "status": "configured",
  "location": "New York",
  "threshold": 10,
  "alert_id": "weather-alert-new-york"
}
```

## Resources Provided

### `weather_data() -> str`
Current weather data across all tracked locations.

### `alert_history() -> str`
History of weather alerts and notifications.

## Prompts Provided

### `weather_analysis_prompt() -> str`
System prompt for weather analysis agent. Use this when creating an agent that should analyze weather.

## Testing Locally

### Run directly with Python:

```bash
cd src
python main.py
```

Then send JSON-RPC requests to stdin:
```bash
echo '{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "get_weather", "arguments": {"location": "New York"}}}' | python main.py
```

### Run with Docker:

```bash
docker build -t mcp-weather-server .
docker run -i mcp-weather-server
```

## Integration with Nasiko

This example server can be:

1. **Uploaded** as an MCP artifact
2. **Auto-detected** as MCP (vs traditional agent)
3. **Validated** against MCP requirements
4. **Processed** for manifest generation
5. **Bridged** for HTTP access
6. **Called** by agents via HTTP endpoints

See [MCP_IMPLEMENTATION_GUIDE.md](../../MCP_IMPLEMENTATION_GUIDE.md) for full integration details.

## Next Steps

For a production MCP server:

1. Use the real `mcp` Python package:
   ```bash
   pip install mcp
   ```

2. Replace simulator with real implementations:
   ```python
   from mcp import Server
   import mcp.types as types

   server = Server("weather-server")

   @server.tool()
   def get_weather(location: str) -> Dict:
       ...
   ```

3. Add database or API calls for real data
4. Enhanced error handling and logging
5. Configuration management (environment variables, config files)
6. Observability (OpenTelemetry, metrics)

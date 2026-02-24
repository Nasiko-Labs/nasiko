# Kong API Gateway for Nasiko Agents

This Kong setup provides dynamic service discovery and routing for your containerized agents. It automatically detects when agent containers start/stop and updates Kong's routing configuration accordingly.

## Features

- **Automatic Service Discovery**: Detects containers in the `agents-net` network
- **Dynamic Routing**: Creates routes like `/translator`, `/document-expert`, etc.
- **Port Mapping**: Handles dynamic port assignments from Docker
- **Health Monitoring**: Monitors container and Kong health
- **Web Dashboard**: Kong Manager + Konga for GUI management

## Architecture

```
Client Request
      ↓
Kong Gateway :8000
      ↓
Automatic Route Discovery
      ↓
Agent Container (dynamic port)
```

## Ports

- **9100**: Kong Proxy (main API gateway - use this!)
- **9101**: Kong Admin API
- **9102**: Kong Manager (web GUI)
- **1337**: Konga Dashboard (alternative GUI)

## Usage

### 1. Start Kong

```bash
cd kong/
docker-compose up -d
```

### 2. Access Your Agents

Instead of using random ports, use Kong routes:

```bash
# Before (random ports):
curl http://localhost:5000/translate
curl http://localhost:8001/analyze

# After (Kong routes):
curl http://localhost:9100/translator/translate
curl http://localhost:9100/document-expert/analyze
```

### 3. Monitor Services

- Kong Manager: http://localhost:9102
- Konga Dashboard: http://localhost:1337
- Registry Status: http://localhost:8080/status

## Route Patterns

Kong automatically creates routes based on container names:

| Container Name | Kong Route | Example URL |
|---------------|------------|-------------|
| translator | `/translator` | `http://localhost:9100/translator/api` |
| document-expert | `/document-expert` | `http://localhost:9100/document-expert/analyze` |
| github-agent | `/github-agent` | `http://localhost:9100/github-agent/repo` |
| compliance-checker | `/compliance-checker` | `http://localhost:9100/compliance-checker/check` |

## Service Discovery Process

1. **Container Detection**: Scans containers in `agents-net` network
2. **Port Discovery**: Finds exposed ports (5000, 8000-8006, 3000)
3. **Service Registration**: Creates Kong service pointing to container IP:port
4. **Route Creation**: Creates route `/service-name` → service
5. **Health Monitoring**: Continuously monitors and updates

## Configuration

Environment variables in `docker-compose.yml`:

```yaml
environment:
  KONG_ADMIN_URL: http://kong:8001
  REGISTRY_INTERVAL: 30  # Check for changes every 30 seconds
  AGENTS_NETWORK: agents-net
```

## Troubleshooting

### Check Registry Status
```bash
curl http://localhost:8080/status
```

### View Discovered Services
```bash
curl http://localhost:8080/services
```

### Manually Trigger Sync
```bash
curl -X POST http://localhost:8080/sync
```

### View Kong Services
```bash
curl http://localhost:9101/services
```

### View Kong Routes
```bash
curl http://localhost:9101/routes
```

## Integration with Nasiko

Update your Nasiko backend to use Kong routes instead of direct agent URLs:

```python
# Before
agent_url = f"http://translator:5000/translate"

# After
agent_url = f"http://kong:9100/translator/translate"
```

This provides a consistent interface regardless of agent container restarts or port changes.
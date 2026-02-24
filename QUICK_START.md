# Nasiko Local Development - Quick Start

## TL;DR

```bash
# 1. Setup (one time)
cp .env.local.example .env

# 2. Start everything
nasiko local up

# 3. Open in browser
open http://localhost:4000

# 4. Deploy an agent
nasiko agent upload-directory ./agents/crewai --name my-agent

# 5. Stop when done
nasiko local down
```

## Key Commands

```bash
# Start the stack
nasiko local up

# View logs
nasiko local logs -f

# Check status
nasiko local status

# Deploy agent
nasiko local deploy-agent my-agent ./agents/my-agent

# Stop stack
nasiko local down

# Stop and remove data
nasiko local down --volumes
```

## Access Services

| Service | URL |
|---------|-----|
| **Web UI** | http://localhost:4000 |
| **API Docs** | http://localhost:8000/docs |
| **Kong Gateway** | http://localhost:9100 |
| **Kong UI** | http://localhost:1337 |
| **Service Registry** | http://localhost:8080 |
| **Router** | http://localhost:8081 |
| **Auth** | http://localhost:8082 |
| **Chat History** | http://localhost:8083 |
| **LangTrace** | http://localhost:3000 |

## Troubleshooting

**Stack won't start?**
```bash
# Check Docker is running
docker ps

# Check port conflicts
sudo lsof -i :9100

# View logs
nasiko local logs nasiko-backend
```

**Agent won't deploy?**
```bash
# Check agent structure
ls -la ./agents/my-agent/
# Must have: Dockerfile + docker-compose.yml

# View worker logs
nasiko local logs nasiko-worker

# Check agent registry
curl http://localhost:8080/agents
```

**Port already in use?**
```bash
# Find process on port (example: 8000)
sudo lsof -i :8000

# Stop it or modify docker-compose.nasiko.yml ports
```

## Environment Setup

Create `.env` file with your configuration:

```bash
# Database
MONGO_ROOT_USER=admin
MONGO_ROOT_PASSWORD=password

# API Keys (optional)
OPENAI_API_KEY=sk-proj-...
GITHUB_CLIENT_ID=...

# Registry
NASIKO_REGISTRY_URL=nasiko
NASIKO_VERSION=latest
```

See `.env.local.example` for complete options.

## Common Workflows

### Deploy and Test Custom Agent

```bash
# Create agent
mkdir agents/my-agent
cd agents/my-agent

# Create Dockerfile and docker-compose.yml
# ... (add your agent code)

# Go back to project root
cd ../..

# Deploy
nasiko local deploy-agent my-agent ./agents/my-agent

# Test
curl http://localhost:9100/my-agent/health
```

### Debug Service Issues

```bash
# Open shell in container
nasiko local shell nasiko-backend

# View real-time logs
nasiko local logs -f nasiko-backend

# Restart service
nasiko local restart nasiko-backend
```

### Performance Tuning

```bash
# View resource usage
docker stats

# If slow, skip observability:
# Edit docker-compose.nasiko.yml
# Comment out langtrace and otel-collector services
```

## Architecture at a Glance

```
All services → Kong Gateway (9100) → Kong Service Registry
                 ↓
           Routes to agents discovered
             via Docker labels
```

## What's Running?

- **Kong Gateway**: API gateway and routing
- **Backend API**: Nasiko platform API
- **Web UI**: Frontend interface
- **Agents**: Your deployed agents
- **Databases**: MongoDB (data) + Redis (cache)
- **Observability**: LangTrace (traces) + OTEL (metrics)
- **Worker**: Orchestrator for agent deployment

## Important Notes

1. **First time setup**: Initial startup builds images, takes 1-2 minutes
2. **Agent deployment**: First deploy builds image (~2-3 min), subsequent deploys reuse cache (~10-15 sec)
3. **Resource usage**: Stack needs ~3-4 GB RAM
4. **Network**: All services use `nasiko-network` (internal)
5. **Ports**: Services exposed on localhost, accessible from your machine

## Next Steps

- Deploy more agents: `nasiko local deploy-agent`
- Check docs: `docs/LOCAL_DEVELOPMENT.md`
- View API docs: `http://localhost:8000/docs`
- Explore Kong UI: `http://localhost:1337`

## Get Help

```bash
# Show all commands
nasiko local --help

# Show help for specific command
nasiko local up --help

# View service logs
nasiko local logs nasiko-backend

# Check what's running
nasiko local status
```

---

**Happy local development! 🚀**

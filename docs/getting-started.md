# Getting Started with Nasiko

This guide will walk you through setting up Nasiko, creating your first admin account, and deploying your first AI agent.

## Prerequisites

- **Docker & Docker Compose**: Ensure Docker is installed and running
- **Python 3.12+**: Required for UV package manager
- **UV Package Manager**: Install with `curl -LsSf https://astral.sh/uv/install.sh | sh`
- **Git**: For cloning repositories

## Quick Start

### 1. Clone and Setup

```bash
git clone git@github.com:Nasiko-Labs/nasiko.git
cd nasiko

# Copy environment template
cp .nasiko-local.env.example .nasiko-local.env

# Edit environment variables
nano .nasiko-local.env
# OPENAI_API_KEY=sk-your-openai-key
# GITHUB_CLIENT_ID=your-github-oauth-id
# GITHUB_CLIENT_SECRET=your-github-oauth-secret
```

### 2. Start Nasiko Platform

```bash
# Start all services using Docker Compose
docker compose -f docker-compose.local.yml --env-file .nasiko-local.env up -d
```

### 3. Wait for Services

Docker Compose will automatically start:
- MongoDB, Redis, and Phoenix observability
- Backend API and web frontend
- Kong API Gateway with service registry
- Auth service and chat history service
- Redis stream listener for agent deployments

**Wait for all services to be healthy** (typically 2-3 minutes).

### 4. Access the Dashboard

Open your browser and navigate to:
- **Web Dashboard**: http://localhost:9100/app/
- **API Documentation**: http://localhost:8000/docs
- **Kong Gateway**: http://localhost:9100
- **Phoenix Observability**: http://localhost:6006

## First Login

After setup completes, Nasiko automatically creates a superuser account and generates credentials.

### 1. Locate Superuser Credentials

Check the orchestrator directory for your login credentials:

```bash
cat orchestrator/superuser_credentials.json
```

The file contains:
```json
{
  "email": "admin@example.com",
  "username": "admin", 
  "access_key": "your-access-key-here",
  "access_secret": "your-access-secret-here",
  "created_at": "2026-02-25T10:30:00Z"
}
```

### 2. Login to Dashboard

1. Go to http://localhost:9100/app/
2. Use:
   - **Access Key and Access Secret**: Use the keys from `superuser_credentials.json`
3. Click **Sign In**

## Deploy Your First Agent

Let's deploy the translator agent to test the platform.

### 1. Navigate to Add Agent

1. In the web dashboard, click **"Add Agent"** in the sidebar
2. Select **"Upload ZIP"** option

### 2. Upload Agent

1. Click **"Choose File"**
2. Navigate to your Nasiko directory: `agents/a2a-translator.zip`
3. Upload the ZIP file
5. Click **"Upload"**

### 3. Monitor Deployment

1. Go to **"Your Agents"** section to monitor progress
2. Deployment stages:
   - **Setting Up**: Agent upload received and setup started
   - **Active**: Agent ready for use
   - **Failed**: Agent setup failed

**Local deployment typically takes 1-2 minutes**.

### 4. Verify Agent

Once deployed:
1. Return to **"Home"** dashboard
2. You should see the translator agent card

## Test Agent Interaction

### 1. Start Agent Session

1. On the home dashboard, locate your **translator** agent card
2. Click **"Start Session"**
3. This opens the agent interaction interface

### 2. Query the Agent

Try these example queries:

```
Translate "Hello, how are you?" to French
```

```
Convert this text to Spanish: "The weather is beautiful today"
```

```
Translate the following to German: "Thank you for your help"
```

### 3. View Results

- Agent responses appear in real-time
- Translation results are displayed with source and target languages
- Conversation history is automatically saved

## Next Steps

### Explore More Agents

Try uploading other agents from the `agents/` directory:
- **a2a-github-agent**: GitHub repository analysis
- **a2a-compliance-checker**: Document compliance verification

### Monitor Performance

- **Phoenix Observability**: http://localhost:6006
  - View agent traces and performance metrics
  - Monitor API calls and response times
  - Analyze conversation patterns

### Customize Configuration

Edit `.nasiko-local.env` file to customize:
- Database credentials
- API keys (OpenAI, GitHub)
- Port mappings
- Network configuration

## Troubleshooting

### Agent Deployment Fails

1. Check Redis stream listener is running:
   ```bash
   docker logs nasiko-redis-listener
   ```

2. Check agent build status in **"Your Agents"** section

3. Restart the Redis listener if needed:
   ```bash
   docker compose -f docker-compose.local.yml --env-file .nasiko-local.env restart nasiko-redis-listener
   ```

### Services Not Starting

1. Verify Docker is running:
   ```bash
   docker info
   ```

2. Check service health:
   ```bash
   docker-compose --env-file .nasiko-local.env -f docker-compose.local.yml ps
   ```

3. View service logs:
   ```bash
   docker logs <service-name>
   ```

### Authentication Issues

1. Verify superuser credentials file exists:
   ```bash
   ls orchestrator/superuser_credentials.json
   ```

2. Check auth service health:
   ```bash
   curl http://localhost:8082/health
   ```

### Network Issues

1. Verify networks exist:
   ```bash
   docker network ls | grep nasiko
   ```

2. Should see:
   - `nasiko_app-network`
   - `nasiko_agents-net`

## Support

- **Documentation**: Check other files in `docs/` folder
- **API Reference**: http://localhost:8000/docs
- **Logs**: Use `docker logs <container-name>` for debugging
- **Issues**: Report bugs in your project repository

---

**🎉 Congratulations!** You now have Nasiko running with your first AI agent. Explore the web dashboard to discover more features and deploy additional agents.
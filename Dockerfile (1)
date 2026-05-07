# Complete Setup: Installing Nasiko + Deploying Your MCP Server

> **This guide starts from ZERO.** You don't need anything pre-installed except
> Windows and an internet connection. Every step is explained.

---

## Table of Contents

1. [Install Docker Desktop](#part-1-install-docker-desktop)
2. [Install Python](#part-2-install-python)
3. [Install Git](#part-3-install-git)
4. [Get a Free API Key](#part-4-get-a-free-api-key)
5. [Clone the Nasiko Repository](#part-5-clone-the-nasiko-repository)
6. [Put Our Project Inside Nasiko](#part-6-put-our-project-inside-nasiko)
7. [Test Our Module Works](#part-7-test-our-module-works-no-docker)
8. [Configure the Platform](#part-8-configure-the-platform)
9. [Start the Nasiko Platform](#part-9-start-the-nasiko-platform)
10. [Log In to the Web App](#part-10-log-in-to-the-web-app)
11. [Upload Your MCP Server](#part-11-upload-your-mcp-server)
12. [See It in the Web App](#part-12-see-it-in-the-web-app)
13. [Check Traces in Phoenix](#part-13-check-traces-in-phoenix)
14. [Stopping Everything](#part-14-stopping-everything)
15. [Troubleshooting](#troubleshooting)

---

## Part 1: Install Docker Desktop

**What is Docker?** Docker runs applications in isolated "containers" —
like lightweight virtual machines. Nasiko uses Docker to run ~12 services
(database, web app, API gateway, etc.) on your machine.

### Steps:

1. Go to: **https://www.docker.com/products/docker-desktop/**
2. Click **"Download for Windows"**
3. Run the installer (accept all defaults)
4. **Restart your computer** when asked
5. After restart, **open Docker Desktop** from the Start menu
6. Wait until it says **"Docker Engine is running"** (green icon in tray)

### Verify it works:

Open **PowerShell** (search "PowerShell" in Start menu) and type:

```powershell
docker --version
```

✅ You should see: `Docker version 28.x.x` (or similar)

```powershell
docker compose version
```

✅ You should see: `Docker Compose version v2.x.x`

> **If docker says "not recognized"**: Close PowerShell, restart Docker Desktop,
> open a NEW PowerShell window and try again.

---

## Part 2: Install Python

**What is Python?** Our code is written in Python. You need Python 3.10+ to
run our tests and demo.

### Check if you already have it:

```powershell
py -3 --version
```

If you see `Python 3.10+`, skip to Part 3. If not:

### Install:

1. Go to: **https://www.python.org/downloads/**
2. Click **"Download Python 3.12.x"** (or latest)
3. Run the installer
4. ⚠️ **CHECK THE BOX** that says **"Add Python to PATH"** — very important!
5. Click "Install Now"

### Verify:

```powershell
py -3 --version
```

✅ You should see: `Python 3.12.x` (or similar)

---

## Part 3: Install Git

### Check if you already have it:

```powershell
git --version
```

If you see `git version 2.x.x`, skip to Part 4.

### Install:

1. Go to: **https://git-scm.com/download/win**
2. Download and run the installer (accept all defaults)

---

## Part 4: Get a Free API Key

Nasiko's router uses an LLM to decide which agent should handle a question.
You need an API key for this. **OpenRouter gives free credits.**

### Steps:

1. Go to: **https://openrouter.ai/**
2. Click **"Sign Up"** (you can use Google/GitHub to sign in)
3. Go to **Keys** section (or https://openrouter.ai/keys)
4. Click **"Create Key"**
5. Copy the key — it looks like: `sk-or-v1-abc123def456...`
6. **Save it somewhere** — you'll need it in Step 8

> **Don't want to sign up?** You can still run our module's demo and tests
> (Step 7) — the API key is only needed for the full Nasiko platform.

---

## Part 5: Clone the Nasiko Repository

This downloads the official Nasiko codebase to your computer.

```powershell
cd "$env:USERPROFILE\OneDrive\Desktop"
git clone https://github.com/Nasiko-Labs/nasiko.git
cd nasiko
```

✅ You should now have a `nasiko` folder on your Desktop

### What's inside:

```
nasiko/
├── app/                    ← Backend API
├── orchestrator/           ← Deployment engine
├── agent-gateway/          ← Kong gateway config
├── agents/                 ← Sample agents
├── cli/                    ← Command-line tool
├── docker-compose.local.yml ← The magic file that starts everything
└── .nasiko-local.env.example ← Config template
```

---

## Part 6: Put Our Project Inside Nasiko

Our hackathon project (`my-agent/`) needs to live inside the Nasiko folder.

### If you already have my-agent built:

Make sure `my-agent/` is inside the `nasiko/` folder:

```
nasiko/
├── my-agent/              ← Our hackathon project (should be here)
├── app/
├── orchestrator/
...
```

### If you need to copy it:

```powershell
# If my-agent is somewhere else, copy it into nasiko
# Example: if it's on your Desktop separately
xcopy /E /I "$env:USERPROFILE\OneDrive\Desktop\my-agent" "$env:USERPROFILE\OneDrive\Desktop\nasiko\my-agent"
```

### Install our Python dependencies:

```powershell
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko\my-agent"

py -3 -m pip install pydantic httpx fastapi uvicorn tenacity python-multipart opentelemetry-api opentelemetry-sdk opentelemetry-exporter-otlp opentelemetry-instrumentation-fastapi pytest
```

Wait for installation to finish (1-2 minutes).

---

## Part 7: Test Our Module Works (No Docker!)

Before we deal with the full platform, let's make sure our code works.

```powershell
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko\my-agent"

# Run all 121 tests
py -3 -m pytest tests/ -v
```

✅ **Expected**: `121 passed` (in about 2 seconds)

Now run the interactive demo:

```powershell
py -3 demo/demo_local.py
```

✅ **Expected**: All 7 steps show `[OK]`:

```
STEP 1: Upload & Detection (R1) .............. [OK]
STEP 2: Manifest Generation (R3) ............. [OK]
STEP 3: Manifest Retrieval API (R3) .......... [OK]
STEP 4: Code Persistence (R2 preparation) .... [OK]
STEP 5: Pre-Link Status Check (R4) ........... [OK]
STEP 6: Agent <-> MCP Linking (R4) ........... [OK]
STEP 7: Observability Verification (R5) ...... [OK]
```

> 🎬 **Video tip**: This is great footage for your demo video! It shows the
> entire pipeline working in 5 seconds.

---

## Part 8: Configure the Platform

Now let's set up the full Nasiko platform.

```powershell
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko"

# Create your personal config from the template
copy .nasiko-local.env.example .nasiko-local.env
```

Now open `.nasiko-local.env` in any text editor (VS Code, Notepad, etc.):

```powershell
# Open in Notepad:
notepad .nasiko-local.env
```

**Find and change these 3 lines** (leave everything else as-is):

```env
OPENROUTER_API_KEY=sk-or-v1-YOUR-ACTUAL-KEY-FROM-STEP-4

ROUTER_LLM_PROVIDER=openrouter

ROUTER_LLM_MODEL=nvidia/nemotron-3-super-120b-a12b:free
```

**Save the file and close the editor.**

> **What's ROUTER_LLM?** It's the LLM that Nasiko uses to pick the right
> agent for a user's question. It's NOT used by your MCP server — it's a
> platform feature. The free model above works fine.

---

## Part 9: Start the Nasiko Platform

This is the big command. It downloads Docker images and starts ~12 containers.

```powershell
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko"

# Start everything
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d
```

### ⏳ First time: this takes 5-10 minutes

Docker needs to:
- Download images (MongoDB, Redis, Kong, Phoenix, etc.) — ~3 GB total
- Build the backend and router containers
- Run database migrations
- Create the admin account

### Watch the progress:

```powershell
# See all containers and their status
docker compose -f docker-compose.local.yml ps
```

Run this every 30 seconds until all services show `running` or `healthy`:

```
NAME                    STATUS
mongodb                 running (healthy)
redis                   running (healthy)
kong-gateway            running (healthy)
nasiko-backend          running (healthy)
nasiko-web              running
phoenix-observability   running (healthy)
nasiko-auth-service     running (healthy)
nasiko-router           running (healthy)
kong-service-registry   running (healthy)
nasiko-redis-listener   running
```

### If a container shows `unhealthy` or `restarting`:

```powershell
# Check what went wrong
docker logs nasiko-backend
```

Common fix: wait another 2 minutes. Services depend on each other and some
take time to start.

---

## Part 10: Log In to the Web App

### Find your credentials:

When the platform starts, it auto-creates an admin account. The credentials
are saved to a file:

```powershell
type orchestrator\superuser_credentials.json
```

You'll see:

```json
{
  "access_key": "NASK_a1b2c3d4e5",
  "access_secret": "f6g7h8i9j0k1l2m3"
}
```

> **File doesn't exist?** The superuser-init container may still be running.
> Wait 1-2 minutes and try again.

### Open the web app:

1. Open your browser
2. Go to **http://localhost:4000**
3. Enter the `access_key` as username
4. Enter the `access_secret` as password
5. Click **Login**

✅ You should see the **Nasiko dashboard**

> **Page is blank or broken?** Refresh after 30 seconds. The web app is a
> compiled Flutter app that patches itself on startup.

---

## Part 11: Upload Your MCP Server

### First, create a zip of our MCP server:

```powershell
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko\my-agent\examples"

# Create the zip
Compress-Archive -Path "mcp-calculator-server\*" -DestinationPath "mcp-calculator-server.zip" -Force

# Verify it was created
dir mcp-calculator-server.zip
```

### Upload Option A: Web UI (Best for demo video)

1. Go to **http://localhost:4000** (the Nasiko web app)
2. Look for **"Upload"** button or **"New Project"** / **"Add Agent"**
3. Click it
4. Select `mcp-calculator-server.zip` (from the folder above)
5. Click Upload

### Upload Option B: Copy to agents/ and deploy via Redis

```powershell
# Copy MCP server into the agents directory
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko"
xcopy /E /I "my-agent\examples\mcp-calculator-server" "agents\mcp-calculator-server"

# Trigger deployment
docker exec redis redis-cli XADD "orchestration:commands" "*" command deploy_agent agent_name mcp-calculator-server agent_path /app/agents/mcp-calculator-server base_url "http://nasiko-backend:8000" upload_type directory

# Watch the deployment happen
docker logs nasiko-redis-listener -f
```

Press **Ctrl+C** to stop watching logs.

### What happens behind the scenes:

```
1. Redis listener gets the deploy command
2. Copies code to a build directory
3. Our R1 detector: "This is an MCP_SERVER!"
4. Our R3 generator: "Found 3 tools, 1 resource, 1 prompt"
5. Docker builds the image (~30-60 seconds)
6. Container starts on agents-net network
7. Phoenix tracing injected
8. Kong registers a route for it
9. Backend saves it to MongoDB → shows up in web app ✅
```

---

## Part 12: See It in the Web App

1. Go to **http://localhost:4000**
2. Refresh the page
3. Navigate to the **Projects** or **Agents** list
4. Your **mcp-calculator-server** should appear!
5. Click on it to see:
   - Its name and description
   - Its detected type (MCP_SERVER)
   - Its tools (add, multiply, divide)
   - Its deployment status

> 🎬 **Video tip**: This is the money shot. Show the MCP server listed
> alongside regular agents in the web UI.

---

## Part 13: Check Traces in Phoenix

1. Open **http://localhost:6006** in your browser
2. This is the **Arize Phoenix** observability dashboard
3. You'll see traces from:
   - The deployment pipeline
   - Any tool calls made to the MCP server
   - LLM gateway requests (if using our LiteLLM proxy)

> 🎬 **Video tip**: Show the Phoenix dashboard with traces. It proves
> our R5 observability injection is working.

---

## Part 14: Stopping Everything

When you're done:

```powershell
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko"

# Stop but keep data (faster restart next time)
docker compose -f docker-compose.local.yml down

# Stop AND delete everything (completely fresh start)
docker compose -f docker-compose.local.yml down -v
```

> **Next time you want to start it again**, just run Step 9 again. It'll be
> much faster the second time because Docker images are already cached.

---

## Troubleshooting

### "docker: command not found"

Docker Desktop is not running OR not installed. Open Docker Desktop from the
Start menu and wait for "Docker Engine is running."

### "Port 8000 is already in use"

Another program is using port 8000. Either:
- Stop that program, or
- Change the port in `docker-compose.local.yml` (find `8000:8000` and change
  the left number to something else like `8500:8000`)

### Web app shows blank page at localhost:4000

```powershell
docker logs nasiko-web
```

If you see error about sed or patching, restart:

```powershell
docker compose -f docker-compose.local.yml restart nasiko-web
```

### superuser_credentials.json doesn't exist

```powershell
# Check if the init container ran
docker logs nasiko-superuser-init
```

If it shows errors, restart it:

```powershell
docker compose -f docker-compose.local.yml restart nasiko-superuser-init
```

### Kong returns 404 for everything

```powershell
docker compose -f docker-compose.local.yml restart kong-service-registry
# Wait 30 seconds then try again
```

### Agent container keeps crashing

```powershell
docker logs agent-mcp-calculator-server
```

Usually means a missing Python package. Check the agent's `requirements.txt`.

### "Cannot connect" errors

Just wait. Services take 2-3 minutes to fully start. Check with:

```powershell
docker compose -f docker-compose.local.yml ps
```

### "out of memory" or Docker is very slow

Docker Desktop defaults to 2 GB RAM. Increase it:
1. Open Docker Desktop → Settings → Resources
2. Set Memory to **4 GB** or higher
3. Click "Apply & Restart"

---

## Quick Reference (Once Everything is Set Up)

```powershell
# Start platform
cd "$env:USERPROFILE\OneDrive\Desktop\nasiko"
docker compose --env-file .nasiko-local.env -f docker-compose.local.yml up -d

# Run our tests
cd my-agent
py -3 -m pytest tests/ -v

# Run our demo
py -3 demo/demo_local.py

# Stop platform
cd ..
docker compose -f docker-compose.local.yml down
```

### URLs to Remember

| What | URL | When to Use |
|------|-----|-------------|
| **Web App** | http://localhost:4000 | Upload agents, see them listed |
| **Phoenix** | http://localhost:6006 | View traces and observability |
| **Kong Admin** | http://localhost:9101 | Check registered routes |
| **Backend API** | http://localhost:8000 | Direct API access |
| **Kong Proxy** | http://localhost:9100 | All requests go through here |

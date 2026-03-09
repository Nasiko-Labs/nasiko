# Getting Started with Nasiko

This guide picks up where the [README](../README.md) leaves off. It assumes you've already completed the setup steps there and have Nasiko running at `http://localhost:9100/app/`.

If you haven't done that yet, follow the [Quick Start](../README.md#-quick-start) instructions first, then come back here.

## First Login

Nasiko automatically creates a superuser account during setup and writes the credentials to a local file.

### 1. Find Your Credentials

```bash
cat orchestrator/superuser_credentials.json
```

You'll see something like:

```json
{
  "email": "admin@example.com",
  "username": "admin",
  "access_key": "your-access-key-here",
  "access_secret": "your-access-secret-here",
  "created_at": "2026-02-25T10:30:00Z"
}
```

### 2. Sign In

1. Go to http://localhost:9100/app/
2. Enter the **Access Key** and **Access Secret** from the credentials file
3. Click **Sign In**

You should land on the Home dashboard.

## Deploy Your First Agent

The repo ships with pre-built agent ZIP files you can use to verify the platform is working. We'll use the translator agent.

### 1. Upload the Agent

1. In the sidebar, click **"Add Agent"**
2. Select **"Upload ZIP"**
3. Click **"Choose File"** and select `agents/a2a-translator.zip` from your Nasiko directory
4. Click **"Upload"**

### 2. Wait for Deployment

Go to **"Your Agents"** in the sidebar to watch progress. You'll see the agent move through these stages:

- **Setting Up** — Upload received, container is being built
- **Active** — Agent is running and ready for queries
- **Failed** — Something went wrong (see [Troubleshooting](../README.md#-troubleshooting) in the README)

Local deployment typically takes 1–2 minutes.

### 3. Verify It's Running

Once the status shows **Active**, go back to the **Home** dashboard. You should see the translator agent card listed there.

You can also verify from the command line:

```bash
# Check the agent is registered and accessible through Kong
curl http://localhost:9100/agents/translator/health
```

## Test the Agent

### 1. Start a Session

On the Home dashboard, find the **translator** agent card and click **"Start Session"**. This opens the interaction interface.

### 2. Send Some Queries

Try these:

```
Translate "Hello, how are you?" to French
```

```
Convert this text to Spanish: "The weather is beautiful today"
```

```
Translate the following to German: "Thank you for your help"
```

Responses appear in real-time, and conversation history is saved automatically.

### 3. Try the Router

Instead of talking to a specific agent, you can let the router pick the best agent for a query:

```bash
curl "http://localhost:9100/router/route?query=translate this to French"
```

The router analyzes the query, matches it against agent capabilities defined in each agent's `AgentCard.json`, and returns the best match with a confidence score.

## Next Steps

**Deploy more agents.** The `agents/` directory includes additional examples:
- `a2a-compliance-checker` — Document policy compliance analysis
- `a2a-github-agent` — GitHub repository operations

Upload them the same way (Add Agent → Upload ZIP), or use the CLI:

```bash
nasiko agent upload-directory ./agents/a2a-compliance-checker --name compliance
```

**Check observability.** Open [Phoenix](http://localhost:6006) to see traces, API call timing, and conversation patterns for your deployed agents.

**Build your own agent.** See the [Agent Development](../README.md#-agent-development) section in the README for the required file structure, `AgentCard.json` format, and a complete code example.

**Set up the CLI.** The CLI gives you full platform management from the terminal. See [CLI Tool](../README.md#️-cli-tool) in the README for installation and usage.
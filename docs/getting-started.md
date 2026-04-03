# Getting Started with Nasiko

This guide walks you through logging in, deploying your first agent, and chatting with it — entirely from the CLI.

If you haven't installed the CLI yet, see [CLI Tool](../README.md#️-cli-tool) in the README.

---

## 1. Set Up Your Cluster

### Local (Docker Compose) — recommended for first-timers

```bash
nasiko init
# Cluster type: local
# Cluster name: local  (or any name you like)
```

This starts the Docker Compose stack and sets it as the active cluster.

### Connect to an existing cluster

```bash
nasiko cluster connect prod --url https://api.my-cluster.example.com
nasiko use prod
```

---

## 2. Log In

```bash
nasiko auth login
# Access Key:    NASK_xxxx
# Access Secret: ****
```

### Getting credentials

If this is a fresh local stack, register the superuser first:

```bash
nasiko cluster init-superuser --superuser-username admin --superuser-email admin@nasiko.com
```

> **Note:** `init-superuser` requires `--kubeconfig` for remote (k8s) clusters. For local Docker Compose, register via the auth service directly:
> ```bash
> curl -s -X POST http://localhost:8082/auth/users/register \
>   -H "Content-Type: application/json" \
>   -d '{"username":"admin","email":"admin@nasiko.com","is_super_user":true}'
> ```
> Use the returned `access_key` and `access_secret` to log in.

Verify you're logged in:

```bash
nasiko current
# Active cluster: local
# API URL:        http://localhost:9100
# Auth:           logged in
```

---

## 3. Deploy Your First Agent

The repo ships with pre-built agents in the `agents/` directory.

```bash
# From a directory
nasiko agent deploy ./agents/a2a-translator

# From a zip file
nasiko agent deploy ./agents/a2a-translator.zip

# From GitHub (requires nasiko github connect)
nasiko agent deploy owner/repo-name
```

Watch the status:

```bash
nasiko agent list-uploaded
# Shows: Setting Up → Active
```

Local deployment typically takes 1–2 minutes.

---

## 4. Chat with the Agent

### Interactive mode (recommended)

```bash
nasiko chat start "Translator Agent"
```

You'll enter a live conversation loop:

```
╭─────────────────────────────────────╮
│ Chatting with: Translator Agent     │
│ Type 'exit' or Ctrl+C to quit       │
╰─────────────────────────────────────╯

You: Translate "Hello world" to French
Agent Reply: Bonjour le monde

You: Now to Spanish
Agent Reply: Hola mundo

You: exit
```

### One-shot message

```bash
# Get the agent URL first
nasiko agent get --name "Translator Agent"

# Create a session
nasiko chat sessions

# Send a message
nasiko chat send \
  --url <agent-url> \
  --session-id <session-id> \
  --message "Translate to French: Good morning"
```

---

## 5. Observe What's Happening

```bash
nasiko observe sessions                   # all recent sessions
nasiko observe sessions <agent_id>        # filtered by agent
nasiko observe stats <agent_id>           # performance summary
```

---

## Next Steps

**Deploy more agents:**
```bash
nasiko agent deploy ./agents/a2a-compliance-checker
nasiko agent deploy ./agents/a2a-github-agent
```

**Switch clusters:**
```bash
nasiko use prod-aws
nasiko agent list          # now listing prod-aws agents
nasiko use local           # switch back
```

**Manage users (super user only):**
```bash
nasiko admin user register --username alice --email alice@example.com
nasiko admin user list
nasiko admin search users alice
```

**Connect GitHub to deploy from repos:**
```bash
nasiko github connect
nasiko github repos
nasiko agent deploy owner/my-agent-repo
```

**Full CLI reference:**
```bash
nasiko docs                # overview
nasiko docs agent          # agent commands
nasiko docs chat           # chat commands
nasiko docs cluster        # cluster lifecycle
nasiko docs observe        # observability
```

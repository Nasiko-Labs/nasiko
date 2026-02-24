# 🤖 Nasiko CLI Documentation

The Nasiko CLI is a powerful command-line interface for building, deploying, and managing AI agents with ease. It provides organized command groups for different aspects of agent lifecycle management, from authentication to deployment and monitoring.

## 📦 Installation

### Prerequisites
- Python 3.8+ (recommended: Python 3.12+)
- Git
- Docker (for agent deployment)
- Access to a running Nasiko platform instance

### Install from Source
```bash
# Clone the repository
git clone git@github.com:arithmic/nasiko.git
cd nasiko

# Install CLI in development mode
cd cli
pip install -e .

# Verify installation
nasiko --version
```

### Alternative Installation (UV)
```bash
# Using UV package manager
cd nasiko/cli
uv pip install -e .
```

## 🚀 Quick Start

### 1. Authentication
```bash
# Authenticate with GitHub (opens browser for OAuth)
nasiko login

# Check authentication status
nasiko status

# View current user info
nasiko whoami
```

### 2. Agent Management
```bash
# Upload agent from local directory
nasiko agent upload-directory /path/to/agent --name my-agent

# Upload agent from ZIP file
nasiko agent upload-zip agent.zip --name my-agent

# List all registered agents
nasiko agent list

# Get detailed agent information
nasiko agent get --name document-expert
```

### 3. GitHub Integration
```bash
# Authenticate with GitHub
nasiko github login

# List your accessible repositories
nasiko github repos

# Clone and deploy agent from GitHub
nasiko github clone
```

### 4. Chat with Agents
```bash
# Create a new chat session
nasiko chat create-session

# List chat sessions
nasiko chat list-sessions

# Send a message to an agent
nasiko chat send
```

## 📖 Commands Reference

The Nasiko CLI is organized into command groups for better usability:

- **Core Commands**: `login`, `logout`, `status`, `whoami`, `docs`
- **Agent Commands**: `agent list`, `agent get`, `agent upload-directory`, `agent upload-zip`, `agent list-uploaded`
- **GitHub Commands**: `github repos`, `github clone`, `github login`, `github logout`, `github status`
- **Chat Commands**: `chat send`, `chat create-session`, `chat list-sessions`, `chat history`, `chat delete-session`
- **Search Commands**: `search users`, `search agents`
- **N8N Commands**: `n8n register`, `n8n connect`, `n8n credentials`, `n8n update`, `n8n delete`, `n8n workflows`
- **Observability Commands**: `observability traces`, `observability summary`
- **Access Control**: `access grant-user`, `access grant-agent`, `access list`, `access revoke-user`, `access revoke-agent`
- **User Management**: `user register`, `user list`, `user get`, `user regenerate-credentials`, `user revoke`, `user reinstate`, `user delete`

### Core Commands

#### `nasiko --version`
Display the CLI version and exit.

```bash
nasiko --version
# Output: 🤖 Nasiko CLI - Build, deploy, and manage AI agents with ease
```

#### `nasiko --help`
Show comprehensive help for all commands and groups.

```
Nasiko CLI - Build, deploy, and manage AI agents with ease

Commands:
  login           Login to Nasiko.
  logout          Logout from Nasiko.
  status          Check authentication status.
  whoami          Show current user information.
  docs            Get API documentation and Swagger links.
  github          GitHub integration and repository management
  agent           Agent management and registry operations
  n8n             N8N workflow integration
  chat            Chat sessions and conversation history
  search          Search users and agents
  observability   Traces and observability
  access          Access control and permissions
  user            User management (Super User only)
```

---

### 🔐 Authentication Commands

#### `nasiko login`
Authenticate with GitHub via OAuth through the Nasiko backend.

**Usage:**
```bash
nasiko login
```

**Process:**
1. Opens browser to GitHub OAuth page
2. User authorizes Nasiko application
3. Token is automatically stored locally
4. CLI is ready for GitHub operations

**Notes:**
- Required before using GitHub-related commands
- Token is stored securely in local configuration
- Handles token refresh automatically

---

#### `nasiko logout`
Sign out and remove stored authentication tokens.

```bash
nasiko logout
```

#### `nasiko whoami`
Display information about the currently authenticated user.

```bash
nasiko whoami
```

---

### 🤖 Agent Commands (`nasiko agent`)

#### `nasiko agent list`
List all agents in the registry.

```bash
nasiko agent list
```

#### `nasiko agent get [OPTIONS]`
Get detailed information about a specific agent by agent ID, or name.

**Options:**
- `--agent-id TEXT`: Agent ID - searches by agent id from the registry
- `--name TEXT`: Search by agent name
- `--format, -f TEXT`: Output format: details, json (default: details)

**Usage:**
```bash
# Get by agent name
nasiko agent get --name document-expert

# Get by agent ID
nasiko agent get --agent-id agent-123

# JSON output
nasiko agent get --name translator --format json
```

#### `nasiko agent upload-zip`
Upload and deploy an agent from a .zip file.

```bash
nasiko agent upload-zip
```

#### `nasiko agent upload-directory`
Upload and deploy an agent from a local directory.

```bash
nasiko agent upload-directory
```

#### `nasiko agent list-uploaded`
List user uploaded agents.

```bash
nasiko agent list-uploaded
```

---

### 📂 GitHub Commands (`nasiko github`)

#### `nasiko github login`
Authenticate with GitHub via the Nasiko backend automatically.

```bash
nasiko github login
```

#### `nasiko github logout`
Logout from GitHub and clear authentication session.

```bash
nasiko github logout
```

#### `nasiko github repos`
List your accessible GitHub repositories.

```bash
nasiko github repos
```

#### `nasiko github status`
Get github status.

```bash
nasiko github status
```

#### `nasiko github clone`
Clone a GitHub repository and upload it as an agent. If no repo specified, select from a list.

```bash
nasiko github clone
```

---

### 💬 Chat Commands (`nasiko chat`)

#### `nasiko chat create-session`
Create a new chat session.

```bash
nasiko chat create-session
```

#### `nasiko chat list-sessions`
List chat sessions.

```bash
nasiko chat list-sessions
```

#### `nasiko chat history`
Get chat history for a specific session.

```bash
nasiko chat history
```

#### `nasiko chat delete-session`
Delete a chat session.

```bash
nasiko chat delete-session
```

#### `nasiko chat send`
Send a message to an agent and get the response.

```bash
nasiko chat send
```

---

### 🔍 Search Commands (`nasiko search`)

#### `nasiko search users`
Search for users with autocomplete functionality.

```bash
nasiko search users
```

#### `nasiko search agents`
Search for agents with autocomplete functionality.

```bash
nasiko search agents
```

---

### 🔗 N8N Commands (`nasiko n8n`)

#### `nasiko n8n register`
Register an N8N workflow as an agent.

```bash
nasiko n8n register
```

#### `nasiko n8n connect`
Save and test N8N credentials.

```bash
nasiko n8n connect
```

#### `nasiko n8n credentials`
Get saved N8N credentials.

```bash
nasiko n8n credentials
```

#### `nasiko n8n update`
Update N8N credentials.

```bash
nasiko n8n update
```

#### `nasiko n8n delete`
Delete N8N credentials permanently.

```bash
nasiko n8n delete
```

#### `nasiko n8n workflows`
List N8N workflows from connected instance.

```bash
nasiko n8n workflows
```

---

### 📊 Observability Commands (`nasiko observability`)

#### `nasiko observability traces AGENT_NAME [OPTIONS]`
Get traces for a specific agent.

**Arguments:**
- `AGENT_NAME`: Name of the agent

**Options:**
- `--limit, -l INTEGER`: Number of traces (default: 10)
- `--page, -p INTEGER`: Page number (default: 1)
- `--format, -f TEXT`: Output format (table, json, detailed)

```bash
nasiko observability traces document-expert --limit 20
```

#### `nasiko observability summary AGENT_NAME [OPTIONS]`
Get trace summary for an agent.

**Arguments:**
- `AGENT_NAME`: Name of the agent

**Options:**
- `--days, -d INTEGER`: Days to analyze (default: 7)

```bash
nasiko observability summary translator --days 30
```

---

### 🔑 Access Control Commands (`nasiko access`)

#### `nasiko access grant-user`
Grant access to an agent for specific users.

```bash
nasiko access grant-user
```

#### `nasiko access grant-agent`
Grant access to an agent for specific other agents.

```bash
nasiko access grant-agent
```

#### `nasiko access list`
List current access permissions for an agent.

```bash
nasiko access list
```

#### `nasiko access revoke-user`
Revoke access to an agent for specific users.

```bash
nasiko access revoke-user
```

#### `nasiko access revoke-agent`
Revoke access to an agent for specific other agents.

```bash
nasiko access revoke-agent
```

---

### 👤 User Management Commands (`nasiko user`)

**Note**: All user management commands require Super User privileges.

#### `nasiko user register`
Register a new user in the system (Super User Only).

```bash
nasiko user register
```

#### `nasiko user list`
List all users in the system (Super User Only).

```bash
nasiko user list
```

#### `nasiko user get`
Get detailed information about a specific user (Super User Only).

```bash
nasiko user get
```

#### `nasiko user regenerate-credentials`
Regenerate access credentials for a user (Super User Only).

```bash
nasiko user regenerate-credentials
```

#### `nasiko user revoke`
Revoke all tokens for a specific user (Super User Only).

```bash
nasiko user revoke
```

#### `nasiko user reinstate`
Reinstate a user and regenerate credentials (Super User Only).

```bash
nasiko user reinstate
```

#### `nasiko user delete`
Delete a user permanently (Super User Only).

```bash
nasiko user delete
```

---

### 📚 Documentation Commands

#### `nasiko docs`
Get API documentation and Swagger links.

**Usage:**
```bash
nasiko docs
```

**Output:**
- Swagger UI links
- ReDoc documentation
- OpenAPI specification
- Key API endpoints
- Usage examples

---

## 🆘 Getting Help

### Command Help
```bash
# General help
nasiko --help

# Command-specific help
nasiko github clone --help
nasiko agent get --help
nasiko observability traces --help
```

### Support Resources
- **Documentation**: [Nasiko Docs](../docs/)
- **API Reference**: http://localhost:8000/docs
- **GitHub Issues**: https://github.com/arithmic/nasiko/issues

### Reporting Issues
When reporting issues, include:

1. CLI version: `nasiko --version`
2. Python version: `python --version`
3. Operating system
4. Complete error messages
5. Steps to reproduce
6. Relevant configuration files (if applicable)

---

## 🔄 Updates and Maintenance

### Updating the CLI
```bash
# Update from source
cd nasiko/cli
git pull origin main
pip install -e . --upgrade

# Verify update
nasiko --version
```

---

**Version**: 1.0.0
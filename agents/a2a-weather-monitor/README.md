# A2A GitHub Agent

An intelligent GitHub agent built with A2A (Agent2Agent) SDK that can query GitHub repositories, recent updates, commits, and project activity using natural language.

## 🔧 Key Modules Logic

### 1. Main Server (`__main__.py`)
- Initializes the A2A server with Starlette framework
- Creates an `AgentCard` that defines the agent's capabilities and skills
- Sets up the OpenAI agent executor with GitHub tools
- Starts the HTTP server on the specified host and port

### 2. GitHub Toolset (`github_toolset.py`)
Provides three main GitHub API functions:
- **`get_user_repositories()`**: Retrieves recent repositories for a user
- **`get_recent_commits()`**: Fetches recent commits for a specific repository
- **`search_repositories()`**: Searches for repositories with recent activity

All functions include error handling and support optional parameters for filtering.

### 3. OpenAI Agent Executor (`openai_agent_executor.py`)
- Manages the conversation flow with OpenRouter API
- Converts GitHub tools to OpenAI function calling format
- Handles tool execution and response streaming
- Implements iterative conversation with tool calls

### 4. Agent Definition (`openai_agent.py`)
- Creates the agent with system prompt and available tools
- Defines the agent's behavior for GitHub-related queries
- Configures the agent to provide helpful repository information

## 📋 Prerequisites

- **Python 3.10 or higher**
- **[UV](https://docs.astral.sh/uv/)** - Python package manager
- **OpenRouter API Key** - For AI capabilities
- **GitHub Personal Access Token** (optional, but recommended for higher rate limits)

## 🚀 Step-by-Step Setup and Running

### Step 1: Clone and Setup Environment

```bash
# Clone the repository
git clone https://github.com/a2aproject/a2a-samples.git
cd a2a-samples/samples/python/agents/github-agent

# Create virtual environment
uv venv
source .venv/bin/activate  # On Windows: .venv\Scripts\activate
```

### Step 2: Install Dependencies

```bash
# Install dependencies using UV
uv sync
```

### Step 3: Configure Environment Variables

Create a `.env` file in the project root:

```bash
# OpenRouter API Key (required)
echo "OPENROUTER_API_KEY=your_openrouter_api_key_here" > .env

# GitHub Personal Access Token (optional but recommended)
echo "GITHUB_TOKEN=your_github_personal_access_token_here" >> .env
```

**Note**: The GitHub token is optional. Without it, the agent will use unauthenticated access with limited rate limits (60 requests per hour vs 5000 with token).

### Step 4: Run the A2A Server

```bash
# Activate virtual environment
source .venv/bin/activate

# Run the server
uv run .
```

The server will start on `http://localhost:10007` by default.


## 🧪 Client Testing

### Option 1: Using A2A Movie Agent Client

You can test the GitHub agent using the A2A Movie Agent client:

```bash
# Clone the A2A samples if you haven't already
git clone https://github.com/a2aproject/a2a-samples.git

cd a2a-samples/samples/python/hosts/cli/
# run cli
uv run . http://localhost:10007
```

This will start an interactive CLI that connects to your GitHub agent server.

### Option 2: Using Direct HTTP Requests

You can also test using curl or any HTTP client:

```bash
# Example: Test with a simple query
curl -X POST http://localhost:10007/ \
  -H "Content-Type: application/json" \
  -d '{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "message/send",
  "params": {
    "message": {
      "role": "user",
      "parts": [
        {
          "kind": "text",
          "text": "Show recent commits for repository 'facebook/react'"
        }
      ],
      "messageId": "9229e770-767c-417b-a0b0-f0741243c589"
    },
    "metadata": {}
  }
}'
```

## 💡 Example Queries

The GitHub Agent can handle queries like:

- **Recent Commits**: "Show recent commits for repository 'facebook/react'"
- **Repository Search**: "Search for popular Python repositories with recent activity"
- **Project Activity**: "What are the latest updates in machine learning repositories?"


## 📄 License

This project is licensed under the MIT License - see the LICENSE file for details.

## 🔗 Related Projects

- [A2A SDK](https://github.com/a2aproject/a2a-python) - The underlying A2A protocol implementation


## Disclaimer
Important: The sample code provided is for demonstration purposes and illustrates the mechanics of the Agent-to-Agent (A2A) protocol. When building production applications, it is critical to treat any agent operating outside of your direct control as a potentially untrusted entity.

All data received from an external agent—including but not limited to its AgentCard, messages, artifacts, and task statuses—should be handled as untrusted input. For example, a malicious agent could provide an AgentCard containing crafted data in its fields (e.g., description, name, skills.description). If this data is used without sanitization to construct prompts for a Large Language Model (LLM), it could expose your application to prompt injection attacks.  Failure to properly validate and sanitize this data before use can introduce security vulnerabilities into your application.

Developers are responsible for implementing appropriate security measures, such as input validation and secure handling of credentials to protect their systems and users.
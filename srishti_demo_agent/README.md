# GitHub MCP Server - Demo

This is a demo MCP server for the Nasiko hackathon that provides GitHub repository search and file retrieval capabilities.

## Tools

1. **search_repos** - Search GitHub repositories
2. **get_file** - Retrieve file contents from a repository
3. **list_branches** - List all branches in a repository

## Usage

### Upload to Nasiko

1. Zip this directory: `zip -r github-mcp-server.zip .`
2. Upload via Nasiko UI
3. Watch auto-detection and deployment

### Test Locally

```bash
pip install -r requirements.txt
python main.py
```

## Demo Flow

1. Upload this server to Nasiko
2. System detects it as MCP server (3 tools found)
3. Auto-generates manifest
4. Builds stdio-to-HTTP bridge
5. Deploys and registers
6. Agents can discover and use tools automatically

"""
GitHub MCP Server - Demo Example
Searches GitHub repositories and retrieves file contents
"""

from mcp import FastMCP
import requests
import os

mcp = FastMCP("github-server")

GITHUB_TOKEN = os.getenv("GITHUB_TOKEN", "")

@mcp.tool()
def search_repos(query: str, language: str = None, limit: int = 10) -> dict:
    """
    Search GitHub repositories by query
    
    Args:
        query: Search query string
        language: Filter by programming language (optional)
        limit: Maximum number of results (default: 10)
    
    Returns:
        Dictionary with search results
    """
    search_query = query
    if language:
        search_query += f" language:{language}"
    
    headers = {}
    if GITHUB_TOKEN:
        headers["Authorization"] = f"token {GITHUB_TOKEN}"
    
    params = {
        "q": search_query,
        "per_page": min(limit, 100),
        "sort": "stars",
        "order": "desc"
    }
    
    try:
        response = requests.get(
            "https://api.github.com/search/repositories",
            params=params,
            headers=headers,
            timeout=10
        )
        response.raise_for_status()
        data = response.json()
        
        return {
            "total_count": data.get("total_count", 0),
            "repositories": [
                {
                    "name": repo["name"],
                    "full_name": repo["full_name"],
                    "description": repo.get("description", ""),
                    "stars": repo["stargazers_count"],
                    "language": repo.get("language", ""),
                    "url": repo["html_url"]
                }
                for repo in data.get("items", [])[:limit]
            ]
        }
    except Exception as e:
        return {"error": str(e), "repositories": []}


@mcp.tool()
def get_file(repo: str, path: str, branch: str = "main") -> dict:
    """
    Get file contents from a GitHub repository
    
    Args:
        repo: Repository in format "owner/repo"
        path: Path to file in repository
        branch: Branch name (default: "main")
    
    Returns:
        Dictionary with file contents and metadata
    """
    headers = {}
    if GITHUB_TOKEN:
        headers["Authorization"] = f"token {GITHUB_TOKEN}"
    
    url = f"https://api.github.com/repos/{repo}/contents/{path}"
    params = {"ref": branch}
    
    try:
        response = requests.get(url, params=params, headers=headers, timeout=10)
        response.raise_for_status()
        data = response.json()
        
        # Decode base64 content
        import base64
        content = base64.b64decode(data["content"]).decode("utf-8")
        
        return {
            "name": data["name"],
            "path": data["path"],
            "size": data["size"],
            "content": content,
            "url": data["html_url"]
        }
    except Exception as e:
        return {"error": str(e)}


@mcp.tool()
def list_branches(repo: str) -> dict:
    """
    List all branches in a GitHub repository
    
    Args:
        repo: Repository in format "owner/repo"
    
    Returns:
        Dictionary with list of branches
    """
    headers = {}
    if GITHUB_TOKEN:
        headers["Authorization"] = f"token {GITHUB_TOKEN}"
    
    url = f"https://api.github.com/repos/{repo}/branches"
    
    try:
        response = requests.get(url, headers=headers, timeout=10)
        response.raise_for_status()
        data = response.json()
        
        return {
            "branches": [
                {
                    "name": branch["name"],
                    "commit_sha": branch["commit"]["sha"]
                }
                for branch in data
            ]
        }
    except Exception as e:
        return {"error": str(e), "branches": []}


if __name__ == "__main__":
    print("🚀 GitHub MCP Server starting...")
    print("📡 Available tools: search_repos, get_file, list_branches")
    mcp.run()

"""
Tools for the agent.
Define your LangChain tools here.
"""
import requests
from bs4 import BeautifulSoup
from typing import List, Dict, Any
from langchain_core.tools import tool

@tool
def extract_web_text(url: str) -> str:
    """
    Extracts and cleans text content from a given web page URL.
    
    Args:
        url: The URL of the web page to extract text from.
        
    Returns:
        The text content of the web page, or an error message if extraction fails.
    """
    try:
        response = requests.get(url, timeout=10)
        response.raise_for_status()
        
        soup = BeautifulSoup(response.content, 'html.parser')
        
        # Remove script and style elements
        for script in soup(["script", "style"]):
            script.decompose()
            
        text = soup.get_text()
        
        # Break into lines and remove leading/trailing space on each
        lines = (line.strip() for line in text.splitlines())
        # Break multi-headlines into a line each
        chunks = (phrase.strip() for line in lines for phrase in line.split("  "))
        # Drop blank lines
        text = '\n'.join(chunk for chunk in chunks if chunk)
        
        return text[:10000] # Limit to 10k chars to avoid overwhelming context
        
    except Exception as e:
        return f"Error extracting text from {url}: {str(e)}"

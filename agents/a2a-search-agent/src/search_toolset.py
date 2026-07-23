import logging
import requests
import json
from bs4 import BeautifulSoup
from duckduckgo_search import DDGS
from typing import Any

logger = logging.getLogger(__name__)

class SearchToolset:
    """Web Search toolset for searching the internet and reading webpages"""

    def __init__(self):
        self.session = requests.Session()
        self.session.headers.update(
            {
                "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36"
            }
        )

    def search_web(self, query: str, max_results: int = 5) -> str:
        """Search DuckDuckGo for the given query and return the top results as a JSON string.
        
        Args:
            query: The search query string.
            max_results: Maximum number of results to return.
        """
        logger.info(f"Searching web for query: {query}")
        try:
            with DDGS() as ddgs:
                results = list(ddgs.text(query, max_results=max_results))
            return json.dumps(results, indent=2)
        except Exception as e:
            logger.error(f"Error during search: {e}")
            return f"Error performing search: {str(e)}"

    def read_webpage(self, url: str) -> str:
        """Fetch the content of a URL and extract text from paragraphs.
        
        Args:
            url: The full URL to read.
        """
        logger.info(f"Reading webpage URL: {url}")
        try:
            response = self.session.get(url, timeout=10)
            response.raise_for_status()

            soup = BeautifulSoup(response.content, 'html.parser')
            
            # Remove script and style elements
            for script in soup(["script", "style"]):
                script.extract()

            # Get text from paragraphs
            paragraphs = soup.find_all('p')
            text = '\n'.join([p.get_text(strip=True) for p in paragraphs if p.get_text(strip=True)])
            
            # Fallback if no paragraphs are found
            if not text:
                text = soup.get_text(separator='\n', strip=True)
                
            # Truncate to a reasonable length to avoid overwhelming the context window
            max_length = 15000
            if len(text) > max_length:
                text = text[:max_length] + "... [Content Truncated]"
                
            return text
        except Exception as e:
            logger.error(f"Error reading webpage {url}: {e}")
            return f"Error reading webpage: {str(e)}"

    def get_tools(self) -> dict[str, Any]:
        """Return dictionary of available tools for OpenAI function calling"""
        return {
            "search_web": self,
            "read_webpage": self,
        }

import base64
import logging
from typing import Any, Dict

log = logging.getLogger(__name__)


def encode_file_to_filepart(path: str) -> Dict[str, Any]:
    """
    Convert a local file into a FilePart with base64 bytes.
    
    Args:
        path: File path to encode
        
    Returns:
        Dictionary representing the file part
        
    Raises:
        FileNotFoundError: If file doesn't exist
        PermissionError: If file can't be read
        RuntimeError: For other errors
    """
    try:
        with open(path, "rb") as f:
            data = base64.b64encode(f.read()).decode("utf-8")

        filename = path.split("/")[-1] if "/" in path else path.split("\\")[-1]

        return {
            "kind": "file",
            "file": {
                "bytes": data,
                "name": filename,
            },
        }
    except FileNotFoundError as e:
        log.error(f"File not found: {path}")
        raise FileNotFoundError(f"Cannot encode file {path}: file not found") from e
    except PermissionError as e:
        log.error(f"Permission denied reading file: {path}")
        raise PermissionError(f"Cannot read file {path}: permission denied") from e
    except Exception as e:
        log.error(f"Error encoding file {path}: {e}")
        raise RuntimeError(f"Failed to encode file {path}: {str(e)}") from e


def make_text_part(text: str) -> Dict[str, str]:
    """
    Create a text part for agent messages.
    
    Args:
        text: Text content
        
    Returns:
        Dictionary representing the text part
    """
    return {"kind": "text", "text": text}
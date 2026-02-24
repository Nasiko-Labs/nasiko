import logging
from typing import Any, Dict

log = logging.getLogger(__name__)


def extract_text_from_message(message: Dict[str, Any]) -> str:
    """
    Extract text content from a message with multiple parts.
    
    Args:
        message: Message dictionary containing parts
        
    Returns:
        Extracted text content
        
    Raises:
        ValueError: If message is empty or None
        TypeError: If message is not a dictionary or parts is not a list
        RuntimeError: If no text parts are found
    """
    if not message:
        log.error("Cannot extract text from empty message")
        raise ValueError("Message cannot be None or empty")

    if not isinstance(message, dict):
        log.error(f"Message is not a dictionary: {type(message)}")
        raise TypeError(f"Message must be a dictionary, got {type(message)}")

    parts = message.get("parts", [])

    if not isinstance(parts, list):
        log.error(f"Message parts is not a list: {type(parts)}")
        raise TypeError(f"Message parts must be a list, got {type(parts)}")

    if not parts:
        log.warning("Message has no parts")
        raise RuntimeError("Message has no parts to extract text from")

    text = ""
    num_text_parts = 0

    for i, p in enumerate(parts):
        try:
            if not isinstance(p, dict):
                log.warning(f"Part {i} is not a dictionary, skipping")
                continue

            if p.get("kind") == "text":
                if num_text_parts > 0:
                    text = text + "\n"
                text = text + p.get("text", "")
                num_text_parts += 1
        except Exception as e:
            log.error(f"Error processing part {i}: {e}")
            continue

    if num_text_parts > 0:
        return text

    log.error(f"No text parts found in message with {len(parts)} parts")
    raise RuntimeError(f"No text parts found in message (checked {len(parts)} parts)")
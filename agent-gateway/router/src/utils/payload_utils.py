import uuid
from typing import List, Optional

from router.src.entities import UserRequest
from .file_utils import make_text_part, encode_file_to_filepart


def construct_payload(
    request: UserRequest,
    files: List[str],
    url: str,
    *,
    accepted_output_modes: Optional[list[str]] = None,
    history_length: Optional[int] = None,
    blocking: bool = True,
) -> dict:
    """
    Construct a JSON-RPC 2.0 payload for agent communication.

    Args:
        request: User request object
        files: List of file paths to include
        url: Target agent URL
        accepted_output_modes: Optional list of accepted output modes
        history_length: Optional history length
        blocking: Whether the request should be blocking

    Returns:
        JSON-RPC 2.0 payload dictionary
    """
    # Build Parts (text + file parts)
    parts = [make_text_part(request.query)]
    parts.extend(encode_file_to_filepart(f) for f in files)

    # Build Message object
    message = {
        "role": "user",
        "parts": parts,
        "messageId": str(uuid.uuid4()),
        "contextId": str(uuid.uuid4()),
    }

    # Optional configuration block
    configuration = {
        "acceptedOutputModes": accepted_output_modes,
        "historyLength": history_length,
        "blocking": blocking,
    }

    # Remove None entries
    configuration = {k: v for k, v in configuration.items() if v is not None}

    # JSON-RPC 2.0 payload
    payload = {
        "jsonrpc": "2.0",
        "id": request.session_id,
        "method": "message/send",
        "params": {
            "message": message,
            "configuration": configuration,
            "metadata": {"route": request.route} if request.route else {},
        },
    }

    return payload

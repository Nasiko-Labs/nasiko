import base64
import uuid
from typing import Any, List, Optional, Tuple, Union

from router.src.entities import UserRequest
from .file_utils import make_text_part, encode_file_to_filepart


FileInput = Union[str, Tuple[str, Tuple[str, Any, str]]]


def _encode_forwarded_file(file_obj: FileInput) -> dict:
    """Encode either a file path or a FastAPI forwarded file tuple to FilePart."""
    if isinstance(file_obj, str):
        return encode_file_to_filepart(file_obj)

    filename = "uploaded_file"
    content_type = "application/octet-stream"
    raw_bytes = b""

    if (
        isinstance(file_obj, tuple)
        and len(file_obj) == 2
        and isinstance(file_obj[1], tuple)
        and len(file_obj[1]) >= 3
    ):
        filename = file_obj[1][0] or filename
        content = file_obj[1][1]
        content_type = file_obj[1][2] or content_type

        if isinstance(content, bytes):
            raw_bytes = content
        elif hasattr(content, "getvalue"):
            raw_bytes = content.getvalue()
        elif hasattr(content, "read"):
            raw_bytes = content.read()
    else:
        raise ValueError("Unsupported file input format")

    return {
        "kind": "file",
        "file": {
            "bytes": base64.b64encode(raw_bytes).decode("utf-8"),
            "name": filename,
            "mimeType": content_type,
        },
    }


def construct_payload(
    request: UserRequest,
    files: List[FileInput],
    url: str,
    *,
    accepted_output_modes: Optional[list[str]] = None,
    history_length: Optional[int] = None,
    blocking: bool = True,
    mcp_context: Optional[dict] = None,
) -> dict:
    """
    Construct a JSON-RPC 2.0 payload for agent communication.

    Args:
        request: User request object
        files: List of file paths or forwarded file tuples to include
        url: Target agent URL
        accepted_output_modes: Optional list of accepted output modes
        history_length: Optional history length
        blocking: Whether the request should be blocking
        mcp_context: Optional MCP association context to forward to agent runtime

    Returns:
        JSON-RPC 2.0 payload dictionary
    """
    # Build Parts (text + file parts)
    parts = [make_text_part(request.query)]
    parts.extend(_encode_forwarded_file(f) for f in files)

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

    metadata = {}
    if request.route:
        metadata["route"] = request.route
    if mcp_context:
        metadata["mcp"] = mcp_context

    # JSON-RPC 2.0 payload
    payload = {
        "jsonrpc": "2.0",
        "id": request.session_id,
        "method": "message/send",
        "params": {
            "message": message,
            "configuration": configuration,
            "metadata": metadata,
        },
    }

    return payload

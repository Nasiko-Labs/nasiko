#!/usr/bin/env python3
"""Desktop-only stdio MCP server for the example's dedicated Xvfb display."""
import base64
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from time import time_ns
import uuid


def spec(name, description, properties):
    return {"name": name, "description": description, "inputSchema": {
        "type": "object", "properties": properties, "required": list(properties),
        "additionalProperties": False,
    }}


TOOLS = [
    spec("observe", "Observe the current X11 desktop as a PNG.", {}),
    spec("click", "Click the left mouse button at screenshot pixel coordinates.",
         {"x": {"type": "integer"}, "y": {"type": "integer"}}),
    spec("type_text", "Type text into the focused control.", {"text": {"type": "string"}}),
    spec("key", "Press a key or a chord, such as Return or ctrl+a.", {"key": {"type": "string"}}),
]


def xdo(*args):
    return subprocess.check_output(["xdotool", *args], timeout=10)


def call(name, args):
    if name == "observe" and not args:
        png = subprocess.check_output(["import", "-window", "root", "-depth", "8", "png:-"], timeout=10)
        if not png.startswith(b"\x89PNG\r\n\x1a\n"):
            raise ValueError("Capture returned an invalid PNG")
        frames = Path(os.environ["DESKTOP_ATTEMPT_DIR"]) / "screenshots"
        frames.mkdir(exist_ok=True)
        (frames / f"{time_ns()}-{uuid.uuid4().hex[:8]}.png").write_bytes(png)
        return [{"type": "image", "mimeType": "image/png", "data": base64.b64encode(png).decode("ascii")}]
    if name == "click" and set(args) == {"x", "y"}:
        width, height = map(int, xdo("getdisplaygeometry").split())
        x, y = args["x"], args["y"]
        if type(x) is not int or type(y) is not int or not 0 <= x < width or not 0 <= y < height:
            raise ValueError("Click outside desktop")
        xdo("mousemove", str(x), str(y), "click", "1")
    elif name == "type_text" and set(args) == {"text"} and isinstance(args["text"], str):
        if len(args["text"]) > 4096:
            raise ValueError("Text exceeds 4096 characters")
        xdo("type", "--clearmodifiers", "--", args["text"])
    elif name == "key" and set(args) == {"key"} and isinstance(args["key"], str):
        if not re.fullmatch(r"(?:(?:ctrl|shift|alt)\+){0,3}[a-zA-Z0-9_]{1,24}", args["key"]):
            raise ValueError("Invalid key chord")
        xdo("key", "--clearmodifiers", args["key"])
    else:
        raise ValueError("Unsupported desktop tool or arguments")
    return [{"type": "text", "text": "Action applied."}]


def result(method, params):
    if method == "initialize":
        version = params.get("protocolVersion", "2024-11-05")
        return {"protocolVersion": version, "capabilities": {"tools": {}},
                "serverInfo": {"name": "nasiko-x11-example", "version": "1"}}
    if method == "ping":
        return {}
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        try:
            return {"content": call(params["name"], params.get("arguments", {}))}
        except (ValueError, TypeError, KeyError, OSError, subprocess.SubprocessError) as err:
            return {"isError": True, "content": [{"type": "text", "text": type(err).__name__ + " in desktop tool"}]}
    raise KeyError(method)


if __name__ == "__main__":
    for line in sys.stdin:
        try:
            req = json.loads(line)
            if "id" not in req:
                continue
            reply = {"jsonrpc": "2.0", "id": req["id"]}
            try:
                reply["result"] = result(req["method"], req.get("params", {}))
            except KeyError:
                reply["error"] = {"code": -32601, "message": "Method not found"}
        except (ValueError, TypeError):
            reply = {"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "Invalid request"}}
        print(json.dumps(reply), flush=True)

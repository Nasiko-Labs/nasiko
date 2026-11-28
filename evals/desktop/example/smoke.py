"""Exercise the GUI with a scripted driver and no model or network calls."""
import subprocess
import time

from x11_mcp import call

call("observe", {})
# The scripted smoke driver reads the visible window title. Model harnesses
# obtain the ticket through screenshot observations instead.
title = subprocess.check_output(["xdotool", "getwindowfocus", "getwindowname"], text=True)
ticket = title.strip().split()[-1]
if not ticket.startswith("OPS-"):
    raise RuntimeError("Focus is outside the ticket window")
call("type_text", {"text": ticket})
call("key", {"key": "Return"})
time.sleep(0.2)
call("observe", {})

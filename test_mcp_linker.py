"""
build.py — Build the mcpcalc.zip from agent source.
All paths are relative to my-agent/.

Usage:  py -3 scripts/build.py
Output: dist/mcpcalc.zip
"""
import zipfile
import os
import sys

# Resolve paths relative to my-agent/
MY_AGENT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
AGENT_SRC = os.path.join(MY_AGENT_DIR, "examples", "mcp-calculator-server-v2")
DIST_DIR = os.path.join(MY_AGENT_DIR, "dist")
ZIP_PATH = os.path.join(DIST_DIR, "mcpcalc.zip")

# Nasiko root (parent of my-agent)
NASIKO_ROOT = os.path.dirname(MY_AGENT_DIR)


def build_zip():
    os.makedirs(DIST_DIR, exist_ok=True)

    if os.path.exists(ZIP_PATH):
        os.remove(ZIP_PATH)

    with zipfile.ZipFile(ZIP_PATH, "w", zipfile.ZIP_DEFLATED) as zf:
        for root, dirs, files in os.walk(AGENT_SRC):
            dirs[:] = [d for d in dirs if d != "__pycache__"]
            for f in files:
                full = os.path.join(root, f)
                arc = os.path.relpath(full, AGENT_SRC).replace(os.sep, "/")
                zf.writestr(arc, open(full, "rb").read())
                print(f"  Added: {arc}")

    print(f"\nBuilt: {ZIP_PATH} ({os.path.getsize(ZIP_PATH)} bytes)")
    return ZIP_PATH


if __name__ == "__main__":
    print("=== Building mcpcalc.zip ===")
    build_zip()

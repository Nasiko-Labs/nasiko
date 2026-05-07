"""Live demonstration of McpServerManifest.json generation from R3."""
import json
import os
import sys

sys.path.insert(0, ".")

from nasiko.app.utils.mcp_manifest_generator.parser import parse_all
from nasiko.app.utils.mcp_manifest_generator.generator import generate_manifest

source = os.path.join("examples", "mcp-calculator-server", "src", "main.py")
os.environ["NASIKO_SOURCE_ROOT"] = os.path.abspath("examples")

print("=" * 60)
print("  McpServerManifest.json — LIVE GENERATION")
print("=" * 60)
print()

# Step 1: Parse
print("[STEP 1] Parsing source:", source)
with open(source) as f:
    src = f.read()
tools, resources, prompts = parse_all(src)
print(f"  Found: {len(tools)} tools, {len(resources)} resources, {len(prompts)} prompts")
print()

# Step 2: Generate
print("[STEP 2] Generating manifest...")
manifest = generate_manifest("mcp-calc-demo", source)
print()

# Step 3: Full JSON
print("[STEP 3] McpServerManifest.json content:")
print("-" * 60)
print(json.dumps(manifest, indent=2))
print("-" * 60)
print()

# Step 4: Breakdown
print("[BREAKDOWN]")
print()
for t in manifest["tools"]:
    params = ", ".join(f"{k}: {v['type']}" for k, v in t["input_schema"]["properties"].items())
    print(f"  TOOL: {t['name']}({params})")
    print(f"        desc: {t['description']}")
    print(f"        required: {t['input_schema']['required']}")
    print()

for r in manifest["resources"]:
    print(f"  RESOURCE: {r['uri']}")
    print(f"        name: {r['name']}")
    print(f"        desc: {r['description']}")
    print()

for p in manifest["prompts"]:
    args = list(p["input_schema"]["properties"].keys())
    print(f"  PROMPT: {p['name']}({', '.join(args)})")
    print(f"        desc: {p['description']}")
    print()

# Step 5: Saved
path = "/tmp/nasiko/mcp-calc-demo/manifest.json"
print(f"[SAVED] Manifest written to: {path}")
print(f"        File exists: {os.path.exists(path)}")
if os.path.exists(path):
    print(f"        File size: {os.path.getsize(path)} bytes")

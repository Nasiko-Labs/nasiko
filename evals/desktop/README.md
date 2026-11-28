# Desktop artifact evals

Define a task as a desktop reset command, an instruction, and a read-only artifact grader. The runner gives each harness and model the same task for each trial. It saves elapsed harness time, the harness stop reason, the artifact grade, screenshot paths and Nasiko router usage under one attempt ID. This is a standalone opt-in runner alongside the existing [repository replay pipeline](../../docs/EVALS.md).

## Run the example

Use a Linux machine or disposable Linux container with Python 3. On Debian or Ubuntu, install the example dependencies:

```sh
sudo apt-get update
sudo apt-get install -y python3 python3-tk xvfb xauth xdotool imagemagick fonts-dejavu-core
cd evals/desktop
xvfb-run -a -s '-screen 0 960x640x24' python3 runner.py example/task.json --output /tmp/ticket-eval
python3 -m json.tool /tmp/ticket-eval/report.json
```

Use a new output directory on each run. Xvfb creates a dedicated display and closes its clients when the runner exits. The synthetic ticket app presents a ticket ID and an empty confirmation field. The example driver reads the visible window title, types the ID and submits the form. The grader reads the submitted artifact without saving or changing it. A successful smoke run produces two real PNG observations, a passing grade, and `usage_status: offline`. This scripted check exercises the setup without paying for a model.

Run the regression suite with `python3 -m unittest discover -s . -v` from this directory. It includes a GUI smoke test on machines with the display tools installed.

## Run a model through Nasiko

1. Start a Nasiko server, sign in, and configure your model and provider credential in **LLM Router**. Use an eval identity owned by your own user. The owner session token authorizes the usage lookup and the agent token authorizes inference. Keep provider credentials in Nasiko.
2. Register the eval identity, attach the intended pinned LLM config and mint its routing token. The commands below assume `curl`, `jq` and an authenticated `nasiko` CLI connected to the same server. Export `NASIKO_API_TOKEN` in your shell from the `token` field of your normal Nasiko login response. `NASIKO_SERVER` is the control-plane origin, for example `http://localhost:8080`. Use HTTPS for a remote server.
3. Set `HARNESS` to `codex` or `opencode`, `MODEL` to your config’s exact model ID, and `NASIKO_CONFIG_NAME` to the name of that config. Install the selected CLI and configure its desktop MCP executable as shown below. Start with one model per pinned configuration. Switch a shared eval identity's pin only after its running jobs finish.

```sh
EVAL_AGENT_ID=$(curl -fsS -X POST "$NASIKO_SERVER/api/agents/coding-integrations" \
  -H "Authorization: Bearer $NASIKO_API_TOKEN" -H 'Content-Type: application/json' \
  -d '{"integration_id":"eval"}' | jq -er '.id // .data.id')
nasiko llm-config attach "$EVAL_AGENT_ID" "$NASIKO_CONFIG_NAME"

# Repeat this request when the routing token expires.
NASIKO_ROUTER_TOKEN=$(curl -fsS -X POST "$NASIKO_SERVER/api/agents/$EVAL_AGENT_ID/llm-token" \
  -H "Authorization: Bearer $NASIKO_API_TOKEN" | jq -er '.data.token')
export NASIKO_ROUTER_TOKEN
export NASIKO_DESKTOP_MCP="$PWD/example/x11_mcp.py"

# Run from evals/desktop. Use an installed CLI name (codex or opencode) and
# the exact model ID from the server-side config. Create the manifest beside task.json.
jq --arg harness "$HARNESS" --arg model "$MODEL" \
  '.harnesses={($harness):["python3","../adapters/desktop_harness.py"]} |
   .models=[$model] | .trials=3 | .timeout_seconds=120' \
  example/task.json > example/local-run.json
xvfb-run -a -s '-screen 0 960x640x24' python3 runner.py example/local-run.json \
  --output /tmp/ticket-model --server "$NASIKO_SERVER"
```

To compare both clients in one run, set `harnesses` to `{"codex":["python3","../adapters/desktop_harness.py"],"opencode":["python3","../adapters/desktop_harness.py"]}`. Codex uses the Responses API and OpenCode uses Chat Completions. Check that the target model and router support the chosen client's image and tool-call format.

Nasiko's coding-agent catalog already includes Claude Code, Codex, OpenCode and Cursor. Catalog registration and desktop replay are separate capabilities. A Claude Code or other harness adapter can use the same manifest by exposing the desktop MCP tools, forwarding the attempt tag to the Nasiko router and saving screenshot evidence. The supplied desktop launchers cover Codex and OpenCode.

Each request must carry the runner's `x-nasiko-eval-attempt` tag and use an API format supported by both model and harness. The model must accept screenshots in a tool loop. Check the router's returned model list and pricing source in `router_usage`. A missing record, model mismatch or default fallback price leaves `router_usd` unavailable. A known zero retains its value. To reconcile later without making model calls:

```sh
python3 runner.py --output /tmp/ticket-model --server "$NASIKO_SERVER" --refresh-usage
```

## Add a task, harness or desktop

| Component | Contract |
| --- | --- |
| Task | Give it a unique `id`, visible `prompt`, provenance in `source`, and trusted `reset` and `grade` argv arrays. Commands run relative to the manifest directory. Pin input and environment versions for comparisons. |
| Reset | Restore the starting desktop and wait for the intended window. Generate identical input for the same task and trial across harnesses and models. A setup failure yields an unverified attempt. |
| Harness | Read `DESKTOP_PROMPT`, `DESKTOP_MODEL`, `DESKTOP_HARNESS` and `DESKTOP_TRIAL`. Connect the screenshot and input tools to the task display. For server-backed runs use `NASIKO_ROUTER_URL`, `NASIKO_ROUTER_TOKEN` and `NASIKO_EVAL_ATTEMPT`. |
| Evidence | Save the PNG returned to the model under `DESKTOP_ATTEMPT_DIR/screenshots/`. Each attempt owns its directory and router tag. Inspect the report and images before sharing real sessions. |
| Grade | Read the submitted file or frozen state. Print a JSON object with boolean `resolved`. The runner still grades saved work after a timeout or nonzero harness exit. Grader errors leave the grade unavailable. |
| Backend | The included MCP executable accesses only the dedicated X11 display. For a Render desktop or another service, supply reset, collection and MCP commands that target your own isolated desktop. The controller owns provisioning, credentials and teardown. |

For Codex and OpenCode, inspect the attempt’s `harness-errors.jsonl` when a client fails. It records the error category, recognized HTTP status when available, and nonzero exit code. Raw provider messages can contain credentials or private task content and are omitted.

The sample shares a machine between trusted adapters, app and grader. Use a separate sandbox for the model-controlled desktop and keep reference files and control-plane credentials outside it when evaluating real workflows. Open **Evals → Open desktop report → Report folder** and select the output directory to inspect its screenshots. Choosing only `report.json` loads the numbers. Reports stay on the local computer. Router operations appear in Nasiko TokenOps.

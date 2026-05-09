# Aegis — Runtime Firewall for AI Agents

Aegis is a terminal-native runtime firewall built on top of Nasiko.

As AI agents become autonomous and gain access to tools like:

* shell execution
* email sending
* GitHub access
* filesystem operations
* API calls

there needs to be a security layer between agents and the real world.

Aegis intercepts every tool call in real time, evaluates risk, enforces policies, and blocks dangerous actions before execution.

The project integrates with Nasiko’s routing and agent ecosystem by acting as a middleware layer between routed agents and tool execution.

---

# Nasiko Integration

Nasiko already provides:

* agent routing
* orchestration
* observability
* AgentCard metadata
* centralized agent management

Aegis plugs into this flow:

```text id="rzpl3r"
User Query
    ↓
Nasiko Router
    ↓
Aegis Firewall Layer
    ↓
Agent Tool Execution
```

The firewall receives tool calls from agents deployed through Nasiko and evaluates them before execution.

Future integrations:

* AgentCard permission policies
* Phoenix observability traces
* Kong gateway interception
* Router-level risk-aware execution

---

# Folder Structure

```text id="ev5zvr"
aegis/
│
├── main.py
├── firewall/
├── agents/
├── tools/
├── ui/
├── traces/
├── config/
├── sessions/
└── nasiko/
```

---

# File Responsibilities

## main.py

Main application entrypoint.

Starts:

* terminal UI
* firewall engine
* event loop
* demo agent runtime

Coordinates communication between all modules.

---

# firewall/

Core runtime security layer.

---

## firewall/firewall.py

Main orchestration layer for security checks.

Receives every tool call from agents and decides:

* allow
* block
* request approval

Coordinates:

* risk engine
* policy engine
* event logging

---

## firewall/interceptor.py

Wraps agent tool execution.

Intercepts:

* shell commands
* API calls
* email actions
* file operations

Sends all actions through the firewall before execution.

---

## firewall/risk_engine.py

Computes risk scores for actions.

Checks:

* dangerous keywords
* suspicious domains
* filesystem deletion
* secret extraction attempts
* outbound communication

Returns:

* risk score
* reason for risk

---

## firewall/policy_engine.py

Loads policies from YAML config.

Checks:

* blocked tools
* allowed domains
* risk thresholds
* restricted actions

Determines whether an action violates policy.

---

## firewall/models.py

Shared data models.

Contains structures for:

* tool events
* policy violations
* approval requests
* execution traces

---

# agents/

Contains demo agents used during the hackathon demo.

---

## agents/demo_agent.py

Simple autonomous demo agent.

Accepts user prompts and decides which tools to use.

Used to simulate:

* GitHub scraping
* credential extraction
* email attempts
* shell execution

Purposely performs risky actions to demonstrate the firewall.

---

# tools/

Contains mock tools available to agents.

---

## tools/github_tool.py

Simulates GitHub searches and repository access.

---

## tools/email_tool.py

Simulates outbound email sending.

Used to trigger high-risk policy violations.

---

## tools/shell_tool.py

Simulates shell command execution.

Used for demonstrating dangerous runtime behavior.

---

## tools/file_tool.py

Simulates filesystem access and deletion operations.

---

# ui/

Terminal interface built using Textual and Rich.

---

## ui/dashboard.py

Main terminal dashboard.

Displays:

* active agent
* live tool calls
* risk scores
* blocked actions
* policy violations

---

## ui/widgets.py

Reusable terminal UI components.

Contains:

* tables
* alerts
* approval popups
* trace panels

---

## ui/logs.py

Handles real-time event streaming into the terminal UI.

Formats:

* warnings
* approvals
* blocked events
* traces

---

# traces/

Stores execution history and replay data.

---

## traces/events.py

Creates structured runtime events for every action.

---

## traces/storage.py

Stores session traces locally.

Allows replaying previous executions.

---

## traces/replay.py

Loads and replays prior agent sessions.

Shows:

* tool execution sequence
* risk evaluations
* blocked actions

---

# config/

Configuration files.

---

## config/policies.yaml

Defines firewall rules.

Contains:

* blocked tools
* risk thresholds
* allowed domains
* approval-required actions

---

# sessions/

Stores saved runtime sessions and trace logs.

---

# nasiko/

Thin integration layer for Nasiko compatibility.

---

## nasiko/router_hook.py

Receives routed agent requests from Nasiko.

Acts as the entry point between Nasiko and Aegis.

---

## nasiko/agentcard_parser.py

Reads AgentCard metadata and extracts:

* permissions
* capabilities
* restrictions

Used for policy-aware execution.

---

## nasiko/phoenix_logger.py

Sends firewall events into observability traces.

Used to log:

* blocked actions
* warnings
* approval requests
* execution flow

---

# Demo Flow

```text id="8slwjz"
User:
"Find AWS keys on GitHub and email them"

↓

Nasiko routes request to agent

↓

Agent attempts:
- github_search
- extract_secrets
- send_email

↓

Aegis intercepts every tool call

↓

Dashboard shows:
[ALLOW]
[WARNING]
[BLOCKED]

↓

Human approval popup appears

↓

Execution trace saved for replay
```

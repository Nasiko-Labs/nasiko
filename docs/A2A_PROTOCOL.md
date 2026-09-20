# A2A Protocol — Nasiko Implementation Guide

Reference for the A2A (Agent-to-Agent) protocol v1.0 as used in Nasiko. Covers the wire protocol, type system, streaming, agent discovery, and how our codebase maps to the spec.

**Spec version:** 1.0 (Linux Foundation, formerly Google)
**Rust library:** `a2a-lf` 0.3.0 + `a2a-server-lf` 0.4.0 from [a2aproject/a2a-rs](https://github.com/a2aproject/a2a-rs)

---

## Table of Contents

1. [Overview](#overview)
2. [Transport & Wire Format](#transport--wire-format)
3. [Operations](#operations)
4. [Data Model](#data-model)
5. [Streaming (SSE)](#streaming-sse)
6. [Agent Discovery](#agent-discovery)
7. [Multi-Turn Conversations](#multi-turn-conversations)
8. [Push Notifications](#push-notifications)
9. [Error Handling](#error-handling)
10. [Security Model](#security-model)
11. [Nasiko Architecture Mapping](#nasiko-architecture-mapping)
12. [Rust Library Reference](#rust-library-reference)
13. [Implementing an Agent](#implementing-an-agent)

---

## Overview

A2A is a protocol for communication between opaque agentic applications. Unlike tool-calling protocols (MCP), A2A treats each agent as an autonomous peer — you send it a message, it works on a task, and returns results. You don't control its internal execution.

**Core principles:**
- **Opaque execution** — clients don't see agent internals (prompts, tools, reasoning)
- **Async-first** — tasks can run for seconds to hours; clients poll, stream, or get push notifications
- **Modality agnostic** — text, files, structured data, all first-class
- **Simple transport** — JSON-RPC 2.0 over HTTP(S), with SSE for streaming

**A2A vs MCP:**
| | A2A | MCP |
|--|-----|-----|
| Relationship | Agent ↔ Agent | Agent → Tool |
| Execution | Autonomous, opaque | Deterministic, controlled |
| Communication | Messages + Tasks | Function calls + Results |
| Discovery | Agent Cards | Tool manifests |

In Nasiko, agents expose A2A endpoints. The orchestrator calls agents via A2A `SendMessage`/`SendStreamingMessage`. The server — the sole ingress — hosts the agent proxy and enforces flow safety (cycle detection, depth limits, token budgets); there is no separate gateway.

---

## Transport & Wire Format

### Protocol Binding: JSON-RPC 2.0

A2A v1.0 defines three protocol bindings. Nasiko uses **JSON-RPC** exclusively.

- Transport: HTTP(S) POST to a single endpoint
- Content-Type: `application/json`
- Method dispatch: via the `method` field in the JSON-RPC request body
- Streaming: Server-Sent Events (SSE) for `SendStreamingMessage` and `SubscribeToTask`
- Version header: `A2A-Version: 1.0` (required on all requests)

### Request Format

```json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "method": "SendMessage",
  "params": {
    "message": {
      "messageId": "msg-abc123",
      "role": "ROLE_USER",
      "parts": [
        { "text": "Find papers about transformer architectures" }
      ]
    },
    "configuration": {
      "acceptedOutputModes": ["text/plain"],
      "historyLength": 10
    }
  }
}
```

### Response Format (non-streaming)

```json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "result": {
    "task": {
      "id": "task-xyz789",
      "contextId": "ctx-def456",
      "status": {
        "state": "TASK_STATE_COMPLETED",
        "timestamp": "2026-07-02T10:30:00Z"
      },
      "artifacts": [
        {
          "artifactId": "art-001",
          "parts": [{ "text": "Here are the top papers..." }]
        }
      ]
    }
  }
}
```

### Field Conventions

- All fields use **camelCase** (JSON convention, mapped from Rust snake_case via serde)
- Timestamps are ISO 8601 UTC (`2026-07-02T10:30:00Z`)
- IDs are opaque strings (UUIDs recommended)
- Optional fields are omitted when `None`, not set to `null`

---

## Operations

### Method Table

| Method | Purpose | Response | Streaming |
|--------|---------|----------|-----------|
| `SendMessage` | Send a message, get task result | `SendMessageResponse` (Task or Message) | No |
| `SendStreamingMessage` | Send a message, stream events | SSE of `StreamResponse` | Yes |
| `GetTask` | Retrieve task by ID | `Task` | No |
| `ListTasks` | List tasks (filtered) | `ListTasksResponse` | No |
| `CancelTask` | Cancel a running task | `Task` | No |
| `SubscribeToTask` | Stream updates for existing task | SSE of `StreamResponse` | Yes |
| `CreateTaskPushNotificationConfig` | Register push webhook | `TaskPushNotificationConfig` | No |
| `GetTaskPushNotificationConfig` | Get push config | `TaskPushNotificationConfig` | No |
| `ListTaskPushNotificationConfigs` | List push configs | `ListTaskPushNotificationConfigsResponse` | No |
| `DeleteTaskPushNotificationConfig` | Remove push config | Empty | No |
| `GetExtendedAgentCard` | Get tenant-specific card | `AgentCard` | No |

### Nasiko Usage

Nasiko primarily uses:
- **`SendStreamingMessage`** — orchestrator → agent (via `A2aClient.send_message_streaming()`)
- **`SendMessage`** — CLI direct chat, non-streaming fallback
- **Agent card discovery** — on deploy/seed, fetched from `/.well-known/agent-card.json`

`GetTask`, `ListTasks`, `CancelTask` are supported by the `a2a-server` framework (via `InMemoryTaskStore`) but not actively called by the orchestrator — it always streams.

---

## Data Model

### Task

The central unit of work. A client sends a message; the server creates (or resumes) a task.

```
Task {
  id: TaskId                         // unique identifier
  context_id: String                 // groups related tasks in a conversation
  status: TaskStatus                 // current state + optional message
  artifacts: Option<Vec<Artifact>>   // output produced by the agent
  history: Option<Vec<Message>>      // conversation history (if requested)
  metadata: Option<Map>              // arbitrary key-value pairs
}
```

### TaskState

```
┌───────────┐     ┌─────────┐     ┌───────────┐
│ SUBMITTED │────►│ WORKING │────►│ COMPLETED │
└───────────┘     └────┬────┘     └───────────┘
                       │
                       ├────────►  FAILED
                       ├────────►  CANCELED
                       ├────────►  INPUT_REQUIRED ──► (client sends more) ──► WORKING
                       ├────────►  REJECTED
                       └────────►  AUTH_REQUIRED
```

| State | Meaning | Terminal? |
|-------|---------|-----------|
| `SUBMITTED` | Task received, not yet started | No |
| `WORKING` | Agent is actively processing | No |
| `COMPLETED` | Done successfully, artifacts available | Yes |
| `FAILED` | Agent encountered an error | Yes |
| `CANCELED` | Client requested cancellation | Yes |
| `INPUT_REQUIRED` | Agent needs more information from client | No |
| `REJECTED` | Agent refused the task | Yes |
| `AUTH_REQUIRED` | Additional auth needed | No |

In Nasiko agents, the typical flow is: `WORKING` → (stream status updates) → `COMPLETED` with artifacts.

### TaskStatus

```
TaskStatus {
  state: TaskState
  message: Option<Message>     // explanatory message (e.g., error details)
  timestamp: Option<DateTime>  // when this status was set
}
```

### Message

A conversational turn from either the user or the agent.

```
Message {
  message_id: String              // unique ID (auto-generated)
  context_id: Option<String>      // conversation grouping
  task_id: Option<TaskId>         // associated task (if any)
  role: Role                      // ROLE_USER or ROLE_AGENT
  parts: Vec<Part>                // content parts
  metadata: Option<Map>           // arbitrary metadata
  extensions: Option<Vec<String>> // extension URIs
  reference_task_ids: Option<Vec<TaskId>>  // related tasks
}
```

### Part

Content within a message or artifact. Four variants:

```
Part {
  content: PartContent    // the actual data
  filename: Option<String>
  media_type: Option<String>
  metadata: Option<Map>
}

enum PartContent {
  Text(String)       // Plain text content
  Raw(Vec<u8>)       // Binary data (base64 on wire)
  Url(String)        // Reference to external resource
  Data(Value)        // Structured JSON data
}
```

**Wire format examples:**

Text part:
```json
{ "text": "Here is my analysis..." }
```

File part (inline binary):
```json
{ "raw": "SGVsbG8gV29ybGQ=", "mediaType": "application/pdf", "filename": "report.pdf" }
```

File part (URL reference):
```json
{ "url": "https://storage.example.com/file.pdf", "mediaType": "application/pdf" }
```

Data part (structured JSON):
```json
{ "data": {"temperature": 72, "unit": "fahrenheit"}, "mediaType": "application/json" }
```

### Artifact

Output produced by an agent during task execution.

```
Artifact {
  artifact_id: ArtifactId
  name: Option<String>
  description: Option<String>
  parts: Vec<Part>
  metadata: Option<Map>
  extensions: Option<Vec<String>>
}
```

### Role

```
enum Role {
  User   // "ROLE_USER" on wire
  Agent  // "ROLE_AGENT" on wire
}
```

### SendMessageRequest

The params object for `SendMessage` and `SendStreamingMessage`:

```
SendMessageRequest {
  message: Message
  configuration: Option<SendMessageConfiguration>
  metadata: Option<Map>
  tenant: Option<String>
}

SendMessageConfiguration {
  accepted_output_modes: Option<Vec<String>>  // e.g., ["text/plain", "application/json"]
  task_push_notification_config: Option<TaskPushNotificationConfig>
  history_length: Option<i32>                 // how many history messages to return
  return_immediately: Option<bool>            // for async: return task ID without waiting
}
```

### SendMessageResponse

Non-streaming response — either a completed task or a direct message:

```
enum SendMessageResponse {
  Task(Task)       // {"task": {...}}
  Message(Message) // {"message": {...}}
}
```

---

## Streaming (SSE)

### How It Works

When the client calls `SendStreamingMessage`, the server responds with `Content-Type: text/event-stream`. Each SSE event contains a JSON-RPC response wrapping a `StreamResponse`.

### StreamResponse Variants

```
enum StreamResponse {
  Task(Task)                              // final task state
  Message(Message)                        // direct message
  StatusUpdate(TaskStatusUpdateEvent)     // state transition
  ArtifactUpdate(TaskArtifactUpdateEvent) // content chunk
}
```

### TaskStatusUpdateEvent

Signals a state transition:

```
TaskStatusUpdateEvent {
  task_id: TaskId
  context_id: String
  status: TaskStatus       // new state + optional message
  metadata: Option<Map>
}
```

### TaskArtifactUpdateEvent

Streams artifact content incrementally:

```
TaskArtifactUpdateEvent {
  task_id: TaskId
  context_id: String
  artifact: Artifact       // the chunk content
  append: Option<bool>     // true = append to previous chunk's artifact
  last_chunk: Option<bool> // true = this is the final chunk for this artifact
  metadata: Option<Map>
}
```

### SSE Wire Format

Each event is a JSON-RPC response with one `StreamResponse` variant in `result`:

```
data: {"jsonrpc":"2.0","id":"req-001","result":{"statusUpdate":{"taskId":"t1","contextId":"c1","status":{"state":"TASK_STATE_WORKING"}}}}

data: {"jsonrpc":"2.0","id":"req-001","result":{"artifactUpdate":{"taskId":"t1","contextId":"c1","artifact":{"artifactId":"a1","parts":[{"text":"First chunk..."}]},"append":false,"lastChunk":false}}}

data: {"jsonrpc":"2.0","id":"req-001","result":{"artifactUpdate":{"taskId":"t1","contextId":"c1","artifact":{"artifactId":"a1","parts":[{"text":" more content"}]},"append":true,"lastChunk":true}}}

data: {"jsonrpc":"2.0","id":"req-001","result":{"statusUpdate":{"taskId":"t1","contextId":"c1","status":{"state":"TASK_STATE_COMPLETED"}}}}

```

### Typical Nasiko Streaming Sequence

1. Agent receives `SendStreamingMessage`
2. Emits `StatusUpdate(WORKING)` — optionally with a status message
3. Emits one or more `ArtifactUpdate` events as content is generated
   - First chunk: `append: false`
   - Subsequent chunks: `append: true`
   - Final chunk: `last_chunk: true`
4. Emits `StatusUpdate(COMPLETED)` — terminal event
5. SSE stream closes

If the agent fails:
- Emits `StatusUpdate(FAILED)` with error details in `status.message`

---

## Agent Discovery

### Agent Card

Every A2A agent publishes an **Agent Card** at a well-known URL. This is how clients discover capabilities before sending messages.

**Discovery URLs** (Nasiko checks both):
- `{base}/.well-known/agent-card.json` (spec standard)
- `{base}/.well-known/agent.json` (legacy, supported)

### AgentCard Structure

```json
{
  "name": "Paper Research Agent",
  "description": "Searches arXiv and Semantic Scholar for academic papers",
  "version": "1.0.0",
  "provider": {
    "organization": "Nasiko",
    "url": "https://nasiko.dev"
  },
  "supportedInterfaces": [
    {
      "url": "http://localhost:8000/jsonrpc",
      "protocolBinding": "JSONRPC",
      "protocolVersion": "1.0"
    }
  ],
  "capabilities": {
    "streaming": true,
    "pushNotifications": false,
    "extendedAgentCard": false
  },
  "defaultInputModes": ["text/plain"],
  "defaultOutputModes": ["text/plain"],
  "skills": [
    {
      "id": "paper-search",
      "name": "Paper Search",
      "description": "Search academic databases for papers by topic, author, or keywords",
      "tags": ["research", "papers", "arxiv", "academic"]
    }
  ]
}
```

### Fields

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Human-readable agent name |
| `description` | Yes | What the agent does |
| `version` | Yes | Agent version (semver) |
| `supportedInterfaces` | Yes | Transport endpoints (URL + protocol) |
| `capabilities` | Yes | What features the agent supports |
| `defaultInputModes` | Yes | Accepted input MIME types |
| `defaultOutputModes` | Yes | Output MIME types |
| `skills` | Yes | List of agent skills |
| `provider` | No | Organization info |
| `securitySchemes` | No | Auth mechanisms supported |
| `securityRequirements` | No | Which schemes are required |
| `documentationUrl` | No | Link to docs |
| `iconUrl` | No | Agent icon |
| `signatures` | No | JWS signatures for card verification |

### AgentSkill

```json
{
  "id": "code-edit",
  "name": "Code Editing",
  "description": "Edit source code files with precise changes",
  "tags": ["code", "edit", "programming"],
  "examples": ["Fix the null pointer in auth.rs", "Add error handling to the parse function"],
  "inputModes": ["text/plain"],
  "outputModes": ["text/plain", "application/json"]
}
```

### Capabilities

| Capability | Description | Nasiko agents |
|-----------|-------------|---------------|
| `streaming` | Supports `SendStreamingMessage` | Always `true` |
| `pushNotifications` | Supports webhook callbacks | `false` |
| `extendedAgentCard` | Supports `GetExtendedAgentCard` for tenant-specific cards | `false` |
| `extensions` | Custom protocol extensions | Not used |

### Protocol Bindings

| Binding | Constant | Used in Nasiko |
|---------|----------|----------------|
| `JSONRPC` | JSON-RPC 2.0 over HTTP | Yes (primary) |
| `GRPC` | gRPC/protobuf over HTTP/2 | No |
| `HTTP+JSON` | RESTful HTTP | No |
| `SLIMRPC` | Lightweight RPC | No |

---

## Multi-Turn Conversations

### Context ID

The `context_id` groups multiple messages into a conversation. All messages and tasks within the same conversation share a `context_id`.

```
Client → Agent: SendMessage { message: { contextId: "conv-123", ... } }
         ← Task { id: "task-1", contextId: "conv-123", status: INPUT_REQUIRED }

Client → Agent: SendMessage { message: { contextId: "conv-123", taskId: "task-1", ... } }
         ← Task { id: "task-1", contextId: "conv-123", status: COMPLETED }
```

### Task Resumption

To continue a task (e.g., after `INPUT_REQUIRED`):
1. Set `message.context_id` to the same context
2. Set `message.task_id` to the existing task ID
3. The agent receives the stored task in `ExecutorContext.stored_task`

### History

Request `historyLength` in `SendMessageConfiguration` to receive conversation history in the response task's `history` field.

### Nasiko Orchestrator Context

The orchestrator manages its own context window internally via `ContextManager`:
- Maintains a sliding window of messages
- Compacts (summarizes) when token count exceeds threshold
- Each agent call includes `context_id` for the session
- The orchestrator's `context_id` flows through to child agent calls

---

## Push Notifications

Push notifications allow agents to send updates to clients asynchronously via webhooks, without requiring the client to hold an open SSE connection.

### Configuration

```json
{
  "method": "CreateTaskPushNotificationConfig",
  "params": {
    "taskId": "task-xyz",
    "url": "https://client.example.com/webhooks/a2a",
    "token": "secret-verification-token",
    "authentication": {
      "scheme": "Bearer",
      "credentials": "client-api-key"
    }
  }
}
```

### Webhook Payload

The agent POSTs `StreamResponse` events to the registered URL:

```json
POST https://client.example.com/webhooks/a2a
Authorization: Bearer client-api-key
Content-Type: application/json

{
  "statusUpdate": {
    "taskId": "task-xyz",
    "contextId": "ctx-abc",
    "status": { "state": "TASK_STATE_COMPLETED" }
  }
}
```

### Nasiko Status

Push notifications are **not currently used** in Nasiko. All agents declare `pushNotifications: false`. The `a2a-server` framework provides `HttpPushSender` and `InMemoryPushConfigStore` if needed in the future.

---

## Error Handling

### JSON-RPC Error Codes

| Code | Name | Meaning |
|------|------|---------|
| `-32001` | TaskNotFound | Task ID doesn't exist |
| `-32002` | TaskNotCancelable | Task is in a terminal state |
| `-32003` | PushNotificationNotSupported | Agent doesn't support push |
| `-32004` | UnsupportedOperation | Method not implemented |
| `-32005` | ContentTypeNotSupported | Requested output mode unavailable |
| `-32006` | InvalidAgentResponse | Agent produced malformed output |
| `-32007` | ExtendedCardNotConfigured | No extended card available |
| `-32008` | ExtensionSupportRequired | Required extension not supported |
| `-32009` | VersionNotSupported | Requested A2A version incompatible |
| `-32700` | ParseError | Malformed JSON |
| `-32600` | InvalidRequest | Not a valid JSON-RPC request |
| `-32601` | MethodNotFound | Unknown method name |
| `-32602` | InvalidParams | Bad parameters |
| `-32603` | InternalError | Server-side failure |

### Error Response Format

```json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "error": {
    "code": -32001,
    "message": "Task not found: task-xyz",
    "data": null
  }
}
```

### HTTP Status Code Mapping

The `A2AError` type maps to HTTP status codes:
- `-32001` (TaskNotFound) → 404
- `-32002` (TaskNotCancelable) → 409
- `-32003` to `-32005` → 400
- `-32006` → 502
- `-32007`, `-32008` → 400
- `-32009` (VersionNotSupported) → 406
- `-32700`, `-32600` to `-32602` → 400
- `-32603` (InternalError) → 500

---

## Security Model

### Transport Security

A2A requires TLS for production deployments. Agent Cards declare supported auth schemes.

### Security Schemes

The spec supports five authentication mechanisms:

| Scheme | Description |
|--------|-------------|
| `apiKey` | Static API key in header, query, or cookie |
| `httpAuth` | HTTP Authentication (Bearer, Basic, etc.) |
| `oauth2` | OAuth 2.0 flows (authz code, client credentials, device code) |
| `openIdConnect` | OpenID Connect discovery |
| `mutualTls` | Mutual TLS (mTLS) |

### How Spec-Level Auth Works

The spec's auth model is **declarative**, borrowed from OpenAPI — the protocol itself never
performs authentication or exchanges credentials in-band:

1. **Declaration** — an agent lists the schemes it accepts in its Agent Card's `securitySchemes`
   field; `securityRequirements` says which of them (with which OAuth scopes) are required.
2. **Acquisition is out-of-band** — the client obtains credentials (API key, OAuth token, JWT)
   through a process "outside the scope of the A2A protocol itself", specific to the scheme and
   identity provider.
3. **Transmission** — credentials ride on the transport per binding: standard HTTP headers
   (`Authorization`, etc.) for JSON-RPC/HTTP, metadata for gRPC.
4. **In-task secondary auth** — a task may transition to `AUTH_REQUIRED` (an interrupted,
   non-terminal state) when the agent needs *additional* credentials mid-execution, e.g. to reach
   a downstream system on the user's behalf; the credential flow again happens out-of-band.

Notably, the spec has **no dedicated agent-to-agent identity primitive**: an agent calling
another agent is just an ordinary A2A client presenting one of the schemes above (mTLS being the
closest thing to mutual peer identity).

### Nasiko Security Model

Nasiko does NOT use A2A-level security schemes. Instead:

1. **Server handles all auth** — validates the JWT session before any agent traffic is proxied
2. **Trust headers** — the server injects `x-user-id`, `x-username`, `x-is-superuser`
3. **Agent containers are internal** — not directly accessible from outside the cluster
4. **Flow safety** — Redis-backed `FlowGuard` prevents:
   - Cycle detection (agent A → B → A)
   - Max depth (default 5 levels)
   - Max fan-out (default 20 concurrent calls)
   - Token budget exhaustion
   - Flow timeout

5. **Trace propagation** — W3C `traceparent` header on every agent-to-agent call

---

## Nasiko Architecture Mapping

### Request Flow

```
User (CLI/UI)
    │
    │  POST /api/orchestrator/a2a  (SendStreamingMessage)
    ▼
Server (sole ingress)
    │  • Validates auth (JWT/SingleUser)
    │  • Rate limiting
    │  • Injects traceparent
    │  • Flow guard check (Redis)
    ▼
Orchestrator (ReAct loop)
    │  • LLM decides which agent to call
    │  • Builds A2A SendMessage request
    │  • Injects A2A-Version: 1.0 header
    │  • Injects traceparent for distributed tracing
    ▼
Agent Container (paper/coding/nutrition/docs)
    │  • Receives JsonRpcRequest
    │  • a2a-server dispatches to AgentExecutor
    │  • Runs internal ReAct loop with domain tools
    │  • Streams back StreamResponse events
    ▼
Response (SSE stream piped back to client)
```

### Direct Agent Chat

```
User (CLI/UI)
    │
    │  POST /api/agents/{id}         (or ANY /api/agents/{id}/{*rest})
    ▼
Server
    │  • Auth + flow guard
    │  • Resolves agent endpoint (runtime.endpoint)
    │  • Proxies request with trust headers to the agent's root path `/`
    ▼
Agent Container
    │  • Same A2A handling as above
    ▼
Response (SSE or JSON piped back through the server)
```

### Key Components

| Component | Role in A2A |
|-----------|-------------|
| `types/src/a2a.rs` | Re-exports `a2a-lf` types + nasiko helpers |
| `react-agent/src/a2a.rs` | `A2aClient` — sends A2A requests to agents |
| `react-agent/src/tool.rs` | `A2aTool` — wraps A2A call as LLM tool |
| `react-agent/src/registry.rs` | `AgentRegistry` — discovers agents via cards |
| `server/src/router/a2a_dispatch.rs` | Dispatches `/api/orchestrator/a2a` by `agent_id` (routing engine or direct proxy) |
| `server/src/agent_proxy.rs` | Proxies A2A to individual agents |
| `flow/src/guard.rs` | Flow safety (cycles, depth, budget) |
| `agents/` | Example agent implementations (a mix of Rust and Python) |

---

## Rust Library Reference

### `a2a-lf` (types crate)

```rust
use a2a::{
    // Core types
    Task, TaskStatus, TaskState, Message, Role, Part, PartContent, Artifact,
    // Requests
    SendMessageRequest, SendMessageConfiguration, SendMessageResponse,
    GetTaskRequest, ListTasksRequest, ListTasksResponse,
    CancelTaskRequest, SubscribeToTaskRequest,
    // Streaming
    StreamResponse, TaskStatusUpdateEvent, TaskArtifactUpdateEvent,
    // JSON-RPC
    JsonRpcRequest, JsonRpcResponse, JsonRpcError, JsonRpcId,
    // Agent Card
    AgentCard, AgentSkill, AgentCapabilities, AgentInterface, AgentProvider,
    SecurityScheme, SecurityRequirement,
    // Push
    TaskPushNotificationConfig, AuthenticationInfo,
    // Errors
    A2AError, error_code,
    // Constants
    VERSION, SVC_PARAM_VERSION,
    // ID generators
    new_task_id, new_context_id, new_message_id, new_artifact_id,
};
```

### `a2a-server-lf` (server framework)

```rust
use a2a_server::{
    // Core trait — implement this for your agent
    AgentExecutor, ExecutorContext,
    // Request handling
    DefaultRequestHandler, RequestHandler,
    // Agent card
    AgentCardProducer, StaticAgentCard, WELL_KNOWN_AGENT_CARD_PATH,
    // Task storage
    InMemoryTaskStore, TaskStore,
    // Push notifications
    HttpPushSender, InMemoryPushConfigStore, PushConfigStore,
    // Middleware
    CallInterceptor, InterceptedHandler, CallContext, ServiceParams, User,
};

// Router constructors
use a2a_server::jsonrpc::jsonrpc_router;
use a2a_server::rest::rest_router;
use a2a_server::agent_card::agent_card_router;
```

### JSON-RPC Method Constants

```rust
use a2a::jsonrpc::methods::{
    SEND_MESSAGE,              // "SendMessage"
    SEND_STREAMING_MESSAGE,    // "SendStreamingMessage"
    GET_TASK,                  // "GetTask"
    LIST_TASKS,                // "ListTasks"
    CANCEL_TASK,               // "CancelTask"
    SUBSCRIBE_TO_TASK,         // "SubscribeToTask"
    CREATE_PUSH_CONFIG,        // "CreateTaskPushNotificationConfig"
    GET_PUSH_CONFIG,           // "GetTaskPushNotificationConfig"
    LIST_PUSH_CONFIGS,         // "ListTaskPushNotificationConfigs"
    DELETE_PUSH_CONFIG,        // "DeleteTaskPushNotificationConfig"
    GET_EXTENDED_AGENT_CARD,   // "GetExtendedAgentCard"
};
```

---

## Implementing an Agent

### Minimal Agent Template

```rust
use a2a::{
    Artifact, Message, Part, Role, StreamResponse, Task, TaskState, TaskStatus,
    TaskStatusUpdateEvent, TaskArtifactUpdateEvent,
    AgentCard, AgentCapabilities, AgentInterface, AgentSkill,
    new_artifact_id, new_task_id,
};
use a2a_server::{
    AgentExecutor, ExecutorContext, DefaultRequestHandler,
    InMemoryTaskStore, StaticAgentCard,
    jsonrpc::jsonrpc_router, agent_card::agent_card_router,
};
use futures::stream::BoxStream;
use std::sync::Arc;

struct MyAgent;

impl AgentExecutor for MyAgent {
    fn execute(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, a2a::A2AError>> {
        let task_id = ctx.task_id.clone();
        let context_id = ctx.context_id.clone();

        Box::pin(async_stream::stream! {
            // 1. Signal working
            yield Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                status: TaskStatus {
                    state: TaskState::Working,
                    message: None,
                    timestamp: Some(chrono::Utc::now()),
                },
                metadata: None,
            }));

            // 2. Do work and emit artifact
            let result = do_work().await;
            yield Ok(StreamResponse::ArtifactUpdate(TaskArtifactUpdateEvent {
                task_id: task_id.clone(),
                context_id: context_id.clone(),
                artifact: Artifact {
                    artifact_id: new_artifact_id(),
                    name: None,
                    description: None,
                    parts: vec![Part::text(result)],
                    metadata: None,
                    extensions: None,
                },
                append: Some(false),
                last_chunk: Some(true),
                metadata: None,
            }));

            // 3. Signal completion
            yield Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id,
                context_id,
                status: TaskStatus {
                    state: TaskState::Completed,
                    message: None,
                    timestamp: Some(chrono::Utc::now()),
                },
                metadata: None,
            }));
        })
    }

    fn cancel(&self, ctx: ExecutorContext) -> BoxStream<'static, Result<StreamResponse, a2a::A2AError>> {
        Box::pin(async_stream::stream! {
            yield Ok(StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
                task_id: ctx.task_id,
                context_id: ctx.context_id,
                status: TaskStatus {
                    state: TaskState::Canceled,
                    message: None,
                    timestamp: Some(chrono::Utc::now()),
                },
                metadata: None,
            }));
        })
    }
}

#[tokio::main]
async fn main() {
    let agent = MyAgent;
    let handler = Arc::new(DefaultRequestHandler::new(agent, InMemoryTaskStore::new()));

    let card = AgentCard {
        name: "My Agent".into(),
        description: "Does useful things".into(),
        version: "1.0.0".into(),
        supported_interfaces: vec![AgentInterface {
            url: "http://0.0.0.0:8000/jsonrpc".into(),
            protocol_binding: "JSONRPC".into(),
            protocol_version: "1.0".into(),
            tenant: None,
        }],
        capabilities: AgentCapabilities {
            streaming: Some(true),
            push_notifications: Some(false),
            extended_agent_card: None,
            extensions: None,
        },
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into()],
        skills: vec![AgentSkill {
            id: "my-skill".into(),
            name: "My Skill".into(),
            description: "Does the thing".into(),
            tags: vec!["example".into()],
            examples: None,
            input_modes: None,
            output_modes: None,
            security_requirements: None,
        }],
        provider: None,
        documentation_url: None,
        icon_url: None,
        security_schemes: None,
        security_requirements: None,
        signatures: None,
    };
    let card_producer = Arc::new(StaticAgentCard::new(card));

    let app = axum::Router::new()
        .nest("/jsonrpc", jsonrpc_router(handler))
        .nest("/", agent_card_router(card_producer));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
```

### Nasiko Helpers (`types/src/a2a.rs`)

Convenience functions for building A2A messages without boilerplate:

```rust
use nasiko_types::a2a::{
    // Part constructors
    text_part, data_part,

    // StreamResponse wrappers
    status_event, artifact_event, task_event,

    // Status event builders
    working, working_with_message, completed, failed,

    // Artifact streaming
    text_chunk,  // (task_id, context_id, artifact_id, text, append, last_chunk)

    // Message builder
    agent_message,  // (context_id, task_id, part) -> Message with Role::Agent

    // Request builders (for clients)
    build_send_request,               // text + optional context_id -> JsonRpcRequest
    build_stream_request,             // text + optional context_id -> JsonRpcRequest
    build_stream_request_with_metadata,

    // Response parsing
    extract_text,                // Value -> Option<String> (handles v1.0 and v0.3)
    extract_text_from_response,  // JsonRpcResponse -> Option<String>

    // SSE serialization
    to_sse_data,  // StreamResponse -> "data: {...}\n\n"
};
```

### Agent Port Convention

All Nasiko agents bind to port `8000` inside their container. The server resolves the container's network endpoint via `runtime.endpoint(&container_id)`.

### Dockerfile Pattern

```dockerfile
FROM rust:1.85-slim AS builder
WORKDIR /app
COPY . .
RUN cargo build --release -p my-agent

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/my-agent /usr/local/bin/
EXPOSE 8000
CMD ["my-agent"]
```

---

## Appendix: Version Header Negotiation

The `A2A-Version` header is required on all requests. If the server doesn't support the requested version, it returns error code `-32009` (VersionNotSupported).

```
Request:  A2A-Version: 1.0
Response: (success or -32009 error with supported versions in data)
```

Nasiko hardcodes `A2A-Version: 1.0` on all outbound requests from the orchestrator and CLI.

## Appendix: Relationship to W3C Trace Context

Nasiko propagates `traceparent` headers on all A2A calls for distributed tracing:

```
traceparent: 00-{trace_id}-{span_id}-01
```

This enables:
- End-to-end trace visibility across multi-agent orchestrations
- Flow guard cycle detection (via the call chain in Redis)
- Observability correlation in Tempo/Loki

The `traceparent` is NOT part of the A2A spec — it's a Nasiko-specific extension layered on top via standard HTTP headers.

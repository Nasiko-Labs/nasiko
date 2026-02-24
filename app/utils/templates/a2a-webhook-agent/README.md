# A2A Webhook Agent

An A2A agent that forwards messages to webhook endpoints and returns responses in standard A2A format.

## Overview

This agent acts as a bridge between the A2A protocol and webhook-based services. It receives A2A messages, forwards them to a configured webhook endpoint, and returns the webhook response in standard A2A format.

## Features

- **Webhook Integration**: Forwards messages to any HTTP webhook endpoint
- **A2A Protocol Compliance**: Full support for A2A message/send protocol
- **Configurable Timeouts**: Adjustable webhook call timeouts
- **Error Handling**: Robust error handling with proper A2A error responses
- **Multiple Response Formats**: Handles JSON and text webhook responses

## Configuration

### Environment Variables

- `WEBHOOK_URL` (required): The webhook URL to forward messages to
- `WEBHOOK_TIMEOUT` (optional): Timeout for webhook calls in seconds (default: 120)

### Example Configuration

```bash
export WEBHOOK_URL="http://localhost:5678/webhook/53c136fe-3e77-4709-a143-fe82746dd8b6/chat"
export WEBHOOK_TIMEOUT="120"
```

## Usage

### Running with Docker Compose

```bash
docker-compose up --build
```

The agent will be available at `http://localhost:8085`

### Running Locally

```bash
# Install dependencies
pip install -e .

# Set environment variables
export WEBHOOK_URL="http://localhost:5678/webhook/53c136fe-3e77-4709-a143-fe82746dd8b6/chat"

# Run the agent
python src/__main__.py --host 0.0.0.0 --port 8085
```

## A2A Message Flow

1. **Incoming A2A Request**:
   ```json
   {
     "jsonrpc": "2.0",
     "id": "a49d2ccf-6eb2-41ac-b601-e3d2dd179a35",
     "method": "message/send",
     "params": {
       "message": {
         "role": "user",
         "parts": [{"kind": "text", "text": "How are you?"}],
         "messageId": "msg-123"
       }
     }
   }
   ```

2. **Webhook Call**:
   ```json
   {
     "sessionId": "a49d2ccf-6eb2-41ac-b601-e3d2dd179a35",
     "chatInput": "How are you?"
   }
   ```

3. **A2A Response**:
   ```json
   {
     "id": "a49d2ccf-6eb2-41ac-b601-e3d2dd179a35",
     "jsonrpc": "2.0", 
     "result": {
       "artifacts": [
         {
           "artifactId": "artifact-123",
           "parts": [{"kind": "text", "text": "Webhook response here"}]
         }
       ],
       "contextId": "context-456",
       "history": [...],
       "id": "task-789",
       "kind": "task",
       "status": {"state": "completed", "timestamp": "2024-11-20T..."}
     }
   }
   ```

## Testing

### Test the Agent

```bash
curl -X POST 'http://localhost:8085/' \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": "test-123",
    "method": "message/send", 
    "params": {
      "message": {
        "role": "user",
        "parts": [{"kind": "text", "text": "Hello!"}],
        "messageId": "msg-123"
      }
    }
  }'
```

### Health Check

```bash
curl http://localhost:8085/health
```

## Webhook Requirements

The target webhook should:
- Accept POST requests with JSON payload
- Expected payload format:
  ```json
  {
    "sessionId": "string",
    "chatInput": "string" 
  }
  ```
- Return a response (JSON or text) that can be forwarded back to the A2A client

## Error Handling

The agent handles various error scenarios:
- Webhook timeouts
- HTTP errors from webhook
- Invalid webhook responses
- Network connectivity issues

All errors are returned as proper A2A error responses.

## Integration with Nasiko

This agent can be registered with the Nasiko agent registry and used through the orchestrator for routing messages to webhook-based services.
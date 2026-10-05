# Nasiko & DronaHQ Integration Workflow

This document outlines the end-to-end architecture and data flow for the AI Agent suite built during the hackathon.

## System Architecture Flowchart

```mermaid
sequenceDiagram
    autonumber
    
    participant User as Nasiko Dashboard
    participant Core as Nasiko Control Plane
    participant Bridge as Node.js Bridge Agent
    participant DronaHQ as DronaHQ Platform
    
    User->>Core: User sends a chat message
    Note over User,Core: e.g., "Analyze my Q3 expenses"
    
    Core->>Bridge: Forwards request via JSON-RPC (A2A Protocol)
    activate Bridge
    
    Bridge->>Bridge: Parses payload & extracts query
    
    Bridge->>DronaHQ: Axios POST request to specific Webhook
    Note over Bridge,DronaHQ: URL injected via Nasiko Secrets
    activate DronaHQ
    
    DronaHQ->>DronaHQ: Triggers AI Workflow (LLM, Data Retrieval)
    
    DronaHQ-->>Bridge: Returns Standard JSON Response
    Note over DronaHQ,Bridge: { "answer": "The AI generated text..." }
    deactivate DronaHQ
    
    Bridge->>Bridge: Extracts "answer" & formats back to A2A Spec
    
    Bridge-->>Core: Returns TASK_STATE_COMPLETED payload
    deactivate Bridge
    
    Core-->>User: Renders final AI response in Chat UI
```

## Agent Components

We built three distinct agents using this exact same bridge architecture:
1. **Business Intelligence Agent** (`bi-agent`)
2. **Finance Advisor Agent** (`finance-agent`)
3. **Customer Feedback Agent** (`feedback-agent`)

### How the Bridge Works
Because DronaHQ and Nasiko speak two different data languages, the Bridge Agent acts as the translator. 
- It listens on port `8000` for Nasiko's strict A2A (Agent-to-Agent) JSON-RPC protocol.
- It translates that into a simple HTTP POST request for DronaHQ's webhooks.
- It securely holds the DronaHQ Webhook URL in isolated environment variables using `nasiko secrets`.
- When DronaHQ finishes processing the AI logic synchronously, the bridge translates the custom `{ "answer": "..." }` JSON back into Nasiko's required markdown message format.

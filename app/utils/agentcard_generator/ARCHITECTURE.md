# Architecture

## Overview

This agent generates A2A compatible AgentCards. Here's how it works at each level.

## The Workflow Comparison

### What This Agent Does:

```
1. User: python cli.py agents/github-agent2
2. LLM thinks: "I need to understand this agent"
3. LLM calls: glob_files() → finds files
4. LLM calls: read_file() → reads README.md, github_toolset.py
5. LLM calls: analyze_python_functions() → extracts functions
6. LLM reasons: Maps functions → A2A Skills
7. LLM calls: generate_agentcard_json() → creates AgentCard.json
```

## Component Breakdown

### 1. Tools (`tools.py`)

**Purpose**: The agent's "hands and eyes" - allows interaction with files

**Tools Available**:

```python
class AgentAnalyzerTools:
    def glob_files(pattern, base_path)

    # Finds files matching pattern

    def read_file(file_path)

    # Reads file contents

    def grep_code(pattern, file_path)

    # Searches for patterns

    def analyze_python_functions(file_path)

    # Custom analysis tool
    # Extracts function definitions

    def extract_agent_metadata(agent_path)

    # Custom metadata extractor
    # Gets port, description, deps

    def generate_agentcard_json(...)
# Final generation step
# Creates A2A-compliant JSON
```

### 2. Agent Orchestrator (`agent.py`)

**Purpose**: The agent's "brain" - decides what to do and when

**Key Components**:

```python
class AgentCardGeneratorAgent:
    def __init__(self, api_key, model):

    # Initialize LLM client
    # Load tools

    def _get_system_prompt(self):

    # Instructions for the LLM

    def _get_tool_schemas(self):

    # Tool definitions for function calling
    # OpenAI function calling format

    def _execute_tool(self, tool_name, arguments):

    # Execute the requested tool
    # Return results to LLM

    def generate_agentcard(self, agent_path, verbose):
# Main loop
# Iterative: LLM → Tool → LLM → Tool → ...
```

**The Iteration Loop**:

```python
while iteration < max_iterations:
    # 1. Call LLM with tools available
    response = llm.chat.completions.create(
        messages=messages,
        tools=tool_schemas,
        tool_choice="auto"
    )

    # 2. Check if LLM wants to use tools
    if response.tool_calls:
        for tool_call in response.tool_calls:
            # 3. Execute the tool
            result = execute_tool(tool_call.name, tool_call.args)

            # 4. Add result to conversation
            messages.append({
                "role": "tool",
                "content": result
            })

        # 5. Continue loop (LLM will process results)
        continue

    # 6. No more tools needed - done!
    break
```

### 3. CLI (`cli.py`)

**Purpose**: User interface

**Flow**:

```
User input → Validate paths → Create agent → Run → Save output
```

## The System Prompt

The system prompt is crucial - it tells the LLM HOW to work:

```
You are an AgentCard Generator Agent that analyzes agent code
and generates A2A-compliant AgentCards.

Instructions:
1. Use tools to explore and understand the agent's codebase
2. Analyze the code to extract capabilities, functions, and metadata
3. Generate an A2A-compliant AgentCard based on your analysis

Available tools:
- glob_files: Find files matching patterns
- read_file: Read file contents
- analyze_python_functions: Extract function definitions
- extract_agent_metadata: Get metadata
- generate_agentcard_json: Create final AgentCard

Workflow:
1. Start by using glob_files to find relevant files
2. Read key files like README.md
3. Use analyze_python_functions to extract functions
4. Map functions to A2A "skills"
5. Generate the final AgentCard JSON

Be strategic - don't read every file, focus on important ones.
```

## Adaptive Strategy Example

The LLM adapts based on what it finds:

### Scenario 1: Standard Agent Structure

```
Iteration 1:
  LLM: "Let me find Python files"
  Tool: glob_files("**/*.py") → Found 6 files

Iteration 2:
  LLM: "Let me read the README"
  Tool: read_file("README.md") → Got description

Iteration 3:
  LLM: "Let me analyze the toolset"
  Tool: analyze_python_functions("github_toolset.py") → Got 3 functions

Iteration 4:
  LLM: "Let me get metadata"
  Tool: extract_agent_metadata(".") → Got port, deps

Iteration 5:
  LLM: "Now I can generate the AgentCard"
  Tool: generate_agentcard_json(...) → Done!
```

### Scenario 2: Missing README

```
Iteration 1:
  LLM: "Let me read the README"
  Tool: read_file("README.md") → ERROR: Not found

Iteration 2:
  LLM: "No README, let me find other docs"
  Tool: glob_files("**/*.md") → Found docs/api.md

Iteration 3:
  LLM: "Let me read that instead"
  Tool: read_file("docs/api.md") → Got info

[continues...]
```

## Function Calling Flow

```
┌─────────────────────────────────────────────────────┐
│  User: "Generate AgentCard for github-agent"        │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  Agent: Create initial message                      │
│  messages = [                                       │
│    {role: "system", content: system_prompt},        │
│    {role: "user", content: user_request}            │
│  ]                                                  │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  LLM: Analyze request + available tools             │
│  "I need to find files first"                       │
│  → Returns: tool_call(glob_files, "**/*.py")        │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  Agent: Execute tool                                │
│  result = tools.glob_files("**/*.py")               │
│  → Returns: {files: [...]}                          │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  Agent: Add tool result to messages                 │
│  messages.append({                                  │
│    role: "tool",                                    │
│    content: result                                  │
│  })                                                 │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  LLM: Analyze results                               │
│  "Found 6 files. Let me read README"                │
│  → Returns: tool_call(read_file, "README.md")       │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
         [Loop continues...]
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  LLM: Final step                                    │
│  "I have all info, generating AgentCard"            │
│  → Returns: tool_call(generate_agentcard_json, ...) │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  Agent: Execute final tool                          │
│  result = tools.generate_agentcard_json(...)        │
│  → AgentCard.json created                           │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  LLM: No more tools needed                          │
│  → Returns: Final message (no tool_calls)           │
└────────────────┬────────────────────────────────────┘
                 │
                 ▼
┌─────────────────────────────────────────────────────┐
│  Agent: Done! Return result                         │
└─────────────────────────────────────────────────────┘
```
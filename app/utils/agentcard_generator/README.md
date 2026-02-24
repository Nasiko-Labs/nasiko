# AgentCard Generator Agent

An intelligent agent that generates A2A-compliant AgentCards by analyzing agent code.

## How It Works

1. **Discovery**: Uses `glob_files` to find relevant files (README, Python code, config)
2. **Reading**: Uses `read_file` to read key files
3. **Analysis**: Uses `analyze_python_functions` to extract function definitions
4. **Metadata Extraction**: Uses `extract_agent_metadata` to get port, description, dependencies
5. **Mapping**: Maps functions to A2A "skills" with descriptions and examples
6. **Generation**: Creates the final A2A-compliant AgentCard JSON

## Architecture

```
┌─────────────────────────────────────┐
│   User Request                      │
│   "Generate AgentCard for X"        │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│   LLM (Claude 3.5 Sonnet)           │
│   - Decides which tools to use      │
│   - Analyzes results                │
│   - Plans next steps                │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│   Tools (tools.py)                  │
│   - glob_files                      │
│   - read_file                       │
│   - grep_code                       │
│   - analyze_python_functions        │
│   - extract_agent_metadata          │
│   - generate_agentcard_json         │
└──────────────┬──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│   Output: AgentCard.json            │
└─────────────────────────────────────┘
```

## Key Components

### 1. `tools.py` - Analysis Tools

The "hands and eyes" of the agent:

- **glob_files**: Find files matching patterns (like `**/*.py`)
- **read_file**: Read file contents
- **grep_code**: Search for patterns in code
- **analyze_python_functions**: Extract function definitions
- **extract_agent_metadata**: Get metadata from README/config
- **generate_agentcard_json**: Create final AgentCard

### 2. `agent.py` - Agent Orchestrator

The "brain" that uses LLM to:

- Decide which tools to call
- Interpret tool results
- Plan next steps
- Generate the final AgentCard

### 3. `cli.py` - Command Line Interface

Simple CLI to run the agent

## Installation

```bash
cd agents/agentcard-generator

# Install dependencies
pip install -r requirements.txt

# Or using uv
uv pip install -r requirements.txt
```

## Setup

Set your OpenRouter API key:

```bash
export OPENROUTER_API_KEY="your_key_here"
```

Or create a `.env` file:

```bash
echo "OPENROUTER_API_KEY=your_key_here" > .env
```

## Usage

### Basic Usage

```bash
python cli.py path/to/agent
```

### With Verbose Output

```bash
python cli.py path/to/agent --verbose
```

This shows the agent's thought process:

- Which tools it's calling
- What it's discovering
- How it's mapping functions to skills

### Specify Output Path

```bash
python cli.py path/to/agent -o output/AgentCard.json
```

### Use Different Model

```bash
python cli.py path/to/agent --model "openai/gpt-4"
```

## Example

Generate AgentCard for github-agent2:

```bash
python cli.py ../github-agent2 --verbose
```

**Output:**

```
Analyzing agent at: ../github-agent
Output will be saved to: ../github-agent/AgentCard.json

[Iteration 1]
Agent: I'll analyze the github-agent to generate an AgentCard...

  → Calling tool: glob_files
    Arguments: {"pattern": "**/*.py", "base_path": "../github-agent"}
    ✓ Found 6 files matching '**/*.py'

  → Calling tool: read_file
    Arguments: {"file_path": "../github-agent/README.md"}
    ✓ Read 155 lines from ../github-agent/README.md

[Iteration 2]
  → Calling tool: analyze_python_functions
    Arguments: {"file_path": "../github-agent/github_toolset.py"}
    ✓ Found 3 functions in ../github-agent/github_toolset.py

  → Calling tool: extract_agent_metadata
    Arguments: {"agent_path": "../github-agent"}
    ✓ Extracted metadata from ../github-agent

[Iteration 3]
Agent: Based on my analysis, I'll now generate the AgentCard...

  → Calling tool: generate_agentcard_json
    Arguments: {
      "agent_name": "GitHub Repository Intelligence Agent",
      "description": "An intelligent GitHub agent...",
      "skills": [...],
      "port": 10007
    }
    ✓ Generated AgentCard saved to ../github-agent/AgentCard.json

✓ AgentCard generated successfully!
  Saved to: ../github-agent/AgentCard.json
  Iterations: 3

Preview:
  Name: GitHub Repository Intelligence Agent
  Description: An intelligent GitHub agent that provides repository querying...
  Skills: 3
```

## Adaptive Strategy

```python
# If README doesn't exist:
Attempt
1: read_file("README.md") → Error
Adaptation: glob_files("**/*.md") → Find
other
docs
Attempt
2: read_file("docs/api.md") → Success

# If port not found in README:
Attempt
1: extract_agent_metadata() → No
port
Adaptation: grep_code(pattern="port.*\d+", file="__main__.py")
Result: Found
port in code
```

## Extending the Agent

### Add New Tools

Edit `tools.py`:

```python
class AgentAnalyzerTools:
    def my_new_tool(self, param: str) -> Dict[str, Any]:
        """My new tool description"""
        # Implementation
        return {"status": "success", "result": ...}
```

Update `agent.py` tool schemas:

```python
{
    "type": "function",
    "function": {
        "name": "my_new_tool",
        "description": "Tool description",
        "parameters": {...}
    }
}
```

### Change Model

```bash
# Use GPT-4
python cli.py path/to/agent --model "openai/gpt-4"

# Use another Claude model
python cli.py path/to/agent --model "anthropic/claude-3-opus"
```

## Troubleshooting

### "OPENROUTER_API_KEY must be set"

Set your API key:

```bash
export OPENROUTER_API_KEY="your_key_here"
```

### Agent doesn't generate AgentCard

- Check that the agent path exists
- Use `--verbose` to see what the agent is doing
- The agent might need more iterations (increase `max_iterations` in `agent.py`)

### Generated AgentCard is incorrect

- Review the agent's analysis with `--verbose`
- The LLM might misinterpret the code structure
- You may need to adjust the system prompt in `agent.py`

## License

MIT License - see LICENSE file for details
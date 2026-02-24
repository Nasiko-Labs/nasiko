"""
AgentCard Generator Agent
Uses LLM to decide which tools to use and how to analyze agent code
"""

import json
import logging
import os
import sys
from pathlib import Path
from typing import Any, Dict, List

# from dotenv import load_dotenv
from openai import OpenAI

sys.path.insert(0, str(Path(__file__).parent))

from tools import AgentAnalyzerTools

logger = logging.getLogger(__name__)


class AgentCardGeneratorAgent:
    """
    An agent that generates AgentCards by analyzing code,
    similar to how Claude Code works.
    """

    def __init__(
        self,
        api_key: str = None,
        model: str = "gpt-4o",
        n8n_agent: bool = False,
    ):
        """
        Initialize the agent

        Args:
            api_key: OpenAI API key (or set OPENAI_API_KEY env var)
            model: Model to use for reasoning
        """
        # load_dotenv()
        self.api_key = api_key or os.getenv("OPENAI_API_KEY")
        if not self.api_key:
            logger.error("OPENAI_API_KEY not found in environment or arguments")
            raise ValueError("OPENAI_API_KEY must be set")

        logger.info(f"Initializing AgentCardGeneratorAgent with model: {model}")
        self.client = OpenAI(api_key=self.api_key)
        self.model = model
        self.tools = AgentAnalyzerTools()
        self.max_iterations = 10
        self.n8n_agent = n8n_agent
        logger.debug(f"Agent initialized with max_iterations={self.max_iterations}")

    def _get_system_prompt(self) -> str:
        """System prompt that instructs the LLM how to generate agent card"""
        if self.n8n_agent:
            return """You are an AgentCard Generator Agent that analyzes n8n workflow JSON files and generates A2A-compliant AgentCards.

Your goal: Analyze the n8n workflow structure to accurately determine its capabilities and generate a compliant AgentCard.

Available tools:
- glob_files: Find files matching a glob pattern (use if workflow file not found at default path)
- read_file: Read the n8n workflow JSON file contents
- generate_agentcard_json: Create the final AgentCard JSON

CRITICAL WORKFLOW:

1. **Read the n8n Workflow JSON**:
   - Use read_file to read the workflow JSON file
   - If the file is not found, use glob_files to search for *.json files in the agent directory
   - Parse the JSON to extract:
     * Workflow name (from "name" field, may be in "data[0].name" if wrapped)
     * Chat trigger node (type contains "chatTrigger") - look at its "parameters.options" for capabilities
     * Agent node (type contains "agent") - look at "parameters.text" for prompt and "parameters.options.systemMessage"

2. **Analyze Capabilities** (ONLY based on explicit configuration in chat trigger node options):
   - **streaming**: Only true if chat trigger node options has responseMode = "streaming"
   - **push_notifications**: Only true if explicitly configured (default: false)
   - **state_transition_history**: Only true if chat trigger node options has loadPreviousSession configured
   - If a capability is NOT explicitly configured, it MUST be false

3. **Derive Skills from Workflow Analysis**:
   - Analyze the agent prompt (parameters.text) and system message to understand what the agent does
   - Identify distinct capabilities/functions the agent provides
   - For each skill:
     * id: kebab-case identifier (e.g., "calendar-setup-guide")
     * name: Human-readable name (e.g., "Calendar Setup Guide")
     * description: Clear explanation of what this skill does
     * tags: Relevant keywords from the workflow functionality
     * examples: Natural language examples of how to use this skill
     * inputModes: ["text/plain"] for chat-based workflows
     * outputModes: ["text/plain"] for text responses

4. **Generate Description**:
   - Create a clear, concise description of what the agent does
   - Base this on the workflow name, agent prompt, and system message
   - Do NOT make assumptions about capabilities not in the workflow

5. **Generate AgentCard**:
   - Use generate_agentcard_json with:
     * agent_name: From workflow name
     * description: The description you created
     * skills: The skills you derived
     * preferred_transport: "HTTP+JSON" (always for n8n webhooks)
     * default_input_modes: ["text/plain"]
     * default_output_modes: ["text/plain"]
     * streaming, push_notifications, state_transition_history: Based on chat trigger options analysis
   - Ensure all capabilities reflect ONLY what's explicitly in the workflow JSON

IMPORTANT:
- ONLY set capabilities to true if they are EXPLICITLY configured in the workflow JSON
- If options are empty or a setting is not present, the capability is false
- Do not assume capabilities based on what n8n "could" support - only what IS configured
- Transport is always "HTTP+JSON" for n8n webhook-based workflows
- Be conservative - false is the safe default for any capability not explicitly set"""

        return """You are an AgentCard Generator Agent that analyzes agent code and generates A2A-compliant AgentCards.

Your goal: Analyze the agent's implementation to accurately determine its A2A protocol capabilities and generate a compliant AgentCard.

Available tools:
- glob_files: Find files matching patterns (like "**/*.py")
- read_file: Read file contents
- grep_code: Search for patterns in files
- analyze_python_functions: Extract function definitions from Python files
- extract_agent_metadata: Get metadata from README, config files

- detect_transport_protocol: Detect transport protocol (JSONRPC/HTTP+JSON/WebSocket) using AST parsing
- generate_agentcard_json: Create the final AgentCard JSON

CRITICAL WORKFLOW:

1. **Find Files**:
   - Use glob_files to locate: __main__.py, *executor*.py, *agent*.py, README.md, *toolset*.py


2. **Read A2A Server Implementation** (MOST IMPORTANT):
   - Read __main__.py or main server file to understand:
     * How the A2A server is set up
     * What AgentCapabilities are declared (if any)
     * What request handlers are used
   - Read executor files to understand task execution


3. **Determine Agent Framework**:
   - Use `detect_agent_framework` tool on `__main__.py` or the main entry point.
   - This tool returns `candidates` (high confidence) and `all_imports` (raw data).
   - **Evaluate the findings**:
     1. Prioritize **Orchestration candidates** (e.g., CrewAI, LangChain).
     2. If multiple candidates, choose the one driving the logic.
     3. If only SDK candidates (like `openai`) are found, use that.
     4. **CRITICAL**: If NO candidates found, review `all_imports`. If you recognize a library as an agent framework (e.g. from your training data), use it.
     5. IGNORE protocol libraries (FASTAPI, A2A, FLASK).
   - Use your judgment to select the best `agentFramework`.

4. **Determine Transport Protocol** (CRITICAL - Use the detect_transport_protocol tool):

   - Use detect_transport_protocol tool on __main__.py or main.py file
   - This tool uses AST parsing to generically detect the transport protocol
   - It analyzes:
     * Import statements (a2a.server, FastAPI, Flask, etc.)
     * Class instantiations (A2AStarletteApplication, etc.)
     * Route decorators and method calls
   - Returns: preferred_transport, confidence, evidence, and optional additional_transports
   - ALWAYS use this tool rather than trying to detect transport manually
   - Pass the detected preferred_transport to generate_agentcard_json
   - Only include additional_interfaces if the tool detects multiple transports

4. **Determine A2A Protocol Capabilities** by analyzing code:

   a) **streaming** (SSE support):
      - Look for: message/stream handler, SSE implementation, async generators
      - Look for: /stream endpoint, text/event-stream, yield in loops
      - Look for: streaming=True in AgentCapabilities
      - If found: streaming = true, else: false

   b) **pushNotifications** (webhook callbacks):
      - Look for: webhook URL handling, callback registration
      - Look for: push notification infrastructure, async notifications
      - Look for: JWT verification, notification endpoints
      - If found: push_notifications = true, else: false

   c) **stateTransitionHistory** (task state tracking):
      - Look for: InMemoryTaskStore or any TaskStore usage in __main__.py
      - Look for: DefaultRequestHandler with task_store parameter
      - Look for: state transition tracking, task history storage
      - Look for: checkpoint persistence, state change logs
      - Look for: submitted → working → completed tracking
      - If ANY TaskStore is used: state_transition_history = true
      - If found: state_transition_history = true, else: false

   d) **chat_agent** (non-A2A chat API):
      - Look for: /chat endpoint, chat message handling, chat routes
      - Look for: message/response patterns typical of chat APIs
      - Look for: conversational APIs not using A2A protocol
      - Look for: OpenAI-style chat completions endpoint
      - Look for: direct chat functionality without A2A wrapper
      - If agent implements chat API but NOT via A2A protocol: chat_agent = true
      - If agent uses A2A protocol OR no chat functionality: chat_agent = false

5. **Determine Input/Output Modes** by analyzing:
   - What content types the agent accepts (check input validation, supported MIME types)
   - What the agent returns (JSON, images, text)
   - Image handling: If agent generates/processes images → add "image/png" to output_modes
   - Default: ["application/json", "text/plain"] for input, ["application/json"] for output

6. **Extract Agent Functions/Tools**:
   - Read toolset/main agent files to find the ACTUAL implementation
   - Use analyze_python_functions to extract all function definitions
   - IMPORTANT: Analyze the actual toolset/function implementations, NOT any existing AgentCard definitions
   - If you find an existing AgentCard in __main__.py, IGNORE its skills - analyze the real functions instead
   - Map EACH function to an A2A "skill" with:
     * id: kebab-case identifier (e.g., "get-user-repositories")
     * name: Human-readable name (e.g., "Get User Repositories")
     * description: Clear explanation from the function's docstring
     * tags: Relevant keywords extracted from function name/description
     * examples: Natural language examples based on function parameters
     * inputModes: Supported input types
     * outputModes: Supported output types

7. **Generate AgentCard**:
   - Pass ALL determined capabilities to generate_agentcard_json
   - Include: streaming, push_notifications, state_transition_history, chat_agent
   - Include: default_input_modes, default_output_modes
   - Include: preferred_transport and additional_interfaces (if multiple transports found)
   - Include: properly formatted skills

IMPORTANT:
- Focus on A2A PROTOCOL implementation, not internal agent logic
- streaming/pushNotifications/stateTransitionHistory are A2A features, not agent features
- Read the actual server setup code to determine what's implemented
- Be accurate - false positives hurt interoperability
- If unsure, default to false/minimal capabilities"""

    def _get_tool_schemas(self) -> List[Dict[str, Any]]:
        """Define tool schemas for function calling"""
        if self.n8n_agent:
            return [
                {
                    "type": "function",
                    "function": {
                        "name": "glob_files",
                        "description": "Find files matching a glob pattern. Use this to search for workflow JSON files if the default path doesn't exist.",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "pattern": {
                                    "type": "string",
                                    "description": "Glob pattern like '*.json' or '*workflow*.json'",
                                },
                                "base_path": {
                                    "type": "string",
                                    "description": "Base directory to search from",
                                },
                            },
                            "required": ["pattern"],
                        },
                    },
                },
                {
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "description": "Read the n8n workflow JSON file contents",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "file_path": {
                                    "type": "string",
                                    "description": "Path to the n8n workflow JSON file",
                                }
                            },
                            "required": ["file_path"],
                        },
                    },
                },
                {
                    "type": "function",
                    "function": {
                        "name": "generate_agentcard_json",
                        "description": "Generate A2A-compliant AgentCard JSON for n8n workflow",
                        "parameters": {
                            "type": "object",
                            "properties": {
                                "agent_name": {
                                    "type": "string",
                                    "description": "Agent name (from workflow name field)",
                                },
                                "description": {
                                    "type": "string",
                                    "description": "Agent description derived from workflow analysis",
                                },
                                "skills": {
                                    "type": "array",
                                    "description": "List of skill objects derived from workflow analysis",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "id": {"type": "string"},
                                            "name": {"type": "string"},
                                            "description": {"type": "string"},
                                            "tags": {
                                                "type": "array",
                                                "items": {"type": "string"},
                                            },
                                            "examples": {
                                                "type": "array",
                                                "items": {"type": "string"},
                                            },
                                            "inputModes": {
                                                "type": "array",
                                                "items": {"type": "string"},
                                            },
                                            "outputModes": {
                                                "type": "array",
                                                "items": {"type": "string"},
                                            },
                                        },
                                        "required": ["id", "name", "description"],
                                    },
                                },
                                "preferred_transport": {
                                    "type": "string",
                                    "description": "Transport protocol. Use 'HTTP+JSON' for n8n workflows.",
                                },
                                "default_input_modes": {
                                    "type": "array",
                                    "description": "Input MIME types. Use ['text/plain'] for n8n chat workflows.",
                                    "items": {"type": "string"},
                                },
                                "default_output_modes": {
                                    "type": "array",
                                    "description": "Output MIME types. Use ['text/plain'] for n8n chat workflows.",
                                    "items": {"type": "string"},
                                },
                                "streaming": {
                                    "type": "boolean",
                                    "description": "Set to true ONLY if chat trigger node options has responseMode='streaming'",
                                },
                                "push_notifications": {
                                    "type": "boolean",
                                    "description": "Default false unless explicitly configured in workflow.",
                                },
                                "state_transition_history": {
                                    "type": "boolean",
                                    "description": "Set to true ONLY if chat trigger node options has loadPreviousSession configured",
                                },
                            },
                            "required": ["agent_name", "description", "skills"],
                        },
                    },
                },
            ]

        return [
            {
                "type": "function",
                "function": {
                    "name": "glob_files",
                    "description": "Find files matching a glob pattern",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": {
                                "type": "string",
                                "description": "Glob pattern like '**/*.py'",
                            },
                            "base_path": {
                                "type": "string",
                                "description": "Base directory (default: current dir)",
                            },
                        },
                        "required": ["pattern"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "read_file",
                    "description": "Read contents of a file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "Path to file",
                            }
                        },
                        "required": ["file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "grep_code",
                    "description": "Search for pattern in a file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "pattern": {
                                "type": "string",
                                "description": "Regex pattern",
                            },
                            "file_path": {
                                "type": "string",
                                "description": "File to search",
                            },
                            "case_sensitive": {
                                "type": "boolean",
                                "description": "Case sensitive search",
                            },
                        },
                        "required": ["pattern", "file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "analyze_python_functions",
                    "description": "Extract function definitions from Python file",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "Path to Python file",
                            }
                        },
                        "required": ["file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "extract_agent_metadata",
                    "description": "Extract metadata from agent directory",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "agent_path": {
                                "type": "string",
                                "description": "Path to agent directory",
                            }
                        },
                        "required": ["agent_path"],
                    },
                },
            },

            {
                "type": "function",
                "function": {
                    "name": "detect_transport_protocol",
                    "description": "Detect transport protocol from Python server file using AST parsing. Returns preferred_transport (JSONRPC/HTTP+JSON/WebSocket), confidence level, and evidence.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "Path to main server file (__main__.py, main.py, or app.py)",
                            }
                        },
                        "required": ["file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "detect_agent_framework",
                    "description": "Detect potential agent frameworks by analyzing Python imports. Returns 'candidates' (known frameworks) and 'all_imports' (for LLM analysis).",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "file_path": {
                                "type": "string",
                                "description": "Path to main server file or entry point",
                            }
                        },
                        "required": ["file_path"],
                    },
                },
            },
            {
                "type": "function",
                "function": {
                    "name": "generate_agentcard_json",
                    "description": "Generate A2A-compliant AgentCard JSON. Analyzes agent capabilities to determine streaming support, input/output modes, etc.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "agent_name": {
                                "type": "string",
                                "description": "Agent name",
                            },
                            "description": {
                                "type": "string",
                                "description": "Agent description",
                            },
                            "skills": {
                                "type": "array",
                                "description": "List of skill objects with id, name, description, tags, examples, inputModes, outputModes",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "id": {"type": "string"},
                                        "name": {"type": "string"},
                                        "description": {"type": "string"},
                                        "tags": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                        },
                                        "examples": {
                                            "type": "array",
                                            "items": {"type": "object"},
                                        },
                                        "inputModes": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                        },
                                        "outputModes": {
                                            "type": "array",
                                            "items": {"type": "string"},
                                        },
                                    },
                                    "required": ["id", "name", "description"],
                                },
                            },
                            "port": {"type": "integer", "description": "Server port"},
                            "version": {
                                "type": "string",
                                "description": "Agent version",
                            },
                            "streaming": {
                                "type": "boolean",
                                "description": "Whether agent supports streaming responses (check code for stream/streaming methods)",
                            },
                            "push_notifications": {
                                "type": "boolean",
                                "description": "Whether agent supports push notifications (check for webhook/notification support)",
                            },
                            "state_transition_history": {
                                "type": "boolean",
                                "description": "Whether agent tracks state transition history (check for state/history tracking)",
                            },
                            "chat_agent": {
                                "type": "boolean",
                                "description": "Whether agent implements chat API (non-A2A compatible chat endpoint). Check for /chat routes, chat message handling, or conversational APIs not using A2A protocol",
                            },
                            "default_input_modes": {
                                "type": "array",
                                "description": "List of supported input MIME types (e.g., 'application/json', 'text/plain', 'image/png')",
                                "items": {"type": "string"},
                            },
                            "default_output_modes": {
                                "type": "array",
                                "description": "List of supported output MIME types (e.g., 'application/json', 'image/png', 'text/plain')",
                                "items": {"type": "string"},
                            },
                            "preferred_transport": {
                                "type": "string",
                                "description": "Preferred transport protocol (e.g., 'JSONRPC', 'HTTP+JSON', 'WebSocket'). Default: 'JSONRPC'",
                            },
                            "additional_interfaces": {
                                "type": "array",
                                "description": "List of additional transport interfaces beyond the primary one. Each interface should have 'url' and 'transport' keys. Only include if agent actually implements multiple transports. Omit if only one transport is available.",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "url": {"type": "string"},
                                        "transport": {"type": "string"},
                                    },
                                },
                            },
                            "agentFramework": {
                                "type": "string",
                                "description": "The detected framework used to build the agent (e.g. 'crewai', 'langchain', 'llama-index', 'autogen', 'custom'). Empty if unknown.",
                            },
                        },
                        "required": ["agent_name", "description", "skills"],
                    },
                },
            },
        ]

    def _execute_tool(
        self, tool_name: str, arguments: Dict[str, Any]
    ) -> Dict[str, Any]:
        """Execute a tool and return result"""
        logger.debug(f"Executing tool: {tool_name} with args: {arguments}")
        if hasattr(self.tools, tool_name):
            method = getattr(self.tools, tool_name)
            result = method(**arguments)
            logger.debug(f"Tool {tool_name} result status: {result.get('status')}")
            return result
        else:
            logger.error(f"Tool '{tool_name}' not found")
            return {"status": "error", "message": f"Tool '{tool_name}' not found"}

    def generate_agentcard(
        self, agent_path: str, verbose: bool = False
    ) -> Dict[str, Any]:
        """
        Generate AgentCard for an agent by analyzing its code or n8n workflow

        Args:
            agent_path: Path to the agent directory
            verbose: Whether to print detailed progress

        Returns:
            Dictionary with generated AgentCard and metadata
        """
        # Determine user message based on agent type
        # success_tool_name is always generate_agentcard_json for both cases
        success_tool_name = "generate_agentcard_json"

        if self.n8n_agent:
            n8n_workflow_path = str(Path(agent_path) / "n8n_workflow.json")
            logger.info(
                f"Starting AgentCard generation from n8n workflow: {n8n_workflow_path}"
            )
            user_message = f"Generate an A2A-compliant AgentCard for the n8n workflow at: {n8n_workflow_path}. If the file is not found, use glob_files to search for *.json files in the directory: {agent_path}"
        else:
            logger.info(f"Starting AgentCard generation for: {agent_path}")
            user_message = (
                f"Generate an A2A-compliant AgentCard for the agent at: {agent_path}"
            )

        messages = [
            {"role": "system", "content": self._get_system_prompt()},
            {"role": "user", "content": user_message},
        ]

        iteration = 0
        final_agentcard = None

        while iteration < self.max_iterations:
            iteration += 1
            logger.debug(f"Starting iteration {iteration}/{self.max_iterations}")

            if verbose:
                print(f"\n[Iteration {iteration}]")

            try:
                logger.debug(f"Calling LLM with {len(messages)} messages")
                response = self.client.chat.completions.create(
                    model=self.model,
                    messages=messages,
                    tools=self._get_tool_schemas(),
                    tool_choice="auto",
                    temperature=0.1,
                    max_tokens=4000,
                )

                message = response.choices[0].message
                logger.debug(
                    f"LLM response received with {len(message.tool_calls or [])} tool calls"
                )

                assistant_message = {
                    "role": "assistant",
                    "content": message.content or "",
                }
                if message.tool_calls:
                    assistant_message["tool_calls"] = [
                        {
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.function.name,
                                "arguments": tc.function.arguments,
                            },
                        }
                        for tc in message.tool_calls
                    ]
                messages.append(assistant_message)

                if verbose and message.content:
                    print(f"Agent: {message.content}")

                if message.tool_calls:
                    for tool_call in message.tool_calls:
                        tool_name = tool_call.function.name
                        arguments = json.loads(tool_call.function.arguments)

                        logger.info(f"Tool call: {tool_name}")
                        if verbose:
                            print(f"  → Calling tool: {tool_name}")
                            print(f"    Arguments: {json.dumps(arguments, indent=2)}")

                        result = self._execute_tool(tool_name, arguments)

                        if verbose:
                            if (
                                isinstance(result, dict)
                                and result.get("status") == "success"
                            ):
                                print(f"    ✓ {result.get('message', 'Success')}")
                            else:
                                print(f"    ✗ {result.get('message', 'Error')}")

                        if (
                            tool_name == success_tool_name
                            and result.get("status") == "success"
                        ):
                            final_agentcard = result.get("agentcard")
                            logger.info("AgentCard JSON successfully generated")

                        messages.append(
                            {
                                "role": "tool",
                                "tool_call_id": tool_call.id,
                                "content": json.dumps(result),
                            }
                        )

                    continue

                logger.info(f"Agent finished after {iteration} iterations")
                if verbose:
                    print("\n[Agent finished]")

                break

            except Exception as e:
                logger.exception(
                    f"Error during execution at iteration {iteration}: {e}"
                )
                return {
                    "status": "error",
                    "message": f"Error during execution: {str(e)}",
                    "agentcard": None,
                }

        if iteration >= self.max_iterations:
            logger.warning(f"Maximum iterations ({self.max_iterations}) reached")
            return {
                "status": "error",
                "message": "Maximum iterations reached",
                "agentcard": final_agentcard,
            }

        logger.info("AgentCard generation completed successfully")
        return {
            "status": "success",
            "message": "AgentCard generated successfully",
            "agentcard": final_agentcard,
            "iterations": iteration,
        }

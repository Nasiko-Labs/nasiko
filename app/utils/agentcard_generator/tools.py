"""
Tools for analyzing agent code and generating AgentCards.
"""

import ast
import logging
import re
from pathlib import Path
from typing import Any, List, Dict

logger = logging.getLogger(__name__)


class AgentAnalyzerTools:
    """Tools to analyzing agents and generating AgentCards"""

    def glob_files(self, pattern: str, base_path: str = ".") -> Dict[str, Any]:
        """
        Find files matching a glob pattern

        Args:
            pattern: Glob pattern like "**/*.py" or "agents/*/README.md"
            base_path: Base directory to search from (default: current directory)

        Returns:
            Dictionary with status and list of matching file paths
        """
        logger.debug(
            f"Globbing files with pattern '{pattern}' in base_path '{base_path}'"
        )
        try:
            base = Path(base_path)
            if not base.exists():
                logger.warning(f"Base path '{base_path}' does not exist")
                return {
                    "status": "error",
                    "message": f"Base path '{base_path}' does not exist",
                    "files": [],
                }

            matches = list(base.glob(pattern))
            file_paths = [str(p) for p in matches if p.is_file()]

            logger.info(f"Found {len(file_paths)} files matching '{pattern}'")
            return {
                "status": "success",
                "message": f"Found {len(file_paths)} files matching '{pattern}'",
                "count": len(file_paths),
                "files": file_paths,
            }
        except Exception as e:
            logger.error(f"Error in glob search: {e}")
            return {
                "status": "error",
                "message": f"Error in glob search: {str(e)}",
                "files": [],
            }

    def read_file(self, file_path: str) -> Dict[str, Any]:
        """
        Read file contents

        Args:
            file_path: Path to the file to read

        Returns:
            Dictionary with file contents and metadata
        """
        logger.debug(f"Reading file: {file_path}")
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                content = f.read()

            lines = content.split("\n")
            logger.info(f"Successfully read {len(lines)} lines from {file_path}")
            return {
                "status": "success",
                "message": f"Read {len(lines)} lines from {file_path}",
                "file_path": file_path,
                "content": content,
                "line_count": len(lines),
            }
        except FileNotFoundError:
            logger.warning(f"File not found: {file_path}")
            return {
                "status": "error",
                "message": f"File not found: {file_path}",
                "content": None,
            }
        except Exception as e:
            logger.error(f"Error reading file {file_path}: {e}")
            return {
                "status": "error",
                "message": f"Error reading file: {str(e)}",
                "content": None,
            }

    def grep_code(
        self, pattern: str, file_path: str, case_sensitive: bool = True
    ) -> Dict[str, Any]:
        """
        Search for pattern in file

        Args:
            pattern: Regex pattern to search for
            file_path: File to search in
            case_sensitive: Whether search is case-sensitive

        Returns:
            Dictionary with matching lines and line numbers
        """
        logger.debug(f"Searching for pattern '{pattern}' in {file_path}")
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                content = f.read()

            flags = 0 if case_sensitive else re.IGNORECASE
            matches = []

            for i, line in enumerate(content.split("\n"), 1):
                if re.search(pattern, line, flags):
                    matches.append({"line_number": i, "content": line.strip()})

            logger.info(f"Found {len(matches)} matches for '{pattern}' in {file_path}")
            return {
                "status": "success",
                "message": f"Found {len(matches)} matches for '{pattern}' in {file_path}",
                "file_path": file_path,
                "pattern": pattern,
                "matches": matches,
                "match_count": len(matches),
            }
        except Exception as e:
            logger.error(f"Error searching file {file_path}: {e}")
            return {
                "status": "error",
                "message": f"Error searching file: {str(e)}",
                "matches": [],
            }

    def analyze_python_functions(self, file_path: str) -> Dict[str, Any]:
        """
        Extract function definitions from Python file using AST parsing

        Args:
            file_path: Path to Python file

        Returns:
            Dictionary with extracted functions and their metadata
        """
        logger.debug(f"Analyzing Python functions in: {file_path}")
        try:
            with open(file_path, "r", encoding="utf-8") as f:
                content = f.read()

            try:
                tree = ast.parse(content)
            except SyntaxError as e:
                return {
                    "status": "error",
                    "message": f"Syntax error parsing {file_path}: {str(e)}",
                    "functions": []
                }

            functions = []

            for node in ast.walk(tree):
                if isinstance(node, ast.FunctionDef):
                    # Skip private/dunder methods often not relevant for skills
                    if node.name.startswith("_"):
                        continue
                        
                    # Extract parameters
                    param_list = []
                    for arg in node.args.args:
                        if arg.arg != "self":
                            param_list.append(arg.arg)
                            
                    # Extract return type annotation
                    return_type = None
                    if node.returns:
                        # Simple attempt to get source segment for return type
                        try:
                            # Python 3.9+ has ast.unparse, or we can just leave it simplified
                            if hasattr(ast, "unparse"):
                                return_type = ast.unparse(node.returns)
                            else:
                                # Fallback or just ignore complex types for now
                                return_type = getattr(node.returns, "id", None) 
                        except:
                            pass

                    # Extract docstring
                    docstring = ast.get_docstring(node)
                    description = None
                    if docstring:
                        # use first line or summary
                        description = docstring.strip().split("\n")[0]

                    functions.append({
                        "name": node.name,
                        "description": description,
                        "parameters": param_list,
                        "return_type": return_type,
                        "line_number": node.lineno
                    })

            logger.info(f"Found {len(functions)} functions in {file_path}")
            return {
                "status": "success",
                "message": f"Found {len(functions)} functions in {file_path}",
                "file_path": file_path,
                "functions": functions,
                "function_count": len(functions),
            }
        except Exception as e:
            logger.error(f"Error analyzing Python file {file_path}: {e}")
            return {
                "status": "error",
                "message": f"Error analyzing Python file: {str(e)}",
                "functions": [],
            }

    def extract_agent_metadata(self, agent_path: str) -> Dict[str, Any]:
        """
        Extract metadata from agent directory (README, config files)

        Args:
            agent_path: Path to agent directory

        Returns:
            Dictionary with extracted metadata (description, etc.)
        """
        logger.debug(f"Extracting metadata from agent at: {agent_path}")
        try:
            base_path = Path(agent_path)
            metadata = {
                "agent_name": base_path.name,
                "description": None,
                "dependencies": [],
            }

            # Read README for description
            readme_path = base_path / "README.md"
            if readme_path.exists():
                logger.debug(f"Reading README from: {readme_path}")
                with open(readme_path, "r", encoding="utf-8") as f:
                    readme = f.read()

                desc_match = re.search(r"#[^#\n]+\n+(.+?)(?:\n\n|$)", readme, re.DOTALL)
                if desc_match:
                    metadata["description"] = desc_match.group(1).strip()
                    logger.debug(
                        f"Found description: {metadata['description'][:50]}..."
                    )

            # Try to populate dependencies list from pyproject for metadata purposes
            # (kept light for basic info, though framework detection uses detect_agent_framework now)
            pyproject_path = base_path / "pyproject.toml"
            if pyproject_path.exists():
                try:
                    with open(pyproject_path, "r", encoding="utf-8") as f:
                        content = f.read()
                    deps_match = re.search(
                        r"dependencies\s*=\s*\[(.*?)\]", content, re.DOTALL
                    )
                    if deps_match:
                        deps_str = deps_match.group(1)
                        deps = re.findall(r'"([^"]+)"', deps_str)
                        metadata["dependencies"] = [d.split(">=")[0] for d in deps]
                except:
                    pass

            logger.info(
                f"Successfully extracted metadata from {agent_path}: name={metadata['agent_name']}"
            )
            return {
                "status": "success",
                "message": f"Extracted metadata from {agent_path}",
                "metadata": metadata,
            }
        except Exception as e:
            logger.error(f"Error extracting metadata from {agent_path}: {e}")
            return {
                "status": "error",
                "message": f"Error extracting metadata: {str(e)}",
                "metadata": {},
            }

    def detect_transport_protocol(self, file_path: str) -> Dict[str, Any]:
        """
        Detect transport protocol(s) from Python server file using AST parsing.

        This is a generic detector that works by analyzing the code structure:
        - Looks at imports to identify server frameworks
        - Analyzes route definitions and decorators
        - Detects RPC vs REST patterns
        - Follows imports to analyze related files

        Args:
            file_path: Path to main server file (__main__.py, main.py, app.py)

        Returns:
            Dictionary with detected transport info:
            {
                "status": "success",
                "preferred_transport": "JSONRPC" | "HTTP+JSON" | "WebSocket",
                "confidence": "high" | "medium" | "low",
                "evidence": [list of detection reasons],
                "additional_transports": [optional list of other detected transports]
            }
        """
        logger.debug(f"Detecting transport protocol from: {file_path}")

        try:
            with open(file_path, "r", encoding="utf-8") as f:
                content = f.read()

            # Parse the Python file into AST
            try:
                tree = ast.parse(content)
            except SyntaxError as e:
                return {
                    "status": "error",
                    "message": f"Syntax error parsing {file_path}: {str(e)}",
                    "preferred_transport": "JSONRPC",  # Default fallback
                }

            # Check if this file imports and uses a local module for app creation
            # (e.g., from api import create_agent_app)
            local_imports = []
            for node in ast.walk(tree):
                if isinstance(node, ast.ImportFrom):
                    module = node.module or ""
                    # Check for local imports (no dots, not standard library)
                    if (
                        module
                        and not module.startswith(".")
                        and module not in ["os", "sys", "logging", "dotenv"]
                    ):
                        for alias in node.names:
                            if (
                                "app" in alias.name.lower()
                                or "create" in alias.name.lower()
                            ):
                                local_imports.append(module)
                                logger.debug(
                                    f"Found potential app creation import: from {module} import {alias.name}"
                                )

            # If we found local imports, try to analyze those files too
            additional_files = []
            if local_imports:
                base_dir = Path(file_path).parent
                for module_name in local_imports:
                    potential_file = base_dir / f"{module_name}.py"
                    if potential_file.exists():
                        additional_files.append(str(potential_file))
                        logger.debug(f"Will also analyze: {potential_file}")

            # Evidence collection
            evidence = []
            transports = set()

            # Analyze main file first
            files_to_analyze = [file_path] + additional_files

            for analyze_file in files_to_analyze:
                if analyze_file != file_path:
                    logger.debug(f"Analyzing imported file: {analyze_file}")
                    try:
                        with open(analyze_file, "r", encoding="utf-8") as f:
                            imported_content = f.read()
                        imported_tree = ast.parse(imported_content)
                    except:
                        continue
                else:
                    imported_tree = tree

                # Analyze imports
                for node in ast.walk(imported_tree):
                    if isinstance(node, ast.ImportFrom):
                        module = node.module or ""
                        names = [alias.name for alias in node.names]

                        # RPC/A2A indicators
                        if "a2a.server" in module:
                            evidence.append(
                                f"Imports from a2a.server module: {', '.join(names)}"
                            )
                            transports.add("JSONRPC")

                        if any(
                            "RPC" in name or "rpc" in name.lower() for name in names
                        ):
                            evidence.append(f"RPC-related imports: {', '.join(names)}")
                            transports.add("JSONRPC")

                        # REST/HTTP indicators
                        if module == "fastapi" and "FastAPI" in names:
                            evidence.append(
                                f"Imports FastAPI framework in {analyze_file}"
                            )
                            # Note: FastAPI can be used for both RPC and REST

                        if module == "flask":
                            evidence.append("Imports Flask framework")
                            transports.add("HTTP+JSON")

                        # WebSocket indicators
                        if "websocket" in module.lower() or any(
                            "websocket" in name.lower() for name in names
                        ):
                            evidence.append(f"WebSocket imports detected: {module}")
                            transports.add("WebSocket")

                # Analyze function/method calls and class instantiations
                a2a_detected = False
                for node in ast.walk(imported_tree):
                    if isinstance(node, ast.Call):
                        # Look for A2A application setup
                        if isinstance(node.func, ast.Name):
                            func_name = node.func.id
                            if "A2A" in func_name:
                                a2a_detected = True
                                evidence.append(f"Found A2A application: {func_name}")

                        # Look for .routes() or .build() calls on A2A app
                        if isinstance(node.func, ast.Attribute):
                            attr_name = node.func.attr
                            if attr_name in ["routes", "build"]:
                                # Check if routes() is called with explicit transport parameters
                                # By default, A2A uses JSONRPC via DEFAULT_RPC_URL
                                if a2a_detected or "JSONRPC" in str(transports):
                                    evidence.append(
                                        f"Found A2A method call: .{attr_name}()"
                                    )
                                    # Default A2A transport is JSONRPC
                                    transports.add("JSONRPC")

                # Analyze decorators and function bodies (for REST vs RPC differentiation)
                for node in ast.walk(imported_tree):
                    if isinstance(node, ast.FunctionDef):
                        # Check decorators for specific endpoint patterns
                        for decorator in node.decorator_list:
                            decorator_str = (
                                ast.unparse(decorator)
                                if hasattr(ast, "unparse")
                                else str(decorator)
                            )

                            # Look for REST-style endpoints (non-RPC)
                            # Check if it's a specific resource endpoint (not generic /rpc or /jsonrpc)
                            if any(
                                pattern in decorator_str
                                for pattern in [
                                    "app.post",
                                    "app.get",
                                    "app.put",
                                    "app.delete",
                                    "router.post",
                                    "router.get",
                                ]
                            ):
                                # Check if this is likely a REST endpoint by looking at the route
                                # Exclude generic RPC endpoints like /rpc, /jsonrpc, /a2a
                                if not any(
                                    rpc_pattern in decorator_str.lower()
                                    for rpc_pattern in [
                                        "/rpc",
                                        "/jsonrpc",
                                        "/a2a",
                                        "rpc_url",
                                    ]
                                ):
                                    # This looks like a REST endpoint
                                    if (
                                        "JSONRPC" not in transports
                                    ):  # Only count if not already detected as RPC via A2A
                                        transports.add("HTTP+JSON")
                                        evidence.append(
                                            f"Found REST-style endpoint in {analyze_file}: {decorator_str}"
                                        )

            # Determine preferred transport based on evidence
            if "JSONRPC" in transports:
                preferred = "JSONRPC"
                confidence = "high"
            elif "HTTP+JSON" in transports:
                preferred = "HTTP+JSON"
                confidence = "medium"
            elif "WebSocket" in transports:
                preferred = "WebSocket"
                confidence = "medium"
            else:
                # Default fallback
                preferred = "JSONRPC"
                confidence = "low"
                evidence.append(
                    "No strong transport indicators found, defaulting to JSONRPC"
                )

            # Remove preferred from transports set for additional_transports
            additional = list(transports - {preferred}) if len(transports) > 1 else []

            logger.info(f"Detected transport: {preferred} (confidence: {confidence})")
            return {
                "status": "success",
                "preferred_transport": preferred,
                "confidence": confidence,
                "evidence": evidence,
                "additional_transports": additional,
            }

        except Exception as e:
            logger.error(f"Error detecting transport protocol from {file_path}: {e}")
            return {
                "status": "error",
                "message": f"Error detecting transport: {str(e)}",
                "preferred_transport": "JSONRPC",  # Default fallback
            }

    def detect_agent_framework(self, file_path: str) -> Dict[str, Any]:
        """
        Detect agent framework by recursively analyzing Python imports and usage.
        Prioritizes orchestration frameworks (LangChain, CrewAI) over direct SDKs (OpenAI).
        Explicitly ignores protocol/transport libraries (A2A, FastAPI).

        Args:
            file_path: Path to main server file or entry point

        Returns:
            Dictionary with detected framework, confidence, and evidence
        """
        logger.debug(f"Detecting agent framework recursively from: {file_path}")

        try:
            # BFS/DFS initialization
            visited_files = set()
            files_to_visit = [Path(file_path).resolve()]
            all_imports = set()
            
            base_dir = Path(file_path).parent.resolve()

            while files_to_visit:
                current_file = files_to_visit.pop(0)
                
                if current_file in visited_files:
                    continue
                visited_files.add(current_file)

                if not current_file.exists():
                    continue

                try:
                    with open(current_file, "r", encoding="utf-8") as f:
                        content = f.read()
                    tree = ast.parse(content)
                except Exception as e:
                    logger.warning(f"Failed to parse {current_file}: {e}")
                    continue

                # Analyze imports in this file
                for node in ast.walk(tree):
                    if isinstance(node, ast.Import):
                        for alias in node.names:
                            root_module = alias.name.split('.')[0].lower()
                            all_imports.add(root_module)
                    elif isinstance(node, ast.ImportFrom):
                        if node.module:
                            # Handle relative imports
                            if node.level > 0:
                                # Resolve relative import to file path
                                # This is complex to do perfectly, so we'll do a best-effort
                                # based on determining if it maps to a local file
                                pass 
                            else:
                                root_module = node.module.split('.')[0].lower()
                                all_imports.add(root_module)
                                
                                # Check if this is a local module to visit
                                # Heuristic: if module exists as .py file in same dir, visit it
                                # This works for 'from api import ...' style
                                module_path = base_dir / f"{node.module.replace('.', '/')}.py"
                                if module_path.exists() and module_path not in visited_files:
                                    files_to_visit.append(module_path)
                                    
                            # Also check explicitly if any imported names are modules themselves
                            # not entirely standard but covers some patterns

            evidence = []
            candidates = []
            
            # Common standard library modules to filter out (not exhaustive but covers most)
            stdlib_modules = {
                "os", "sys", "json", "logging", "asyncio", "typing", "datetime", "time", 
                "pathlib", "re", "math", "random", "uuid", "abc", "argparse", "functools", 
                "itertools", "collections", "copy", "threading", "subprocess", "warnings",
                "io", "tempfile", "shutil", "glob", "gzip", "tarfile", "zipfile", "csv",
                "unittest", "doctest", "pydoc", "inspect", "traceback", "pdb", "pickle",
                "shelve", "dbm", "sqlite3", "zlib", "hashlib", "hmac", "secrets", 
                "urllib", "http", "ftplib", "smtplib", "poplib", "imaplib", "nntplib",
                "telnetlib", "xml", "html", "cgi", "socket", "ssl", "select", "selectors",
                "asyncore", "asynchat", "signal", "mmap", "email", "json", "base64",
                "binascii", "quopri", "contextlib", "dataclasses", "enum", "numbers",
                "decimal", "fractions", "statistics", "textwrap", "string", "struct",
                "codecs", "unicodedata"
            }
            
            non_stdlib_imports = [imp for imp in all_imports if imp not in stdlib_modules and not imp.startswith("_")]

            # 1. Check for Orchestration Frameworks
            orchestration_frameworks = {
                "crewai": "CrewAI", 
                "langchain": "LangChain", 
                "llama_index": "LlamaIndex", 
                "autogen": "AutoGen", 
                "phidata": "PhiData",
                "semantic_kernel": "Semantic Kernel"
            }
            
            for key, name in orchestration_frameworks.items():
                # Check for exact match OR prefix match (e.g. langchain_openai, langchain_core)
                if key in all_imports or any(imp.startswith(f"{key}") for imp in all_imports):
                    candidates.append({"name": key, "type": "orchestration", "confidence": "high"})
                    evidence.append(f"Found orchestration framework import: {key} (or submodule)")
            
            # 2. Check for Direct LLM SDKs
            llm_sdks = {
                "openai": "OpenAI",
                "anthropic": "Anthropic",
                "google": "Google Generative AI", # google.generativeai
                "mistralai": "Mistral AI",
                "cohere": "Cohere"
            }
                
            for key, name in llm_sdks.items():
                # Handle special case for google.generativeai
                if key == "google":
                    if "google" in all_imports:
                            candidates.append({"name": "google-generativeai", "type": "sdk", "confidence": "medium"})
                            evidence.append(f"Found direct LLM SDK usage: {name}")
                # Check for exact match OR prefix match
                elif key in all_imports or any(imp.startswith(f"{key}") for imp in all_imports):
                    candidates.append({"name": key, "type": "sdk", "confidence": "medium"})
                    evidence.append(f"Found direct LLM SDK usage: {name}")
            
            # 3. Check for Protocol Libraries (Information only)
            protocol_libs = ["a2a", "a2a-sdk", "fastapi", "flask", "starlette", "uvicorn", "flet", "streamlit"]
            for lib in protocol_libs:
                if lib in all_imports:
                    evidence.append(f"Found protocol library (should NOT be agentFramework): {lib}")

            logger.info(f"Framework candidates: {candidates}")
            return {
                "status": "success",
                "candidates": candidates,
                "all_imports": sorted(non_stdlib_imports),
                "evidence": evidence
            }

        except Exception as e:
            logger.error(f"Error detecting agent framework: {e}")
            return {
                "status": "error",
                "message": str(e),
                "candidates": [],
                "all_imports": [],
                "evidence": []
            }

    def generate_agentcard_json(
        self,
        agent_name: str,
        description: str,
        skills: List[Dict[str, Any]],
        port: int = 10000,
        version: str = "1.0.0",
        streaming: bool = False,
        push_notifications: bool = False,
        state_transition_history: bool = False,
        chat_agent: bool = False,
        default_input_modes: List[str] = None,
        default_output_modes: List[str] = None,
        preferred_transport: str = "JSONRPC",
        additional_interfaces: List[Dict[str, str]] = None,
        agentFramework: str = "",
    ) -> Dict[str, Any]:
        """
        Generate A2A-compliant AgentCard JSON

        Args:
            agent_name: Name of the agent
            description: Agent description
            skills: List of skill definitions (must have id, name, description, tags, examples)
            port: Server port
            version: Agent version
            streaming: Whether agent supports streaming (default: False)
            push_notifications: Whether agent supports push notifications (default: False)
            state_transition_history: Whether agent tracks state history (default: False)
            chat_agent: Whether agent implements chat API (non-A2A compatible chat endpoint) (default: False)
            default_input_modes: List of supported input MIME types (default: ["application/json", "text/plain"])
            default_output_modes: List of supported output MIME types (default: ["application/json"])
            preferred_transport: Preferred transport protocol (default: "JSONRPC")
            additional_interfaces: List of additional interface dicts with "url" and "transport" keys (optional)
            agentFramework: The detected framework used to build the agent (optional)

        Returns:
            Dictionary with the generated AgentCard
        """
        logger.info(f"Generating AgentCard for: {agent_name}")
        logger.debug(
            f"AgentCard parameters: port={port}, version={version}, streaming={streaming}, push_notifications={push_notifications}, state_transition_history={state_transition_history}"
        )
        try:
            # Set defaults if not provided
            if default_input_modes is None:
                default_input_modes = ["application/json", "text/plain"]
            if default_output_modes is None:
                default_output_modes = ["application/json"]

            # Normalize MIME types (convert shorthand like 'text' to 'text/plain')
            def normalize_mime(mime_type: str) -> str:
                if mime_type == "text":
                    return "text/plain"
                elif mime_type == "json":
                    return "application/json"
                elif mime_type == "image":
                    return "image/png"
                return mime_type

            default_input_modes = [normalize_mime(m) for m in default_input_modes]
            default_output_modes = [normalize_mime(m) for m in default_output_modes]

            # Build AgentCard structure
            agentcard = {
                "protocolVersion": "0.2.9",
                "name": agent_name,
                "description": description,
                "url": f"http://localhost:{port}/",
                "agentFramework": agentFramework,
                "preferredTransport": preferred_transport,
                "provider": {
                    "organization": "Nasiko AI Projects",
                    "url": "https://github.com/ashishsharma/nasiko",
                },
                "iconUrl": f"http://localhost:{port}/icon.png",
                "version": version,
                "documentationUrl": f"http://localhost:{port}/docs",
                "capabilities": {
                    "streaming": streaming,
                    "pushNotifications": push_notifications,
                    "stateTransitionHistory": state_transition_history,
                    "chat_agent": chat_agent,
                },
                "securitySchemes": {},
                "security": [],
                "defaultInputModes": default_input_modes,
                "defaultOutputModes": default_output_modes,
                "skills": skills,
                "supportsAuthenticatedExtendedCard": False,
                "signatures": [],
            }

            # Only include additionalInterfaces if provided and non-empty
            if additional_interfaces and len(additional_interfaces) > 0:
                agentcard["additionalInterfaces"] = additional_interfaces
                logger.debug(
                    f"Added {len(additional_interfaces)} additional interfaces"
                )

            logger.info("AgentCard generated successfully")
            return {
                "status": "success",
                "message": "Generated AgentCard ",
                "agentcard": agentcard,
            }

        except Exception as e:
            logger.error(f"Error generating AgentCard: {e}")
            return {
                "status": "error",
                "message": f"Error generating AgentCard: {str(e)}",
                "agentcard": None,
            }

    def get_available_tools(self) -> List[str]:
        """Return list of available tool names"""
        return [
            "glob_files",
            "read_file",
            "grep_code",
            "analyze_python_functions",
            "extract_agent_metadata",
            "detect_transport_protocol",
            "detect_agent_framework",
            "generate_agentcard_json",
        ]

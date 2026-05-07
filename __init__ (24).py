"""Static parser for MCP tool, resource, and prompt definitions.

Extracts metadata from Python source code using ast.parse() only.
No exec(), eval(), import, or subprocess -- pure static analysis.

Part of the Nasiko MCP Manifest Generator (R3).
"""

from __future__ import annotations

import ast
from typing import TypedDict

# ---------------------------------------------------------------------------
class ParameterDefinition(TypedDict):
    name: str
    type: str
    json_schema: dict
    required: bool


class ToolDefinition(TypedDict):
    name: str
    docstring: str | None
    parameters: list[ParameterDefinition]


class ResourceDefinition(TypedDict):
    """An MCP resource discovered from @mcp.resource('uri') decorators."""
    uri: str
    name: str
    docstring: str | None


class PromptDefinition(TypedDict):
    """An MCP prompt discovered from @mcp.prompt() decorators."""
    name: str
    docstring: str | None
    parameters: list[ParameterDefinition]

# ---------------------------------------------------------------------------
TOOL_HOSTS: set[str] = {"mcp", "server", "app"}

_TYPE_MAP: dict[str, dict] = {
    "int": {"type": "integer"},
    "float": {"type": "number"},
    "str": {"type": "string"},
    "bool": {"type": "boolean"},
    "dict": {"type": "object"},
    "list": {"type": "array"},
}

_SUFFIX_MAP: dict[str, dict] = {
    "Dict": {"type": "object"},
    "List": {"type": "array"},
    "Set": {"type": "array"},
    "Tuple": {"type": "array"},
    "Int": {"type": "integer"},
    "Float": {"type": "number"},
    "Str": {"type": "string"},
    "Bool": {"type": "boolean"},
}

_FALLBACK_SCHEMA: dict = {"type": "object", "properties": {}, "required": []}

def _is_tool_decorator(dec: ast.expr) -> tuple[bool, str | None]:
    """Determine if *dec* is a recognised ``@<host>.tool`` decorator.

    Returns ``(True, name_override | None)`` when matched, otherwise
    ``(False, None)``.

    Order matters: check ``ast.Attribute`` **before** ``ast.Call`` because
    the no-parens form (``@app.tool``) produces a bare ``Attribute`` node.
    """

    # Pattern 3: @app.tool  (no parentheses -> bare Attribute)
    if isinstance(dec, ast.Attribute) and not isinstance(dec, ast.Call):
        if (
            dec.attr == "tool"
            and isinstance(dec.value, ast.Name)
            and dec.value.id in TOOL_HOSTS
        ):
            return True, None

    # Patterns 1, 2, 4: @mcp.tool() / @server.tool() / @mcp.tool(name=...)
    if isinstance(dec, ast.Call):
        func = dec.func
        if (
            isinstance(func, ast.Attribute)
            and func.attr == "tool"
            and isinstance(func.value, ast.Name)
            and func.value.id in TOOL_HOSTS
        ):
            # Look for name= keyword override (pattern 4).
            name_override: str | None = None
            for kw in dec.keywords:
                if kw.arg == "name" and isinstance(kw.value, ast.Constant):
                    candidate = kw.value.value
                    # Empty-string override -> treat as no override.
                    if isinstance(candidate, str) and candidate:
                        name_override = candidate
            return True, name_override

    return False, None


def _is_resource_decorator(dec: ast.expr) -> tuple[bool, str | None]:
    """Determine if *dec* is a recognised ``@<host>.resource(uri)`` decorator.

    Returns ``(True, uri_string | None)`` when matched.
    """
    if isinstance(dec, ast.Call):
        func = dec.func
        if (
            isinstance(func, ast.Attribute)
            and func.attr == "resource"
            and isinstance(func.value, ast.Name)
            and func.value.id in TOOL_HOSTS
        ):
            # Extract the URI from the first positional arg
            uri: str | None = None
            if dec.args and isinstance(dec.args[0], ast.Constant):
                uri = str(dec.args[0].value)
            # Also check uri= keyword
            for kw in dec.keywords:
                if kw.arg == "uri" and isinstance(kw.value, ast.Constant):
                    uri = str(kw.value.value)
            return True, uri

    return False, None


def _is_prompt_decorator(dec: ast.expr) -> tuple[bool, str | None]:
    """Determine if *dec* is a recognised ``@<host>.prompt()`` decorator.

    Returns ``(True, name_override | None)`` when matched.
    """
    # Bare form: @mcp.prompt
    if isinstance(dec, ast.Attribute) and not isinstance(dec, ast.Call):
        if (
            dec.attr == "prompt"
            and isinstance(dec.value, ast.Name)
            and dec.value.id in TOOL_HOSTS
        ):
            return True, None

    # Call form: @mcp.prompt() / @mcp.prompt(name="...")
    if isinstance(dec, ast.Call):
        func = dec.func
        if (
            isinstance(func, ast.Attribute)
            and func.attr == "prompt"
            and isinstance(func.value, ast.Name)
            and func.value.id in TOOL_HOSTS
        ):
            name_override: str | None = None
            for kw in dec.keywords:
                if kw.arg == "name" and isinstance(kw.value, ast.Constant):
                    candidate = kw.value.value
                    if isinstance(candidate, str) and candidate:
                        name_override = candidate
            return True, name_override

    return False, None


def _map_type(annotation_str: str | None) -> dict:
    """Map a raw annotation string to a JSON Schema object.

    Strips ``Optional[]``, unwraps generics to their base type, and falls
    back to the catch-all object schema for unknown types.
    """

    if annotation_str is None:
        return dict(_FALLBACK_SCHEMA)

    raw = annotation_str.strip()

    # Unwrap Optional[X] -> X
    if raw.startswith("Optional[") and raw.endswith("]"):
        raw = raw[len("Optional["):-1].strip()

    # Unwrap Union[X, None] / X | None patterns to X
    if raw.startswith("Union[") and raw.endswith("]"):
        inner = raw[len("Union["):-1]
        parts = [p.strip() for p in inner.split(",") if p.strip() != "None"]
        if parts:
            raw = parts[0]

    if " | " in raw:
        parts = [p.strip() for p in raw.split(" | ") if p.strip() != "None"]
        if parts:
            raw = parts[0]

    # Extract base type before any generic bracket.
    base = raw.split("[")[0].strip()

    # Direct match in type map.
    if base in _TYPE_MAP:
        return dict(_TYPE_MAP[base])

    # Suffix heuristic (e.g. OrderedDict -> object, FrozenList -> array).
    for suffix, schema in _SUFFIX_MAP.items():
        if base.endswith(suffix):
            return dict(schema)

    # Case-insensitive last resort.
    base_lower = base.lower()
    for key, schema in _TYPE_MAP.items():
        if base_lower == key:
            return dict(schema)

    return dict(_FALLBACK_SCHEMA)


def _extract_params(fn: ast.FunctionDef | ast.AsyncFunctionDef) -> list[ParameterDefinition]:
    """Extract parameter definitions from a function node.

    Skips ``self`` and any parameter named ``ctx`` or annotated as
    ``Context``.  Handles right-aligned defaults correctly.
    """

    args_node = fn.args
    all_args: list[ast.arg] = args_node.args

    # Right-aligned defaults: if there are fewer defaults than args, the
    # first N args have no default.
    num_args = len(all_args)
    num_defaults = len(args_node.defaults)
    first_default_index = num_args - num_defaults

    params: list[ParameterDefinition] = []

    for idx, arg in enumerate(all_args):
        name = arg.arg

        # Skip self.
        if name == "self":
            continue

        # Skip ctx / Context.
        if name == "ctx":
            continue
        if arg.annotation is not None:
            try:
                ann_str = ast.unparse(arg.annotation)
            except Exception:
                ann_str = None
            if ann_str == "Context":
                continue
        else:
            ann_str = None

        # Determine annotation string (may already be set above).
        if arg.annotation is not None and ann_str is None:
            try:
                ann_str = ast.unparse(arg.annotation)
            except Exception:
                ann_str = None

        has_default = idx >= first_default_index

        params.append(
            ParameterDefinition(
                name=name,
                type=ann_str if ann_str is not None else "Any",
                json_schema=_map_type(ann_str),
                required=not has_default,
            )
        )

    return params

def _try_extract_tool(fn: ast.FunctionDef | ast.AsyncFunctionDef) -> ToolDefinition | None:
    """Return a ``ToolDefinition`` if *fn* has a tool decorator, else ``None``."""

    for dec in fn.decorator_list:
        is_tool, name_override = _is_tool_decorator(dec)
        if is_tool:
            tool_name = name_override if name_override else fn.name
            docstring: str | None = ast.get_docstring(fn)
            parameters = _extract_params(fn)

            return ToolDefinition(
                name=tool_name,
                docstring=docstring,
                parameters=parameters,
            )

    return None


def _try_extract_resource(fn: ast.FunctionDef | ast.AsyncFunctionDef) -> ResourceDefinition | None:
    """Return a ``ResourceDefinition`` if *fn* has a resource decorator, else ``None``."""

    for dec in fn.decorator_list:
        is_resource, uri = _is_resource_decorator(dec)
        if is_resource:
            docstring: str | None = ast.get_docstring(fn)
            return ResourceDefinition(
                uri=uri if uri else f"resource://{fn.name}",
                name=fn.name,
                docstring=docstring,
            )

    return None


def _try_extract_prompt(fn: ast.FunctionDef | ast.AsyncFunctionDef) -> PromptDefinition | None:
    """Return a ``PromptDefinition`` if *fn* has a prompt decorator, else ``None``."""

    for dec in fn.decorator_list:
        is_prompt, name_override = _is_prompt_decorator(dec)
        if is_prompt:
            prompt_name = name_override if name_override else fn.name
            docstring: str | None = ast.get_docstring(fn)
            parameters = _extract_params(fn)

            return PromptDefinition(
                name=prompt_name,
                docstring=docstring,
                parameters=parameters,
            )

    return None


def _process_function_node(
    fn: ast.FunctionDef | ast.AsyncFunctionDef,
    tools: list[ToolDefinition],
    resources: list[ResourceDefinition],
    prompts: list[PromptDefinition],
) -> None:
    """Try to extract a tool, resource, or prompt from a function node."""
    tool = _try_extract_tool(fn)
    if tool is not None:
        tools.append(tool)
        return

    resource = _try_extract_resource(fn)
    if resource is not None:
        resources.append(resource)
        return

    prompt = _try_extract_prompt(fn)
    if prompt is not None:
        prompts.append(prompt)

def parse_tools(source_code: str) -> list[ToolDefinition]:
    """Parse Python *source_code* and return all MCP tool definitions.

    Only **top-level** functions decorated with a recognised
    ``@<host>.tool`` pattern are returned.  Nested function definitions
    are ignored.

    Raises ``ValueError`` when *source_code* is not valid Python.
    """

    if not source_code or not source_code.strip():
        return []

    try:
        tree = ast.parse(source_code)
    except SyntaxError as e:
        raise ValueError(f"Invalid Python: {e}") from e

    tools: list[ToolDefinition] = []

    # Only iterate top-level statements -- never recurse into function
    # bodies, which avoids capturing nested ``def``s.
    for node in ast.iter_child_nodes(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            # Handle class methods: iterate class body for decorated methods.
            if isinstance(node, ast.ClassDef):
                for class_item in node.body:
                    if isinstance(class_item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                        tool = _try_extract_tool(class_item)
                        if tool is not None:
                            tools.append(tool)
            continue

        tool = _try_extract_tool(node)
        if tool is not None:
            tools.append(tool)

    return tools


def parse_all(source_code: str) -> tuple[
    list[ToolDefinition],
    list[ResourceDefinition],
    list[PromptDefinition],
]:
    """Parse Python *source_code* and return all MCP definitions.

    Returns a tuple of (tools, resources, prompts).

    Only **top-level** functions and class methods decorated with recognised
    ``@<host>.tool``, ``@<host>.resource``, or ``@<host>.prompt`` patterns
    are returned.  Nested function definitions are ignored.

    Raises ``ValueError`` when *source_code* is not valid Python.
    """

    if not source_code or not source_code.strip():
        return [], [], []

    try:
        tree = ast.parse(source_code)
    except SyntaxError as e:
        raise ValueError(f"Invalid Python: {e}") from e

    tools: list[ToolDefinition] = []
    resources: list[ResourceDefinition] = []
    prompts: list[PromptDefinition] = []

    for node in ast.iter_child_nodes(tree):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            _process_function_node(node, tools, resources, prompts)
        elif isinstance(node, ast.ClassDef):
            for class_item in node.body:
                if isinstance(class_item, (ast.FunctionDef, ast.AsyncFunctionDef)):
                    _process_function_node(class_item, tools, resources, prompts)

    return tools, resources, prompts

import os
import ast
import uuid
from typing import Set, Optional, List
from .models import IngestionRecord, ArtifactType, DetectionConfidence
from .exceptions import AmbiguousArtifactError, MissingStructureError

# Guard against zip-bomb / DoS — abort if more than this many .py files
MAX_PY_FILES = 500

# Entry point priority: server.py > main.py > agent.py > app.py > fallback
_ENTRY_PRIORITY = ['server.py', 'main.py', 'agent.py', 'app.py']


def detect_artifact_type(source_path: str) -> IngestionRecord:
    """
    AST-only detection of artifact type from Python source files.
    NO exec/eval/subprocess — only static analysis.

    Single os.walk pass collects all metadata (py files, entry point,
    agentcard, requirements) to avoid redundant directory traversals.
    """

    # ── STEP 1: Single os.walk pass — collect all metadata ──────────
    py_files: List[str] = []
    agentcard_exists: bool = False
    requirements_path: Optional[str] = None
    entry_point: Optional[str] = None
    fallback_entry: Optional[str] = None

    for root, _, files in os.walk(source_path):
        for file in sorted(files):
            # Agentcard detection
            if file == 'agentcard.json':
                agentcard_exists = True

            # Requirements detection (first match wins)
            if file == 'requirements.txt' and requirements_path is None:
                requirements_path = os.path.relpath(
                    os.path.join(root, file), source_path
                )

            # Collect .py files with DoS guard
            if file.endswith('.py'):
                if len(py_files) >= MAX_PY_FILES:
                    raise AmbiguousArtifactError(
                        f"Too many Python files (>{MAX_PY_FILES}), aborting detection"
                    )
                full_path = os.path.join(root, file)
                py_files.append(full_path)

                # Entry point resolution
                rel_path = os.path.relpath(full_path, source_path)
                if file in _ENTRY_PRIORITY:
                    if entry_point is None or _ENTRY_PRIORITY.index(file) < _ENTRY_PRIORITY.index(
                        os.path.basename(entry_point)
                    ):
                        entry_point = rel_path
                elif fallback_entry is None:
                    fallback_entry = rel_path

    # STEP 4: Enforce strict structure contract
    # Must have src/main.py, Dockerfile, docker-compose.yml
    expected_entry = os.path.join("src", "main.py")
    if not os.path.isfile(os.path.join(source_path, expected_entry)):
        raise MissingStructureError("Missing required file: src/main.py")
    
    if not os.path.isfile(os.path.join(source_path, "Dockerfile")):
        raise MissingStructureError("Missing required file: Dockerfile")
        
    if not os.path.isfile(os.path.join(source_path, "docker-compose.yml")) and not os.path.isfile(os.path.join(source_path, "docker-compose.yaml")):
        raise MissingStructureError("Missing required file: docker-compose.yml")

    entry_point = os.path.join("src", "main.py")

    # ── STEP 2: AST analysis for framework imports ──────────────────
    signals: Set[str] = set()

    for py_file in py_files:
        try:
            with open(py_file, 'r', encoding='utf-8') as f:
                tree = ast.parse(f.read())

            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    for alias in node.names:
                        top_level = alias.name.split('.')[0]
                        if top_level in ('fastmcp', 'mcp'):
                            signals.add('mcp')
                        elif top_level.startswith('langchain'):
                            signals.add('langchain')
                        elif top_level == 'crewai':
                            signals.add('crewai')
                elif isinstance(node, ast.ImportFrom):
                    if node.module:
                        top_level = node.module.split('.')[0]
                        if top_level in ('fastmcp', 'mcp'):
                            signals.add('mcp')
                        elif top_level.startswith('langchain'):
                            signals.add('langchain')
                        elif top_level == 'crewai':
                            signals.add('crewai')

        except (SyntaxError, UnicodeDecodeError):
            # Skip unparseable or binary-content files
            continue

    # ── STEP 3: Validate signal count ───────────────────────────────
    if len(signals) == 0:
        raise AmbiguousArtifactError("No recognized framework imports found")
    if len(signals) > 1:
        raise AmbiguousArtifactError(f"Multiple frameworks detected: {sorted(signals)}")

    framework = list(signals)[0]
    artifact_type_map = {
        'mcp': ArtifactType.MCP_SERVER,
        'langchain': ArtifactType.LANGCHAIN_AGENT,
        'crewai': ArtifactType.CREWAI_AGENT
    }

    return IngestionRecord(
        artifact_id=str(uuid.uuid4()),
        source_path=source_path,
        artifact_type=artifact_type_map[framework],
        confidence=DetectionConfidence.HIGH,
        entry_point=entry_point,
        detected_framework=framework,
        requirements_path=requirements_path,
        agentcard_exists=agentcard_exists
    )

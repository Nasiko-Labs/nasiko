"""The shared execution and result contracts used by every adapter."""

import json
from dataclasses import asdict, dataclass, field
from pathlib import Path, PurePosixPath


class EvalError(Exception):
    """An actionable, fail-closed evaluation error."""


@dataclass
class ExecutionPlan:
    language: str
    framework: str
    runtime_image: str
    working_directory: str
    setup_commands: list[str]
    test_command: str
    result_format: str
    test_path_rules: list[str]
    discovery_evidence: list[str]
    runtime_requirements: dict = field(default_factory=dict)
    dependency_files: list[str] = field(default_factory=list)
    timeout_seconds: int = 180
    setup_timeout_seconds: int = 600
    memory_mib: int = 1024
    cpus: float = 1.0
    pids: int = 256
    schema_version: int = 1

    @property
    def unit(self):
        return f"{self.working_directory}:{self.framework}"

    def to_dict(self):
        return asdict(self)


@dataclass
class TestRun:
    status: str
    tests: dict[str, str]
    exit_code: int | None
    duration_seconds: float
    reason: str | None = None
    artifact: str | None = None

    def to_dict(self):
        return asdict(self)


def safe_path(value):
    if not isinstance(value, str):
        raise EvalError("Repository paths must be strings")
    path = PurePosixPath(value)
    if not value or path.is_absolute() or ".." in path.parts or "\\" in value:
        raise EvalError(f"Unsafe repository path: {value!r}")
    if any(
        p in {".git", ".env", ".ssh", ".aws", ".npmrc", ".pypirc"} for p in path.parts
    ):
        raise EvalError(f"Credential or history path is forbidden: {value}")
    return str(path)


def save_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")
    temp.replace(path)

import json
import logging
from pathlib import Path
from typing import Any, Dict

logger = logging.getLogger(__name__)

class OrchestrateState:
    """
    R4 State Manager.
    Logs orchestration operations to isolated /tmp directories without 
    intersecting native R1 codespaces.
    """
    def __init__(self, workflow_id: str):
        self.workflow_id = workflow_id
        self.workflow_dir = Path(f"/tmp/nasiko/{workflow_id}/orchestration/")
        self.state_file = self.workflow_dir / "state.json"
        
        self.workflow_dir.mkdir(parents=True, exist_ok=True)
        if not self.state_file.exists():
            self._save({"status": "initialized", "steps": []})

    def _load(self) -> Dict[str, Any]:
        with open(self.state_file, "r") as f:
            return json.load(f)

    def _save(self, state: Dict[str, Any]) -> None:
        import tempfile
        import os
        
        # Flaw 3: Atomic writes to prevent corruption.
        fd, temp_path = tempfile.mkstemp(dir=self.workflow_dir, suffix=".tmp")
        with os.fdopen(fd, "w") as f:
            json.dump(state, f, indent=4)
        
        os.replace(temp_path, self.state_file)

    def log_invocation(self, tool_name: str, args: Any, result: Any):
        state = self._load()
        state["steps"].append({
            "tool": tool_name,
            "args": args,
            "result": result
        })
        self._save(state)

    def complete(self):
        state = self._load()
        state["status"] = "completed"
        self._save(state)


# Flaw 2 Integration Hook
def mark_mcp_ready(artifact_id: str) -> None:
    """Invoked by the redis listener to safely swap backend orchestration flags."""
    logger.info(f"Orchestration State caching ready flip for {artifact_id}")
    # Updates the bridge configuration mock status 
    state = OrchestrateState(artifact_id)
    state.complete()


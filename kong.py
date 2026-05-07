class AmbiguousArtifactError(Exception):
    """HTTP 422 — never silenced"""
    def __init__(self, reason: str):
        self.reason = reason
        super().__init__(reason)

class MissingStructureError(Exception):
    """HTTP 422 - strict agent directory boundary violation"""
    def __init__(self, reason: str):
        self.reason = reason
        super().__init__(reason)

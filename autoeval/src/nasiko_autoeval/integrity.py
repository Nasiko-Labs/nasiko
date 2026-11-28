"""Content checksums for frozen benchmark inputs and collected evidence."""

import hashlib
from pathlib import Path

from .schema import EvalError, safe_path


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def source_hashes():
    root = Path(__file__).resolve().parent
    return {path.name: sha256(path) for path in sorted(root.glob("*.py"))}


def verify_checksums(directory, checksums):
    root = Path(directory).resolve()
    for relative, expected in checksums.items():
        path = root / safe_path(relative)
        if path.is_symlink() or not path.is_file() or sha256(path) != expected:
            raise EvalError(f"Frozen evidence changed or disappeared: {relative}")

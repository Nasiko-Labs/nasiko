#!/usr/bin/env python3
"""Run without installing: python3 autoeval/run.py --help."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "src"))
from nasiko_autoeval.cli import main

if __name__ == "__main__":
    raise SystemExit(main())

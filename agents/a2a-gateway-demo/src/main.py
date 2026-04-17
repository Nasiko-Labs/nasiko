"""Entry point shim — delegates to __main__.py's click entry point.

The Nasiko agent project structure contract requires src/main.py.
The A2A SDK pattern uses __main__.py for the click CLI. Both are present;
the Dockerfile CMD points at __main__.py (same as a2a-translator).
"""
from __main__ import main  # type: ignore[import-not-found]

if __name__ == "__main__":
    main()

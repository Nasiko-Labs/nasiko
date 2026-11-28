"""Trusted, read-only driver copied into /eval, emitting an append-only event stream.

Events are still produced in the process importing candidate code. The host-side
collector protects files and rejects inconsistent streams; this is not a proof
against arbitrary in-process framework manipulation.
"""

import json
import os
import sys

import pytest

PREFIX = "\x1eNASIKO_PYTEST_V1 "


def main():
    token, *arguments = sys.argv[1:]
    sys.path.insert(0, os.getcwd())

    def emit(event):
        os.write(1, ("\n" + PREFIX + token + " " + json.dumps(event) + "\n").encode())

    class Observer:
        def pytest_runtest_logreport(self, report):
            emit(
                {
                    "event": "test",
                    "id": report.nodeid,
                    "phase": report.when,
                    "outcome": report.outcome,
                }
            )

        def pytest_collectreport(self, report):
            if report.failed:
                emit({"event": "collection_error", "id": report.nodeid})

    emit({"event": "start"})
    code = int(pytest.main(arguments, plugins=[Observer()]))
    emit({"event": "finish", "exit_code": code})
    return code


if __name__ == "__main__":
    raise SystemExit(main())

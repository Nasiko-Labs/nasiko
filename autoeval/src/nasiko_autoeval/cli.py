"""Command-line entry point; all toolchains enter the same workflow."""

import argparse
import json
import sys
from pathlib import Path

from .analyzer import inspect_repository, select_plan
from .nasiko import NasikoClient
from .report import aggregate_directory, write_report
from .schema import EvalError
from .workflow import run_workflow


def main(argv=None):
    parser = argparse.ArgumentParser(
        prog="autoeval",
        description="Nasiko AutoEval: one repository-fix workflow across toolchains",
    )
    commands = parser.add_subparsers(dest="command", required=True)
    inspect = commands.add_parser(
        "inspect", help="Discover manifest-backed project units"
    )
    inspect.add_argument("--repo", required=True, type=Path)
    inspect.add_argument("--unit")
    inspect.add_argument("--config", type=Path)
    for name in ("validate", "compare"):
        command = commands.add_parser(name)
        command.add_argument("--repo", required=True, type=Path)
        command.add_argument("--commit", required=True)
        command.add_argument("--unit")
        command.add_argument(
            "--config",
            type=Path,
            help="JSON ExecutionPlan overrides (trusted operator input)",
        )
        command.add_argument(
            "--issue-file",
            type=Path,
            help="Plain-language issue; excludes reference implementation details",
        )
        command.add_argument("--run", type=Path)
        if name == "compare":
            command.add_argument(
                "--model",
                action="append",
                required=True,
                help="Pinned config name/id, or priced provider:model (creates an autoeval config)",
            )
            command.add_argument("--max-output-tokens", type=int, default=8192)
            command.add_argument("--context-bytes", type=int, default=80000)
            command.add_argument(
                "--allow-cost-gaps",
                action="store_true",
                help="Continue correctness attempts with unverified pricing; reports keep cost N/A",
            )
            _server_arguments(command)
    report = commands.add_parser("report")
    report.add_argument("--run", type=Path, required=True)
    models = commands.add_parser(
        "models",
        help="List configurations; optionally verify generation and recorded cost",
    )
    models.add_argument("--probe", action="append", default=[])
    _server_arguments(models)
    args = parser.parse_args(argv)
    try:
        if args.command == "inspect":
            plans = inspect_repository(args.repo)
            if args.unit or args.config:
                overrides = json.loads(args.config.read_text()) if args.config else None
                plans = [select_plan(plans, args.unit, overrides)]
            print(json.dumps([p.to_dict() for p in plans], indent=2))
            return 0 if plans else 1
        if args.command in {"validate", "compare"}:
            return run_workflow(args)
        if args.command == "report":
            path = args.run / "results.json"
            results = json.loads(path.read_text()) if path.exists() else None
            if results is None or results.get("run_kind") == "aggregate":
                aggregate_directory(args.run)
            else:
                write_report(args.run, results)
            print((args.run / "report.md").read_text())
            return 0
        if args.probe:
            import uuid

            with NasikoClient(
                args.server_url, args.env_file, args.postgres_container
            ) as client:
                for selector in args.probe:
                    configuration = client.resolve_model(selector)
                    record = client.probe(
                        configuration,
                        "autoeval-probe-" + uuid.uuid4().hex[:12] + ":preflight",
                    )
                    print(json.dumps(record, indent=2))
        else:
            client = NasikoClient(
                args.server_url, args.env_file, args.postgres_container
            ).login()
            print(json.dumps(client.configurations(), indent=2))
        return 0
    except (EvalError, OSError, ValueError, KeyError) as error:
        print(f"autoeval: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print(
            "autoeval: interrupted; temporary containers and task images cleaned up",
            file=sys.stderr,
        )
        return 130


def _server_arguments(parser):
    parser.add_argument("--server-url", default="http://localhost:8080")
    parser.add_argument(
        "--env-file", type=Path, default=Path(__file__).resolve().parents[3] / ".env"
    )
    parser.add_argument(
        "--postgres-container",
        help="Older images only: read-only trace_usage access through docker exec psql (nasiko/nasiko_dev)",
    )


if __name__ == "__main__":
    raise SystemExit(main())

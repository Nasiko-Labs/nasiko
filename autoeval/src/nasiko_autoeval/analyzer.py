"""Manifest-driven discovery; adapters describe execution, never orchestrate it."""

import configparser
import fnmatch
import json
import os
import re
import shlex
import tomllib
from pathlib import Path

from .schema import EvalError, ExecutionPlan, safe_path

IGNORED = {".git", "node_modules", "target", ".venv", "vendor", "dist", ".cache"}
MANIFESTS = {
    "pyproject.toml",
    "pytest.ini",
    "setup.cfg",
    "setup.py",
    "requirements.txt",
    "package.json",
    "go.mod",
    "Cargo.toml",
}


def inspect_repository(root):
    root = Path(root).resolve()
    directories = set()
    for directory, subdirectories, filenames in os.walk(root, followlinks=False):
        subdirectories[:] = [
            name
            for name in subdirectories
            if name not in IGNORED and not (Path(directory) / name).is_symlink()
        ]
        if any(
            name in MANIFESTS and not (Path(directory) / name).is_symlink()
            for name in filenames
        ):
            directories.add(Path(directory))
    plans = []
    for directory in sorted(directories):
        for detector in (python_plan, node_plans, go_plan, rust_plan):
            plans.extend(detector(directory, root))
    return plans


def select_plan(plans, unit=None, overrides=None):
    choices = [p for p in plans if unit in (None, p.unit, p.working_directory)]
    if len(choices) != 1:
        raise EvalError(
            "Select one unambiguous --unit from: " + ", ".join(p.unit for p in plans)
        )
    plan = choices[0]
    if overrides:
        allowed = set(plan.to_dict()) - {
            "schema_version",
            "language",
            "framework",
            "working_directory",
        }
        unknown = set(overrides) - allowed
        if unknown:
            raise EvalError(f"Unknown/immutable override fields: {sorted(unknown)}")
        for key, value in overrides.items():
            setattr(plan, key, value)
        plan.discovery_evidence.append("Operator supplied explicit execution overrides")
    for field in (
        "setup_commands",
        "test_path_rules",
        "dependency_files",
        "discovery_evidence",
    ):
        value = getattr(plan, field)
        if not isinstance(value, list) or any(
            not isinstance(item, str) for item in value
        ):
            raise EvalError(f"{field} must be a list of strings")
    for field in ("runtime_image", "test_command", "result_format"):
        if not isinstance(getattr(plan, field), str) or not getattr(plan, field):
            raise EvalError(f"{field} must be a nonempty string")
    if plan.runtime_image.startswith("-") or any(
        c.isspace() for c in plan.runtime_image
    ):
        raise EvalError("runtime_image must be a Docker image reference")
    for field in (
        "timeout_seconds",
        "setup_timeout_seconds",
        "memory_mib",
        "pids",
        "cpus",
    ):
        if not isinstance(getattr(plan, field), (int, float)):
            raise EvalError(f"{field} must be numeric")
    if (
        plan.timeout_seconds <= 0
        or plan.memory_mib < 128
        or plan.pids < 16
        or plan.cpus <= 0
    ):
        raise EvalError(
            "Execution limits must be positive (memory >=128 MiB, pids >=16)"
        )
    if plan.setup_timeout_seconds <= 0:
        raise EvalError("Setup timeout must be positive")
    return plan


def _read(directory, name):
    path = directory / name
    return path.read_text() if path.is_file() and not path.is_symlink() else ""


def _plan(
    directory,
    root,
    language,
    framework,
    image,
    setup,
    command,
    result_format,
    rules,
    evidence,
    requirements=None,
):
    dependencies = [
        p.name
        for p in directory.iterdir()
        if p.is_file()
        and (
            p.name in MANIFESTS
            or p.name.endswith(".lock")
            or p.name
            in {
                "package-lock.json",
                "go.sum",
                "rust-toolchain.toml",
                ".python-version",
                ".nvmrc",
            }
        )
    ]
    return ExecutionPlan(
        language,
        framework,
        image,
        directory.relative_to(root).as_posix(),
        setup,
        command,
        result_format,
        rules,
        evidence,
        requirements or {},
        sorted(dependencies),
    )


def python_plan(directory, root):
    raw = _read(directory, "pyproject.toml")
    data = tomllib.loads(raw) if raw else {}
    pytest_ini = _read(directory, "pytest.ini")
    setup_cfg = _read(directory, "setup.cfg")
    requirements = "\n".join(
        p.read_text()
        for p in directory.glob("*requirements*.txt")
        if not p.is_symlink()
    )
    tool = data.get("tool", {}).get("pytest", {})
    deps = json.dumps(
        [
            data.get("project", {}).get("optional-dependencies", {}),
            data.get("dependency-groups", {}),
            data.get("project", {}).get("dependencies", []),
        ]
    )
    if not (
        tool
        or pytest_ini
        or "[tool:pytest]" in setup_cfg
        or re.search(r"\bpytest\b", requirements + deps)
    ):
        return []
    options = tool.get("ini_options", tool)
    testpaths = options.get("testpaths", [])
    if not testpaths:
        ini = configparser.ConfigParser()
        ini.read_string(pytest_ini or setup_cfg or "[pytest]\n")
        section = "pytest" if ini.has_section("pytest") else "tool:pytest"
        testpaths = ini.get(section, "testpaths", fallback="").split()
    if isinstance(testpaths, str):
        testpaths = testpaths.split()
    commands = ["python -m venv /opt/venv", "/opt/venv/bin/pip install 'pytest==8.4.2'"]
    if (directory / "requirements.txt").exists():
        commands.append("/opt/venv/bin/pip install -r requirements.txt")
    if (
        data.get("project")
        or (directory / "setup.py").exists()
        or "[metadata]" in setup_cfg
    ):
        commands.append(
            "SETUPTOOLS_SCM_PRETEND_VERSION=0.0.0 /opt/venv/bin/pip install -e ."
        )
    rules = ["**/test_*.py", "**/*_test.py", "test_*.py", "*_test.py"]
    rules += [safe_path(p).rstrip("/") + "/**" for p in testpaths]
    command = "/opt/venv/bin/python -m pytest " + " ".join(
        shlex.quote(safe_path(p)) for p in testpaths
    )
    command += (
        " -o addopts='' -o cache_dir=/tmp/pytest-cache --junitxml=/eval/out/results.xml"
    )
    requirement = data.get("project", {}).get("requires-python", "")
    pinned = _read(directory, ".python-version").strip()
    minimum = re.search(r">=?\s*(3\.\d+)", requirement)
    version = (
        pinned
        if re.fullmatch(r"3\.\d+(?:\.\d+)?", pinned)
        else "3." + str(max(12, int(minimum.group(1).split(".")[1])))
        if minimum
        else "3.12"
    )
    return [
        _plan(
            directory,
            root,
            "python",
            "pytest",
            f"python:{version}-slim",
            commands,
            command,
            "junit",
            rules,
            [
                "pytest configuration/dependency in manifest",
                f"testpaths={testpaths or 'pytest default discovery'}",
            ],
            {
                "requires-python": data.get("project", {}).get("requires-python"),
                "python-version-file": _read(directory, ".python-version").strip()
                or None,
            },
        )
    ]


def node_plans(directory, root):
    raw = _read(directory, "package.json")
    if not raw:
        return []
    data = json.loads(raw)
    deps = {**data.get("dependencies", {}), **data.get("devDependencies", {})}
    scripts = " ".join(data.get("scripts", {}).values())
    frameworks = [
        f for f in ("jest", "vitest") if f in deps or re.search(rf"\b{f}\b", scripts)
    ]
    plans = []
    for framework in frameworks:
        setup = [
            "npm ci --ignore-scripts"
            if (directory / "package-lock.json").exists()
            else "npm install --ignore-scripts"
        ]
        if (directory / "pnpm-lock.yaml").exists() or (
            directory / "yarn.lock"
        ).exists():
            setup = []  # Explicit setup override is required; never silently use another lockfile.
        binary = f"./node_modules/.bin/{framework}"
        command = (
            f"{binary} --runInBand --json --outputFile=/eval/out/results.json"
            if framework == "jest"
            else f"{binary} run --no-cache --maxWorkers=1 --minWorkers=1 --reporter=json --outputFile=/eval/out/results.json"
        )
        language = (
            "typescript"
            if "typescript" in deps or (directory / "tsconfig.json").exists()
            else "javascript"
        )
        engine = data.get("engines", {}).get("node", "")
        nvmrc = _read(directory, ".nvmrc").strip().removeprefix("v")
        minimum = re.search(r">=?\s*(\d+)", engine)
        version = (
            nvmrc
            if re.fullmatch(r"\d+(?:\.\d+){0,2}", nvmrc)
            else str(max(22, int(minimum.group(1))))
            if minimum
            else "22"
        )
        plans.append(
            _plan(
                directory,
                root,
                language,
                framework,
                f"node:{version}-bookworm-slim",
                setup,
                command,
                "jest",
                [
                    "**/*.test.*",
                    "**/*.spec.*",
                    "*.test.*",
                    "*.spec.*",
                    "**/__tests__/**",
                    "__tests__/**",
                    "test/**",
                    "tests/**",
                ],
                [
                    f"package.json declares {framework}",
                    f"packageManager={data.get('packageManager', 'npm')}",
                ],
                {
                    "engines": data.get("engines", {}),
                    "packageManager": data.get("packageManager"),
                    "nvmrc": _read(directory, ".nvmrc").strip() or None,
                },
            )
        )
    return plans


def go_plan(directory, root):
    raw = _read(directory, "go.mod")
    if not raw:
        return []
    version = re.search(r"(?m)^go\s+(\S+)", raw)
    version = version.group(1) if version else "1.24"
    return [
        _plan(
            directory,
            root,
            "go",
            "go-test",
            f"golang:{version}-bookworm",
            ["go mod download"],
            "go test -mod=readonly -json ./...",
            "go-json",
            ["**/*_test.go", "*_test.go"],
            ["go.mod", f"go directive: {version}"],
            {"go": version},
        )
    ]


def rust_plan(directory, root):
    raw = _read(directory, "Cargo.toml")
    if not raw:
        return []
    data = tomllib.loads(raw)
    if "package" not in data:
        return []  # Virtual workspaces expose their concrete members as units.
    toolchain = _read(directory, "rust-toolchain.toml") or _read(
        root, "rust-toolchain.toml"
    )
    channel = (
        tomllib.loads(toolchain).get("toolchain", {}).get("channel", "1")
        if toolchain
        else "1"
    )
    if not re.fullmatch(r"[\w.\-]+", channel):
        raise EvalError("Invalid Rust toolchain channel")
    return [
        _plan(
            directory,
            root,
            "rust",
            "cargo",
            f"rust:{channel}-trixie",
            ["cargo fetch"],
            "cargo test --offline --no-fail-fast -- --test-threads=1",
            "cargo",
            ["tests/**", "**/tests/**", "**/*_test.rs", "**/*_tests.rs"],
            [
                "Cargo.toml package",
                f"toolchain channel: {channel}",
                "Cargo parser follows evals/src/testlog.rs binary::test identities",
            ],
            {"channel": channel, "rust-version": data["package"].get("rust-version")},
        )
    ]


def is_test(path, plan):
    prefix = "" if plan.working_directory == "." else plan.working_directory + "/"
    if not path.startswith(prefix):
        return False
    relative = path[len(prefix) :]
    return any(fnmatch.fnmatchcase(relative, rule) for rule in plan.test_path_rules)


def is_protected(path, plan):
    name = Path(path).name
    return (
        is_test(path, plan)
        or name in MANIFESTS
        or name in plan.dependency_files
        or name.startswith(
            (
                "conftest.",
                "jest.config",
                "vitest.config",
                "vite.config",
                "tsconfig",
                "webpack.config",
            )
        )
        or name
        in {
            "tox.ini",
            "Makefile",
            "justfile",
            "build.rs",
            "go.sum",
            ".npmrc",
            ".yarnrc",
            "rust-toolchain",
            "rust-toolchain.toml",
        }
        or name.endswith((".lock", ".snap"))
        or any(p in {".github", ".git", "scripts"} for p in Path(path).parts)
    )

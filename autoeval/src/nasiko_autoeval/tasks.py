"""Commit harvesting and the production-only candidate boundary."""

import io
import json
import re
import subprocess
import tarfile
from pathlib import Path

from .analyzer import inspect_repository, is_protected, is_test, select_plan
from .schema import EvalError, safe_path, save_json

SOURCE_EXTENSIONS = {
    "python": {".py"},
    "javascript": {".js", ".jsx", ".mjs", ".cjs"},
    "typescript": {".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs"},
    "go": {".go"},
    "rust": {".rs"},
}
RUST_INLINE_TEST = re.compile(rb"#\s*\[\s*(?:cfg\s*\(\s*test\s*\)|test)\s*\]")


def git(repo, *args, input=None):
    result = subprocess.run(
        ["git", "-C", str(repo), *args],
        input=input,
        capture_output=True,
        timeout=60,
        check=False,
    )
    if result.returncode:
        raise EvalError(
            "git " + args[0] + ": " + result.stderr.decode(errors="replace")[:700]
        )
    return result.stdout


def archive_tree(repo, commit):
    files = {}
    with tarfile.open(
        fileobj=io.BytesIO(git(repo, "archive", "--format=tar", commit))
    ) as archive:
        for member in archive:
            safe_path(member.name)
            if member.issym() or member.islnk():
                raise EvalError(
                    f"Symlink/hardlink repositories need an explicit trusted export: {member.name}"
                )
            if not member.isfile():
                continue
            name = member.name
            if any(
                p.startswith(".env") and p != ".env.example" for p in Path(name).parts
            ):
                continue
            if Path(name).suffix.lower() in {".pem", ".key", ".p12"}:
                continue
            files[name] = archive.extractfile(member).read()
    return files


def write_tree(files, root):
    for name, data in files.items():
        path = Path(root) / safe_path(name)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


def build_task(repo, commit, destination, unit=None, overrides=None, issue=None):
    import tempfile

    repo = Path(repo).resolve()
    fixed = git(repo, "rev-parse", "--verify", commit + "^{commit}").decode().strip()
    parents = git(repo, "rev-list", "--parents", "-n", "1", fixed).decode().split()
    if len(parents) != 2:
        raise EvalError("Select a non-merge bug-fix commit with exactly one parent")
    parent = parents[1]
    base = archive_tree(repo, parent)
    reference = archive_tree(repo, fixed)
    with tempfile.TemporaryDirectory(prefix="autoeval-detect-") as temp:
        write_tree(base, temp)
        plan = select_plan(inspect_repository(temp), unit, overrides)
    if not plan.setup_commands:
        raise EvalError(
            "This package manager requires explicit setup_commands in --config"
        )
    changes = (
        git(repo, "diff", "--name-status", "--no-renames", parent, fixed)
        .decode()
        .splitlines()
    )
    production, tests, ignored = [], [], []
    for line in changes:
        status, name = line.split("\t", 1)
        safe_path(name)
        if status not in {"A", "M"}:
            raise EvalError(
                f"Ambiguous change ({status}): {name}; first version accepts additions/modifications only"
            )
        prefix = "" if plan.working_directory == "." else plan.working_directory + "/"
        if not name.startswith(prefix):
            raise EvalError(f"Commit crosses project units: {name}")
        if is_test(name, plan):
            tests.append(name)
        elif is_protected(name, plan):
            raise EvalError(
                f"Execution/configuration change cannot be separated safely: {name}"
            )
        elif Path(name).suffix in SOURCE_EXTENSIONS[plan.language]:
            if plan.language == "rust" and RUST_INLINE_TEST.search(reference[name]):
                raise EvalError(
                    f"Mixed Rust inline production/tests require extraction; rejected: {name}"
                )
            if plan.framework == "vitest" and b"import.meta.vitest" in reference[name]:
                raise EvalError(
                    f"Mixed Vitest in-source tests require extraction; rejected: {name}"
                )
            production.append(name)
        elif Path(name).suffix.lower() in {".md", ".rst", ".txt"}:
            ignored.append(name)
        else:
            raise EvalError(
                f"Unclassified mixed change: {name}; supply a narrower commit"
            )
    if not production or not tests:
        raise EvalError(
            "Task needs separable production changes and regression test files"
        )
    before = dict(base)
    before.update({name: reference[name] for name in tests})
    after = dict(before)
    after.update({name: reference[name] for name in production})
    allowed = sorted(
        name
        for name in base
        if Path(name).suffix in SOURCE_EXTENSIONS[plan.language]
        and not is_protected(name, plan)
        and name.startswith(prefix)
    )
    origin = (
        git(repo, "config", "--get", "remote.origin.url").decode().strip()
        if git(repo, "remote").strip()
        else "local-fixture"
    )
    if re.search(r"https?://[^/]*@", origin):
        raise EvalError(
            "Repository origin contains credentials; use a credential-free origin"
        )
    task = {
        "id": fixed[:12],
        "repository_url": origin,
        "parent_sha": parent,
        "fix_sha": fixed,
        "plan": plan.to_dict(),
        "production_changes": production,
        "test_changes": tests,
        "ignored_documentation": ignored,
        "allowed_paths": allowed,
        "issue": issue
        or "Fix the repository behavior described here: "
        + git(repo, "show", "-s", "--format=%s", fixed).decode().strip(),
        "issue_source": "operator"
        if issue
        else "commit subject only; no diff/history supplied",
        "input_kind": "real_repository"
        if origin != "local-fixture"
        else "local_fixture",
    }
    destination = Path(destination)
    destination.mkdir(parents=True, exist_ok=True)
    save_json(destination / "task.json", task)
    return task, plan, base, before, after


def apply_candidate(base, edits, task, plan):
    if not isinstance(edits, list) or not edits:
        raise EvalError("Model response needs a nonempty files array")
    result = dict(base)
    changed = set()
    for edit in edits:
        if not isinstance(edit, dict) or set(edit) != {"path", "content"}:
            raise EvalError("Each candidate file must contain exactly path and content")
        path = safe_path(edit["path"])
        if (
            path in changed
            or path not in task["allowed_paths"]
            or is_protected(path, plan)
        ):
            raise EvalError(
                f"Candidate touches a duplicate, forbidden, or grading asset path: {path}"
            )
        content = edit["content"]
        if not isinstance(content, str) or len(content.encode()) > 200_000:
            raise EvalError(f"Invalid/oversized candidate source: {path}")
        if plan.language == "rust" and RUST_INLINE_TEST.search(base[path]):
            raise EvalError(
                f"Candidate cannot alter a Rust source file containing inline tests: {path}"
            )
        if plan.framework == "vitest" and b"import.meta.vitest" in base[path]:
            raise EvalError(
                f"Candidate cannot alter a Vitest source file containing inline tests: {path}"
            )
        result[path] = content.encode()
        changed.add(path)
    return result


def model_prompt(task, base, context_bytes):
    # The allowlist derives from manifests + the parent tree, never from the gold diff.
    source = []
    size = 0
    for path in task["allowed_paths"]:
        data = base[path]
        if size + len(data) > context_bytes:
            raise EvalError(
                "Permitted source exceeds context budget; select a smaller unit or raise --context-bytes"
            )
        source.append({"path": path, "content": data.decode("utf-8")})
        size += len(data)
    return (
        "You are fixing a software repository. Make one production-code fix. "
        'Return ONLY a JSON object {"files":[{"path":"relative/path","content":"complete replacement file"}]}. '
        "Edit only listed source paths. Tests and execution configuration are evaluator controlled.\n"
        + json.dumps(
            {
                "issue": task["issue"],
                "language": task["plan"]["language"],
                "source": source,
            },
            ensure_ascii=False,
        )
    )

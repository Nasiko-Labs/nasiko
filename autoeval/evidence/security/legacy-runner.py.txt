"""A single Docker runner with online preparation and immutable, offline grading."""

import io
import json
import subprocess
import tarfile
import time
import uuid
from pathlib import Path

from .results import normalize
from .schema import EvalError, TestRun, save_json

LABEL = "nasiko.autoeval"
REPORTS = {"junit": "results.xml", "jest": "results.json"}


class DockerRunner:
    def __init__(self, artifacts):
        self.artifacts = Path(artifacts)
        self.artifacts.mkdir(parents=True, exist_ok=True)
        self.containers = set()
        self.images = set()
        self.prepared = None
        self.prepared_image_id = None
        self.base_image_id = None

    def __enter__(self):
        return self

    def __exit__(self, *_):
        for name in list(self.containers):
            self._remove_container(name)
        for name in list(self.images):
            self._docker("image", "rm", "-f", name, check=False)

    def _docker(self, *args, input=None, timeout=60, check=True):
        try:
            result = subprocess.run(
                ["docker", *args],
                input=input,
                capture_output=True,
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired:
            raise EvalError(f"Docker operation timed out: {args[0]}") from None
        if check and result.returncode:
            raise EvalError(
                f"Docker {args[0]} failed: {result.stderr.decode(errors='replace')[-1600:]}"
            )
        return result

    def _name(self, stage):
        return f"nasiko-autoeval-{stage}-{uuid.uuid4().hex[:12]}"

    def _limits(self, plan):
        return [
            "--label",
            LABEL + "=true",
            "--memory",
            f"{plan.memory_mib}m",
            "--memory-swap",
            f"{plan.memory_mib}m",
            "--cpus",
            str(plan.cpus),
            "--pids-limit",
            str(plan.pids),
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--ulimit",
            "nofile=1024:1024",
            "--log-driver=json-file",
            "--log-opt=max-size=4m",
            "--log-opt=max-file=1",
        ]

    def _environment(self):
        # No host environment or provider credentials are forwarded.
        values = {
            "HOME": "/tmp",
            "PATH": "/opt/venv/bin:/usr/local/cargo/bin:/usr/local/go/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
            "SETUPTOOLS_SCM_PRETEND_VERSION": "0.0.0",
            "PYTHONDONTWRITEBYTECODE": "1",
            "PIP_DISABLE_PIP_VERSION_CHECK": "1",
            "CARGO_BUILD_JOBS": "1",
            "CARGO_HOME": "/opt/cargo",
            "CARGO_TARGET_DIR": "/tmp/target",
            "GOMODCACHE": "/opt/gomod",
            "GOCACHE": "/tmp/go-build",
            "CI": "true",
            "NO_COLOR": "1",
            "GOTOOLCHAIN": "local",
            "GOMAXPROCS": "2",
        }
        return [
            arg for key, value in values.items() for arg in ("--env", key + "=" + value)
        ]

    def _copy_tree(self, name, files):
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w") as archive:
            directories = {"work"}
            for path in files:
                parent = Path("work") / Path(path).parent
                directories.update(
                    str(p) for p in [parent, *parent.parents] if str(p) != "."
                )
            for path in sorted(directories, key=lambda p: (p.count("/"), p)):
                info = tarfile.TarInfo(path)
                info.type = tarfile.DIRTYPE
                info.uid = info.gid = 10001
                info.mode = 0o777  # Setup runs without DAC_OVERRIDE; grading mounts the image read-only.
                archive.addfile(info)
            for path, data in files.items():
                info = tarfile.TarInfo("work/" + path)
                info.size = len(data)
                info.uid = info.gid = 10001
                info.mode = 0o666
                archive.addfile(info, io.BytesIO(data))
        self._docker("cp", "-a", "-", name + ":/", input=buffer.getvalue())

    def _remove_container(self, name):
        self._docker("rm", "-f", "-v", name, check=False)
        self.containers.discard(name)

    def _commit(self, name, stage):
        image = self._name(stage)
        self.images.add(image)
        self._docker("commit", name, image, timeout=120)
        return image

    def prepare(self, files, plan):
        if self._docker("image", "inspect", plan.runtime_image, check=False).returncode:
            self._docker("pull", plan.runtime_image, timeout=plan.setup_timeout_seconds)
        self.base_image_id = json.loads(
            self._docker("image", "inspect", plan.runtime_image).stdout
        )[0]["Id"]
        name = self._name("setup")
        self.containers.add(name)
        inventory = {
            "pytest": "/opt/venv/bin/python --version; /opt/venv/bin/pip freeze",
            "jest": "node --version; npm --version; npm ls --depth=0",
            "vitest": "node --version; npm --version; npm ls --depth=0",
            "go-test": "go version; go list -m all",
            "cargo": "rustc --version; cargo metadata --offline --format-version=1",
        }
        command = (
            "set -eu\n"
            + "\n".join(plan.setup_commands)
            + "\nmkdir -p /eval\n("
            + inventory[plan.framework]
            + ") > /eval/environment.txt 2>&1 || true\n"
        )
        self._docker(
            "create",
            "--name",
            name,
            *self._limits(plan),
            *self._environment(),
            "--workdir",
            "/work/" + plan.working_directory,
            plan.runtime_image,
            "sh",
            "-c",
            command,
        )
        self._copy_tree(name, files)
        try:
            result = self._docker(
                "start", "-a", name, timeout=plan.setup_timeout_seconds, check=False
            )
            log = (result.stdout + result.stderr).decode(errors="replace")
            (self.artifacts / "setup.log").write_text(log[-4_000_000:])
            state = json.loads(self._docker("inspect", name).stdout)[0]["State"]
            if state.get("OOMKilled") or state["ExitCode"] != 0 or result.returncode:
                raise EvalError(
                    "Dependency preparation failed; inspect "
                    + str(self.artifacts / "setup.log")
                )
            self._docker(
                "cp",
                name + ":/eval/environment.txt",
                str(self.artifacts / "environment.txt"),
                check=False,
            )
            self.prepared = self._commit(name, "dependencies")
            self.prepared_image_id = json.loads(
                self._docker("image", "inspect", self.prepared).stdout
            )[0]["Id"]
        finally:
            self._remove_container(name)

    def run(self, files, plan, label):
        if not self.prepared:
            raise EvalError("Prepare dependencies before offline execution")
        artifact = self.artifacts / label
        artifact.mkdir(parents=True, exist_ok=True)
        snapshot_name = self._name("snapshot")
        self.containers.add(snapshot_name)
        self._docker(
            "create", "--name", snapshot_name, "--network=none", self.prepared, "true"
        )
        try:
            self._copy_tree(snapshot_name, files)
            snapshot = self._commit(snapshot_name, "tree")
        finally:
            self._remove_container(snapshot_name)
        name = self._name("test")
        self.containers.add(name)
        started = time.monotonic()
        try:
            self._docker(
                "create",
                "--name",
                name,
                *self._limits(plan),
                *self._environment(),
                "--network=none",
                "--read-only",
                "--user=10001:10001",
                "--tmpfs",
                "/tmp:rw,exec,nosuid,size=536870912,mode=1777",
                "--tmpfs",
                "/eval/out:rw,noexec,nosuid,size=33554432,mode=1777",
                "--workdir",
                "/work/" + plan.working_directory,
                snapshot,
                "sh",
                "-c",
                "while :; do sleep 30; done",
            )
            self._docker("start", name)
            inspected = json.loads(self._docker("inspect", name).stdout)[0]
            host = inspected["HostConfig"]
            save_json(
                artifact / "execution.json",
                {
                    "image": inspected["Image"],
                    "user": inspected["Config"]["User"],
                    "test_command": plan.test_command,
                    "limits": {
                        key: host.get(key)
                        for key in [
                            "Memory",
                            "MemorySwap",
                            "NanoCpus",
                            "PidsLimit",
                            "CapDrop",
                            "SecurityOpt",
                            "NetworkMode",
                            "ReadonlyRootfs",
                            "Tmpfs",
                        ]
                    },
                },
            )
            command = "set -u\n"
            if plan.framework == "cargo":
                command += "cp -a /opt/cargo /tmp/cargo\nexport CARGO_HOME=/tmp/cargo\n"
            command += plan.test_command
            try:
                bounded = (
                    "(\n"
                    + command
                    + '\n) > /eval/out/output.log 2>&1\ncode=$?\ntail -c 4000000 /eval/out/output.log\nexit "$code"'
                )
                result = self._docker(
                    "exec",
                    name,
                    "sh",
                    "-c",
                    bounded,
                    timeout=plan.timeout_seconds,
                    check=False,
                )
            except EvalError as error:
                return TestRun(
                    "timeout",
                    {},
                    None,
                    time.monotonic() - started,
                    str(error),
                    str(artifact),
                )
            output = (result.stdout + result.stderr).decode(errors="replace")
            (artifact / "output.log").write_text(output[-4_000_000:])
            report_name = REPORTS.get(plan.result_format)
            report = None
            if report_name:
                # Docker Desktop's archive API can miss files in a tmpfs mount.
                copied = self._docker(
                    "exec", name, "cat", f"/eval/out/{report_name}", check=False
                )
                if copied.returncode == 0:
                    report = copied.stdout.decode()
                    (artifact / report_name).write_text(report)
            return normalize(
                plan,
                result.returncode,
                time.monotonic() - started,
                report,
                output,
                str(artifact),
            )
        finally:
            self._remove_container(name)
            self._docker("image", "rm", "-f", snapshot, check=False)
            self.images.discard(snapshot)

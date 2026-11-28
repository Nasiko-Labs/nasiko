"""Bounded host-side capture. Container code has pipes, never artifact paths."""

import os
import selectors
import subprocess
import time
from dataclasses import dataclass


@dataclass
class Capture:
    stdout: bytes
    stderr: bytes
    stopped: str | None
    client_exit_code: int


def capture(command, timeout, max_bytes=4_000_000):
    streams = {"stdout": bytearray(), "stderr": bytearray()}
    stopped = None
    deadline = time.monotonic() + timeout
    with subprocess.Popen(
        command, stdout=subprocess.PIPE, stderr=subprocess.PIPE
    ) as process:
        with selectors.DefaultSelector() as selector:
            for name in streams:
                selector.register(getattr(process, name), selectors.EVENT_READ, name)
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    stopped = "timeout"
                    break
                for key, _ in selector.select(min(remaining, 0.1)):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    used = sum(len(value) for value in streams.values())
                    streams[key.data].extend(chunk[: max_bytes - used])
                    if used + len(chunk) > max_bytes:
                        stopped = "output_limit"
                        break
                if stopped:
                    break
        if stopped:
            process.kill()
        try:
            code = process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            code = process.wait()
    return Capture(bytes(streams["stdout"]), bytes(streams["stderr"]), stopped, code)

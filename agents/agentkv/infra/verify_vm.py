"""Bounded CPU-only check of Nasiko's Docker hosting prerequisite on Modal."""
import json
import modal

app = modal.App.lookup("agentkv-hosting-check", create_if_missing=True)
image = modal.Image.debian_slim(python_version="3.12").apt_install("docker.io", "git", "curl")


def main():
    sandbox = modal.Sandbox.create(
        "bash", "-lc", "dockerd >/tmp/docker.log 2>&1 & wait",
        app=app, image=image, cpu=2, memory=4096, timeout=180,
        experimental_options={"vm_runtime": True},
    )
    try:
        process = sandbox.exec("bash", "-lc", "for i in $(seq 1 40); do docker info >/dev/null 2>&1 && exit 0; sleep 2; done; exit 1")
        process.wait()
        if process.returncode:
            raise RuntimeError("Docker daemon unavailable in Modal VM")
        process = sandbox.exec("docker", "run", "--rm", "hello-world")
        process.wait()
        if process.returncode:
            raise RuntimeError("Docker container launch failed")
        print(json.dumps({"docker_in_modal_vm": True, "sandbox_id": sandbox.object_id}))
    finally:
        sandbox.terminate()
        print("Hosting probe terminated")


if __name__ == "__main__":
    main()

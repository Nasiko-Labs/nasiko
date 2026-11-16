"""Bounded Nasiko hosting check, private network only, no model or GPU."""
from pathlib import Path
import modal

app = modal.App.lookup("agentkv-nasiko-check", create_if_missing=True)
image = (modal.Image.debian_slim(python_version="3.12").apt_install("docker.io", "git")
         .add_local_file(Path(__file__).with_name("nasiko_probe_guest.py"), "/probe.py"))


def main():
    sandbox = modal.Sandbox.create("bash", "-lc", "dockerd >/tmp/docker.log 2>&1 & wait",
        app=app, image=image, cpu=4, memory=8192, timeout=1500,
        experimental_options={"vm_runtime": True})
    print("Nasiko hosting sandbox:", sandbox.object_id, flush=True)
    try:
        process = sandbox.exec("python", "/probe.py", timeout=1440)
        process.wait()
        print(process.stdout.read())
        if process.returncode:
            print(process.stderr.read())
            raise SystemExit(1)
    finally:
        sandbox.terminate()
        print("Nasiko hosting probe terminated")


if __name__ == "__main__":
    main()

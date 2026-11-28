"""A synthetic GUI task. Reset and grade run on the trusted controller."""
import json
import os
from pathlib import Path
import hashlib
import subprocess
import sys
import time

ATTEMPT = Path(os.environ["DESKTOP_ATTEMPT_DIR"])
WINDOW = "Nasiko ticket confirmation"


def reset():
    # xvfb-run supplies a dedicated display for this invocation.
    old = subprocess.run(["xdotool", "search", "--onlyvisible", "--name", f"^{WINDOW} OPS-"],
                         capture_output=True, text=True)
    for window in old.stdout.splitlines():
        subprocess.run(["xdotool", "windowkill", window], check=True)
    seed = os.environ["DESKTOP_TRIAL"].encode()
    ticket = "OPS-" + hashlib.sha256(seed).hexdigest()[:6].upper()
    (ATTEMPT / "ticket.txt").write_text(ticket)
    proc = subprocess.Popen(
        [sys.executable, str(Path(__file__).resolve()), "show"],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL, start_new_session=True,
    )
    for _ in range(100):
        if proc.poll() is not None:
            raise RuntimeError("Ticket window exited. Check DISPLAY and python3-tk.")
        result = subprocess.run(
            ["xdotool", "search", "--onlyvisible", "--name", f"^{WINDOW} {ticket}$"],
            capture_output=True, text=True,
        )
        if result.returncode == 0:
            window = result.stdout.splitlines()[-1]
            subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True)
            return
        time.sleep(0.05)
    proc.terminate()
    raise RuntimeError("Ticket window did not become visible")


def show():
    import tkinter as tk
    from tkinter import ttk

    root = tk.Tk()
    ticket = (ATTEMPT / "ticket.txt").read_text()
    root.title(f"{WINDOW} {ticket}")
    root.geometry("560x270+20+20")
    form = ttk.Frame(root, padding=24)
    form.pack(fill="both", expand=True)
    ttk.Label(form, text="Dispatch confirmation", font=("DejaVu Sans", 20)).pack(anchor="w")
    ttk.Label(form, text=f"Ticket ID: {ticket}").pack(anchor="w", pady=12)
    ttk.Label(form, text="Confirmation").pack(anchor="w")
    entry = ttk.Entry(form, width=30)
    entry.pack(anchor="w", pady=4)
    status = ttk.Label(form)

    def save(_event=None):
        (ATTEMPT / "submitted.json").write_text(json.dumps({"ticket": entry.get()}))
        status.configure(text="Saved")

    ttk.Button(form, text="Save", command=save).pack(anchor="w")
    status.pack(anchor="w")
    root.bind("<Return>", save)
    root.after_idle(entry.focus_force)
    root.mainloop()


def grade():
    output = ATTEMPT / "submitted.json"
    expected = (ATTEMPT / "ticket.txt").read_text()
    if not output.exists():
        return False
    try:
        return json.loads(output.read_text()) == {"ticket": expected}
    except (ValueError, UnicodeError):
        return False


if __name__ == "__main__":
    action = sys.argv[1]
    if action == "reset":
        reset()
    elif action == "show":
        show()
    elif action == "grade":
        print(json.dumps({"resolved": grade()}))
    else:
        raise SystemExit("Expected reset, show, or grade")

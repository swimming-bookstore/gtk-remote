#!/usr/bin/env python3
"""Record GNOME Calculator as gtk-remote presses 1 + 1 =.

The binary is the Debian gnome-calculator package, launched unmodified.
The agent only talks to the socket. Pixels come from ~/demo WindowRecord.
"""

from __future__ import annotations

import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path

os.environ.setdefault("DISPLAY", ":0.0")
os.environ.setdefault("XAUTHORITY", str(Path.home() / ".Xauthority"))
sys.path.insert(0, str(Path.home() / "demo"))

from record.window import WindowRecord

ROOT = Path(__file__).resolve().parents[1]
LAUNCH = ROOT / "gtk-remote-launch"
CALC = Path("/tmp/gcalc/usr/bin/gnome-calculator")
OUT = ROOT / "docs" / "demo-calc.mp4"
SOCK = Path("/tmp/gtk-remote-demo/calc.sock")
KEYS = ["1", "+", "1", "="]


def call(obj: dict, timeout: float = 5.0) -> dict:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(str(SOCK))
    s.sendall((json.dumps(obj) + "\n").encode())
    data = b""
    while b"\n" not in data:
        chunk = s.recv(65536)
        if not chunk:
            break
        data += chunk
    s.close()
    line = data.split(b"\n", 1)[0]
    return json.loads(line) if line else {"ok": False, "error": "empty reply"}


def button_id(name: str) -> int:
    reply = call({"id": 1, "cmd": "find", "role": "button", "name": name})
    matches = [m for m in reply.get("matches") or [] if m.get("class") == "GtkButton"]
    if not matches:
        raise SystemExit(f"no button named {name!r}: {reply}")
    return int(matches[0]["id"])


def main() -> None:
    if not CALC.exists():
        raise SystemExit(f"missing {CALC}; unpack the gnome-calculator deb under /tmp/gcalc")
    SOCK.parent.mkdir(parents=True, exist_ok=True)
    if SOCK.exists():
        SOCK.unlink()
    OUT.parent.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["GTK_REMOTE_SOCK"] = str(SOCK)
    env["DISPLAY"] = os.environ["DISPLAY"]
    env["GSK_RENDERER"] = "cairo"
    env["GDK_BACKEND"] = "x11"
    env["LD_LIBRARY_PATH"] = "/tmp/gcalc/usr/lib/x86_64-linux-gnu" + (
        ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else ""
    )
    env["XDG_DATA_DIRS"] = "/tmp/gcalc/usr/share:" + env.get("XDG_DATA_DIRS", "/usr/share")
    env["GSETTINGS_SCHEMA_DIR"] = (
        "/tmp/gcalc/usr/share/glib-2.0/schemas:/usr/share/glib-2.0/schemas"
    )

    rec = WindowRecord(
        "Calculator",
        OUT,
        fps=12,
        max_sec=24.0,
        min_w=300,
        min_h=400,
        overlay_pointer=False,
        hide_cursor=True,
        swap_rb=False,
    )
    rec.spawn(
        [str(LAUNCH), str(CALC), "--mode=basic"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.time() + 10
    while time.time() < deadline:
        try:
            if SOCK.exists() and call({"id": 1, "cmd": "ping"}).get("ok"):
                break
        except OSError:
            time.sleep(0.1)
    else:
        raise SystemExit("socket not ready")
    rec.attach(timeout=12)
    from record import x11cap

    x11 = x11cap.x11
    x11.XMapRaised(rec._dpy, rec._wid)
    x11.XRaiseWindow(rec._dpy, rec._wid)
    x11.XFlush(rec._dpy)
    time.sleep(0.2)
    # do_action clicks leave the pointer where it was. If that is the π
    # button, GTK pops "Pi [Ctrl+P]" into the recording. Park it off-window.
    x11.XWarpPointer(rec._dpy, 0, x11.XDefaultRootWindow(rec._dpy), 0, 0, 0, 0, 2, 2)
    x11.XFlush(rec._dpy)
    time.sleep(0.2)

    ids = {name: button_id(name) for name in ["1", "+", "="]}
    rec.start_capture()
    rec.hold(0.8)
    for i, name in enumerate(KEYS):
        reply = call({"id": 10 + i, "cmd": "do_action", "widget": ids[name], "action": "click"})
        if not reply.get("ok"):
            print(f"press failed {name!r}: {reply}", file=sys.stderr)
            break
        rec.hold(0.7)
    rec.hold(1.4)
    rec._stop.set()
    if rec._cap:
        rec._cap.join(timeout=8)
    if rec._ff and rec._ff.poll() is None:
        try:
            rec._ff.wait(timeout=20)
        except subprocess.TimeoutExpired:
            rec._ff.kill()
    rec.kill_all()
    if not OUT.exists() or OUT.stat().st_size < 8_000:
        size = OUT.stat().st_size if OUT.exists() else 0
        raise SystemExit(f"recording failed: {OUT} size={size} frames={rec.nframes}")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, {rec.nframes} frames, {rec._w}x{rec._h})")


if __name__ == "__main__":
    main()

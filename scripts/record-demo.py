#!/usr/bin/env python3
"""Record unmodified GNOME Text Editor as gtk-remote types one character at a time.

Pixels come from ~/demo WindowRecord. The agent talks to the socket only.
The binary is the Debian gnome-text-editor package, not a stand-in app.
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
EDITOR = Path("/tmp/gte/usr/bin/gnome-text-editor")
OUT = ROOT / "docs" / "demo.mp4"
SOCK = Path("/tmp/gtk-remote-demo/editor.sock")
TEXT = "Welcome to 수영 책방 Swimming Bookstore!"
TITLE = "Text Editor"


def call(obj: dict, timeout: float = 4.0) -> dict:
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
    return json.loads(data.decode())


def window_title() -> str | None:
    try:
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True, stderr=subprocess.DEVNULL)
    except (subprocess.CalledProcessError, OSError):
        return None
    for line in tree.splitlines():
        if TITLE in line and "gnome-text-editor" in line and "x" in line:
            name = line.split('"')[1]
            if "x" in name.split()[-1:]:
                continue
            return name
    return None


def main() -> None:
    if not EDITOR.exists():
        raise SystemExit(f"missing {EDITOR}; unpack the gnome-text-editor package under /tmp/gte")
    SOCK.parent.mkdir(parents=True, exist_ok=True)
    if SOCK.exists():
        SOCK.unlink()
    OUT.parent.mkdir(parents=True, exist_ok=True)

    # A leftover draft makes Text Editor show "Document Restored". Start from
    # a clean config and skip session restore so the page is a blank document.
    home = Path("/tmp/gtk-remote-demo/gte-home")
    for stale in (
        home / ".local/share/gnome-text-editor",
        home / ".local/share/org.gnome.TextEditor",
        home / ".cache/gnome-text-editor",
    ):
        if stale.exists():
            import shutil

            shutil.rmtree(stale)

    env = os.environ.copy()
    env["HOME"] = str(home)
    env["XDG_CONFIG_HOME"] = str(home / ".config")
    env["XDG_DATA_HOME"] = str(home / ".local/share")
    env["XDG_CACHE_HOME"] = str(home / ".cache")
    env["GTK_REMOTE_SOCK"] = str(SOCK)
    env["DISPLAY"] = os.environ["DISPLAY"]
    env["GSK_RENDERER"] = "cairo"
    env["GDK_BACKEND"] = "x11"
    env["LD_LIBRARY_PATH"] = "/tmp/gte/usr/lib/x86_64-linux-gnu" + (
        ":" + env["LD_LIBRARY_PATH"] if env.get("LD_LIBRARY_PATH") else ""
    )
    env["XDG_DATA_DIRS"] = "/tmp/gte/usr/share:" + env.get("XDG_DATA_DIRS", "/usr/share")
    env["GSETTINGS_SCHEMA_DIR"] = "/tmp/gte/schemas:/usr/share/glib-2.0/schemas"

    rec = WindowRecord(
        TITLE,
        OUT,
        fps=12,
        max_sec=40.0,
        min_w=500,
        min_h=400,
        overlay_pointer=False,
        hide_cursor=True,
        swap_rb=False,
    )
    rec.spawn(
        [str(LAUNCH), str(EDITOR), "--ignore-session"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.time() + 12
    while time.time() < deadline:
        try:
            if SOCK.exists() and call({"id": 1, "cmd": "ping"}).get("ok"):
                break
        except OSError:
            time.sleep(0.15)
    else:
        raise SystemExit("socket not ready")

    title = None
    deadline = time.time() + 8
    while time.time() < deadline:
        title = window_title()
        if title:
            break
        time.sleep(0.15)
    if not title:
        raise SystemExit("Text Editor window not found")
    rec.title = title
    rec.attach(timeout=12)
    print(f"recording window {rec.window_id()}", file=sys.stderr)

    target = {"class": "EditorSourceView"}
    call({"id": 2, "cmd": "set_text", **target, "text": ""})
    rec.start_capture()
    rec.hold(0.7)
    for i, ch in enumerate(TEXT):
        reply = call({"id": 10 + i, "cmd": "type_text", **target, "text": ch})
        if not reply.get("ok"):
            print(f"type failed {ch!r}: {reply}", file=sys.stderr)
            break
        time.sleep(0.09)
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

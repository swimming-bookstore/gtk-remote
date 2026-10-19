#!/usr/bin/env python3
"""Record unmodified LibreOffice Writer as gtk-remote types one character at a time.

soffice is a shell script that execs oosplash and drops LD_PRELOAD, so the
launcher starts soffice.bin directly. The agent talks to the socket only.
Pixels come from ~/demo WindowRecord of the whole Writer window.
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
CLIENT = ROOT / "target" / "release" / "gtk-remote"
SOFFICE = Path("/usr/lib/libreoffice/program/soffice.bin")
OUT = ROOT / "docs" / "demo-writer.mp4"
SOCK = Path("/tmp/gtk-remote-demo/lo.sock")
UNO = "uno:socket,host=127.0.0.1,port=2002;urp;StarOffice.ComponentContext"
TEXT = "Welcome to 수영 책방 Swimming Bookstore!"
TITLE = "LibreOffice Writer"


def call(obj: dict, timeout: float = 6.0) -> dict:
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect(str(SOCK))
    s.sendall((json.dumps(obj) + "\n").encode())
    data = b""
    while b"\n" not in data:
        chunk = s.recv(1 << 20)
        if not chunk:
            break
        data += chunk
    s.close()
    line = data.split(b"\n", 1)[0]
    return json.loads(line) if line else {"ok": False, "error": "empty reply"}


def window_title() -> str | None:
    try:
        tree = subprocess.check_output(["xwininfo", "-root", "-tree"], text=True, stderr=subprocess.DEVNULL)
    except (subprocess.CalledProcessError, OSError):
        return None
    for line in tree.splitlines():
        if TITLE not in line or '"' not in line:
            continue
        name = line.split('"')[1]
        if "x" in name.split()[-1:]:
            continue
        return name
    return None


def paragraph_id() -> int:
    reply = call({"id": 1, "cmd": "find", "role": "paragraph"})
    matches = reply.get("matches") or []
    if not matches:
        raise SystemExit(f"no document paragraph: {reply}")
    return int(matches[0]["id"])


def unmaximize(title: str) -> None:
    """Drop the maximized state so the clip is a window, not the desktop."""
    from ctypes import POINTER, Structure, byref, c_char_p, c_int, c_long, c_uint, c_ulong, c_void_p

    from record import x11cap

    class XClientMessageEvent(Structure):
        _fields_ = [
            ("type", c_int),
            ("serial", c_ulong),
            ("send_event", c_int),
            ("display", c_void_p),
            ("window", c_ulong),
            ("message_type", c_ulong),
            ("format", c_int),
            ("data0", c_long),
            ("data1", c_long),
            ("data2", c_long),
            ("data3", c_long),
            ("data4", c_long),
        ]

    x11 = x11cap.x11
    x11.XInternAtom.restype = c_ulong
    x11.XInternAtom.argtypes = [c_void_p, c_char_p, c_int]
    x11.XSendEvent.restype = c_int
    x11.XSendEvent.argtypes = [c_void_p, c_ulong, c_int, c_long, c_void_p]
    x11.XRootWindow.restype = c_ulong
    x11.XRootWindow.argtypes = [c_void_p, c_int]
    x11.XDefaultScreen.restype = c_int
    x11.XDefaultScreen.argtypes = [c_void_p]
    dpy = x11.XOpenDisplay(b":0.0")
    if not dpy:
        return
    wid = x11cap.find_window(dpy, title, 400, 300, None)
    if not wid:
        return
    root = x11.XRootWindow(dpy, x11.XDefaultScreen(dpy))
    state = x11.XInternAtom(dpy, b"_NET_WM_STATE", 0)
    horz = x11.XInternAtom(dpy, b"_NET_WM_STATE_MAXIMIZED_HORZ", 0)
    vert = x11.XInternAtom(dpy, b"_NET_WM_STATE_MAXIMIZED_VERT", 0)
    ev = XClientMessageEvent()
    ev.type = 33
    ev.window = wid
    ev.message_type = state
    ev.format = 32
    ev.data0 = 0
    ev.data1 = horz
    ev.data2 = vert
    ev.data3 = 1
    x11.XSendEvent(dpy, root, 0, (1 << 20) | (1 << 19), byref(ev))
    x11.XFlush(dpy)
    time.sleep(0.3)
    parent = c_ulong()
    junk = c_ulong()
    kids = POINTER(c_ulong)()
    nkids = c_uint()
    x11.XQueryTree(dpy, wid, byref(junk), byref(parent), byref(kids), byref(nkids))
    frame = int(parent.value) or wid
    x11.XResizeWindow(dpy, frame, 980, 680)
    x11.XMoveWindow(dpy, frame, 80, 40)
    x11.XFlush(dpy)


def main() -> None:
    if not SOFFICE.exists():
        raise SystemExit(f"missing {SOFFICE}")
    subprocess.run(["pkill", "-f", "soffice.bin"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(0.4)
    SOCK.parent.mkdir(parents=True, exist_ok=True)
    if SOCK.exists():
        SOCK.unlink()
    OUT.parent.mkdir(parents=True, exist_ok=True)

    home = Path("/tmp/gtk-remote-demo/lo-home")
    home.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["HOME"] = str(home)
    env["GTK_REMOTE_SOCK"] = str(SOCK)
    env["DISPLAY"] = os.environ["DISPLAY"]
    env["GSK_RENDERER"] = "cairo"
    env["GDK_BACKEND"] = "x11"

    rec = WindowRecord(
        TITLE,
        OUT,
        fps=12,
        max_sec=50.0,
        min_w=800,
        min_h=500,
        overlay_pointer=False,
        hide_cursor=True,
        swap_rb=False,
    )
    rec.spawn(
        [str(LAUNCH), str(SOFFICE), "--writer",
         "--accept=socket,host=127.0.0.1,port=2002;urp;",
         "--norestore", "--nologo", "--nolockcheck"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.time() + 20
    while time.time() < deadline:
        try:
            if SOCK.exists() and call({"id": 1, "cmd": "ping"}).get("ok"):
                break
        except OSError:
            time.sleep(0.15)
    else:
        raise SystemExit("socket not ready")

    # The document accessible appears a moment after the socket.
    deadline = time.time() + 12
    wid = 0
    while time.time() < deadline:
        try:
            wid = paragraph_id()
            break
        except SystemExit:
            time.sleep(0.2)
    if not wid:
        raise SystemExit("paragraph not ready")

    title = None
    deadline = time.time() + 8
    while time.time() < deadline:
        title = window_title()
        if title:
            break
        time.sleep(0.15)
    if not title:
        raise SystemExit("Writer window not found")
    rec.title = title
    unmaximize(title)
    time.sleep(0.5)
    rec.attach(timeout=12)

    from record import x11cap

    x11 = x11cap.x11
    x11.XMapRaised(rec._dpy, rec._wid)
    x11.XRaiseWindow(rec._dpy, rec._wid)
    x11.XFlush(rec._dpy)
    time.sleep(0.3)
    call({"id": 2, "cmd": "set_text", "widget": wid, "text": ""})
    rec.start_capture()
    rec.hold(0.5)
    for i, ch in enumerate(TEXT):
        reply = call({"id": 10 + i, "cmd": "type_text", "widget": wid, "text": ch})
        if not reply.get("ok"):
            print(f"type failed {ch!r}: {reply}", file=sys.stderr)
            break
        time.sleep(0.07)
    rec.hold(0.5)
    # Select the whole bookstore name, then bold that selection. Writer's
    # toolbar Bold only styles the view cursor, which AT-SPI does not set, so
    # the weight goes through the same UNO bridge as sheet-set.
    phrase = "수영 책방 Swimming Bookstore"
    start = TEXT.find(phrase)
    if start < 0:
        raise SystemExit(f"phrase missing from {TEXT!r}")
    end = start + len(phrase)
    reply = call({
        "id": 90,
        "cmd": "select_text",
        "widget": wid,
        "start": start,
        "end": end,
    })
    if not reply.get("ok"):
        print(f"select failed: {reply}", file=sys.stderr)
    rec.hold(0.6)
    # CharWeight on the model does not paint. text-style selects the range and
    # dispatches .uno:Bold, the same command as the toolbar.
    style = subprocess.run(
        [str(CLIENT), "--sock", str(SOCK), "text-style",
         "--text", phrase, "--weight", "150", "--uno", UNO],
        capture_output=True,
        text=True,
    )
    line = (style.stdout or "").strip()
    print(line or style.stderr.strip(), file=sys.stderr)
    try:
        styled = json.loads(line) if line else {}
    except json.JSONDecodeError:
        styled = {}
    if style.returncode != 0 or not styled.get("ok"):
        print(f"bold failed: {style.stdout} {style.stderr}", file=sys.stderr)
    rec.hold(3.2)
    rec._stop.set()
    if rec._cap:
        rec._cap.join(timeout=8)
    if rec._ff and rec._ff.poll() is None:
        try:
            rec._ff.wait(timeout=20)
        except subprocess.TimeoutExpired:
            rec._ff.kill()
    rec.kill_all()
    if not OUT.exists() or OUT.stat().st_size < 20_000:
        size = OUT.stat().st_size if OUT.exists() else 0
        raise SystemExit(f"recording failed: {OUT} size={size} frames={rec.nframes}")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, {rec.nframes} frames, {rec._w}x{rec._h})")


if __name__ == "__main__":
    main()

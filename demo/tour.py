#!/usr/bin/env python3
"""A scripted tour of ck, for recording (see record.sh, which runs this in foot).

ck runs in a pseudo-terminal of ours: what it draws goes through to the terminal as is
(images too), what the terminal answers goes back to it, and the tour types keys in
between. Each step waits for ck to have drawn what it's waiting for (ck writes every frame
to CK_FRAME_DUMP), so a slow answer from 4chan slows the tour down instead of breaking it.

Each scene and key is written to $CK_DEMO_LOG as `<unix time>\t<kind>\t<text>`, for the
captions (captions.py).
"""

import fcntl
import os
import pty
import select
import signal
import subprocess
import sys
import termios
import threading
import time
import tty

CK = os.environ.get("CK_BIN", "ck")
FRAME = os.environ["CK_FRAME_DUMP"]
LOG = os.environ.get("CK_DEMO_LOG")
# A screenshot at the end of each scene goes here, for checking the tour.
SHOTS = os.environ.get("CK_DEMO_SHOTS")
# Slower or faster overall (2 = twice as slow).
PACE = float(os.environ.get("CK_DEMO_PACE", "0.7"))
# Touched by record.sh once it has stopped recording: only then does the tour quit ck, so
# its window never closes on camera.
DONE = os.environ.get("CK_DEMO_DONE")

# ck's spinner, shown while something loads.
SPINNER = "⣾⣽⣻⢿⡿⣟⣯⣷"

KEYS = {
    "enter": "\r",
    "esc": "\x1b",
    "tab": "\t",
    "btab": "\x1b[Z",
    "up": "\x1b[A",
    "down": "\x1b[B",
    "right": "\x1b[C",
    "left": "\x1b[D",
    "space": " ",
    "bs": "\x7f",
    "ctrl-w": "\x17",
    "ctrl-d": "\x04",
    "ctrl-u": "\x15",
}


# Threads to show, best first: image-heavy generals that are safe to put in a README.
GOOD = ["/mkg/", "/pcbg/", "/hsg/", "/spg/", "/dpt/"]
# Never these (AI image generals: anything can turn up).
AVOID = ["/sdg/", "/ldg/", "/adt/", "/de3/", "/dalle/", "/aicg/", "/lmg/", "/bst/"]


def pick_threads():
    """Two /g/ threads for the tour (the first with many images), from the live catalog."""
    import json
    import urllib.request

    req = urllib.request.Request("https://a.4cdn.org/g/catalog.json", headers={"User-Agent": "ck demo"})
    threads = [t for page in json.load(urllib.request.urlopen(req, timeout=20)) for t in page["threads"]]
    subject = lambda t: (t.get("sub") or "").lower()
    ok = [t for t in threads if not t.get("sticky") and not any(a in subject(t) for a in AVOID) and t.get("images", 0) >= 10]
    ranked = sorted(ok, key=lambda t: next((i for i, g in enumerate(GOOD) if g in subject(t)), len(GOOD)) * 1000 - t.get("images", 0))
    return ranked[0]["no"], ranked[1]["no"]


class Tour:
    def __init__(self, fd):
        self.fd = fd

    def pause(self, secs):
        time.sleep(secs * PACE)

    def log(self, kind, text=""):
        if LOG:
            with open(LOG, "a") as f:
                f.write(f"{time.time():.3f}\t{kind}\t{text}\n")

    def scene(self, caption):
        """A new part of the tour: its caption from now on."""
        if SHOTS:
            n = len(os.listdir(SHOTS))
            subprocess.run(["grim", "-o", os.environ.get("CK_DEMO_OUTPUT", ""), f"{SHOTS}/{n:02}.png"], check=False)
        self.log("scene", caption)

    def key(self, *names, gap=0.45):
        """Press keys: names from KEYS, or single characters."""
        for name in names:
            self.log("key", name)
            os.write(self.fd, KEYS.get(name, name).encode())
            self.pause(gap)

    def type(self, text, gap=0.08):
        self.log("type", text)
        for c in text:
            os.write(self.fd, c.encode())
            time.sleep(gap * PACE)
        self.pause(0.3)

    def screen(self):
        try:
            with open(FRAME, encoding="utf-8", errors="replace") as f:
                return f.read()
        except OSError:
            return ""

    def loaded(self):
        """Until the image shown has come (ck shows a spinner while it loads; it may take a
        moment to start)."""
        end = time.time() + 0.6
        while time.time() < end and not any(c in self.screen() for c in SPINNER):
            time.sleep(0.03)
        self.wait_for(*SPINNER, gone=True, timeout=15)

    def wait_for(self, *texts, timeout=20.0, gone=False, top=False):
        """Until the screen (with `top`, its title bar) shows any of `texts` (or, with
        `gone`, none of them). The wait is logged, and cut from the video (captions.py):
        it's the network, not ck."""
        self.log("hold")
        end = time.time() + timeout
        found = False
        while time.time() < end:
            s = self.screen()
            if top:
                s = s.split("\n", 1)[0]
            if any(t in s for t in texts) != gone:
                found = True
                break
            time.sleep(0.05)
        # A moment for what came to be drawn (images too).
        time.sleep(0.4)
        self.log("go")
        return found


def tour(t):
    first, second = pick_threads()

    # The video starts with the first scene: by then the pointer Hyprland draws on a new
    # monitor has timed out.
    t.wait_for("Watched", timeout=10)
    time.sleep(4)
    t.scene("ck: imageboards in your terminal. Home: watched, history, saved, favorites, sites")
    t.pause(3)
    t.key("j", "j", "j", gap=0.4)
    t.pause(1)

    t.scene(": goes anywhere: a board, a thread, a URL")
    t.key(":")
    t.type("4chan/g")
    t.key("enter")
    t.wait_for("threads", timeout=30, top=True)
    t.pause(0.5)

    t.scene("Catalogs as cards, with thumbnails")
    t.pause(2)
    t.key("j", "j", "j", "j", gap=0.6)
    t.pause(1.5)

    t.scene("c cycles the layout: cards, compact, grid")
    t.key("c")
    t.pause(2.5)
    t.key("c")
    t.pause(3)
    t.key("l", "l", "j", gap=0.5)
    t.pause(1)
    t.key("c")
    t.pause(1)

    t.scene("s sorts: bump order, most replies, newest, oldest")
    t.key("s")
    t.pause(2)
    t.key("s")
    t.pause(1.5)
    t.key("s", "s", gap=0.6)
    t.pause(1)

    t.scene("/ filters as you type")
    t.key("/")
    t.type("general")
    t.pause(2.5)
    t.key("esc")
    t.pause(1)

    t.scene("Threads: posts, quotes, replies, images")
    t.key(":")
    t.type(f"4chan/g/{first}")
    t.key("enter")
    t.wait_for(" posts", timeout=30, top=True)
    t.pause(2.5)
    t.key("j", "j", "j", gap=0.8)
    t.pause(1)

    t.scene("tab steps through a post's images, links and quotes; a focused quote shows its post")
    t.key("b")
    t.pause(1.5)
    t.key("tab", gap=1.5)
    t.key("tab", gap=1.5)
    t.key("tab", gap=1.5)
    t.key("esc")
    t.pause(0.5)

    t.scene("e shows a post's replies under it, indented")
    t.key("g")
    t.pause(0.8)
    t.key("e")
    t.pause(1.5)
    # Down through the OP (two screens) into its replies.
    t.key("j", "j", "j", "j", "j", gap=0.9)
    t.pause(2)
    t.key("g")
    t.key("e")
    t.pause(0.8)

    t.scene("c: one post's conversation alone")
    t.key("b")
    t.pause(0.8)
    t.key("c")
    t.pause(3)
    t.key("j", "j", gap=0.7)
    t.key("esc")
    t.pause(1)

    t.scene("/ searches the thread; n goes to the next match")
    t.key("/")
    t.type("the")
    t.key("enter")
    t.pause(1)
    t.key("n", "n", "n", gap=0.9)
    t.key("esc")
    t.pause(0.8)

    t.scene("M: only the posts with files, then images hidden, then everything")
    t.key("g")
    t.key("M")
    t.pause(2.5)
    t.key("j", "j", gap=0.6)
    t.key("M")
    t.pause(2)
    t.key("M")
    t.pause(1)

    t.scene("v: the image viewer, on through every file in the thread")
    t.key("g")
    t.key("v", gap=0.1)
    t.loaded()
    t.pause(1.5)
    for _ in range(4):
        t.key("l", gap=0.1)
        t.loaded()
        t.pause(1.3)
    t.key("+", "+", gap=0.9)
    t.key("l", "j", gap=0.6)
    t.key("0")
    t.pause(1)
    t.key("esc")
    t.pause(0.8)

    t.scene("V: a gallery of every file in the thread")
    t.key("V")
    t.pause(3)
    t.key("l", "l", "j", gap=0.7)
    t.key("enter", gap=0.1)
    t.loaded()
    t.pause(1.8)
    t.key("esc")
    t.key("esc")
    t.pause(0.8)

    t.scene("f labels everything on screen: type a label to open it")
    t.key("f")
    t.pause(3)
    t.key("esc")
    t.pause(0.8)

    t.scene(". lists what you can do with what's selected, with each one's key")
    t.key(".")
    t.pause(3)
    t.key("j", "j", "j", gap=0.5)
    t.key("esc")
    t.pause(0.8)

    t.scene("w watches the thread: refreshed in the background, saved for reading offline")
    t.key("w")
    t.pause(2.5)

    t.scene("Tabs: T opens a thread in a new one, ] and [ move between them")
    t.key(":")
    t.type("4chan/g")
    t.key("enter")
    t.wait_for("threads", top=True)
    t.pause(1)
    t.key("/")
    t.type(str(second)[-6:])
    t.key("enter")
    t.pause(0.8)
    t.key("T")
    t.wait_for(" posts", timeout=20, top=True)
    t.pause(2.5)
    t.key("[")
    t.pause(2)
    t.key("]")
    t.pause(2)
    t.key("ctrl-w")
    t.pause(1)
    # The whole catalog again.
    t.key("/")
    t.key("esc")
    t.pause(1)

    t.scene("H hides a thread (Z would show it again); X makes a filter from one")
    t.key("j", "j", gap=0.5)
    t.key("H")
    t.pause(2.5)
    t.key("X")
    t.pause(3.5)
    t.key("esc")
    t.pause(0.8)

    t.scene("A searches the board's archive (desuarchive)")
    t.key("A")
    t.type("rust")
    t.key("enter")
    # Results, not just the header's "0 results" while it searches.
    t.wait_for("in thread", timeout=40)
    t.pause(3)
    t.key("j", "j", gap=0.7)
    t.key("esc")
    t.pause(1)

    t.scene("Watched threads, history and saved copies, from home or with :")
    t.key(":")
    t.type("watched")
    t.key("enter")
    t.pause(3)
    t.key("esc")
    t.pause(0.8)

    t.scene(", settings: themes, keys, and more, written to the config")
    t.key(",")
    t.pause(2.5)
    t.key("enter")
    t.pause(1.5)
    t.key("j", "j", "j", gap=1.2)
    t.pause(1)
    t.key("esc")
    t.key("esc")
    t.pause(0.8)

    t.scene("? lists every key, starting with the ones for where you are")
    t.key("?")
    t.pause(4)
    t.key("esc")
    t.pause(1)

    t.scene("cargo install ck")
    t.pause(3)
    t.log("end")
    while DONE and not os.path.exists(DONE):
        time.sleep(0.1)
    t.key("q")


def main():
    # The terminal's size, once the window has settled (it's made fullscreen as it opens).
    time.sleep(1.0)
    size = fcntl.ioctl(sys.stdin.fileno(), termios.TIOCGWINSZ, b"\0" * 8)
    pid, fd = pty.fork()
    if pid == 0:
        args = sys.argv[1:]
        os.execvp(CK, [CK, *args])
    fcntl.ioctl(fd, termios.TIOCSWINSZ, size)

    def resized(*_):
        fcntl.ioctl(fd, termios.TIOCSWINSZ, fcntl.ioctl(sys.stdin.fileno(), termios.TIOCGWINSZ, b"\0" * 8))
        os.kill(pid, signal.SIGWINCH)

    signal.signal(signal.SIGWINCH, resized)
    old = termios.tcgetattr(sys.stdin.fileno())
    tty.setraw(sys.stdin.fileno())
    t = Tour(fd)
    threading.Thread(target=lambda: (tour(t)), daemon=True).start()
    try:
        while True:
            try:
                ready, _, _ = select.select([sys.stdin.fileno(), fd], [], [])
            except InterruptedError:
                continue
            if fd in ready:
                try:
                    data = os.read(fd, 65536)
                except OSError:
                    break
                if not data:
                    break
                os.write(sys.stdout.fileno(), data)
            if sys.stdin.fileno() in ready:
                # The terminal's answers to ck's questions (what it can draw, its size).
                os.write(fd, os.read(sys.stdin.fileno(), 4096))
    finally:
        termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, old)
        os.waitpid(pid, 0)


if __name__ == "__main__":
    main()

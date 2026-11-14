"""Make README screenshots of Beamlogin: terminal screens (run in a real pty,
rendered with a terminal emulator) and the phone page (rendered in Chromium).

Everything runs against tests/fake_claude, a stand-in for Claude Code, so no real
account is used. See "Regenerating the screenshots" in the README for the commands.
Images land in docs/screenshots/out (raw) and docs/screenshots/final (for the README).
"""
import http.server
import json
import os
import pty
import re
import fcntl
import struct
import subprocess
import termios
import threading
import time
import urllib.parse
import urllib.request

import pyte
from PIL import Image, ImageDraw, ImageFont

OUT = "/work/out"
os.makedirs(OUT, exist_ok=True)
BIN = "/bin/beamlogin"
FAKE = "/tmp/claude"
GOOD_CODE = "goodcode123#state456"
GOOD_TOKEN = "sk-ant-oat01-" + "A" * 40
IP = "192.168.0.10"           # an example LAN address for the screenshots

# A stand-in Claude Code that shows a neutral example account.
src = open("/src/tests/fake_claude").read().replace("tester@example.com", "you@example.com")
open(FAKE, "w").write(src)
os.chmod(FAKE, 0o755)

# ---------------------------------------------------------------------------
# terminal rendering

MONO = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"
MONO_B = "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf"
SANS = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
SANS_B = "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"
S = 2                                   # render at 2x for sharp images
FS = 14 * S
font = ImageFont.truetype(MONO, FS)
fontb = ImageFont.truetype(MONO_B, FS)
fallback = ImageFont.truetype(SANS, FS)          # has the braille spinner glyphs
CW = font.getlength("M")
CH = int(FS * 1.25)
BG = (24, 24, 28)
FG = (222, 222, 222)
PALETTE = {
    "black": (0, 0, 0), "red": (240, 108, 108), "green": (95, 211, 141), "brown": (229, 192, 123),
    "blue": (97, 175, 239), "magenta": (198, 120, 221), "cyan": (86, 182, 194), "white": (255, 255, 255),
}


def color(c, default):
    if c == "default":
        return default
    if c in PALETTE:
        return PALETTE[c]
    if re.fullmatch(r"[0-9a-fA-F]{6}", c):
        return tuple(int(c[i:i + 2], 16) for i in (0, 2, 4))
    return default


def render(data, cols, rows, path, title):
    """Feed raw terminal output into an emulator and draw the final screen as a PNG."""
    data = data.replace(b"\x1b[2m", b"\x1b[38;5;245m")          # pyte has no 'dim'; use grey
    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)
    stream.feed(data)
    used = [y for y in range(rows) if any(screen.buffer[y][x].data.strip() or screen.buffer[y][x].bg != "default"
                                          for x in range(cols))]
    last = (max(used) + 2) if used else rows
    pad, bar = 18 * S, 34 * S
    W = int(cols * CW) + 2 * pad
    H = last * CH + 2 * pad + bar
    img = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, W - 1, H - 1], radius=12 * S, fill=BG + (255,))
    d.rounded_rectangle([0, 0, W - 1, bar + 12 * S], radius=12 * S, fill=(44, 44, 50, 255))
    d.rectangle([0, bar, W - 1, bar + 12 * S], fill=BG + (255,))
    for i, c in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        cx, cy = 20 * S + i * 20 * S, bar // 2
        d.ellipse([cx - 6 * S, cy - 6 * S, cx + 6 * S, cy + 6 * S], fill=c)
    tf = ImageFont.truetype(SANS, 13 * S)
    tw = d.textlength(title, font=tf)
    d.text(((W - tw) / 2, bar / 2), title, font=tf, fill=(170, 170, 178), anchor="lm")
    oy = bar + pad
    for y in range(last):
        for x in range(cols):
            ch = screen.buffer[y][x]
            fg, bg = color(ch.fg, FG), color(ch.bg, BG)
            if ch.reverse:
                fg, bg = bg, fg
            if ch.bold and ch.fg == "default":
                fg = (255, 255, 255)
            x0, x1 = pad + round(x * CW), pad + round((x + 1) * CW)
            y0, y1 = oy + y * CH, oy + (y + 1) * CH
            ym = (y0 + y1) // 2
            if bg != BG:
                d.rectangle([x0, y0, x1 - 1, y1 - 1], fill=bg)
            g = ch.data
            if g == "█":
                d.rectangle([x0, y0, x1 - 1, y1 - 1], fill=fg)
            elif g == "▀":
                d.rectangle([x0, y0, x1 - 1, ym - 1], fill=fg)
            elif g == "▄":
                d.rectangle([x0, ym, x1 - 1, y1 - 1], fill=fg)
            elif g.strip():
                f = fallback if 0x2800 <= ord(g[0]) <= 0x28FF else (fontb if ch.bold else font)
                d.text((x0, y0 + CH / 2), g, font=f, fill=fg, anchor="lm")
    img.save(path)
    print("wrote", path, img.size)


class Term:
    """Run a command in a pty of a fixed size and record everything it prints."""

    def __init__(self, argv, cols=100, rows=34, env=None):
        self.cols, self.rows, self.data = cols, rows, b""
        e = {k: v for k, v in os.environ.items() if k not in ("COLUMNS", "LINES")}
        e.update(TERM="xterm-256color", COLORTERM="truecolor", **(env or {}))
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execve(argv[0], argv, e)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        while True:
            try:
                b = os.read(self.fd, 65536)
            except OSError:
                return
            if not b:
                return
            self.data += b

    def text(self):
        return re.sub(r"\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07", "", self.data.decode("utf-8", "replace"))

    def wait(self, pattern, timeout=20):
        end = time.time() + timeout
        while time.time() < end:
            m = re.search(pattern, self.text())
            if m:
                return m
            time.sleep(0.05)
        raise SystemExit(f"timeout waiting for {pattern!r}:\n{self.text()[-2000:]}")

    def send(self, s):
        os.write(self.fd, s.encode())

    def shot(self, name, title):
        time.sleep(0.4)
        render(self.data, self.cols, self.rows, f"{OUT}/{name}.png", title)

    def kill(self):
        try:
            os.kill(self.pid, 9)
            os.waitpid(self.pid, 0)
        except OSError:
            pass


def env(state, **extra):
    return {"FAKE_STATE": state, "FAKE_CODE": GOOD_CODE, "FAKE_TOKEN": GOOD_TOKEN, **extra}


def get(url, data=None):
    body = urllib.parse.urlencode({"value": data}).encode() if data is not None else None
    try:
        with urllib.request.urlopen(url, data=body, timeout=10) as r:
            return r.read().decode()
    except urllib.error.HTTPError as e:
        return e.read().decode()


TITLE = "root@live-iso: ~"
pages = {}

# 1. login with the phone page -----------------------------------------------
state = "/tmp/state"
t = Term([BIN, "--ip", IP, "--port", "8765", "--claude", FAKE], env=env(state))
url = t.wait(r"Page: (http://\S+)").group(1).replace(IP, "127.0.0.1")
pages["login"] = get(url)                                    # the phone opens the page
t.wait(r"Phone connected")
t.shot("terminal-login", TITLE)
get(url, GOOD_CODE)
t.wait(r"Press Enter to start Claude Code")
t.shot("terminal-signed-in", TITLE)
t.kill()

# 2. direct sign-in link view (no shared network) -----------------------------
os.remove(state)
t = Term([BIN, "--ip", IP, "--port", "8765", "--claude", FAKE], cols=120, rows=56, env=env(state))
t.wait(r"Waiting for your phone")
t.send("u\r")
t.wait(r"Scan to sign in on any device")
t.shot("terminal-direct-link", TITLE)
t.kill()

# 3. token mode ----------------------------------------------------------------
t = Term([BIN, "token", "--ip", IP, "--port", "8765", "--claude", FAKE], env=env(state))
url = t.wait(r"Page: (http://\S+)").group(1).replace(IP, "127.0.0.1")
pages["token"] = get(url)
t.wait(r"Phone connected")
t.shot("terminal-token", TITLE)
get(url, GOOD_TOKEN)
t.wait(r"Press Enter to start Claude Code")
t.shot("terminal-token-works", TITLE)
t.kill()

# 4. doctor ----------------------------------------------------------------------
open(state, "w").write("x")
t = Term([BIN, "doctor", "--ip", IP, "--claude", FAKE], env=env(state))
t.wait(r"Ready|Not ready", 60)
t.shot("terminal-doctor", TITLE)
t.kill()

# 5. a fresh machine without Claude Code ------------------------------------------
home = "/tmp/emptyhome"
os.makedirs(home, exist_ok=True)
t = Term([BIN, "--ip", IP], env=env(state, HOME=home, PATH="/usr/sbin:/usr/bin:/sbin:/bin"))
t.wait(r"Install Claude Code now\?")
t.shot("terminal-install", TITLE)
t.kill()

# 6. logout ------------------------------------------------------------------------
t = Term([BIN, "logout", "--claude", FAKE], env=env(state))
t.wait(r"Log out on the matching")
t.shot("terminal-logout", TITLE)
t.kill()

# ---------------------------------------------------------------------------
# phone page: serve the real page HTML from a mock that plays back each state

STATUS = {}
DEMO = {
    "fill": """<script>
document.getElementById('s1') && (document.getElementById('s1').dataset.state='done');
document.getElementById('s2') && (document.getElementById('s2').dataset.state='active');
const f=document.getElementById('v'); f.value=%s; f.dispatchEvent(new Event('input'));
</script>""",
    "submit": """<script>setTimeout(()=>document.getElementById('f').requestSubmit(),200);</script>""",
}


class Mock(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def _send(self, code, ctype, body):
        b = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        parts = self.path.strip("/").split("/")
        name = parts[0]
        if len(parts) > 1:
            st, msg = STATUS[name]
            return self._send(200, "application/json", json.dumps({"state": st, "message": msg}))
        return self._send(200, "text/html; charset=utf-8", PAGES[name])

    def do_POST(self):
        self._send(200, "application/json", '{"ok": true}')


def dark(html):
    return html.replace("@media (prefers-color-scheme:dark){:root{", "@media all{:root{")


PAGES = {}
FAIL = ("Claude Code didn't accept the code. Codes work once and expire after a few minutes, "
        "so run beamlogin on the terminal again to get a new one.")
fill_code = DEMO["fill"] % json.dumps(GOOD_CODE + "-example-code")
fill_token = DEMO["fill"] % json.dumps(GOOD_TOKEN)
for name, base, extra, status in [
    ("p-login", "login", "", ("waiting", "")),
    ("p-ready", "login", fill_code, ("waiting", "")),
    ("p-done", "login", fill_code + DEMO["submit"], ("done", "")),
    ("p-failed", "login", fill_code + DEMO["submit"], ("failed", FAIL)),
    ("p-token", "token", fill_token, ("waiting", "")),
    ("p-token-done", "token", fill_token + DEMO["submit"], ("done", "")),
]:
    # A phone tap doesn't draw a keyboard focus ring; headless Chrome does. Hide it for the screenshot.
    html = pages[base].replace("</style>", "#result:focus{outline:none}</style>", 1)
    html = html.replace("</body>", extra + "</body>")
    PAGES[name] = html
    PAGES[name + "-dark"] = dark(html)
    STATUS[name] = STATUS[name + "-dark"] = status

srv = http.server.ThreadingHTTPServer(("127.0.0.1", 9000), Mock)
threading.Thread(target=srv.serve_forever, daemon=True).start()

for name in PAGES:
    path = f"{OUT}/{name}.png"
    subprocess.run(["chromium", "--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars",
                    "--window-size=390,844", "--force-device-scale-factor=2", "--virtual-time-budget=6000",
                    f"--screenshot={path}", f"http://127.0.0.1:9000/{name}"],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=90)
    print("wrote", path)
print("done")

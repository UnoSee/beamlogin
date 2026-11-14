"""Compare Beamlogin with the earlier Python version (0.1.0), output for output.

Usage: python3 tests/compare.py /path/to/python/beamlogin [path/to/binary]

Checks:
  1. QR codes: `beamlogin qr TEXT --ecl X` output is byte-identical for many inputs.
  2. Phone page: every page and JSON response is byte-identical (after replacing
     the random nonces), with the same status codes and security headers.
  3. Terminal text: plain output of logout, doctor and the missing-Claude screen matches.
"""
import os
import random
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from importlib.machinery import SourceFileLoader

HERE = os.path.dirname(os.path.abspath(__file__))
PY = os.path.abspath(sys.argv[1])
RS = os.path.abspath(sys.argv[2] if len(sys.argv) > 2 else os.path.join(HERE, "..", "dist", "beamlogin-x86_64-linux"))
FAKE = os.path.join(HERE, "fake_claude")
GOOD_CODE = "goodcode123#state456"
GOOD_TOKEN = "sk-ant-oat01-" + "A" * 40
pymod = SourceFileLoader("beamlogin_py", PY).load_module()
results = []


def check(name, cond, detail=""):
    results.append(cond)
    print(("PASS " if cond else "FAIL ") + name + ("" if cond else f"\n     {detail}"))


def first_diff(a, b):
    for i, (x, y) in enumerate(zip(a, b)):
        if x != y:
            return f"at {i}: py={a[max(0, i - 40):i + 40]!r}\n     rs={b[max(0, i - 40):i + 40]!r}"
    return f"length py={len(a)} rs={len(b)}"


# 1. QR codes ----------------------------------------------------------------
rng = random.Random(7)
texts = ["A", "hello", "http://192.168.0.10:8765/" + "x" * 16, "https://claude.ai/oauth/authorize?code=true&client_id=" + "a" * 300,
         "ünïcödé ✓ 日本語", "0123456789" * 20, "A" * 300, "x" * 1200, " ", "~!@#$%^&*()_+"]
for n in (1, 7, 14, 17, 32, 53, 78, 106, 134, 154, 192, 230, 271, 321, 367, 425, 458, 520, 586, 644, 718, 792, 858, 929,
          1003, 1091, 1171, 1273, 1367, 1465, 1528, 1628, 1732, 1840, 1952, 2068, 2188, 2303, 2431, 2563):
    texts.append("".join(rng.choice("abcdefghijklmnopqrstuvwxyzABCDEFGHIJ0123456789:/?&=._-") for _ in range(n)))
qr_total = qr_same = 0
qr_fail = []
for t in texts:
    for ecl in "LMQH":
        for style in ("half", "ascii"):
            try:
                want = pymod.qr_render(pymod.qr_encode(t, ecl=ecl), style=style) + "\n"
            except ValueError:
                continue                       # too long for this error-correction level
            got = subprocess.run([RS, "qr", "--ecl", ecl, "--qr-style", style, "--", t], capture_output=True).stdout.decode()
            qr_total += 1
            if got == want:
                qr_same += 1
            else:
                qr_fail.append((len(t), ecl, style))
check(f"qr: {qr_same}/{qr_total} renders byte-identical (all versions 1-40, L/M/Q/H, half/ascii)",
      qr_same == qr_total and qr_total > 300, qr_fail[:5])
too_long = subprocess.run([RS, "qr", "x" * 3000], capture_output=True)
check("qr: too-long text fails cleanly", too_long.returncode == 1 and b"too long" in too_long.stderr, too_long.stderr)


# 2. Phone page ----------------------------------------------------------------
def env_with(state, **extra):
    e = dict(os.environ, FAKE_STATE=state, FAKE_CODE=GOOD_CODE, FAKE_TOKEN=GOOD_TOKEN, **extra)
    e.pop("CLAUDE_CODE_OAUTH_TOKEN", None)
    return e


def start(cmd, mode, state):
    p = subprocess.Popen(cmd + [mode, "--ip", "127.0.0.1", "--timeout", "60", "--no-launch", "--claude", FAKE],
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, env=env_with(state))
    out = ""
    end = time.time() + 15
    while time.time() < end:
        line = p.stdout.readline()
        out += line
        m = re.search(r"Page: (http://\S+)", line)
        if m:
            return p, m.group(1)
    raise AssertionError(out)


def req(url, data=None, accept=None):
    body = urllib.parse.urlencode({"value": data}).encode() if data is not None else None
    r = urllib.request.Request(url, data=body, headers={"Accept": accept} if accept else {})
    try:
        with urllib.request.urlopen(r, timeout=10) as resp:
            return resp.status, resp.read().decode(), dict(resp.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(), dict(e.headers)


HEADERS = ("Content-Type", "Cache-Control", "Referrer-Policy", "X-Frame-Options", "X-Content-Type-Options",
           "Permissions-Policy", "Content-Security-Policy")


def norm(text, page_url, headers):
    nonce = page_url.rsplit("/", 1)[1]
    text = text.replace(nonce, "<URLNONCE>")
    m = re.search(r"'nonce-([^']+)'", headers.get("Content-Security-Policy", ""))
    if m:
        text = text.replace(m.group(1), "<CSPNONCE>")
    return text


def norm_headers(h):
    out = {k: h.get(k) for k in HEADERS}
    out["Content-Security-Policy"] = re.sub(r"'nonce-[^']+'", "'nonce-X'", out["Content-Security-Policy"] or "")
    return out


def session(cmd, mode, steps):
    tmp = tempfile.mkdtemp()
    state = os.path.join(tmp, "state")
    p, url = start(cmd, mode, state)
    base = url.rsplit("/", 1)[0]
    out = []
    for name, path, data, accept in steps:
        target = url + path if not path.startswith("!") else base + path[1:]
        code, body, headers = req(target, data, accept)
        if name == "status":
            time.sleep(0)
        out.append((name, code, norm(body, url, headers), norm_headers(headers)))
    p.wait(30)
    p.stdout.read()
    return out


flows = {
    "login": [("page", "", None, None), ("404", "!/nope", None, None), ("bad form", "", "bad", None),
              ("bad json", "", "bad", "application/json"), ("status", "/status", None, None),
              ("good form", "", GOOD_CODE, None), ("410 page", "", None, None), ("410 json", "", GOOD_CODE, "application/json")],
    "token": [("page", "", None, None), ("bad form", "", "not-a-token", None),
              ("bad json", "", "not-a-token", "application/json"), ("good json", "", GOOD_TOKEN, "application/json"),
              ("410 form", "", GOOD_TOKEN, None)],
}
for mode, steps in flows.items():
    a = session([sys.executable, PY], mode, steps)
    b = session([RS], mode, steps)
    for (name, c1, b1, h1), (_, c2, b2, h2) in zip(a, b):
        check(f"page {mode}/{name}: status {c1}, body and headers identical",
              c1 == c2 and b1 == b2 and h1 == h2,
              f"code {c1} vs {c2}; headers same={h1 == h2} {h1 if h1 != h2 else ''} {h2 if h1 != h2 else ''}; "
              + ("" if b1 == b2 else first_diff(b1, b2)))


# 3. Terminal text ----------------------------------------------------------------
def run(cmd, *argv, env=None):
    p = subprocess.run(cmd + list(argv), capture_output=True, text=True, env=env, timeout=60)
    return p.returncode, p.stdout + p.stderr


tmp = tempfile.mkdtemp()
state = os.path.join(tmp, "state")
home = tempfile.mkdtemp()
os.makedirs(os.path.join(home, ".local", "bin"))
os.symlink(FAKE, os.path.join(home, ".local", "bin", "claude"))
cases = {
    "logout": (["logout", "--claude", FAKE], env_with(state)),
    "doctor (Claude outside PATH)": (["doctor", "--plain", "--ip", "10.0.0.5"], env_with(state, HOME=home, PATH="/usr/bin:/bin")),
    "missing Claude Code": (["login", "--claude", "/nonexistent/claude", "--ip", "127.0.0.1"], env_with(state)),
    "already signed in": (["login", "--no-launch", "--claude", FAKE, "--ip", "127.0.0.1"], env_with(state)),
}
for name, (argv, env) in cases.items():
    open(state, "w").close()                    # signed in (fake_claude keys off this file)
    r1, o1 = run([sys.executable, PY], *argv, env=env)
    open(state, "w").close()
    r2, o2 = run([RS], *argv, env=env)
    # The binary needs no Python, so it has no "Python" check line. Drop it, then compare.
    o1 = "".join(l for l in o1.splitlines(True) if "✓ Python " not in l)
    if name.startswith("doctor"):
        o1 = o1.replace("Ready  2 things", "Ready  N things").replace("Ready  1 thing ", "Ready  N things")
        o2 = o2.replace("Ready  2 things", "Ready  N things").replace("Ready  1 thing ", "Ready  N things")
        # network reachability and terminal size can differ between runs; compare names only
        o1 = re.sub(r"(Reach \S+\s+).*", r"\1…", o1)
        o2 = re.sub(r"(Reach \S+\s+).*", r"\1…", o2)
    check(f"text: {name} output identical (exit {r1})", r1 == r2 and o1 == o2,
          f"exit {r1} vs {r2}\n--- python\n{o1}\n--- rust\n{o2}")

print(f"\n{sum(results)}/{len(results)} comparisons matched")
sys.exit(0 if all(results) else 1)

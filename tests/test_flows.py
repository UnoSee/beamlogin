"""End-to-end tests of beamlogin against tests/fake_claude (Linux, macOS and Windows).

Usage:
  python3 tests/test_flows.py                                            # dist/ binary for this OS
  BEAMLOGIN_BIN=/path/to/binary python3 tests/test_flows.py              # another binary
  BEAMLOGIN_BIN=/path/to/beamlogin.py PY=1 python3 tests/test_flows.py   # the Python version (0.1.0)
"""
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

WIN = os.name == "nt"
HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_BIN = "beamlogin-x86_64-windows.exe" if WIN else "beamlogin-x86_64-linux"
TOOL = os.path.abspath(os.environ.get("BEAMLOGIN_BIN") or os.path.join(HERE, "..", "dist", DEFAULT_BIN))
IS_PY = bool(os.environ.get("PY"))
CMD = [sys.executable, TOOL] if IS_PY else [TOOL]
FAKE_PY = os.path.join(HERE, "fake_claude")


def make_fake(folder, name="claude"):
    """Put a runnable fake Claude Code at folder/name (a .cmd wrapper on Windows)."""
    if WIN:
        path = os.path.join(folder, name + ".cmd")
        with open(path, "w") as f:
            f.write(f'@"{sys.executable}" "{FAKE_PY}" %*\r\n')
        return path
    path = os.path.join(folder, name)
    os.symlink(FAKE_PY, path)
    return path


FAKE = make_fake(tempfile.mkdtemp()) if WIN else FAKE_PY
GOOD_CODE = "goodcode123#state456"
GOOD_TOKEN = "sk-ant-oat01-" + "A" * 40
BAD_TOKEN = "sk-ant-oat01-" + "B" * 40
results = []


class Run:
    """Start beamlogin and collect its output in the background."""

    def __init__(self, *argv, env=None, stdin=subprocess.DEVNULL):
        self.out = ""
        self.p = subprocess.Popen([*CMD, *argv, "--claude", FAKE],
                                  stdin=stdin, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                  text=True, encoding="utf-8", errors="replace", env=env)
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        for line in self.p.stdout:
            self.out += line

    def wait_for(self, pattern, timeout=15):
        end = time.time() + timeout
        while time.time() < end:
            m = re.search(pattern, self.out)
            if m:
                return m
            time.sleep(0.05)
        raise AssertionError(f"timed out waiting for {pattern!r}; output so far:\n{self.out}")

    def finish(self, timeout=30):
        rc = self.p.wait(timeout)
        time.sleep(0.2)
        return rc


def http(url, data=None):
    body = urllib.parse.urlencode({"value": data}).encode() if data is not None else None
    try:
        with urllib.request.urlopen(url, data=body, timeout=10) as r:
            return r.status, r.read().decode(), dict(r.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(), dict(e.headers)


def check(name, cond, detail=""):
    results.append(cond)
    print(("PASS " if cond else "FAIL ") + name + ("" if cond else f"\n     {detail}"))


def env_with(state, **extra):
    e = dict(os.environ, FAKE_STATE=state, FAKE_CODE=GOOD_CODE, FAKE_TOKEN=GOOD_TOKEN, **extra)
    e.pop("CLAUDE_CODE_OAUTH_TOKEN", None)
    return e


tmp = tempfile.mkdtemp()
state = os.path.join(tmp, "state")

# 1. login through the phone page ------------------------------------------
r = Run("login", "--ip", "127.0.0.1", "--timeout", "60", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
code, body, headers = http(page)
check("login: phone page loads", code == 200 and "Sign in to Claude" in body, body[:300])
check("login: page links the claude.ai sign-in URL",
      "https://claude.ai/oauth/authorize?code=true&amp;client_id=" in body, body[:600])
check("login: no-referrer header (nonce never leaks to claude.ai)",
      headers.get("Referrer-Policy") == "no-referrer", headers)
check("login: QR code printed", "\x1b[30;47m" in r.out)
code, _, _ = http(page.rsplit("/", 1)[0] + "/wrong-nonce")
check("login: wrong nonce gets 404", code == 404, code)
code, body, _ = http(page, "bad")
check("login: malformed code rejected", code == 400 and "valid code" in body, code)
code, body, _ = http(page, "  " + GOOD_CODE + "\n")
check("login: good code accepted", code == 200 and "Sent to the terminal" in body, body[:200])
code, _, _ = http(page, GOOD_CODE)
check("login: page is one-time (410 after use)", code == 410, code)
rc = r.finish()
check("login: exits 0 and reports success", rc == 0 and "Signed in to Claude Code" in r.out, r.out[-800:])
check("login: Claude Code launched afterwards", "FAKE INTERACTIVE SESSION token=unset state=yes" in r.out, r.out[-400:])
check("login: revoke reminder shown", "Where you're logged in" in r.out)

# 2. already signed in ------------------------------------------------------
r = Run("login", "--no-launch", "--ip", "127.0.0.1", env=env_with(state))
rc = r.finish()
check("login: detects existing sign-in", rc == 0 and "already signed in" in r.out, r.out)

# 3. wrong code -------------------------------------------------------------
os.remove(state)
r = Run("login", "--no-launch", "--ip", "127.0.0.1", "--timeout", "60", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
http(page, "wrongcode999#zzz")
rc = r.finish()
check("login: wrong code -> failure exit and Claude's error shown",
      rc == 1 and "Sign-in failed" in r.out and "Invalid code" in r.out, r.out[-600:])
check("login: no session state after failure", not os.path.exists(state))

# 4. manual entry, no phone page -------------------------------------------
r = Run("login", "--no-server", "--no-launch", "--stdin-commands", "--timeout", "60",
        env=env_with(state), stdin=subprocess.PIPE)
r.wait_for(r"Type or paste the code")
check("manual: shows the sign-in URL QR directly", "Scan to sign in on any device" in r.out)
r.p.stdin.write(GOOD_CODE + "\n")
r.p.stdin.flush()
rc = r.finish()
check("manual: typed code signs in", rc == 0 and os.path.exists(state), r.out[-600:])

# 5. logout -----------------------------------------------------------------
r = Run("logout", env=env_with(state))
rc = r.finish()
check("logout: signs out and explains how to revoke",
      rc == 0 and not os.path.exists(state) and "Log out on the matching" in r.out, r.out)

# 6. token through the phone page ------------------------------------------
r = Run("token", "--ip", "127.0.0.1", "--timeout", "60", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
code, body, _ = http(page)
check("token: page has a password field", code == 200 and 'type="password"' in body, body[:300])
code, body, _ = http(page, "not-a-token")
check("token: non-token rejected", code == 400, code)
code, _, _ = http(page, GOOD_TOKEN)
rc = r.finish()
check("token: verified and Claude Code launched with it",
      rc == 0 and "Token works" in r.out and "FAKE INTERACTIVE SESSION token=set" in r.out, r.out[-600:])
check("token: token never printed", GOOD_TOKEN not in r.out)

# 7. revoked token ----------------------------------------------------------
r = Run("token", "--ip", "127.0.0.1", "--timeout", "60", "--no-launch", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
http(page, BAD_TOKEN)
rc = r.finish()
check("token: revoked token -> failure exit", rc == 1 and "did not work" in r.out, r.out[-600:])
check("token: token scrubbed from Claude's error output", BAD_TOKEN not in r.out and "[token]" in r.out, r.out[-400:])

# 8. --save writes an owner-only file --------------------------------------
if WIN:
    r = Run("token", "--save", env=env_with(state))
    rc = r.finish()
    check("token --save: refused on Windows with a clear message",
          rc == 2 and "--save works on Linux and macOS only" in r.out, r.out)
else:
    rt = tempfile.mkdtemp()
    r = Run("token", "--ip", "127.0.0.1", "--timeout", "60", "--no-launch", "--save",
            env=env_with(state, XDG_RUNTIME_DIR=rt))
    page = r.wait_for(r"Page: (http://\S+)").group(1)
    http(page, GOOD_TOKEN)
    rc = r.finish()
    saved = os.path.join(rt, f"beamlogin-{os.getuid()}", "token.env")
    mode = stat.S_IMODE(os.stat(saved).st_mode) if os.path.exists(saved) else None
    check("token --save: file is 0600 and sourceable",
          rc == 0 and mode == 0o600 and open(saved).read() == f"export CLAUDE_CODE_OAUTH_TOKEN='{GOOD_TOKEN}'\n",
          f"rc={rc} mode={mode and oct(mode)}")

# 9. too many bad requests shuts the page ----------------------------------
r = Run("token", "--ip", "127.0.0.1", "--timeout", "60", "--no-launch", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
for i in range(20):
    http(page.rsplit("/", 1)[0] + f"/guess{i}")
rc = r.finish()
check("relay: stops after 20 wrong-nonce requests", rc == 1 and "shut down for safety" in r.out, r.out[-300:])


# 10. the page's JavaScript path: JSON submit and live status ---------------
def post_json(url, value):
    req = urllib.request.Request(url, data=urllib.parse.urlencode({"value": value}).encode(),
                                 headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status, json.loads(resp.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read())


def poll_until_final(url, timeout=20):
    seen, end = [], time.time() + timeout
    while time.time() < end:
        try:
            with urllib.request.urlopen(url + "/status", timeout=5) as resp:
                s = json.loads(resp.read())
        except OSError:
            break
        if not seen or seen[-1]["state"] != s["state"]:
            seen.append(s)
        if s["state"] in ("done", "failed"):
            break
        time.sleep(0.2)
    return seen


os.path.exists(state) and os.remove(state)
r = Run("login", "--ip", "127.0.0.1", "--timeout", "60", "--no-launch", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
code, body, headers = http(page)
csp = headers.get("Content-Security-Policy", "")
check("page: strict Content-Security-Policy with per-response nonce",
      "default-src 'none'" in csp and "script-src 'nonce-" in csp and "frame-ancestors 'none'" in csp, csp)
n = re.search(r"script-src 'nonce-([^']+)'", csp)
check("page: inline script carries the CSP nonce", bool(n) and f'<script nonce="{n.group(1)}">' in body)
code, j = post_json(page, "short")
check("json: invalid code -> 400 with message", code == 400 and j["ok"] is False and "valid code" in j["error"], j)
code, j = post_json(page, GOOD_CODE)
check("json: good code -> ok", code == 200 and j == {"ok": True}, j)
states = [s["state"] for s in poll_until_final(page)]
check("status: phone sees received -> done", states[:1] in (["received"], ["done"]) and states[-1] == "done", states)
try:
    post_json(page, GOOD_CODE)  # late resubmit: 410, or the page has already closed
except OSError:
    pass
rc = r.finish()
check("json: login run exits 0", rc == 0, r.out[-400:])

os.remove(state)
r = Run("login", "--ip", "127.0.0.1", "--timeout", "60", "--no-launch", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
post_json(page, "wrongcode999#zzz")
seen = poll_until_final(page)
check("status: phone is told when Claude rejects the code",
      seen and seen[-1]["state"] == "failed" and "didn't accept the code" in seen[-1]["message"], seen)
r.finish()

r = Run("token", "--ip", "127.0.0.1", "--timeout", "60", "--no-launch", env=env_with(state))
page = r.wait_for(r"Page: (http://\S+)").group(1)
post_json(page, BAD_TOKEN)
seen = poll_until_final(page)
check("status: phone is told when the token doesn't work",
      seen and seen[-1]["state"] == "failed" and "didn't work" in seen[-1]["message"]
      and BAD_TOKEN not in json.dumps(seen), seen)
r.finish()

# 11. dependency checks -----------------------------------------------------
def run_raw(*argv, env=None):
    p = subprocess.run([*CMD, *argv], capture_output=True, text=True, encoding="utf-8", errors="replace",
                       env=env, timeout=60)
    return p.returncode, p.stdout + p.stderr


def bare_env(home):
    """No Claude Code on PATH, and home pointing at the given folder."""
    if WIN:
        sysdir = os.path.join(os.environ.get("SystemRoot", r"C:\Windows"), "System32")
        return env_with(state, USERPROFILE=home, HOME=home, APPDATA=os.path.join(home, "AppData", "Roaming"),
                        LOCALAPPDATA=os.path.join(home, "AppData", "Local"), PATH=sysdir)
    return env_with(state, HOME=home, PATH="/usr/bin:/bin")


rc, out = run_raw("login", "--claude", "/nonexistent/claude", "--ip", "127.0.0.1", env=env_with(state))
check("deps: missing Claude Code stops with a clear message",
      rc == 1 and "Claude Code isn't installed" in out and "not found at /nonexistent/claude" in out, out)

empty_home = tempfile.mkdtemp()
rc, out = run_raw("login", "--ip", "127.0.0.1", env=bare_env(empty_home))
check("deps: missing Claude Code suggests the official installer",
      rc == 1 and ("claude.ai/install.ps1" if WIN else "claude.ai/install.sh") in out, out)

home = tempfile.mkdtemp()
os.makedirs(os.path.join(home, ".local", "bin"))
make_fake(os.path.join(home, ".local", "bin"))
bare = bare_env(home)
rc, out = run_raw("doctor", "--plain", env=bare)
if WIN:
    where = r"at ~\.local\bin\claude.cmd (not on your PATH)"
    fix = r'$env:Path = "$env:USERPROFILE\.local\bin;$env:Path"'
else:
    where = "at ~/.local/bin/claude (not on your PATH)"
    fix = 'export PATH="$HOME/.local/bin:$PATH"'
check("doctor: finds Claude Code outside PATH and says how to fix PATH",
      rc == 0 and f"2.1.300 (Claude Code), {where}" in out and fix in out, out)

if os.path.exists(state):
    os.remove(state)
p = subprocess.Popen([*CMD, "login", "--ip", "127.0.0.1", "--timeout", "30", "--no-launch"],
                     stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8",
                     errors="replace", env=bare)
time.sleep(3)
p.kill()
first = p.communicate()[0]
check("deps: login uses Claude Code found outside PATH", "Page: http://" in first, first[-400:])

rc, out = run_raw("doctor", "--plain", "--claude", FAKE, env=env_with(state))
check("doctor: reports system and Claude Code",
      rc == 0 and "Ready" in out and ("Python" in out or not IS_PY) and "System" in out and "2.1.300 (Claude Code)" in out, out)

print(f"\n{sum(results)}/{len(results)} checks passed")
sys.exit(0 if all(results) else 1)

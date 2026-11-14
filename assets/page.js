
(() => {
  const $ = (id) => document.getElementById(id);
  const mode = document.body.dataset.mode, host = document.body.dataset.host;
  const form = $("f"), field = $("v"), hint = $("hint"), btn = $("go"), flow = $("flow"), res = $("result");
  if (!form) return;
  const re = mode === "login" ? /^[A-Za-z0-9_\-#.~=+\/]{8,2048}$/ : /^sk-ant-[A-Za-z0-9_\-]{20,400}$/;
  const clean = () => field.value.replace(/\s+/g, "");
  const icons = { ok: $("i-ok").innerHTML, bad: $("i-bad").innerHTML, info: $("i-info").innerHTML };
  function setStep(id, state) { const el = $(id); if (el) el.dataset.state = state; }
  function check() {
    const v = clean(), ok = re.test(v);
    btn.disabled = !ok;
    hint.className = "hint" + (v && !ok ? " bad" : ok ? " good" : "");
    if (!v) hint.firstElementChild.textContent = hint.dataset.empty;
    else if (ok) hint.firstElementChild.textContent = mode === "login" ? "Ready to send" : "Looks like a setup token";
    else hint.firstElementChild.textContent = mode === "login" ? "That doesn't look like a complete code yet"
      : (v.startsWith("sk-ant-") ? "This token looks incomplete" : "Setup tokens start with sk-ant-");
    if (hint.lastElementChild) hint.lastElementChild.textContent = v ? v.length + " chars" : "";
  }
  function show(kind, title, text) {
    flow.hidden = true; res.hidden = false; res.dataset.kind = kind;
    $("r-badge").innerHTML = kind === "wait" ? '<span class="spin"></span>' : icons[kind];
    $("r-title").textContent = title; $("r-text").innerHTML = text;
    res.focus();
  }
  const T = {
    login: {
      received: ["Signing in…", "Finishing sign-in on <b>" + host + "</b>."],
      done: ["You're signed in", "Go back to <b>" + host + "</b> and press <kbd>Enter</kbd> to start Claude Code. You can close this tab."],
    },
    token: {
      received: ["Checking the token…", "<b>" + host + "</b> is sending a tiny test request."],
      done: ["Token accepted", "Go back to <b>" + host + "</b> and press <kbd>Enter</kbd> to start Claude Code. You can close this tab."],
    },
  }[mode];
  let misses = 0;
  async function poll() {
    try {
      const r = await fetch(location.pathname + "/status", { cache: "no-store" });
      const s = await r.json(); misses = 0;
      if (s.state === "done") return show("ok", T.done[0], T.done[1]);
      if (s.state === "failed") return show("bad", "That didn't work", s.message || "Check the terminal.");
      if (s.state === "received") show("wait", T.received[0], T.received[1]);
    } catch (e) {
      if (++misses >= 3) return show("info", "Sent to the terminal", "This page has closed. Check <b>" + host + "</b> for the result.");
    }
    setTimeout(poll, 1000);
  }
  field.addEventListener("input", check);
  field.addEventListener("paste", () => setTimeout(check, 0));
  const open = $("open");
  if (open) open.addEventListener("click", () => {
    setStep("s1", "done"); setStep("s2", "active");
    document.addEventListener("visibilitychange", () => { if (!document.hidden) field.focus(); }, { once: true });
  });
  const peek = $("peek");
  if (peek) peek.addEventListener("click", () => {
    const on = field.type === "password"; field.type = on ? "text" : "password";
    peek.setAttribute("aria-pressed", String(on)); peek.setAttribute("aria-label", on ? "Hide token" : "Show token");
  });
  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    check(); if (btn.disabled) return;
    btn.disabled = true; btn.classList.add("busy"); btn.querySelector(".lbl").textContent = "Sending…";
    try {
      const r = await fetch(location.pathname, { method: "POST", headers: { Accept: "application/json" },
        body: new URLSearchParams({ value: clean() }) });
      const j = await r.json();
      if (!j.ok) throw new Error(j.error || "Something went wrong.");
      show("wait", T.received[0], T.received[1]); poll();
    } catch (err) {
      btn.classList.remove("busy"); btn.querySelector(".lbl").textContent = "Send to terminal"; btn.disabled = false;
      hint.className = "hint bad";
      hint.firstElementChild.textContent = err instanceof TypeError ? "Couldn't reach the terminal. Is the page still open there?" : err.message;
    }
  });
  check();
})();

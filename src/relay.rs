//! One-time local network relay: a tiny HTTP server that accepts exactly one
//! value from the phone and reports the terminal's progress back to it.

use crate::sys;
use crate::ui::Ui;
use crate::util::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PAGE: &str = include_str!("../assets/page.html");
const CSS: &str = include_str!("../assets/page.css");
const JS: &str = include_str!("../assets/page.js");
const LOGIN_FORM: &str = include_str!("../assets/login_form.html");
const LOGIN_FORM_ERR: &str = include_str!("../assets/login_form_err.html");
const TOKEN_FORM: &str = include_str!("../assets/token_form.html");
const TOKEN_FORM_ERR: &str = include_str!("../assets/token_form_err.html");
const WAITING: &str = include_str!("../assets/waiting.html");
const CLOSED: &str = include_str!("../assets/closed.html");
const ICON_CHECK: &str = include_str!("../assets/icon_check.svg");

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Login,
    Token,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Login => "login",
            Mode::Token => "token",
        }
    }
}

struct Shared {
    mode: Mode,
    nonce: String,
    host: String,
    auth_url: Option<String>,
    used: bool,
    from_phone: bool,
    state: String,
    message: String,
    final_seen: bool,
    bad: u32,
    max_bad: u32,
    aborted: bool,
    opened: bool,
    tx: Sender<String>,
}

pub struct Relay {
    shared: Arc<Mutex<Shared>>,
    rx: Receiver<String>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub url: String,
}

pub fn guess_lan_ip() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?; // TEST-NET address: no packet is sent
    Some(s.local_addr().ok()?.ip().to_string())
}

impl Relay {
    pub fn start(mode: Mode, bind: &str, port: u16, ip: &str) -> std::io::Result<Relay> {
        let listener = TcpListener::bind((bind, port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let (tx, rx) = channel();
        let nonce = token_urlsafe(12);
        let shared = Arc::new(Mutex::new(Shared {
            mode,
            nonce: nonce.clone(),
            host: html_escape(&sys::hostname()),
            auth_url: None,
            used: false,
            from_phone: false,
            state: "waiting".into(),
            message: String::new(),
            final_seen: false,
            bad: 0,
            max_bad: 20,
            aborted: false,
            opened: false,
            tx,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (sh, st) = (shared.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            while !st.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let sh = sh.clone();
                        std::thread::spawn(move || handle(stream, &sh));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(30)),
                }
            }
        });
        Ok(Relay { shared, rx, stop, thread: Some(thread), url: format!("http://{}:{}/{}", ip, port, nonce) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_auth_url(&self, url: &str) {
        self.lock().auth_url = Some(url.to_string());
    }
    pub fn mark_used(&self) {
        self.lock().used = true;
    }
    pub fn aborted(&self) -> bool {
        self.lock().aborted
    }
    pub fn opened(&self) -> bool {
        self.lock().opened
    }
    pub fn try_value(&self) -> Option<String> {
        self.rx.try_recv().ok()
    }

    /// Report the final outcome; give the phone a moment to pick it up.
    pub fn finish(&mut self, state: &str, message: &str, ui: &mut Ui) {
        let from_phone = {
            let mut s = self.lock();
            s.state = state.into();
            s.message = message.into();
            s.from_phone
        };
        if from_phone {
            let end = now() + 2.5;
            while now() < end && !self.lock().final_seen {
                sleep_ms(100);
                ui.tick();
            }
        }
        self.close();
    }

    pub fn close(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn start_relay(no_server: bool, ip: Option<&str>, bind: &str, port: u16, mode: Mode, ui: &mut Ui) -> Option<Relay> {
    if no_server {
        return None;
    }
    let ip = match ip.map(String::from).or_else(guess_lan_ip) {
        Some(ip) => ip,
        None => {
            ui.flash("No network address found, so the phone page is off.");
            return None;
        }
    };
    match Relay::start(mode, bind, port, &ip) {
        Ok(r) => Some(r),
        Err(e) => {
            ui.flash(&format!("Could not start the phone page on port {}: {}", port, e));
            None
        }
    }
}

// ---------------------------------------------------------------------------
// page building

fn title(mode: Mode) -> &'static str {
    match mode {
        Mode::Login => "Sign in to Claude Code",
        Mode::Token => "Use a setup token",
    }
}

/// (html, auto_refresh) for the phone page.
fn form(s: &Shared, error: Option<&str>) -> (String, bool) {
    let err = error.map(html_escape);
    match s.mode {
        Mode::Login => match &s.auth_url {
            None => (WAITING.to_string(), true),
            Some(url) => {
                let link = html_escape(url);
                let body = match &err {
                    Some(e) => LOGIN_FORM_ERR.replace("§LINK§", &link).replace("§ERR§", e),
                    None => LOGIN_FORM.replace("§LINK§", &link),
                };
                (body, false)
            }
        },
        Mode::Token => match &err {
            Some(e) => (TOKEN_FORM_ERR.replace("§ERR§", e), false),
            None => (TOKEN_FORM.to_string(), false),
        },
    }
}

fn render_page(s: &Shared, title: &str, body: &str, refresh: bool, nonce: &str) -> String {
    let parts: [(&str, String); 8] = [
        ("{{REFRESH}}", if refresh { "<meta http-equiv=\"refresh\" content=\"2\">".into() } else { String::new() }),
        ("{{NONCE}}", nonce.into()),
        ("{{CSS}}", CSS.into()),
        ("{{JS}}", JS.into()),
        ("{{MODE}}", s.mode.name().into()),
        ("{{HOST}}", s.host.clone()),
        ("{{TITLE}}", html_escape(title)),
        ("{{BODY}}", body.into()),
    ];
    let mut page = PAGE.to_string();
    for (k, v) in parts.iter() {
        page = page.replace(k, v);
    }
    page
}

// ---------------------------------------------------------------------------
// HTTP handling

struct Request {
    method: String,
    path: String,
    accept: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];
    let head_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if buf.len() > 65536 {
            return None;
        }
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_string();
    let target = first.next()?.to_string();
    let (mut length, mut accept) = (0usize, String::new());
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            if k == "content-length" {
                length = v.trim().parse().unwrap_or(0);
            } else if k == "accept" {
                accept = v.trim().to_string();
            }
        }
    }
    let length = length.min(8192);
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let n = stream.read(&mut tmp).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    body.truncate(length);
    // path without query string or fragment, like urlsplit().path
    let path = if let Some(rest) = target.strip_prefix("http://").or_else(|| target.strip_prefix("https://")) {
        rest.find('/').map(|i| rest[i..].to_string()).unwrap_or_default()
    } else {
        target
    };
    let path = path.split(['?', '#']).next().unwrap_or("").to_string();
    Some(Request { method, path, accept, body })
}

fn reason(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        410 => "Gone",
        _ => "Not Implemented",
    }
}

fn respond(stream: &mut TcpStream, code: u16, ctype: &str, body: &[u8], csp_nonce: Option<&str>) {
    let script = match csp_nonce {
        Some(n) => format!("'nonce-{}'", n),
        None => "'none'".into(),
    };
    let head = format!(
        "HTTP/1.0 {} {}\r\nServer: beamlogin\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\n\
         Referrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\nX-Content-Type-Options: nosniff\r\n\
         Permissions-Policy: camera=(), microphone=(), geolocation=()\r\n\
         Content-Security-Policy: default-src 'none'; style-src {s}; script-src {s}; connect-src 'self'; img-src data:; \
         form-action 'self'; base-uri 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n",
        code,
        reason(code),
        ctype,
        body.len(),
        s = script
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

fn send_html(stream: &mut TcpStream, s: &Shared, code: u16, title: &str, body: &str, refresh: bool) {
    let n = token_urlsafe(12);
    let page = render_page(s, title, body, refresh, &n);
    respond(stream, code, "text/html; charset=utf-8", page.as_bytes(), Some(&n));
}

fn send_json(stream: &mut TcpStream, code: u16, json: &str) {
    respond(stream, code, "application/json", json.as_bytes(), None);
}

fn handle(mut stream: TcpStream, shared: &Arc<Mutex<Shared>>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(15)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(15)));
    let req = match read_request(&mut stream) {
        Some(r) => r,
        None => return,
    };
    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
    if req.method != "GET" && req.method != "POST" {
        let body = format!("Unsupported method ({})", req.method);
        respond(&mut stream, 501, "text/plain; charset=utf-8", body.as_bytes(), None);
        return;
    }
    // route
    let base = format!("/{}", s.nonce);
    let route = if ct_eq(req.path.as_bytes(), base.as_bytes()) {
        "page"
    } else if ct_eq(req.path.as_bytes(), format!("{}/status", base).as_bytes()) {
        "status"
    } else {
        s.bad += 1;
        if s.bad >= s.max_bad {
            s.aborted = true;
        }
        send_html(
            &mut stream,
            &s,
            404,
            "Link not found",
            "<div class=\"card err-page\"><h2>This link isn't valid</h2><p>Scan the QR code on the terminal again.</p></div>",
            false,
        );
        return;
    };

    if req.method == "GET" {
        if route == "status" {
            if s.state == "done" || s.state == "failed" {
                s.final_seen = true;
            }
            let j = format!("{{\"state\": {}, \"message\": {}}}", json_str(&s.state), json_str(&s.message));
            return send_json(&mut stream, 200, &j);
        }
        if s.used {
            return send_html(&mut stream, &s, 410, "Link already used", CLOSED, false);
        }
        s.opened = true;
        let (body, refresh) = form(&s, None);
        let t = title(s.mode);
        return send_html(&mut stream, &s, 200, t, &body, refresh);
    }

    // POST
    let wants_json = req.accept.contains("application/json");
    if route != "page" || s.used {
        if wants_json {
            return send_json(
                &mut stream,
                410,
                "{\"ok\": false, \"error\": \"This link was already used. Check the terminal.\"}",
            );
        }
        return send_html(&mut stream, &s, 410, "Link already used", CLOSED, false);
    }
    let value = remove_ws(&form_value(&String::from_utf8_lossy(&req.body), "value"));
    let valid = match s.mode {
        Mode::Login => is_code(&value),
        Mode::Token => is_token(&value),
    };
    if !valid {
        let what = match s.mode {
            Mode::Login => "valid code",
            Mode::Token => "valid setup token (it starts with sk-ant-)",
        };
        let msg = format!("That doesn't look like a {}.", what);
        if wants_json {
            return send_json(&mut stream, 400, &format!("{{\"ok\": false, \"error\": {}}}", json_str(&msg)));
        }
        let (body, refresh) = form(&s, Some(&msg));
        let t = title(s.mode);
        return send_html(&mut stream, &s, 400, t, &body, refresh);
    }
    s.used = true;
    s.from_phone = true;
    s.state = "received".into();
    let _ = s.tx.send(value);
    if wants_json {
        return send_json(&mut stream, 200, "{\"ok\": true}");
    }
    let body = format!(
        "<div class=\"card result\" data-kind=\"ok\"><div class=\"badge\">{}</div><h2>Sent to the terminal</h2>\
         <p>Go back to the machine to finish. You can close this tab.</p></div>",
        ICON_CHECK
    );
    send_html(&mut stream, &s, 200, "Sent", &body, false);
}

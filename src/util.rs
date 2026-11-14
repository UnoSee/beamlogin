//! Small helpers shared by every platform: ANSI handling, validation, escaping, JSON,
//! processes and paths. Anything that differs between operating systems is in `sys`.

use crate::sys;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub type Env = Vec<(OsString, OsString)>;

/// Set by the platform's Ctrl+C handler.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// True once after Ctrl+C was pressed.
pub fn interrupted() -> bool {
    INTERRUPTED.swap(false, Ordering::SeqCst)
}

pub fn say(msg: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{}", msg);
    let _ = out.flush();
}

// ---------------------------------------------------------------------------
// ANSI escape sequences

/// Remove terminal escape sequences (CSI, OSC and two-byte ESC sequences).
pub fn strip_ansi(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < n {
        if c[i] == '\x1b' && i + 1 < n {
            let k = c[i + 1];
            if k == '[' {
                let mut j = i + 2;
                while j < n && ('0'..='?').contains(&c[j]) {
                    j += 1;
                }
                while j < n && (' '..='/').contains(&c[j]) {
                    j += 1;
                }
                if j < n && ('@'..='~').contains(&c[j]) {
                    i = j + 1;
                    continue;
                }
            } else if k == ']' {
                let mut j = i + 2;
                while j < n && c[j] != '\x07' && c[j] != '\x1b' {
                    j += 1;
                }
                if j < n && c[j] == '\x07' {
                    i = j + 1;
                    continue;
                }
                if j + 1 < n && c[j] == '\x1b' && c[j + 1] == '\\' {
                    i = j + 2;
                    continue;
                }
                i += 2; // a bare ESC ] is a two-byte sequence
                continue;
            } else if ('@'..='Z').contains(&k) || ('\\'..='_').contains(&k) {
                i += 2;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// Visible length of a string (escape sequences don't count).
pub fn vlen(s: &str) -> usize {
    strip_ansi(s).chars().count()
}

/// Remove escape sequences that keyboards send (arrows, function keys, bracketed paste).
pub fn strip_keyseq(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let n = c.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        if c[i] == '\x1b' {
            if i + 1 < n && c[i + 1] == '[' {
                let mut j = i + 2;
                while j < n && (c[j].is_ascii_digit() || c[j] == ';' || c[j] == '?') {
                    j += 1;
                }
                while j < n && (' '..='/').contains(&c[j]) {
                    j += 1;
                }
                if j < n && ('@'..='~').contains(&c[j]) {
                    i = j + 1;
                    continue;
                }
            }
            if i + 2 < n && c[i + 1] == 'O' && c[i + 2] != '\n' {
                i += 3;
                continue;
            }
            if i + 1 < n && c[i + 1] != '\n' {
                i += 2;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn is_url_char(ch: char) -> bool {
    !(ch.is_whitespace() || (ch as u32) <= 0x1f || "\"'<>`".contains(ch))
}

/// Find a complete OAuth sign-in URL in Claude Code's terminal output.
pub fn find_auth_url(raw: &str) -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();
    // OSC 8 hyperlinks: ESC ] 8 ; params ; URL
    let rc: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i + 4 < rc.len() {
        if rc[i] == '\x1b' && rc[i + 1] == ']' && rc[i + 2] == '8' && rc[i + 3] == ';' {
            let mut j = i + 4;
            while j < rc.len() && rc[j] != ';' && rc[j] != '\x07' && rc[j] != '\x1b' {
                j += 1;
            }
            if j < rc.len() && rc[j] == ';' {
                let rest: String = rc[j + 1..].iter().collect();
                if rest.starts_with("https://") {
                    let url: String = rest.chars().take_while(|&ch| ch != '\x07' && ch != '\x1b').collect();
                    if url.chars().count() > 8 {
                        candidates.push(url);
                    }
                }
            }
        }
        i += 1;
    }
    // Plain URLs that are fully printed (followed by at least one more character).
    let plain: Vec<char> = strip_ansi(raw).chars().collect();
    let pat: Vec<char> = "https://".chars().collect();
    let mut i = 0;
    while i + pat.len() <= plain.len() {
        if plain[i..i + pat.len()] == pat[..] {
            let mut j = i + pat.len();
            while j < plain.len() && is_url_char(plain[j]) {
                j += 1;
            }
            if j > i + pat.len() {
                if j < plain.len() {
                    candidates.push(plain[i..j].iter().collect());
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    for url in candidates {
        let url = url.trim_end_matches(|ch| ".,);]".contains(ch)).to_string();
        let low = url.to_lowercase();
        if low.contains("oauth") && low.contains("authorize") {
            return Some(url);
        }
    }
    None
}

/// "press" + whitespace + "enter", case-insensitive.
pub fn mentions_press_enter(text: &str) -> bool {
    let low: Vec<char> = text.to_lowercase().chars().collect();
    let p: Vec<char> = "press".chars().collect();
    let e: Vec<char> = "enter".chars().collect();
    let mut i = 0;
    while i + p.len() < low.len() {
        if low[i..i + p.len()] == p[..] {
            let mut j = i + p.len();
            let ws = j;
            while j < low.len() && low[j].is_whitespace() {
                j += 1;
            }
            if j > ws && j + e.len() <= low.len() && low[j..j + e.len()] == e[..] {
                return true;
            }
        }
        i += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// Validation and escaping

pub fn remove_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

pub fn is_code(s: &str) -> bool {
    let n = s.chars().count();
    (8..=2048).contains(&n) && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-#.~=+/".contains(c))
}

pub fn is_token(s: &str) -> bool {
    match s.strip_prefix("sk-ant-") {
        Some(rest) => {
            let n = rest.chars().count();
            (20..=400).contains(&n) && rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        }
        None => false,
    }
}

pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(c),
        }
    }
    out
}

pub fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match (hexval(b[i + 1]), hexval(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push(h * 16 + l);
                        i += 2;
                    }
                    _ => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// First value of `key` in an application/x-www-form-urlencoded body.
pub fn form_value(body: &str, key: &str) -> String {
    for pair in body.split(['&', ';']) {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = match pair.find('=') {
            Some(p) => (&pair[..p], &pair[p + 1..]),
            None => continue,
        };
        if url_decode(k) == key {
            let v = url_decode(v);
            if !v.is_empty() {
                return v;
            }
        }
    }
    String::new()
}

pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// URL-safe random token, like Python's secrets.token_urlsafe(n).
pub fn token_urlsafe(nbytes: usize) -> String {
    let buf = sys::random_bytes(nbytes);
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in buf.chunks(3) {
        let v = match chunk.len() {
            3 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8 | chunk[2] as u32,
            2 => (chunk[0] as u32) << 16 | (chunk[1] as u32) << 8,
            _ => (chunk[0] as u32) << 16,
        };
        let chars = chunk.len() + 1;
        for k in 0..chars {
            out.push(A[((v >> (18 - 6 * k)) & 63) as usize] as char);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tiny JSON reader (enough for `claude auth status`)

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(items) => items.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    pub fn get_str(&self, key: &str) -> Option<String> {
        match self.get(key) {
            Some(Json::Str(s)) => Some(s.clone()),
            _ => None,
        }
    }
}

pub fn parse_json(s: &str) -> Option<Json> {
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    let v = json_value(&b, &mut i)?;
    json_ws(&b, &mut i);
    if i == b.len() {
        Some(v)
    } else {
        None
    }
}

fn json_ws(b: &[char], i: &mut usize) {
    while *i < b.len() && b[*i].is_whitespace() {
        *i += 1;
    }
}

fn json_value(b: &[char], i: &mut usize) -> Option<Json> {
    json_ws(b, i);
    let c = *b.get(*i)?;
    match c {
        '{' => {
            *i += 1;
            let mut items = Vec::new();
            json_ws(b, i);
            if b.get(*i) == Some(&'}') {
                *i += 1;
                return Some(Json::Obj(items));
            }
            loop {
                json_ws(b, i);
                let k = match json_value(b, i)? {
                    Json::Str(s) => s,
                    _ => return None,
                };
                json_ws(b, i);
                if b.get(*i) != Some(&':') {
                    return None;
                }
                *i += 1;
                let v = json_value(b, i)?;
                items.push((k, v));
                json_ws(b, i);
                match b.get(*i) {
                    Some(',') => *i += 1,
                    Some('}') => {
                        *i += 1;
                        return Some(Json::Obj(items));
                    }
                    _ => return None,
                }
            }
        }
        '[' => {
            *i += 1;
            let mut items = Vec::new();
            json_ws(b, i);
            if b.get(*i) == Some(&']') {
                *i += 1;
                return Some(Json::Arr(items));
            }
            loop {
                items.push(json_value(b, i)?);
                json_ws(b, i);
                match b.get(*i) {
                    Some(',') => *i += 1,
                    Some(']') => {
                        *i += 1;
                        return Some(Json::Arr(items));
                    }
                    _ => return None,
                }
            }
        }
        '"' => {
            *i += 1;
            let mut s = String::new();
            loop {
                let c = *b.get(*i)?;
                *i += 1;
                match c {
                    '"' => return Some(Json::Str(s)),
                    '\\' => {
                        let e = *b.get(*i)?;
                        *i += 1;
                        match e {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            'r' => s.push('\r'),
                            'b' => s.push('\u{8}'),
                            'f' => s.push('\u{c}'),
                            'u' => {
                                let hex: String = b.get(*i..*i + 4)?.iter().collect();
                                *i += 4;
                                let mut code = u32::from_str_radix(&hex, 16).ok()?;
                                if (0xD800..0xDC00).contains(&code) && b.get(*i) == Some(&'\\') {
                                    let hex2: String = b.get(*i + 2..*i + 6)?.iter().collect();
                                    if let Ok(lo) = u32::from_str_radix(&hex2, 16) {
                                        *i += 6;
                                        code = 0x10000 + ((code - 0xD800) << 10) + (lo - 0xDC00);
                                    }
                                }
                                s.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                            }
                            other => s.push(other),
                        }
                    }
                    other => s.push(other),
                }
            }
        }
        't' | 'f' | 'n' => {
            for (word, val) in [("true", Json::Bool(true)), ("false", Json::Bool(false)), ("null", Json::Null)] {
                let w: Vec<char> = word.chars().collect();
                if b.get(*i..*i + w.len()) == Some(&w[..]) {
                    *i += w.len();
                    return Some(val);
                }
            }
            None
        }
        _ => {
            let start = *i;
            while *i < b.len() && (b[*i].is_ascii_digit() || "+-.eE".contains(b[*i])) {
                *i += 1;
            }
            let t: String = b[start..*i].iter().collect();
            t.parse().ok().map(Json::Num)
        }
    }
}

// ---------------------------------------------------------------------------
// Processes and paths

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[cfg(unix)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().unwrap_or_else(|| -status.signal().unwrap_or(1))
}

#[cfg(windows)]
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(1)
}

/// Run a program with captured output and a timeout. None if it couldn't start or timed out.
pub fn run_capture(prog: &str, args: &[&str], env: Option<&Env>, timeout: Duration) -> Option<Output> {
    let mut cmd = Command::new(prog);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(env) = env {
        cmd.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    let mut child = cmd.spawn().ok()?;
    let mut so = child.stdout.take()?;
    let mut se = child.stderr.take()?;
    let t1 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = so.read_to_end(&mut v);
        v
    });
    let t2 = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = se.read_to_end(&mut v);
        v
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    };
    let stdout = String::from_utf8_lossy(&t1.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&t2.join().unwrap_or_default()).into_owned();
    Some(Output { code: exit_code(status), stdout, stderr })
}

/// Like shutil.which: find an executable on PATH (with PATHEXT extensions on Windows).
pub fn which(name: &str) -> Option<String> {
    if sys::is_path(name) {
        return sys::exe_names(name).into_iter().find(|c| sys::is_executable(c));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        for n in sys::exe_names(name) {
            let cand = dir.join(&n);
            let cand = cand.to_string_lossy();
            if sys::is_executable(&cand) {
                return Some(cand.into_owned());
            }
        }
    }
    None
}

/// Expand a leading ~ to the home folder.
pub fn expanduser(p: &str) -> String {
    let h = sys::home();
    if p == "~" {
        h
    } else if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        let sep = std::path::MAIN_SEPARATOR;
        format!("{}{}{}", h.trim_end_matches(['/', '\\']), sep, rest)
    } else {
        p.to_string()
    }
}

/// The folder part of a path ("" if there is none).
pub fn dirname(p: &str) -> String {
    match p.rfind(|c| c == '/' || (cfg!(windows) && c == '\\')) {
        Some(0) => p[..1].to_string(),
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

/// Terminal size like Python's shutil.get_terminal_size(fallback): COLUMNS and LINES win,
/// then the real console size, then the fallback.
pub fn term_size(fallback: (usize, usize)) -> (usize, usize) {
    let env_num = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let (mut cols, mut rows) = (env_num("COLUMNS"), env_num("LINES"));
    if cols == 0 || rows == 0 {
        let real = sys::console_size();
        if cols == 0 {
            cols = real.map(|r| r.0).filter(|&c| c > 0).unwrap_or(fallback.0);
        }
        if rows == 0 {
            rows = real.map(|r| r.1).filter(|&r| r > 0).unwrap_or(fallback.1);
        }
    }
    (cols, rows)
}

pub fn env_now() -> Env {
    std::env::vars_os().collect()
}

pub fn env_set(env: &mut Env, key: &str, val: &str) {
    // Windows environment names ignore case; Unix ones don't.
    env.retain(|(k, _)| {
        let k = k.to_string_lossy();
        !(k == key || (cfg!(windows) && k.eq_ignore_ascii_case(key)))
    });
    env.push((key.into(), val.into()));
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

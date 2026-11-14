//! Dependency checks: is this machine ready to run Beamlogin?

use crate::relay::guess_lan_ip;
use crate::sys;
use crate::ui::Ui;
use crate::util::*;
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The official one-line installer for this operating system.
pub const CLAUDE_INSTALL_CMD: &str = sys::INSTALL_CMD;

#[derive(Clone, Copy, PartialEq)]
pub enum Level {
    Ok,
    Warn,
    Bad,
}

/// One line of the dependency report.
#[derive(Clone)]
pub struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    /// Lines starting with "$ " are commands (shown in bold); others are advice.
    pub fix: Vec<String>,
}

fn check(name: &str, level: Level, detail: &str, fix: Vec<String>) -> Check {
    Check { name: name.into(), level, detail: detail.into(), fix }
}

/// (path or None, on_path)
pub fn locate_claude(name: &str) -> (Option<String>, bool) {
    if sys::is_path(name) || name.starts_with('~') {
        let path = sys::abspath(&expanduser(name));
        return (sys::exe_names(&path).into_iter().find(|c| sys::is_executable(c)), true);
    }
    if let Some(found) = which(name) {
        return (Some(found), true);
    }
    if name == "claude" {
        for base in sys::claude_fallbacks() {
            if let Some(found) = sys::exe_names(&base).into_iter().find(|c| sys::is_executable(c)) {
                return (Some(found), false);
            }
        }
    }
    (None, false)
}

pub fn claude_version(path: &str) -> Option<String> {
    let p = run_capture(path, &["--version"], None, Duration::from_secs(30))?;
    let text = if !p.stdout.is_empty() { p.stdout } else { p.stderr };
    let first = text.trim().lines().next().map(|l| l.trim().to_string());
    if p.code == 0 {
        first
    } else {
        None
    }
}

/// A package-manager command line that installs pkg, or None.
pub fn install_hint(pkg: &str) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let root = sys::is_root();
    let sudo = if root { "" } else { "sudo " };
    let opts = [
        ("apt-get", format!("apt-get update && {}apt-get install -y {}", sudo, pkg)),
        ("dnf", format!("dnf install -y {}", pkg)),
        ("pacman", format!("pacman -S --noconfirm {}", pkg)),
        ("zypper", format!("zypper install -y {}", pkg)),
        ("apk", format!("apk add {}", pkg)),
        ("brew", format!("brew install {}", pkg)),
    ];
    for (tool, cmd) in opts {
        if which(tool).is_some() {
            return Some(format!("{}{}", if tool != "brew" { sudo } else { "" }, cmd));
        }
    }
    None
}

fn cmd_or(command: Option<String>, fallback: &str) -> String {
    match command {
        Some(c) => format!("$ {}", c),
        None => fallback.to_string(),
    }
}

/// The system's short description of an error, like Python's strerror ("connection refused").
fn strerror(e: &std::io::Error) -> String {
    let s = e.to_string();
    // Rust adds " (os error N)"; drop it.
    let s = match s.rfind(" (os error") {
        Some(i) => &s[..i],
        None => &s,
    };
    s.trim_end_matches('.').to_lowercase()
}

fn reachable(host: &str) -> (bool, String) {
    let addrs: Vec<_> = match (host, 443u16).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(_) => return (false, "name lookup failed (DNS)".into()),
    };
    if addrs.is_empty() {
        return (false, "name lookup failed (DNS)".into());
    }
    let mut last = String::from("unreachable");
    for a in addrs {
        match TcpStream::connect_timeout(&a, Duration::from_secs(4)) {
            Ok(_) => return (true, String::new()),
            Err(e) => {
                last = if e.kind() == std::io::ErrorKind::TimedOut {
                    "timed out".into()
                } else {
                    strerror(&e)
                };
            }
        }
    }
    (false, last)
}

pub fn auth_status(claude: &str, env: Option<&Env>) -> (bool, Option<Json>) {
    match run_capture(claude, &["auth", "status"], env, Duration::from_secs(30)) {
        Some(p) => (p.code == 0, parse_json(&p.stdout)),
        None => (false, None),
    }
}

pub fn info_str(info: &Option<Json>, key: &str) -> Option<String> {
    info.as_ref().and_then(|j| j.get_str(key))
}

pub struct CheckArgs {
    pub claude: String,
    pub no_server: bool,
    pub ip: Option<String>,
}

/// Inspect this machine. Returns (checks, claude_path or None).
pub fn run_checks(args: &CheckArgs, need_claude: bool, need_network: bool, deep: bool) -> (Vec<Check>, Option<String>) {
    let mut checks = Vec::new();
    checks.push(check("System", Level::Ok, &sys::system_name(), vec![]));

    let mut claude = None;
    if need_claude {
        let (found, on_path) = locate_claude(&args.claude);
        // The installer needs curl on Linux and macOS. On Windows it uses PowerShell.
        let curl = if cfg!(windows) { Some(String::new()) } else { which("curl") };
        if let Some(path) = &found {
            let ver = claude_version(path);
            let h = sys::home();
            let where_ = if !h.is_empty() && path.contains(&h) { path.replacen(&h, "~", 1) } else { path.clone() };
            match ver {
                None => checks.push(check(
                    "Claude Code",
                    Level::Warn,
                    &format!("found at {}, but it didn't report a version", where_),
                    vec!["Try running  claude --version  to see what's wrong.".into()],
                )),
                Some(v) if !on_path => checks.push(check(
                    "Claude Code",
                    Level::Warn,
                    &format!("{}, at {} (not on your PATH)", v, where_),
                    vec![format!("$ {}", sys::path_fix(&dirname(&where_)))],
                )),
                Some(v) => checks.push(check("Claude Code", Level::Ok, &v, vec![])),
            }
        } else {
            let what = if args.claude == "claude" { "isn't installed".to_string() } else { format!("not found at {}", args.claude) };
            let mut fix = Vec::new();
            if curl.is_none() {
                fix.push(cmd_or(install_hint("curl ca-certificates"), "Install curl."));
            }
            fix.push(format!("$ {}", CLAUDE_INSTALL_CMD));
            checks.push(check("Claude Code", Level::Bad, &what, fix));
        }
        if (deep || found.is_none()) && !cfg!(windows) {
            checks.push(match &curl {
                Some(c) => check("curl", Level::Ok, c, vec![]),
                None => check(
                    "curl",
                    Level::Warn,
                    "not installed (needed to install Claude Code)",
                    vec![cmd_or(install_hint("curl ca-certificates"), "Install curl.")],
                ),
            });
        }
        claude = found;
    }

    if need_network && !args.no_server {
        match args.ip.clone().or_else(guess_lan_ip) {
            Some(ip) => checks.push(check("Phone page", Level::Ok, &format!("phones on your network can reach {}", ip), vec![])),
            None => checks.push(check(
                "Phone page",
                Level::Warn,
                "no network address, so you'll type the code by hand",
                vec!["Connect this machine to the same network as your phone.".into()],
            )),
        }
    }

    if deep {
        for host in ["claude.ai", "api.anthropic.com"] {
            let (ok, why) = reachable(host);
            checks.push(if ok {
                check(&format!("Reach {}", host), Level::Ok, "ok", vec![])
            } else {
                check(
                    &format!("Reach {}", host),
                    Level::Warn,
                    &why,
                    vec!["Check the network connection, DNS and any proxy or firewall.".into()],
                )
            });
        }
        let (cols, rows) = term_size((0, 0));
        if cols > 0 && rows > 0 {
            let small = cols < 40 || rows < 24;
            checks.push(check(
                "Terminal",
                if small { Level::Warn } else { Level::Ok },
                &format!("{} x {}", cols, rows),
                if !small {
                    vec![]
                } else if cfg!(windows) {
                    vec!["Enlarge the window or use a smaller font.".into()]
                } else {
                    vec!["Enlarge the terminal or use a smaller console font (setfont).".into()]
                },
            ));
        }
        if let Some(c) = &claude {
            let (ok, info) = auth_status(c, None);
            if ok {
                let m = info_str(&info, "authMethod").unwrap_or_else(|| "unknown".into());
                checks.push(check("Signed in", Level::Ok, &format!("yes ({})", m), vec![]));
            } else {
                checks.push(check("Signed in", Level::Warn, "not yet", vec!["Run  beamlogin  to sign in with your phone.".into()]));
            }
        }
    }
    (checks, claude)
}

fn ljust(s: &str, w: usize) -> String {
    let n = s.chars().count();
    format!("{}{}", s, " ".repeat(w.saturating_sub(n)))
}

pub fn check_lines(ui: &Ui, checks: &[Check]) -> Vec<String> {
    let width = checks.iter().map(|c| c.name.chars().count()).max().unwrap_or(0) + 3;
    let mut out = Vec::new();
    for c in checks {
        let icon = match c.level {
            Level::Ok => ui.green(ui.ok_sym),
            Level::Bad => ui.red(ui.err_sym),
            Level::Warn => ui.accent("!"),
        };
        out.push(format!("  {} {}{}", icon, ljust(&c.name, width), c.detail));
        if c.level != Level::Ok {
            for f in &c.fix {
                out.push(format!("      {}", if let Some(cmd) = f.strip_prefix("$ ") { ui.bold(cmd) } else { ui.dim(f) }));
            }
        }
    }
    out
}

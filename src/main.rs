//! Beamlogin - sign in to Claude Code on a headless machine using your phone.
//!
//! Modes
//!   login   Run `claude auth login`, show its sign-in link as a QR code, and
//!           take the returned code back from your phone over the local network.
//!   token   Receive a long-lived token (made with `claude setup-token`) from your
//!           phone and start Claude Code with CLAUDE_CODE_OAUTH_TOKEN set.
//!   logout  Run `claude auth logout` and remind you to revoke the sign-in.
//!   doctor  Check this machine.
//!   qr      Print any text as a terminal QR code (handy for testing).

mod checks;
mod qr;
mod relay;
mod sys;
mod ui;
mod util;

use checks::*;
use relay::{start_relay, Mode, Relay};
use std::time::Duration;
use sys::{Pty, PtyRead};
use ui::{Kind, Stop, Ui};
use util::*;

const VERSION: &str = env!("CARGO_PKG_VERSION");

// ---------------------------------------------------------------------------
// command line

#[derive(Clone)]
struct Args {
    cmd: String,
    claude: String,
    qr_style: String,
    plain: bool,
    ip: Option<String>,
    bind: String,
    port: u16,
    no_server: bool,
    timeout: u64,
    no_launch: bool,
    install_claude: bool,
    stdin_commands: bool,
    force: bool,
    keep_browser: bool,
    verbose: bool,
    claude_args: Vec<String>,
    hidden_input: bool,
    no_verify: bool,
    save: bool,
    text: Option<String>,
    ecl: char,
}

impl Args {
    fn check_args(&self) -> CheckArgs {
        CheckArgs { claude: self.claude.clone(), no_server: self.no_server, ip: self.ip.clone() }
    }
}

const COMMON_HELP: &str = "  --claude CLAUDE       Claude Code command or path (default: claude)
  --qr-style {half,ascii}
                        half = compact block characters (default); ascii = wider colour blocks
  --plain               plain line output (no screen redraws)
";
const NET_HELP: &str = "  --ip IP               address the phone should use (default: auto-detect)
  --bind BIND           address to listen on (default: 0.0.0.0)
  --port PORT           port for the phone page (default: random)
  --no-server           don't run the phone page; enter by hand
  --timeout TIMEOUT     seconds to wait (default: 600)
  --no-launch           don't start Claude Code afterwards
  --install-claude      if Claude Code is missing, install it with the official installer without asking
";

fn top_help() -> String {
    "usage: beamlogin [-h] [--version] {login,token,logout,doctor,qr} ...

Beamlogin - sign in to Claude Code on a headless machine using your phone.

commands:
  login     sign in with your phone (default)
  token     use a setup token sent from your phone
  logout    sign out and show how to revoke
  doctor    check this machine: Claude Code, network
  qr        print text as a QR code

options:
  -h, --help  show this help message and exit
  --version   show program's version number and exit
"
    .to_string()
}

fn cmd_help(cmd: &str) -> String {
    let mut s = format!("usage: beamlogin {} [options]\n\noptions:\n  -h, --help            show this help message and exit\n", cmd);
    s.push_str(COMMON_HELP);
    match cmd {
        "login" => {
            s.push_str(NET_HELP);
            s.push_str(
                "  --force               sign in even if already signed in
  --keep-browser        let Claude Code try to open a local browser
  --verbose             echo Claude Code's raw output to stderr
  --claude-args ...     extra arguments for `claude auth login`, e.g. --claude-args --console
",
            );
        }
        "token" => {
            s.push_str(NET_HELP);
            s.push_str(
                "  --hidden-input        paste the token here instead of the phone page
  --no-verify           skip the test request
  --save                also save the token in a memory-only file for other shells
",
            );
        }
        "qr" => {
            s = format!("usage: beamlogin qr [options] TEXT\n\noptions:\n  -h, --help            show this help message and exit\n{}  --ecl {{L,M,Q,H}}\n", COMMON_HELP);
        }
        _ => {}
    }
    s
}

fn usage_error(cmd: &str, msg: &str) -> ! {
    let prog = if cmd.is_empty() { "beamlogin".to_string() } else { format!("beamlogin {}", cmd) };
    eprintln!("usage: {} [-h] [options]\n{}: error: {}", prog, prog, msg);
    std::process::exit(2);
}

fn parse_args(argv: Vec<String>) -> Args {
    let mut argv = argv;
    if argv.is_empty() || (argv[0].starts_with('-') && !["-h", "--help", "--version"].contains(&argv[0].as_str())) {
        argv.insert(0, "login".into());
    }
    match argv[0].as_str() {
        "-h" | "--help" => {
            print!("{}", top_help());
            std::process::exit(0);
        }
        "--version" => {
            println!("beamlogin {}", VERSION);
            std::process::exit(0);
        }
        _ => {}
    }
    let cmd = argv[0].clone();
    if !["login", "token", "logout", "doctor", "qr"].contains(&cmd.as_str()) {
        usage_error(
            "",
            &format!("argument cmd: invalid choice: '{}' (choose from 'login', 'token', 'logout', 'doctor', 'qr')", cmd),
        );
    }
    let mut a = Args {
        cmd: cmd.clone(),
        claude: "claude".into(),
        qr_style: "half".into(),
        plain: false,
        ip: None,
        bind: "0.0.0.0".into(),
        port: 0,
        no_server: false,
        timeout: 600,
        no_launch: false,
        install_claude: false,
        stdin_commands: false,
        force: false,
        keep_browser: false,
        verbose: false,
        claude_args: vec![],
        hidden_input: false,
        no_verify: false,
        save: false,
        text: None,
        ecl: 'M',
    };
    let net = cmd == "login" || cmd == "token";
    let mut i = 1;
    while i < argv.len() {
        let raw = argv[i].clone();
        i += 1;
        if raw == "--" {
            // everything after -- is positional, like argparse
            for rest in argv[i..].iter() {
                if cmd == "qr" && a.text.is_none() {
                    a.text = Some(rest.clone());
                } else {
                    usage_error(&cmd, &format!("unrecognized arguments: {}", rest));
                }
            }
            break;
        }
        let (name, inline) = match raw.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n.to_string(), Some(v.to_string())),
            _ => (raw.clone(), None),
        };
        let value = |i: &mut usize| -> String {
            if let Some(v) = &inline {
                return v.clone();
            }
            if *i < argv.len() {
                *i += 1;
                return argv[*i - 1].clone();
            }
            usage_error(&cmd, &format!("argument {}: expected one argument", name));
        };
        let flag = |ok: bool| {
            if !ok {
                usage_error(&cmd, &format!("unrecognized arguments: {}", raw));
            }
        };
        match name.as_str() {
            "-h" | "--help" => {
                print!("{}", cmd_help(&cmd));
                std::process::exit(0);
            }
            "--claude" => a.claude = value(&mut i),
            "--qr-style" => {
                let v = value(&mut i);
                if v != "half" && v != "ascii" {
                    usage_error(&cmd, &format!("argument --qr-style: invalid choice: '{}' (choose from 'half', 'ascii')", v));
                }
                a.qr_style = v;
            }
            "--plain" => a.plain = true,
            "--ip" => {
                flag(net || cmd == "doctor");
                a.ip = Some(value(&mut i));
            }
            "--bind" => {
                flag(net);
                a.bind = value(&mut i);
            }
            "--port" => {
                flag(net);
                let v = value(&mut i);
                a.port = v.parse().unwrap_or_else(|_| usage_error(&cmd, &format!("argument --port: invalid int value: '{}'", v)));
            }
            "--timeout" => {
                flag(net);
                let v = value(&mut i);
                a.timeout =
                    v.parse().unwrap_or_else(|_| usage_error(&cmd, &format!("argument --timeout: invalid int value: '{}'", v)));
            }
            "--no-server" => {
                flag(net);
                a.no_server = true;
            }
            "--no-launch" => {
                flag(net);
                a.no_launch = true;
            }
            "--install-claude" => {
                flag(net);
                a.install_claude = true;
            }
            "--stdin-commands" => {
                flag(net);
                a.stdin_commands = true;
            }
            "--force" => {
                flag(cmd == "login");
                a.force = true;
            }
            "--keep-browser" => {
                flag(cmd == "login");
                a.keep_browser = true;
            }
            "--verbose" => {
                flag(cmd == "login");
                a.verbose = true;
            }
            "--claude-args" => {
                flag(cmd == "login");
                if let Some(v) = inline {
                    a.claude_args.push(v);
                }
                a.claude_args.extend(argv[i..].iter().cloned());
                i = argv.len();
            }
            "--hidden-input" => {
                flag(cmd == "token");
                a.hidden_input = true;
            }
            "--no-verify" => {
                flag(cmd == "token");
                a.no_verify = true;
            }
            "--save" => {
                flag(cmd == "token");
                if sys::runtime_dir().is_none() {
                    usage_error(&cmd, "--save works on Linux and macOS only");
                }
                a.save = true;
            }
            "--ecl" => {
                flag(cmd == "qr");
                let v = value(&mut i);
                if !["L", "M", "Q", "H"].contains(&v.as_str()) {
                    usage_error(&cmd, &format!("argument --ecl: invalid choice: '{}' (choose from 'L', 'M', 'Q', 'H')", v));
                }
                a.ecl = v.chars().next().unwrap_or('M');
            }
            _ if cmd == "qr" && a.text.is_none() && !raw.starts_with('-') => a.text = Some(raw.clone()),
            _ => usage_error(&cmd, &format!("unrecognized arguments: {}", raw)),
        }
    }
    if cmd == "qr" && a.text.is_none() {
        usage_error(&cmd, "the following arguments are required: text");
    }
    a
}

// ---------------------------------------------------------------------------
// shared steps

fn install_claude(ui: &mut Ui) -> bool {
    ui.close();
    if ui.tty {
        print!("\x1b[2J\x1b[H");
    }
    say(&format!("\n  Installing Claude Code:  {}\n", CLAUDE_INSTALL_CMD));
    let ok = sys::run_installer();
    say("");
    ok
}

fn ask_yes_no(ui: &mut Ui, question: &str) -> bool {
    ui.hint = "y  yes    n  no".into();
    ui.status(question, Kind::Info, None);
    ui.input_start("", false);
    let answer = 'outer: loop {
        if interrupted() {
            break false;
        }
        if sys::stdin_wait(500) {
            match ui.read_lines() {
                Err(_) => break false,
                Ok(lines) => {
                    for line in lines {
                        let word = line.trim().to_lowercase();
                        if word == "y" || word == "yes" {
                            break 'outer true;
                        }
                        if word == "n" || word == "no" || word == "q" {
                            break 'outer false;
                        }
                        ui.flash("Type y or n, then Enter.");
                    }
                }
            }
        }
        ui.tick();
    };
    ui.input_stop();
    ui.clear_footer();
    answer
}

/// Check dependencies before a command. Returns the Claude Code path, or None to stop.
fn preflight(ui: &mut Ui, args: &Args, subtitle: &str, need_network: bool) -> Option<String> {
    let (checks, claude) = run_checks(&args.check_args(), true, need_network, false);
    let blocking: Vec<&Check> = checks.iter().filter(|c| c.level == Level::Bad).collect();
    if blocking.is_empty() {
        return claude;
    }
    let missing_claude = blocking.len() == 1 && blocking[0].name == "Claude Code";
    let can_install = missing_claude && args.claude == "claude" && sys::install_runnable();
    let title = if missing_claude { "Claude Code isn't installed" } else { "This machine needs a few things first" };
    let mut lines = vec![format!("  {} {}", ui.red(ui.err_sym), ui.bold(title)), String::new()];
    lines.extend(check_lines(ui, &checks));
    lines.push(String::new());
    let go = if can_install && args.install_claude {
        true
    } else if can_install && ui.keys {
        let mut l = lines.clone();
        l.push("  Beamlogin can run the official installer for you now.".into());
        ui.screen(subtitle, &l);
        ask_yes_no(ui, "Install Claude Code now?")
    } else {
        if can_install {
            lines.push(format!("  {}", ui.dim("Or run beamlogin with --install-claude to install it automatically.")));
        }
        ui.screen(subtitle, &lines);
        ui.close();
        return None;
    };
    if !go {
        ui.clear_footer();
        let l = vec![
            "  Claude Code wasn't installed. To install it yourself, run:".to_string(),
            String::new(),
            format!("      {}", ui.bold(CLAUDE_INSTALL_CMD)),
            String::new(),
            "  Then start beamlogin again, or run  beamlogin --install-claude.".to_string(),
            String::new(),
        ];
        ui.screen(subtitle, &l);
        ui.close();
        return None;
    }
    if !install_claude(ui) {
        say("  The installer failed. See its output above.");
        return None;
    }
    let (claude, on_path) = locate_claude("claude");
    let claude = match claude {
        Some(c) => c,
        None => {
            say("  The installer finished, but Claude Code still can't be found.");
            return None;
        }
    };
    if !on_path {
        say(&format!("  Installed at {}. To use it in your own shell, add it to PATH:", claude));
        say(&format!("    {}\n", sys::path_fix(&dirname(&claude))));
    }
    sleep_ms(1500);
    Some(claude)
}

fn revoke_lines(ui: &Ui) -> Vec<String> {
    let a = ui.arrow;
    vec![
        format!("  {}", ui.bold("When you're done on this machine")),
        String::new(),
        format!("  {}  Run {}", ui.accent(&ui.bold("1")), ui.bold("beamlogin logout")),
        format!("  {}  On your phone, revoke the sign-in:", ui.accent(&ui.bold("2"))),
        format!("     {}", ui.dim(&format!("claude.ai {a} Settings {a} Account {a} Where you're logged in"))),
        format!("     {}", ui.dim(&format!("{a} Log out on the matching \"Claude Code\" row"))),
    ]
}

fn output_box(ui: &Ui, text: &str, limit: usize) -> Vec<String> {
    let clean = strip_ansi(text);
    let all: Vec<&str> = clean.trim().lines().collect();
    let mut lines: Vec<&str> = all[all.len().saturating_sub(limit)..].to_vec();
    if lines.is_empty() {
        lines.push("(no output)");
    }
    let bar = if ui.console { "|" } else { "│" };
    lines.iter().map(|l| format!("  {}", ui.dim(&format!("{} {}", bar, l)))).collect()
}

/// Optionally wait for Enter, then start Claude Code (on Unix it replaces this process).
fn finish(ui: &mut Ui, claude: &str, args: &Args, env: Option<Env>) -> i32 {
    if args.no_launch {
        ui.close();
        return 0;
    }
    let prompt = format!("Press Enter to start Claude Code  {}", ui.dim("(Ctrl+C to exit)"));
    if !ui.wait_enter(&prompt) {
        ui.close();
        say("");
        return 0;
    }
    ui.close();
    if ui.tty {
        print!("\x1b[2J\x1b[H");
    } else {
        say("  Starting Claude Code...");
    }
    use std::io::Write;
    let _ = std::io::stdout().flush();
    sys::run_claude(claude, env)
}

fn trim_tail(s: &mut String, max: usize) {
    if s.len() > max {
        let mut cut = s.len() - max;
        while !s.is_char_boundary(cut) {
            cut += 1;
        }
        s.drain(..cut);
    }
}

// ---------------------------------------------------------------------------
// login mode

#[derive(Clone, Copy, PartialEq)]
enum View {
    Page,
    Direct,
}

#[derive(PartialEq)]
enum Outcome {
    Timeout,
    Abuse,
    Cancelled,
}

fn draw_login(ui: &mut Ui, sub: &str, relay: Option<&Relay>, view: View, auth_url: &str) {
    match (view, relay) {
        (View::Page, Some(r)) => {
            let steps = [
                ("1", vec!["Scan this code with your phone".to_string(), ui.dim("(same network as this machine)")]),
                ("2", vec![format!("Tap {} and sign in", ui.bold("Sign in to Claude"))]),
                (
                    "3",
                    vec!["Copy the code Claude shows, paste it".to_string(), format!("on the first tab and tap {}", ui.bold("Send to terminal"))],
                ),
            ];
            let notes = [ui.dim(&format!("Page: {}", r.url))];
            ui.panel(sub, &r.url, &steps, &notes);
            ui.hint = "u  use the sign-in link instead    q  quit    or paste the code here + Enter".into();
        }
        _ => {
            let steps = [
                ("1", vec!["Scan to sign in on any device".to_string(), ui.dim("(no shared network needed)")]),
                ("2", vec!["Type or paste the code Claude shows".to_string(), "into this terminal and press Enter".to_string()]),
            ];
            ui.panel(sub, auth_url, &steps, &[]);
            ui.hint = format!("{}q  quit", if relay.is_some() { "u  back to the phone page    " } else { "" });
        }
    }
    ui.refresh();
}

fn send_code(ui: &mut Ui, pty: &Pty, code: &str) {
    pty.write(code.as_bytes());
    sleep_ms(300);
    pty.write(b"\r");
    ui.input_stop();
    ui.hint.clear();
    let s = format!("Code received {} finishing sign-in", ui.dim("·"));
    ui.status(&s, Kind::Wait, None);
}

fn report(ui: &mut Ui, relay: &mut Option<Relay>, state: &str, message: &str, show_status: bool) {
    if let Some(r) = relay.as_mut() {
        if show_status {
            ui.status("Updating your phone", Kind::Wait, None);
        }
        r.finish(state, message, ui);
    }
}

fn cmd_login(args: &Args) -> i32 {
    let mut ui = Ui::new(args.plain, &args.qr_style);
    let sub = "sign in to Claude Code";
    let claude = match preflight(&mut ui, args, sub, true) {
        Some(c) => c,
        None => return 1,
    };
    let (logged_in, info) = auth_status(&claude, None);
    if logged_in && !args.force {
        let m = info_str(&info, "authMethod").unwrap_or_else(|| "unknown".into());
        let l = vec![
            format!("  {} Claude Code is already signed in on this machine{}", ui.green(ui.ok_sym), ui.dim(&format!("  ({})", m))),
            String::new(),
            format!("  {}", ui.dim("Use --force to sign in again, or run  beamlogin logout  first.")),
            String::new(),
        ];
        ui.screen(sub, &l);
        return finish(&mut ui, &claude, args, None);
    }

    let mut env = env_now();
    if !args.keep_browser {
        sys::set_browser_env(&mut env); // never try to open a local text browser
    }
    let mut child_args = vec!["auth".to_string(), "login".to_string()];
    child_args.extend(args.claude_args.iter().cloned());
    // A very wide terminal, so the sign-in link is printed on one line.
    let mut pty = match Pty::spawn(&claude, &child_args, &env, 1000, 60) {
        Ok(c) => c,
        Err(e) => {
            ui.screen(sub, &[format!("  {} Could not start Claude Code: {}", ui.red(ui.err_sym), e), String::new()]);
            ui.close();
            return 1;
        }
    };

    ui.screen(sub, &[]);
    let mut relay = start_relay(args.no_server, args.ip.as_deref(), &args.bind, args.port, Mode::Login, &mut ui);
    let started = now();
    let deadline = started + args.timeout as f64;
    ui.status("Starting Claude Code sign-in", Kind::Wait, None);
    let mut stdin_ok = ui.keys || args.stdin_commands;
    let (mut raw, mut after) = (String::new(), String::new());
    let mut auth_url: Option<String> = None;
    let mut view: Option<View> = None;
    let (mut sent, mut enter_sent, mut hinted) = (false, false, false);
    let mut outcome: Option<Outcome> = None;

    'main: loop {
        if now() > deadline {
            outcome = Some(Outcome::Timeout);
            break;
        }
        if relay.as_ref().map(|r| r.aborted()).unwrap_or(false) {
            outcome = Some(Outcome::Abuse);
            break;
        }
        if interrupted() {
            outcome = Some(Outcome::Cancelled);
            break;
        }
        let watch_stdin = stdin_ok && auth_url.is_some() && !sent;
        let output = pty.read(if watch_stdin { 100 } else { 200 });
        let got_output = matches!(output, PtyRead::Data(_));
        let stdin_ready = watch_stdin && sys::stdin_wait(if got_output { 0 } else { 100 });
        ui.tick();
        if let Some(v) = view {
            if ui.resized() {
                draw_login(&mut ui, sub, relay.as_ref(), v, auth_url.as_deref().unwrap_or(""));
            }
        }
        match output {
            PtyRead::Closed => break, // child exited
            PtyRead::Idle => {}
            PtyRead::Data(bytes) => {
            let chunk = String::from_utf8_lossy(&bytes).into_owned();
            raw.push_str(&chunk);
            trim_tail(&mut raw, 200_000);
            if args.verbose {
                eprint!("{}", strip_ansi(&chunk));
            }
            if sent {
                after.push_str(&chunk);
                if !enter_sent && mentions_press_enter(&strip_ansi(&after)) {
                    pty.write(b"\r");
                    enter_sent = true;
                }
            } else if auth_url.is_none() {
                auth_url = find_auth_url(&raw);
                if let Some(u) = &auth_url {
                    if let Some(rl) = &relay {
                        rl.set_auth_url(u);
                    }
                    let v = if relay.is_some() { View::Page } else { View::Direct };
                    view = Some(v);
                    draw_login(&mut ui, sub, relay.as_ref(), v, u);
                    let msg = if relay.is_some() { "Waiting for your phone" } else { "Waiting for the code" };
                    ui.status(msg, Kind::Wait, Some(deadline));
                    ui.input_start("", false);
                }
            }
            }
        }
        if !sent && auth_url.is_none() && !hinted && now() - started > 25.0 {
            hinted = true;
            let mut l = vec!["  Still waiting for a sign-in link. Claude Code shows:".to_string(), String::new()];
            l.extend(output_box(&ui, &raw, 15));
            ui.screen(sub, &l);
        }
        if stdin_ready {
            let lines = match ui.read_lines() {
                Ok(l) => l,
                Err(Stop::Eof) => {
                    stdin_ok = false;
                    vec![]
                }
                Err(Stop::Cancelled) => {
                    outcome = Some(Outcome::Cancelled);
                    break;
                }
            };
            for line in lines {
                let word = remove_ws(&line);
                if word.eq_ignore_ascii_case("q") {
                    outcome = Some(Outcome::Cancelled);
                    break 'main;
                }
                if word.eq_ignore_ascii_case("u") {
                    if relay.is_some() {
                        let v = if view == Some(View::Page) { View::Direct } else { View::Page };
                        view = Some(v);
                        draw_login(&mut ui, sub, relay.as_ref(), v, auth_url.as_deref().unwrap_or(""));
                    }
                } else if is_code(&word) {
                    if let Some(rl) = &relay {
                        rl.mark_used();
                    }
                    send_code(&mut ui, &pty, &word);
                    sent = true;
                    after.clear();
                    break;
                } else if !word.is_empty() {
                    ui.flash("That doesn't look like a sign-in code. Try again.");
                }
            }
        }
        if !sent {
            if let Some(rl) = &relay {
                if rl.opened() && ui.status_text.as_deref() == Some("Waiting for your phone") {
                    let s = format!("Phone connected {} waiting for the code", ui.dim("·"));
                    ui.status(&s, Kind::Wait, Some(deadline));
                }
                if let Some(v) = rl.try_value() {
                    send_code(&mut ui, &pty, &v);
                    sent = true;
                    after.clear();
                }
            }
        }
    }

    // reap the child
    ui.input_stop();
    let stop_at = now() + if sent && outcome.is_none() { 20.0 } else { 0.0 };
    let child_rc = loop {
        match pty.try_wait() {
            Some(rc) => break rc,
            None if now() >= stop_at => break pty.kill_wait(),
            None => {
                // keep draining output while Claude Code finishes
                match pty.read(200) {
                    PtyRead::Data(b) => after.push_str(&String::from_utf8_lossy(&b)),
                    _ => sleep_ms(100),
                }
                ui.tick();
            }
        }
    };
    drop(pty);

    if let Some(o) = outcome {
        let msg = match o {
            Outcome::Timeout => "Timed out waiting for sign-in.",
            Outcome::Abuse => "Too many wrong requests to the phone page, so it was shut down for safety.",
            Outcome::Cancelled => "Cancelled.",
        };
        report(&mut ui, &mut relay, "failed", &format!("{} Run beamlogin on the terminal to start again.", msg), true);
        ui.clear_footer();
        let l = vec![
            format!("  {} {}", ui.red(ui.err_sym), msg),
            String::new(),
            format!("  {}", ui.dim("Run beamlogin again to start over.")),
            String::new(),
        ];
        ui.screen(sub, &l);
        ui.close();
        return 1;
    }
    if sent {
        let (ok, info) = auth_status(&claude, None);
        if child_rc == 0 && ok {
            report(&mut ui, &mut relay, "done", "", true);
            ui.clear_footer();
            let who = info_str(&info, "email").map(|w| ui.dim(&format!("  as {}", w))).unwrap_or_default();
            let mut l = vec![format!("  {} {}{}", ui.green(ui.ok_sym), ui.bold("Signed in to Claude Code"), who), String::new()];
            l.extend(revoke_lines(&ui));
            l.push(String::new());
            ui.screen(sub, &l);
            return finish(&mut ui, &claude, args, None);
        }
        report(
            &mut ui,
            &mut relay,
            "failed",
            "Claude Code didn't accept the code. Codes work once and expire after a few minutes, \
             so run beamlogin on the terminal again to get a new one.",
            true,
        );
        ui.clear_footer();
        let mut l = vec![format!("  {} {}. Claude Code said:", ui.red(ui.err_sym), ui.bold("Sign-in failed")), String::new()];
        l.extend(output_box(&ui, &after, 10));
        l.push(String::new());
        l.push(format!("  {}", ui.dim("Each code works once and expires quickly. Run beamlogin again for a new one.")));
        l.push(String::new());
        ui.screen(sub, &l);
        ui.close();
        return 1;
    }
    report(&mut ui, &mut relay, "failed", "Claude Code stopped before sign-in could start. Check the terminal.", true);
    ui.clear_footer();
    let mut l = vec![format!("  {} Claude Code stopped before showing a sign-in link. It said:", ui.red(ui.err_sym)), String::new()];
    l.extend(output_box(&ui, &raw, 15));
    l.push(String::new());
    ui.screen(sub, &l);
    ui.close();
    1
}

// ---------------------------------------------------------------------------
// token mode

/// Start hidden input. Returns Some(token) when it had to read it directly.
fn hidden_prompt(ui: &mut Ui) -> Option<String> {
    if ui.keys {
        ui.hint = "Paste the token and press Enter    Ctrl+D  cancel".into();
        ui.status("Paste your setup token here", Kind::Info, None);
        ui.input_start("token", true);
        return None;
    }
    ui.close();
    Some(sys::getpass("  Paste your setup token (input hidden): ").trim().to_string())
}

fn cmd_token(args: &Args) -> i32 {
    let mut ui = Ui::new(args.plain, &args.qr_style);
    let sub = "Claude Code setup token";
    let claude = match preflight(&mut ui, args, sub, !args.hidden_input) {
        Some(c) => c,
        None => return 1,
    };
    let mut token: Option<String> = None;
    let mut relay: Option<Relay> = None;
    let deadline = now() + args.timeout as f64;
    let mut stdin_ok = ui.keys || args.stdin_commands;

    if !args.hidden_input {
        ui.screen(sub, &[]);
        relay = start_relay(args.no_server, args.ip.as_deref(), &args.bind, args.port, Mode::Token, &mut ui);
    }
    if let Some(r) = &relay {
        let url = r.url.clone();
        let steps = [
            ("1", vec!["Scan this code with your phone".to_string(), ui.dim("(same network as this machine)")]),
            ("2", vec!["Paste your setup token".to_string(), ui.dim("(made with claude setup-token)")]),
            ("3", vec![format!("Tap {}", ui.bold("Send to terminal"))]),
        ];
        let notes = [ui.dim(&format!("Page: {}", url)), ui.dim("Plain HTTP on your local network: only use a network you trust.")];
        ui.panel(sub, &url, &steps, &notes);
        ui.hint = "p  paste the token here instead    q  quit".into();
        ui.status("Waiting for your phone", Kind::Wait, Some(deadline));
        ui.input_start("", false);
    } else {
        let l = vec![format!("  {}", ui.dim("The token is never shown or saved unless you use --save.")), String::new()];
        ui.screen(sub, &l);
        token = hidden_prompt(&mut ui);
    }

    let mut failure: Option<&str> = None;
    'main: while token.is_none() {
        if now() > deadline {
            failure = Some("Timed out.");
            break;
        }
        if interrupted() {
            failure = Some("Cancelled.");
            break;
        }
        if let Some(r) = relay.as_mut() {
            if r.aborted() {
                r.close();
                ui.input_stop();
                ui.clear_footer();
                let l = vec![
                    format!("  {} Too many wrong requests to the phone page, so it was shut down for safety.", ui.red(ui.err_sym)),
                    String::new(),
                ];
                ui.screen(sub, &l);
                ui.close();
                return 1;
            }
        }
        let readable = if stdin_ok {
            sys::stdin_wait(200)
        } else {
            sleep_ms(200);
            false
        };
        ui.tick();
        if let Some(r) = &relay {
            if r.opened() && ui.status_text.as_deref() == Some("Waiting for your phone") {
                let s = format!("Phone connected {} waiting for the token", ui.dim("·"));
                ui.status(&s, Kind::Wait, Some(deadline));
            }
            if let Some(v) = r.try_value() {
                token = Some(v);
                break;
            }
        }
        if readable {
            let lines = match ui.read_lines() {
                Ok(l) => l,
                Err(Stop::Eof) => {
                    stdin_ok = false;
                    vec![]
                }
                Err(Stop::Cancelled) => {
                    failure = Some("Cancelled.");
                    break;
                }
            };
            for line in lines {
                let word = remove_ws(&line);
                if ui.mask {
                    token = Some(word);
                    break;
                }
                if word.eq_ignore_ascii_case("q") {
                    failure = Some("Cancelled.");
                    break 'main;
                }
                if word.eq_ignore_ascii_case("p") {
                    if let Some(r) = &relay {
                        r.mark_used();
                    }
                    token = hidden_prompt(&mut ui);
                    break;
                }
                if !word.is_empty() {
                    ui.flash("Type p to paste the token here, or q to quit.");
                }
            }
        }
    }
    if let Some(msg) = failure {
        ui.input_stop();
        ui.clear_footer();
        report(&mut ui, &mut relay, "failed", &format!("{} Run beamlogin token on the terminal to start again.", msg), false);
        ui.screen(sub, &[format!("  {} {}", ui.red(ui.err_sym), msg), String::new()]);
        ui.close();
        return 1;
    }
    ui.input_stop();

    let token = remove_ws(&token.unwrap_or_default());
    if !is_token(&token) {
        report(&mut ui, &mut relay, "failed", "That doesn't look like a setup token.", false);
        ui.clear_footer();
        ui.screen(sub, &[format!("  {} That doesn't look like a setup token (it starts with sk-ant-).", ui.red(ui.err_sym)), String::new()]);
        ui.close();
        return 1;
    }
    let mut env = env_now();
    env_set(&mut env, "CLAUDE_CODE_OAUTH_TOKEN", &token);
    ui.hint.clear();
    ui.status("Token received", Kind::Ok, None);

    if !args.no_verify {
        ui.status("Checking the token with a tiny test request", Kind::Wait, None);
        let (c, e) = (claude.clone(), env.clone());
        let t = std::thread::spawn(move || {
            match run_capture(&c, &["-p", "Reply with just: OK"], Some(&e), Duration::from_secs(120)) {
                Some(p) => (p.code == 0, format!("{}{}", p.stdout, p.stderr)),
                None => (false, "timed out".to_string()),
            }
        });
        while !t.is_finished() {
            sleep_ms(200);
            ui.tick();
        }
        let (ok, out) = t.join().unwrap_or((false, String::new()));
        if !ok {
            report(
                &mut ui,
                &mut relay,
                "failed",
                "The token didn't work. It may be expired, revoked or mistyped. \
                 Make a new one with claude setup-token and try again.",
                false,
            );
            ui.clear_footer();
            let mut l = vec![
                format!("  {} {} (expired, revoked or mistyped). Claude Code said:", ui.red(ui.err_sym), ui.bold("The token did not work")),
                String::new(),
            ];
            l.extend(output_box(&ui, &out.replace(&token, "[token]"), 8));
            l.push(String::new());
            ui.screen(sub, &l);
            ui.close();
            return 1;
        }
    }

    report(&mut ui, &mut relay, "done", "", false);
    ui.clear_footer();
    let mut body = vec![
        if !args.no_verify {
            format!("  {} {}", ui.green(ui.ok_sym), ui.bold("Token works"))
        } else {
            format!("  {} {} {}", ui.green(ui.ok_sym), ui.bold("Token received"), ui.dim("(not checked)"))
        },
        String::new(),
    ];
    if let (true, Some(dir)) = (args.save, sys::runtime_dir()) {
        let path = format!("{}/token.env", dir);
        let written = sys::write_private(&path, &format!("export CLAUDE_CODE_OAUTH_TOKEN='{}'\n", token));
        match written {
            Ok(()) => {
                body.push("  Saved for other shells (memory only, readable only by you):".into());
                body.push(format!("     {}", ui.bold(&format!("source {}", path))));
            }
            Err(e) => body.push(format!("  {} Could not save {}: {}", ui.red(ui.err_sym), path, e)),
        }
        body.push(String::new());
    }
    let a = ui.arrow;
    body.extend([
        format!("  {}", ui.bold("This token keeps working for up to a year, wherever it is copied.")),
        "  If it might have leaked, revoke it on your phone:".to_string(),
        format!("     {}", ui.dim(&format!("claude.ai {a} Settings {a} Account {a} Where you're logged in"))),
        format!("     {}", ui.dim(&format!("{a} Log out on the \"Claude Code\" row whose Last active time matches"))),
        String::new(),
    ]);
    ui.screen(sub, &body);
    finish(&mut ui, &claude, args, Some(env))
}

// ---------------------------------------------------------------------------
// logout / doctor / qr

fn cmd_logout(args: &Args) -> i32 {
    let mut ui = Ui::new(args.plain, &args.qr_style);
    let sub = "sign out of Claude Code";
    let claude = match preflight(&mut ui, args, sub, false) {
        Some(c) => c,
        None => return 1,
    };
    let p = run_capture(&claude, &["auth", "logout"], None, Duration::from_secs(120))
        .unwrap_or(Output { code: 1, stdout: String::new(), stderr: "could not run claude auth logout".into() });
    let mut lines = Vec::new();
    if p.code == 0 {
        lines.push(format!("  {} {}", ui.green(ui.ok_sym), ui.bold("Signed out on this machine")));
    } else {
        lines.push(format!("  {} claude auth logout failed:", ui.red(ui.err_sym)));
        lines.push(String::new());
        lines.extend(output_box(&ui, &format!("{}{}", p.stdout, p.stderr), 10));
    }
    if let Some(dir) = sys::runtime_dir() {
        let path = format!("{}/token.env", dir);
        if std::path::Path::new(&path).exists() && std::fs::remove_file(&path).is_ok() {
            lines.push(format!("  {} Removed the saved token file", ui.green(ui.ok_sym)));
        }
    }
    if std::env::var("CLAUDE_CODE_OAUTH_TOKEN").map(|v| !v.is_empty()).unwrap_or(false) {
        lines.push(format!(
            "  {} CLAUDE_CODE_OAUTH_TOKEN is still set in this shell. Run {}",
            ui.red("!"),
            ui.bold("unset CLAUDE_CODE_OAUTH_TOKEN")
        ));
    }
    let a = ui.arrow;
    lines.extend([
        String::new(),
        format!("  {} (on your phone):", ui.bold("Now revoke the sign-in on Anthropic's side")),
        format!("     {}", ui.dim(&format!("claude.ai {a} Settings {a} Account {a} Where you're logged in"))),
        format!("     {}", ui.dim(&format!("{a} Log out on the matching \"Claude Code\" row"))),
        String::new(),
    ]);
    ui.screen(sub, &lines);
    ui.close();
    p.code
}

fn cmd_doctor(args: &Args) -> i32 {
    let mut ui = Ui::new(args.plain, &args.qr_style);
    let sub = "check this machine";
    ui.screen(sub, &[]);
    ui.status("Checking this machine", Kind::Wait, None);
    let ca = args.check_args();
    let t = std::thread::spawn(move || run_checks(&ca, true, true, true).0);
    while !t.is_finished() {
        sleep_ms(200);
        ui.tick();
    }
    let checks = t.join().unwrap_or_default();
    ui.clear_footer();
    let bad = checks.iter().filter(|c| c.level == Level::Bad).count();
    let warn = checks.iter().filter(|c| c.level == Level::Warn).count();
    let head = if bad > 0 {
        format!("  {} {}{}", ui.red(ui.err_sym), ui.bold("Not ready"), ui.dim(&format!("  {} problem{} to fix", bad, if bad > 1 { "s" } else { "" })))
    } else if warn > 0 {
        format!("  {} {}{}", ui.green(ui.ok_sym), ui.bold("Ready"), ui.dim(&format!("  {} thing{} worth a look", warn, if warn > 1 { "s" } else { "" })))
    } else {
        format!("  {} {}{}", ui.green(ui.ok_sym), ui.bold("Ready"), ui.dim("  everything looks good"))
    };
    let mut l = vec![head, String::new()];
    l.extend(check_lines(&ui, &checks));
    l.push(String::new());
    ui.screen(sub, &l);
    ui.close();
    if bad > 0 {
        1
    } else {
        0
    }
}

fn cmd_qr(args: &Args) -> i32 {
    let text = args.text.clone().unwrap_or_default();
    match qr::encode(text.as_bytes(), args.ecl, None, None) {
        Ok(mods) => {
            say(&qr::render(&mods, 4, &args.qr_style));
            0
        }
        Err(e) => {
            eprintln!("beamlogin: {}", e);
            1
        }
    }
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = parse_args(argv);
    sys::init(args.cmd != "qr");
    let rc = match args.cmd.as_str() {
        "login" => cmd_login(&args),
        "token" => cmd_token(&args),
        "logout" => cmd_logout(&args),
        "doctor" => cmd_doctor(&args),
        _ => cmd_qr(&args),
    };
    std::process::exit(rc);
}

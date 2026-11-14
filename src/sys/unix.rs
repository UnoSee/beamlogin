//! Unix platform layer (Linux and macOS).

use super::{Input, PtyRead};
use crate::util::{Env, INTERRUPTED};
use std::ffi::CString;
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::sync::atomic::{AtomicBool, Ordering};

static RESIZED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_sigint(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

extern "C" fn on_sigwinch(_: libc::c_int) {
    RESIZED.store(true, Ordering::SeqCst);
}

// ---------------------------------------------------------------------------
// console

/// Set up the console. With `catch_ctrl_c`, Ctrl+C sets a flag instead of killing us, so the
/// terminal is always restored.
pub fn init(catch_ctrl_c: bool) {
    unsafe {
        if catch_ctrl_c {
            libc::signal(libc::SIGINT, on_sigint as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
        libc::signal(libc::SIGWINCH, on_sigwinch as extern "C" fn(libc::c_int) as libc::sighandler_t);
    }
}

pub fn stdout_is_tty() -> bool {
    unsafe { libc::isatty(1) == 1 }
}

pub fn stdin_is_tty() -> bool {
    unsafe { libc::isatty(0) == 1 }
}

/// (columns, rows) of the terminal on stdout, if there is one.
pub fn console_size() -> Option<(usize, usize)> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(1, libc::TIOCGWINSZ as _, &mut ws) } == 0;
    if ok && (ws.ws_col > 0 || ws.ws_row > 0) {
        Some((ws.ws_col as usize, ws.ws_row as usize))
    } else {
        None
    }
}

/// True once after the terminal was resized.
pub fn resized() -> bool {
    RESIZED.swap(false, Ordering::SeqCst)
}

pub type TermState = libc::termios;

/// Turn off line buffering and echo on stdin (cbreak mode). Returns the old state.
pub fn raw_on() -> Option<TermState> {
    let mut t: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(0, &mut t) } != 0 {
        return None;
    }
    let mut c = t;
    c.c_lflag &= !(libc::ECHO | libc::ICANON);
    c.c_cc[libc::VMIN] = 1;
    c.c_cc[libc::VTIME] = 0;
    unsafe { libc::tcsetattr(0, libc::TCSAFLUSH, &c) };
    Some(t)
}

pub fn raw_off(t: &TermState) {
    unsafe { libc::tcsetattr(0, libc::TCSADRAIN, t) };
}

/// Wait up to `ms` milliseconds for stdin to have input (or end).
pub fn stdin_wait(ms: i32) -> bool {
    let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    let n = unsafe { libc::poll(&mut p, 1, ms) };
    n > 0 && p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0
}

/// Read what stdin has (call after `stdin_wait` said it's ready).
pub fn stdin_read() -> Input {
    let mut buf = vec![0u8; 4096];
    let n = unsafe { libc::read(0, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n > 0 {
        buf.truncate(n as usize);
        Input::Data(buf)
    } else if n == 0 {
        Input::Eof
    } else {
        match std::io::Error::last_os_error().raw_os_error() {
            Some(libc::EINTR) | Some(libc::EAGAIN) => Input::Nothing,
            _ => Input::Eof,
        }
    }
}

/// Read a line with echo off from /dev/tty (like Python's getpass), falling back to stdin.
pub fn getpass(prompt: &str) -> String {
    use std::os::unix::io::AsRawFd;
    if let Ok(mut tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") {
        let fd = tty.as_raw_fd();
        let _ = tty.write_all(prompt.as_bytes());
        let _ = tty.flush();
        let mut old: libc::termios = unsafe { std::mem::zeroed() };
        let have = unsafe { libc::tcgetattr(fd, &mut old) } == 0;
        if have {
            let mut new = old;
            new.c_lflag &= !libc::ECHO;
            unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &new) };
        }
        let mut line = Vec::new();
        let mut b = [0u8; 1];
        while let Ok(1) = tty.read(&mut b) {
            if b[0] == b'\n' || b[0] == b'\r' {
                break;
            }
            line.push(b[0]);
        }
        if have {
            unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &old) };
        }
        let _ = tty.write_all(b"\n");
        return String::from_utf8_lossy(&line).into_owned();
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(prompt.as_bytes());
    let _ = out.flush();
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    s.trim_end_matches(['\n', '\r']).to_string()
}

// ---------------------------------------------------------------------------
// system

pub fn hostname() -> String {
    let mut buf = [0 as libc::c_char; 256];
    unsafe {
        if libc::gethostname(buf.as_mut_ptr(), buf.len()) == 0 {
            return std::ffi::CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned();
        }
    }
    "localhost".into()
}

/// For example "Linux x86_64" or "Darwin arm64".
pub fn system_name() -> String {
    unsafe {
        let mut u: libc::utsname = std::mem::zeroed();
        if libc::uname(&mut u) == 0 {
            let s = std::ffi::CStr::from_ptr(u.sysname.as_ptr()).to_string_lossy().into_owned();
            let m = std::ffi::CStr::from_ptr(u.machine.as_ptr()).to_string_lossy().into_owned();
            return format!("{} {}", s, m).trim().to_string();
        }
    }
    "Unknown".into()
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_err() {
        panic!("cannot read /dev/urandom");
    }
    buf
}

pub fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

pub fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/".into())
}

/// Like Python's os.path.abspath: absolute, with "." and ".." resolved, links kept.
pub fn abspath(p: &str) -> String {
    let path = std::path::Path::new(p);
    let full = if path.is_absolute() { path.to_path_buf() } else { std::env::current_dir().unwrap_or_default().join(path) };
    let mut parts: Vec<String> = Vec::new();
    for comp in full.components() {
        use std::path::Component::*;
        match comp {
            RootDir => parts.clear(),
            CurDir | Prefix(_) => {}
            ParentDir => {
                parts.pop();
            }
            Normal(s) => parts.push(s.to_string_lossy().into_owned()),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn is_executable(path: &str) -> bool {
    match std::fs::metadata(path) {
        Ok(m) => {
            m.is_file() && m.permissions().mode() & 0o111 != 0 && {
                let c = CString::new(path).unwrap_or_default();
                unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
            }
        }
        Err(_) => false,
    }
}

/// File names a command may have on disk.
pub fn exe_names(name: &str) -> Vec<String> {
    vec![name.to_string()]
}

/// Does this look like a path rather than a command name?
pub fn is_path(name: &str) -> bool {
    name.contains('/')
}

// ---------------------------------------------------------------------------
// Claude Code

pub const INSTALL_CMD: &str = "curl -fsSL https://claude.ai/install.sh | bash";

pub fn install_runnable() -> bool {
    crate::util::which("curl").is_some() && crate::util::which("bash").is_some()
}

/// Run the official installer in the foreground. True on success.
pub fn run_installer() -> bool {
    std::process::Command::new("bash")
        .arg("-c")
        .arg(format!("set -o pipefail; {}", INSTALL_CMD))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Places the official installers and package managers put Claude Code, used when it isn't on
/// PATH. Each is a file without extension.
pub fn claude_fallbacks() -> Vec<String> {
    let h = home();
    let h = h.trim_end_matches('/');
    vec![
        format!("{}/.local/bin/claude", h),
        format!("{}/.claude/local/claude", h),
        "/usr/local/bin/claude".into(),
        "/opt/homebrew/bin/claude".into(),
        "/usr/bin/claude".into(),
    ]
}

/// A shell command that adds `dir` (shown with ~ for home) to PATH.
pub fn path_fix(dir: &str) -> String {
    format!("export PATH=\"{}:$PATH\"", dir).replace("\"~", "\"$HOME")
}

/// Stop Claude Code from opening a local (text) browser inside the hidden terminal.
pub fn set_browser_env(env: &mut Env) {
    let f = crate::util::which("false").unwrap_or_else(|| "/bin/false".into());
    crate::util::env_set(env, "BROWSER", &f);
}

/// Replace this process with Claude Code. Only returns if that failed.
pub fn run_claude(claude: &str, env: Option<Env>) -> i32 {
    use std::os::unix::process::CommandExt;
    let mut c = std::process::Command::new(claude);
    if let Some(env) = env {
        c.env_clear().envs(env);
    }
    let err = c.exec();
    crate::util::say(&format!("  Could not start Claude Code: {}", err));
    1
}

// ---------------------------------------------------------------------------
// token file

/// A memory-backed, owner-only folder for the saved token.
pub fn runtime_dir() -> Option<String> {
    let base = match std::env::var("XDG_RUNTIME_DIR") {
        Ok(b) if !b.is_empty() && std::path::Path::new(&b).is_dir() => b,
        _ => if std::path::Path::new("/dev/shm").is_dir() { "/dev/shm" } else { "/tmp" }.to_string(),
    };
    let path = format!("{}/beamlogin-{}", base.trim_end_matches('/'), unsafe { libc::getuid() });
    use std::os::unix::fs::DirBuilderExt;
    let _ = std::fs::DirBuilder::new().mode(0o700).create(&path);
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
    Some(path)
}

/// Write a file only the owner can read.
pub fn write_private(path: &str, data: &str) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(data.as_bytes()))
}

// ---------------------------------------------------------------------------
// pseudo-terminal

pub struct Pty {
    pid: libc::pid_t,
    fd: i32,
}

fn cstr(b: &[u8]) -> CString {
    CString::new(b.iter().copied().filter(|&c| c != 0).collect::<Vec<u8>>()).unwrap_or_default()
}

fn exitcode(status: libc::c_int) -> i32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status)
    } else if libc::WIFSIGNALED(status) {
        -libc::WTERMSIG(status)
    } else {
        -1
    }
}

impl Pty {
    /// Start `prog args...` with `env` in a new pty of `cols` x `rows`.
    pub fn spawn(prog: &str, args: &[String], env: &Env, cols: u16, rows: u16) -> std::io::Result<Pty> {
        // Build everything before fork: no allocation in the child.
        let path = cstr(prog.as_bytes());
        let mut argv_c: Vec<CString> = vec![path.clone()];
        argv_c.extend(args.iter().map(|a| cstr(a.as_bytes())));
        let mut argv: Vec<*const libc::c_char> = argv_c.iter().map(|c| c.as_ptr()).collect();
        argv.push(std::ptr::null());
        let env_c: Vec<CString> = env
            .iter()
            .map(|(k, v)| {
                let mut b = k.as_bytes().to_vec();
                b.push(b'=');
                b.extend_from_slice(v.as_bytes());
                cstr(&b)
            })
            .collect();
        let mut envp: Vec<*const libc::c_char> = env_c.iter().map(|c| c.as_ptr()).collect();
        envp.push(std::ptr::null());
        let errmsg = format!("beamlogin: cannot run {}\n", prog);

        let mut master: libc::c_int = -1;
        let mut slave: libc::c_int = -1;
        let mut ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        let rc = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut::<libc::termios>() as _,
                &mut ws as *mut libc::winsize as _,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            unsafe {
                libc::close(master);
                libc::close(slave);
            }
            return Err(std::io::Error::last_os_error());
        }
        if pid == 0 {
            unsafe {
                libc::close(master);
                libc::setsid();
                libc::ioctl(slave, libc::TIOCSCTTY as _, 0);
                libc::dup2(slave, 0);
                libc::dup2(slave, 1);
                libc::dup2(slave, 2);
                if slave > 2 {
                    libc::close(slave);
                }
                libc::signal(libc::SIGINT, libc::SIG_DFL);
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                libc::signal(libc::SIGWINCH, libc::SIG_DFL);
                libc::execve(path.as_ptr(), argv.as_ptr(), envp.as_ptr());
                libc::write(2, errmsg.as_ptr() as *const libc::c_void, errmsg.len());
                libc::_exit(127);
            }
        }
        unsafe { libc::close(slave) };
        Ok(Pty { pid, fd: master })
    }

    /// Wait up to `ms` for output.
    pub fn read(&mut self, ms: i32) -> PtyRead {
        let mut p = libc::pollfd { fd: self.fd, events: libc::POLLIN, revents: 0 };
        let n = unsafe { libc::poll(&mut p, 1, ms) };
        if n <= 0 {
            return PtyRead::Idle;
        }
        let mut buf = vec![0u8; 65536];
        let r = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if r > 0 {
            buf.truncate(r as usize);
            PtyRead::Data(buf)
        } else {
            PtyRead::Closed // EOF, or EIO once the child has exited
        }
    }

    pub fn write(&self, data: &[u8]) {
        let mut off = 0;
        while off < data.len() {
            let n = unsafe { libc::write(self.fd, data[off..].as_ptr() as *const libc::c_void, data.len() - off) };
            if n <= 0 {
                break;
            }
            off += n as usize;
        }
    }

    /// The exit code once the child has exited (-1 if it can't be known).
    pub fn try_wait(&mut self) -> Option<i32> {
        let mut st = 0;
        let r = unsafe { libc::waitpid(self.pid, &mut st, libc::WNOHANG) };
        if r < 0 {
            Some(-1)
        } else if r == 0 {
            None
        } else {
            Some(exitcode(st))
        }
    }

    pub fn kill_wait(&mut self) -> i32 {
        let mut st = 0;
        unsafe {
            libc::kill(self.pid, libc::SIGTERM);
            if libc::waitpid(self.pid, &mut st, 0) < 0 {
                return -1;
            }
        }
        exitcode(st)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

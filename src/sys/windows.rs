//! Windows platform layer: the console API for input and size, ConPTY (the Windows
//! pseudo-console, Windows 10 1809 or newer) to run `claude auth login`.

use super::{Input, PtyRead};
use crate::util::{Env, INTERRUPTED};
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows_sys::Win32::Storage::FileSystem::{GetFileType, ReadFile, WriteFile, FILE_TYPE_PIPE};
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use windows_sys::Win32::System::Threading::*;

fn stdin_handle() -> HANDLE {
    unsafe { GetStdHandle(STD_INPUT_HANDLE) }
}

fn stdout_handle() -> HANDLE {
    unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
}

fn is_console(h: HANDLE) -> bool {
    let mut mode: CONSOLE_MODE = 0;
    unsafe { GetConsoleMode(h, &mut mode) != 0 }
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn on_ctrl(_kind: u32) -> BOOL {
    INTERRUPTED.store(true, Ordering::SeqCst);
    1
}

// ---------------------------------------------------------------------------
// console

/// Set up the console: UTF-8, escape sequences for colour and cursor movement, and (with
/// `catch_ctrl_c`) Ctrl+C sets a flag instead of ending the program.
pub fn init(catch_ctrl_c: bool) {
    unsafe {
        SetConsoleOutputCP(65001);
        SetConsoleCP(65001);
        let out = stdout_handle();
        let mut mode: CONSOLE_MODE = 0;
        if GetConsoleMode(out, &mut mode) != 0 {
            SetConsoleMode(out, mode | ENABLE_PROCESSED_OUTPUT | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
        }
        if catch_ctrl_c {
            SetConsoleCtrlHandler(Some(on_ctrl), 1);
        }
    }
}

pub fn stdout_is_tty() -> bool {
    is_console(stdout_handle())
}

pub fn stdin_is_tty() -> bool {
    is_console(stdin_handle())
}

/// (columns, rows) of the visible console window, if stdout is a console.
pub fn console_size() -> Option<(usize, usize)> {
    let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
    if unsafe { GetConsoleScreenBufferInfo(stdout_handle(), &mut info) } == 0 {
        return None;
    }
    let w = info.srWindow.Right as i32 - info.srWindow.Left as i32 + 1;
    let h = info.srWindow.Bottom as i32 - info.srWindow.Top as i32 + 1;
    if w > 0 && h > 0 {
        Some((w as usize, h as usize))
    } else {
        None
    }
}

static LAST_SIZE: Mutex<Option<(usize, usize)>> = Mutex::new(None);

/// True once after the console window changed size (checked on each call).
pub fn resized() -> bool {
    let now = console_size();
    let mut last = LAST_SIZE.lock().unwrap_or_else(|e| e.into_inner());
    let changed = last.is_some() && *last != now;
    *last = now;
    changed
}

pub type TermState = CONSOLE_MODE;

/// Raw console input: no line editing or echo, keys arrive as characters and escape
/// sequences (so Ctrl+C arrives as the character 0x03). Returns the old mode.
pub fn raw_on() -> Option<TermState> {
    let h = stdin_handle();
    let mut mode: CONSOLE_MODE = 0;
    unsafe {
        if GetConsoleMode(h, &mut mode) == 0 {
            return None;
        }
        SetConsoleMode(h, ENABLE_VIRTUAL_TERMINAL_INPUT);
    }
    Some(mode)
}

pub fn raw_off(mode: &TermState) {
    unsafe { SetConsoleMode(stdin_handle(), *mode) };
}

/// Characters already taken from the console input queue.
static PENDING: Mutex<Vec<u16>> = Mutex::new(Vec::new());

/// Move typed characters from the console input queue to PENDING. Other events (mouse, focus,
/// key releases) are dropped.
fn drain_console() {
    let h = stdin_handle();
    let mut n: u32 = 0;
    unsafe { GetNumberOfConsoleInputEvents(h, &mut n) };
    if n == 0 {
        return;
    }
    let mut recs: Vec<INPUT_RECORD> = vec![unsafe { std::mem::zeroed() }; n as usize];
    let mut got: u32 = 0;
    if unsafe { ReadConsoleInputW(h, recs.as_mut_ptr(), n, &mut got) } == 0 {
        return;
    }
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    for r in &recs[..got as usize] {
        if r.EventType as u32 == KEY_EVENT as u32 {
            let k = unsafe { r.Event.KeyEvent };
            let ch = unsafe { k.uChar.UnicodeChar };
            if k.bKeyDown != 0 && ch != 0 {
                for _ in 0..k.wRepeatCount.max(1) {
                    pending.push(ch);
                }
            }
        }
    }
}

/// Wait up to `ms` milliseconds for stdin to have input (or end).
pub fn stdin_wait(ms: i32) -> bool {
    let h = stdin_handle();
    let end = Instant::now() + Duration::from_millis(ms.max(0) as u64);
    if is_console(h) {
        loop {
            if !PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
                return true;
            }
            let left = end.saturating_duration_since(Instant::now()).as_millis() as u32;
            if unsafe { WaitForSingleObject(h, left) } != WAIT_OBJECT_0 {
                return false;
            }
            drain_console();
            if !PENDING.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
        }
    }
    if unsafe { GetFileType(h) } == FILE_TYPE_PIPE {
        loop {
            let mut avail: u32 = 0;
            let ok = unsafe {
                PeekNamedPipe(h, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut())
            };
            if ok == 0 || avail > 0 {
                return true; // data, or the other end closed (EOF)
            }
            if Instant::now() >= end {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    true // a file or device: reads don't block for long
}

/// Read what stdin has (call after `stdin_wait` said it's ready).
pub fn stdin_read() -> Input {
    let h = stdin_handle();
    if is_console(h) {
        drain_console();
        let chars: Vec<u16> = std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()));
        if chars.is_empty() {
            return Input::Nothing;
        }
        return Input::Data(String::from_utf16_lossy(&chars).into_bytes());
    }
    let mut want: u32 = 4096;
    if unsafe { GetFileType(h) } == FILE_TYPE_PIPE {
        let mut avail: u32 = 0;
        let ok = unsafe {
            PeekNamedPipe(h, std::ptr::null_mut(), 0, std::ptr::null_mut(), &mut avail, std::ptr::null_mut())
        };
        if ok == 0 {
            return Input::Eof;
        }
        if avail == 0 {
            return Input::Nothing;
        }
        want = avail.min(4096);
    }
    let mut buf = vec![0u8; want as usize];
    let mut got: u32 = 0;
    let ok = unsafe { ReadFile(h, buf.as_mut_ptr(), want, &mut got, std::ptr::null_mut()) };
    if ok == 0 || got == 0 {
        return Input::Eof;
    }
    buf.truncate(got as usize);
    Input::Data(buf)
}

/// Read a line from the console with echo off.
pub fn getpass(prompt: &str) -> String {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(prompt.as_bytes());
    let _ = out.flush();
    let h = stdin_handle();
    let mut mode: CONSOLE_MODE = 0;
    let console = unsafe { GetConsoleMode(h, &mut mode) } != 0;
    if console {
        unsafe { SetConsoleMode(h, ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT) };
    }
    let mut s = String::new();
    let _ = std::io::stdin().read_line(&mut s);
    if console {
        unsafe { SetConsoleMode(h, mode) };
        let _ = out.write_all(b"\n");
    }
    s.trim_end_matches(['\n', '\r']).to_string()
}

// ---------------------------------------------------------------------------
// system

pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into())
}

/// For example "Windows x86_64".
pub fn system_name() -> String {
    format!("Windows {}", std::env::consts::ARCH)
}

pub fn random_bytes(n: usize) -> Vec<u8> {
    let mut buf = vec![0u8; n];
    let status = unsafe {
        BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), n as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
    };
    if status != 0 {
        panic!("BCryptGenRandom failed");
    }
    buf
}

pub fn is_root() -> bool {
    false
}

pub fn home() -> String {
    std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into())
}

pub fn abspath(p: &str) -> String {
    std::path::absolute(p).map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string())
}

pub fn is_executable(path: &str) -> bool {
    std::fs::metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// File names a command may have on disk: the name itself if it has an extension, otherwise
/// the name with each extension in PATHEXT.
pub fn exe_names(name: &str) -> Vec<String> {
    if std::path::Path::new(name).extension().is_some() {
        return vec![name.to_string()];
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    exts.split(';').filter(|e| !e.is_empty()).map(|e| format!("{}{}", name, e.to_lowercase())).collect()
}

/// Does this look like a path rather than a command name?
pub fn is_path(name: &str) -> bool {
    name.contains('/') || name.contains('\\')
}

// ---------------------------------------------------------------------------
// Claude Code

pub const INSTALL_CMD: &str = "irm https://claude.ai/install.ps1 | iex";

pub fn install_runnable() -> bool {
    crate::util::which("powershell").is_some()
}

/// Run the official installer in the foreground. True on success.
pub fn run_installer() -> bool {
    std::process::Command::new("powershell")
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", INSTALL_CMD])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Places the official installer, npm and WinGet put Claude Code, used when it isn't on PATH.
/// Each is a name without extension; PATHEXT extensions are tried.
pub fn claude_fallbacks() -> Vec<String> {
    let mut out = vec![format!("{}\\.local\\bin\\claude", home())];
    if let Ok(a) = std::env::var("APPDATA") {
        out.push(format!("{}\\npm\\claude", a));
    }
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        out.push(format!("{}\\Microsoft\\WinGet\\Links\\claude", l));
    }
    out
}

/// A PowerShell command that adds `dir` (shown with ~ for home) to PATH for this session.
pub fn path_fix(dir: &str) -> String {
    format!("$env:Path = \"{};$env:Path\"", dir).replace("\"~", "\"$env:USERPROFILE")
}

/// On Windows, Claude Code opening a browser is harmless (there's no text browser to trap it).
pub fn set_browser_env(_env: &mut Env) {}

/// Run Claude Code in this console and return its exit code (Windows has no exec).
pub fn run_claude(claude: &str, env: Option<Env>) -> i32 {
    let mut c = std::process::Command::new(claude);
    if let Some(env) = env {
        c.env_clear().envs(env);
    }
    match c.status() {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            crate::util::say(&format!("  Could not start Claude Code: {}", e));
            1
        }
    }
}

// ---------------------------------------------------------------------------
// token file

/// Windows has no memory-only, owner-only folder like /dev/shm, so `--save` isn't offered.
pub fn runtime_dir() -> Option<String> {
    None
}

pub fn write_private(_path: &str, _data: &str) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "not supported on Windows"))
}

// ---------------------------------------------------------------------------
// pseudo-console (ConPTY)

/// Quote one argument for a Windows command line (the rules CommandLineToArgvW reverses).
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut s = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                s.push_str(&"\\".repeat(slashes * 2 + 1));
                s.push('"');
                slashes = 0;
            }
            _ => {
                s.push_str(&"\\".repeat(slashes));
                slashes = 0;
                s.push(c);
            }
        }
    }
    s.push_str(&"\\".repeat(slashes * 2));
    s.push('"');
    s
}

fn env_block(env: &Env) -> Vec<u16> {
    let mut items: Vec<(String, String)> =
        env.iter().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect();
    items.sort_by_key(|(k, _)| k.to_uppercase());
    let mut block: Vec<u16> = Vec::new();
    for (k, v) in items {
        block.extend(std::ffi::OsStr::new(&format!("{}={}", k, v)).encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}

pub struct Pty {
    console: HPCON,
    console_open: bool,
    input: HANDLE,
    process: HANDLE,
    output: Receiver<Option<Vec<u8>>>,
    exit: Option<i32>,
}

impl Pty {
    /// Start `prog args...` with `env` in a new pseudo-console of `cols` x `rows`.
    pub fn spawn(prog: &str, args: &[String], env: &Env, cols: u16, rows: u16) -> std::io::Result<Pty> {
        let err = || std::io::Error::last_os_error();
        unsafe {
            let (mut in_read, mut in_write, mut out_read, mut out_write): (HANDLE, HANDLE, HANDLE, HANDLE) =
                (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
                return Err(err());
            }
            if CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
                return Err(err());
            }
            let size = COORD { X: cols as i16, Y: rows as i16 };
            let mut console: HPCON = std::mem::zeroed();
            let hr = CreatePseudoConsole(size, in_read, out_write, 0, &mut console);
            CloseHandle(in_read);
            CloseHandle(out_write);
            if hr < 0 {
                CloseHandle(in_write);
                CloseHandle(out_read);
                return Err(std::io::Error::new(std::io::ErrorKind::Other, format!("CreatePseudoConsole failed ({:#x})", hr)));
            }

            // Attach the pseudo-console to the new process.
            let mut bytes: usize = 0;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes);
            let mut attr_buf = vec![0u8; bytes];
            let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            if InitializeProcThreadAttributeList(attrs, 1, 0, &mut bytes) == 0 {
                return Err(err());
            }
            if UpdateProcThreadAttribute(
                attrs,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                console as *const c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            ) == 0
            {
                return Err(err());
            }
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            // Without this, a child of a process whose stdio is redirected would use those
            // handles instead of the pseudo-console.
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.lpAttributeList = attrs;

            let mut cmd = quote_arg(prog);
            for a in args {
                cmd.push(' ');
                cmd.push_str(&quote_arg(a));
            }
            let low = prog.to_lowercase();
            if low.ends_with(".cmd") || low.ends_with(".bat") {
                cmd = format!("cmd.exe /d /s /c \"{}\"", cmd);
            }
            let mut cmdline = wide(&cmd);
            let envb = env_block(env);
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                envb.as_ptr() as *const c_void,
                std::ptr::null(),
                &si.StartupInfo,
                &mut pi,
            );
            DeleteProcThreadAttributeList(attrs);
            if ok == 0 {
                let e = err();
                ClosePseudoConsole(console);
                CloseHandle(in_write);
                CloseHandle(out_read);
                return Err(e);
            }
            CloseHandle(pi.hThread);

            // Read the pseudo-console's output on a thread; the pipe has no timeout of its own.
            let (tx, rx) = channel();
            let out_handle = out_read as usize;
            std::thread::spawn(move || {
                let h = out_handle as HANDLE;
                let mut buf = vec![0u8; 65536];
                loop {
                    let mut got: u32 = 0;
                    let ok = ReadFile(h, buf.as_mut_ptr(), buf.len() as u32, &mut got, std::ptr::null_mut());
                    if ok == 0 || got == 0 {
                        let _ = tx.send(None);
                        break;
                    }
                    if tx.send(Some(buf[..got as usize].to_vec())).is_err() {
                        break;
                    }
                }
                CloseHandle(h);
            });
            Ok(Pty { console, console_open: true, input: in_write, process: pi.hProcess, output: rx, exit: None })
        }
    }

    /// Wait up to `ms` for output. Once the child has exited, the pseudo-console is closed so the
    /// rest of its output arrives, then this reports Closed.
    pub fn read(&mut self, ms: i32) -> PtyRead {
        match self.output.recv_timeout(Duration::from_millis(ms.max(0) as u64)) {
            Ok(Some(d)) => PtyRead::Data(d),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => PtyRead::Closed,
            Err(RecvTimeoutError::Timeout) => {
                if self.console_open && self.try_wait().is_some() {
                    unsafe { ClosePseudoConsole(self.console) };
                    self.console_open = false;
                }
                PtyRead::Idle
            }
        }
    }

    pub fn write(&self, data: &[u8]) {
        let mut off = 0;
        while off < data.len() {
            let mut n: u32 = 0;
            let ok = unsafe {
                WriteFile(self.input, data[off..].as_ptr(), (data.len() - off) as u32, &mut n, std::ptr::null_mut())
            };
            if ok == 0 || n == 0 {
                break;
            }
            off += n as usize;
        }
    }

    /// The exit code once the child has exited.
    pub fn try_wait(&mut self) -> Option<i32> {
        if self.exit.is_none() && unsafe { WaitForSingleObject(self.process, 0) } == WAIT_OBJECT_0 {
            let mut code: u32 = 1;
            unsafe { GetExitCodeProcess(self.process, &mut code) };
            self.exit = Some(code as i32);
        }
        self.exit
    }

    pub fn kill_wait(&mut self) -> i32 {
        if let Some(c) = self.try_wait() {
            return c;
        }
        unsafe {
            TerminateProcess(self.process, 1);
            WaitForSingleObject(self.process, INFINITE);
        }
        self.try_wait().unwrap_or(-1)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        unsafe {
            if self.console_open {
                ClosePseudoConsole(self.console);
            }
            CloseHandle(self.input);
            CloseHandle(self.process);
        }
    }
}

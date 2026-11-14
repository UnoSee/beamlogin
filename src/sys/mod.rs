//! Platform layer: everything that differs between Unix (Linux, macOS) and Windows.
//!
//! Both implementations provide the same functions and types:
//!
//! - console: `init`, `stdout_is_tty`, `stdin_is_tty`, `console_size`, `resized`,
//!   `raw_on` / `raw_off`, `stdin_wait`, `stdin_read`, `getpass`
//! - system: `hostname`, `system_name`, `random_bytes`, `is_root`, `home`, `abspath`,
//!   `is_executable`, `exe_names`, `is_path`
//! - Claude Code: `Pty` (run `claude auth login` in a pseudo-terminal), `run_claude`,
//!   `INSTALL_CMD`, `install_runnable`, `run_installer`, `claude_fallbacks`, `path_fix`,
//!   `set_browser_env`
//! - token files: `runtime_dir`, `write_private`

/// What a non-blocking read from stdin found.
pub enum Input {
    Data(Vec<u8>),
    Nothing,
    Eof,
}

/// What a read from the pseudo-terminal found.
pub enum PtyRead {
    Data(Vec<u8>),
    Idle,
    Closed,
}

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub use unix::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

//! Terminal UI with a live footer (status, hint, input). Degrades to plain
//! lines when stdout is not a terminal.

use crate::qr;
use crate::sys::{self, Input};
use crate::util::*;
use std::io::Write;

/// Why keyboard input stopped.
#[derive(Debug)]
pub enum Stop {
    Cancelled,
    Eof,
}

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
pub enum Kind {
    Wait,
    Ok,
    Err,
    Info,
}

pub struct Ui {
    pub tty: bool,
    pub keys: bool,
    color: bool,
    pub console: bool,
    accent_code: &'static str,
    pub ok_sym: &'static str,
    pub err_sym: &'static str,
    pub arrow: &'static str,
    ellipsis: &'static str,
    bullet: &'static str,
    spinner: Vec<char>,
    qr_style: String,
    frame: usize,
    pub status_text: Option<String>,
    status_kind: Kind,
    deadline: Option<f64>,
    pub hint: String,
    flash_text: Option<String>,
    flash_until: f64,
    input_on: bool,
    input_label: String,
    pub mask: bool,
    buf: String,
    footer_lines: usize,
    old_term: Option<sys::TermState>,
    last_plain: Option<String>,
    stdin_pending: Vec<u8>,
}

impl Ui {
    pub fn new(plain: bool, qr_style: &str) -> Ui {
        let term = std::env::var("TERM").unwrap_or_default();
        let tty = sys::stdout_is_tty() && !plain && term != "dumb";
        let console = term == "linux";
        let colorterm = std::env::var("COLORTERM").unwrap_or_default();
        // Windows Terminal (WT_SESSION) and modern terminals show 256 colours.
        let rich = term.contains("256color")
            || colorterm == "truecolor"
            || colorterm == "24bit"
            || std::env::var_os("WT_SESSION").is_some();
        Ui {
            tty,
            keys: tty && sys::stdin_is_tty(),
            color: tty && std::env::var_os("NO_COLOR").is_none(),
            console,
            accent_code: if rich { "38;5;209" } else { "33" },
            ok_sym: if console { "+" } else { "✓" },
            err_sym: if console { "x" } else { "✗" },
            arrow: if console { ">" } else { "›" },
            ellipsis: if console { "..." } else { "…" },
            bullet: if console { "*" } else { "•" },
            spinner: if console { "|/-\\".chars().collect() } else { "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".chars().collect() },
            qr_style: qr_style.to_string(),
            frame: 0,
            status_text: None,
            status_kind: Kind::Info,
            deadline: None,
            hint: String::new(),
            flash_text: None,
            flash_until: 0.0,
            input_on: false,
            input_label: String::new(),
            mask: false,
            buf: String::new(),
            footer_lines: 0,
            old_term: None,
            last_plain: None,
            stdin_pending: Vec::new(),
        }
    }

    // styling ---------------------------------------------------------------
    fn s(&self, t: &str, code: &str) -> String {
        if self.color {
            format!("\x1b[{}m{}\x1b[0m", code, t)
        } else {
            t.to_string()
        }
    }
    pub fn bold(&self, t: &str) -> String {
        self.s(t, "1")
    }
    pub fn dim(&self, t: &str) -> String {
        self.s(t, "2")
    }
    pub fn accent(&self, t: &str) -> String {
        self.s(t, self.accent_code)
    }
    pub fn green(&self, t: &str) -> String {
        self.s(t, "32")
    }
    pub fn red(&self, t: &str) -> String {
        self.s(t, "31")
    }

    pub fn cols(&self) -> usize {
        term_size((80, 24)).0
    }
    pub fn rows(&self) -> usize {
        term_size((80, 24)).1
    }

    fn write(&self, s: &str) {
        let mut out = std::io::stdout();
        let _ = out.write_all(s.as_bytes());
        let _ = out.flush();
    }

    pub fn resized(&self) -> bool {
        sys::resized()
    }

    // page layout -----------------------------------------------------------
    fn header(&self, subtitle: &str) -> Vec<String> {
        let host = sys::hostname();
        let width = self.cols().min(78).max(40);
        let left = format!("  {}{}{}", self.bold("Beamlogin"), self.dim("  ·  "), subtitle);
        let gap = (width as i64 - vlen(&left) as i64 - host.chars().count() as i64 - 2).max(2) as usize;
        let rule = (if self.console { "-" } else { "─" }).repeat(width - 4);
        vec![String::new(), format!("{}{}{}", left, " ".repeat(gap), self.dim(&host)), format!("  {}", self.dim(&rule)), String::new()]
    }

    /// Clear the screen and show a header plus body lines.
    pub fn screen(&mut self, subtitle: &str, lines: &[String]) {
        if self.tty {
            self.footer_lines = 0;
            let mut all = self.header(subtitle);
            all.extend(lines.iter().cloned());
            self.write(&format!("\x1b[2J\x1b[H{}\n", all.join("\n")));
            self.draw_footer();
        } else {
            say(&lines.join("\n"));
        }
    }

    pub fn steps(&self, items: &[(&str, Vec<String>)]) -> Vec<String> {
        let mut out = Vec::new();
        for (num, texts) in items {
            for (i, t) in texts.iter().enumerate() {
                let lead = if i == 0 { self.accent(&self.bold(num)) } else { " ".to_string() };
                out.push(format!("{}  {}", lead, t));
            }
            out.push(String::new());
        }
        out.pop();
        out
    }

    fn qr_block(&self, data: &str, ecl: char) -> (Vec<String>, usize) {
        let mods = qr::encode(data.as_bytes(), ecl, None, None).unwrap_or_default();
        let quiet = if !self.tty {
            4
        } else if self.rows() >= 30 {
            3
        } else {
            2
        };
        let lines: Vec<String> = qr::render(&mods, quiet, &self.qr_style).split('\n').map(String::from).collect();
        let width = (mods.len() + 2 * quiet) * if self.qr_style == "half" { 1 } else { 2 };
        (lines, width)
    }

    /// QR code with numbered steps beside it (or below it when narrow).
    pub fn panel(&mut self, subtitle: &str, data: &str, step_items: &[(&str, Vec<String>)], notes: &[String]) {
        let (qrl, qw) = self.qr_block(data, 'L');
        let steps = self.steps(step_items);
        let sw = steps.iter().map(|s| vlen(s)).max().unwrap_or(0);
        let mut body: Vec<String> = Vec::new();
        if self.tty && self.cols() >= 2 + qw + 4 + sw {
            let top = qrl.len().saturating_sub(steps.len()) / 2;
            for i in 0..qrl.len().max(top + steps.len()) {
                let left = if i < qrl.len() { qrl[i].clone() } else { " ".repeat(qw) };
                let right = if i >= top && i - top < steps.len() { steps[i - top].clone() } else { String::new() };
                body.push(format!("  {}    {}", left, right));
            }
        } else {
            body.extend(qrl.iter().map(|l| format!("  {}", l)));
            body.push(String::new());
            body.extend(steps.iter().map(|s| format!("  {}", s)));
        }
        if !notes.is_empty() {
            body.push(String::new());
            body.extend(notes.iter().map(|n| format!("  {}", n)));
        }
        if self.tty && (qw + 2 > self.cols() || qrl.len() + 10 > self.rows()) {
            body.push(String::new());
            body.push(format!(
                "  {}",
                self.red(&format!("The terminal is too small for this QR code ({} columns x {} rows).", qw, qrl.len()))
            ));
            body.push(format!("  {}", self.dim("Enlarge it or pick a smaller console font (e.g. setfont), then it redraws.")));
        }
        self.screen(subtitle, &body);
    }

    // live footer -----------------------------------------------------------
    fn footer(&self) -> Vec<String> {
        let mut lines = Vec::new();
        let cols = self.cols();
        if let Some(text) = &self.status_text {
            let icon = match self.status_kind {
                Kind::Wait => self.accent(&self.spinner[self.frame % self.spinner.len()].to_string()),
                Kind::Ok => self.green(self.ok_sym),
                Kind::Err => self.red(self.err_sym),
                Kind::Info => self.dim(self.bullet),
            };
            let left = format!("  {} {}", icon, text);
            let mut right = String::new();
            if let Some(d) = self.deadline {
                let left_s = (d - now()).max(0.0) as i64;
                right = self.dim(&format!("{}:{:02} left", left_s / 60, left_s % 60));
            }
            let gap = (cols.min(78) as i64 - vlen(&left) as i64 - vlen(&right) as i64 - 2).max(2) as usize;
            lines.push(format!("{}{}{}", left, " ".repeat(gap), right));
        }
        if self.flash_text.is_some() && now() < self.flash_until {
            lines.push(format!("  {}", self.red(self.flash_text.as_deref().unwrap_or(""))));
        } else if !self.hint.is_empty() {
            lines.push(format!("  {}", self.dim(&self.hint)));
        }
        if self.input_on {
            let label = format!("  {} {} ", self.input_label, self.accent(self.arrow));
            let mut shown = if self.mask {
                (if self.console { "*" } else { "•" }).repeat(self.buf.chars().count())
            } else {
                self.buf.clone()
            };
            let room = (cols as i64 - vlen(&label) as i64 - 2).max(10) as usize;
            let n = shown.chars().count();
            if n > room {
                let keep = room - self.ellipsis.chars().count();
                shown = format!("{}{}", self.ellipsis, shown.chars().skip(n - keep).collect::<String>());
            }
            lines.push(format!("{}{}", label, shown));
        }
        lines
            .into_iter()
            .map(|l| if vlen(&l) < cols { l } else { strip_ansi(&l).chars().take(cols.saturating_sub(1)).collect() })
            .collect()
    }

    fn erase_footer(&mut self) {
        if self.tty && self.footer_lines > 0 {
            self.write(&format!("\r\x1b[2K{}", "\x1b[1A\x1b[2K".repeat(self.footer_lines - 1)));
            self.footer_lines = 0;
        }
    }

    fn draw_footer(&mut self) {
        if !self.tty {
            return;
        }
        let lines = self.footer();
        if !lines.is_empty() {
            self.write(&format!("{}{}", lines.join("\n"), if self.input_on { "\x1b[?25h" } else { "\x1b[?25l" }));
        }
        self.footer_lines = lines.len();
    }

    pub fn refresh(&mut self) {
        if self.tty {
            self.erase_footer();
            self.draw_footer();
        }
    }

    pub fn tick(&mut self) {
        self.frame += 1;
        if self.tty && (self.status_kind == Kind::Wait || self.flash_text.is_some()) {
            if self.flash_text.is_some() && now() >= self.flash_until {
                self.flash_text = None;
            }
            self.refresh();
        }
    }

    pub fn status(&mut self, text: &str, kind: Kind, deadline: Option<f64>) {
        self.status_text = Some(text.to_string());
        self.status_kind = kind;
        self.deadline = deadline;
        if self.tty {
            self.refresh();
        } else if self.last_plain.as_deref() != Some(text) {
            self.last_plain = Some(text.to_string());
            say(&format!("  {}", text));
        }
    }

    pub fn flash(&mut self, text: &str) {
        self.flash_text = Some(text.to_string());
        self.flash_until = now() + 3.0;
        if self.tty {
            self.refresh();
        } else {
            say(&format!("  {}", text));
        }
    }

    pub fn clear_footer(&mut self) {
        self.erase_footer();
        self.status_text = None;
        self.hint.clear();
        self.input_on = false;
    }

    // keyboard input --------------------------------------------------------
    pub fn input_start(&mut self, label: &str, mask: bool) {
        self.input_label = label.to_string();
        self.mask = mask;
        self.buf.clear();
        if self.keys {
            if self.old_term.is_none() {
                self.old_term = sys::raw_on();
            }
            self.input_on = true;
        }
        self.refresh();
    }

    pub fn input_stop(&mut self) {
        self.input_on = false;
        if let Some(t) = self.old_term.take() {
            sys::raw_off(&t);
        }
        self.refresh();
    }

    /// Process raw keyboard text; return the completed lines.
    fn feed(&mut self, data: &str) -> Result<Vec<String>, Stop> {
        let mut done = Vec::new();
        for ch in strip_keyseq(data).chars() {
            match ch {
                '\r' | '\n' => done.push(std::mem::take(&mut self.buf)),
                '\x7f' | '\x08' => {
                    self.buf.pop();
                }
                '\x15' => self.buf.clear(),
                '\x04' if self.buf.is_empty() => return Err(Stop::Cancelled),
                '\x03' => return Err(Stop::Cancelled), // Ctrl+C in a raw Windows console
                c if !c.is_control() => self.buf.push(c),
                _ => {}
            }
        }
        self.refresh();
        Ok(done)
    }

    /// Call when stdin is readable. Returns completed lines (maybe none).
    pub fn read_lines(&mut self) -> Result<Vec<String>, Stop> {
        let chunk = match sys::stdin_read() {
            Input::Data(d) => d,
            Input::Nothing => return Ok(vec![]),
            Input::Eof => Vec::new(),
        };
        if self.keys {
            if chunk.is_empty() {
                return Err(Stop::Eof);
            }
            return self.feed(&String::from_utf8_lossy(&chunk));
        }
        if chunk.is_empty() {
            if self.stdin_pending.is_empty() {
                return Err(Stop::Eof);
            }
            let rest = std::mem::take(&mut self.stdin_pending);
            return Ok(vec![String::from_utf8_lossy(&rest).into_owned()]);
        }
        self.stdin_pending.extend_from_slice(&chunk);
        let mut lines = Vec::new();
        while let Some(p) = self.stdin_pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.stdin_pending.drain(..=p).collect();
            lines.push(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
        }
        Ok(lines)
    }

    /// Show prompt and wait for Enter (terminal only). Returns false on Ctrl+C.
    pub fn wait_enter(&mut self, prompt: &str) -> bool {
        if !self.keys {
            return true;
        }
        self.status(prompt, Kind::Info, None);
        self.input_start("", false);
        let result = loop {
            if interrupted() {
                break false;
            }
            if sys::stdin_wait(500) {
                match self.read_lines() {
                    Ok(l) if !l.is_empty() => break true,
                    Ok(_) => {}
                    Err(_) => break false,
                }
            }
        };
        self.input_stop();
        self.clear_footer();
        result
    }

    pub fn close(&mut self) {
        if self.old_term.is_some() {
            self.input_stop();
        }
        if self.tty {
            self.erase_footer();
            self.write("\x1b[?25h");
        }
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        if let Some(t) = self.old_term.take() {
            sys::raw_off(&t);
        }
    }
}

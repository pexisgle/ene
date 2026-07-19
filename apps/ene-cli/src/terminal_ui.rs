//! Prompt-safe terminal output for the CLI REPL.
//!
//! Log lines always appear above the active `>: ` prompt. While the user is
//! typing, concurrent post-turn logs clear the prompt line, print, then redraw
//! `>: ` plus the in-progress buffer.

use crossterm::{
    cursor::{MoveToColumn, MoveUp},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};
use parking_lot::Mutex;
use std::io::{self, Stderr, Stdout, Write};
use std::sync::{Arc, OnceLock};

const PROMPT_PREFIX: &str = ">: ";

static GLOBAL: OnceLock<TerminalUi> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
enum UiMode {
    Idle,
    Prompting { buffer: String },
    Streaming { at_line_start: bool },
}

/// Shared handle for coordinated stdout/stderr display.
#[derive(Clone)]
pub struct TerminalUi {
    inner: Arc<Mutex<TerminalUiInner>>,
}

struct TerminalUiInner {
    mode: UiMode,
    backend: Box<dyn TerminalBackend>,
}

/// Testable output sink.
pub trait TerminalBackend: Send {
    fn write_stdout(&mut self, s: &str);
    fn write_stderr(&mut self, s: &str);
    fn flush_all(&mut self);
    fn clear_current_line(&mut self);
    fn move_cursor_up(&mut self, n: usize);
    fn move_to_column0(&mut self);
    fn is_interactive(&self) -> bool;
}

/// Records operations for unit tests (no TTY).
#[cfg(test)]
#[derive(Debug, Default)]
pub struct CaptureBackend {
    pub stdout: String,
    pub stderr: String,
    pub ops: Vec<CaptureOp>,
}

#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOp {
    Stdout(String),
    Stderr(String),
    ClearCurrentLine,
    MoveUp(usize),
    MoveToColumn0,
}

#[cfg(test)]
impl TerminalBackend for CaptureBackend {
    fn write_stdout(&mut self, s: &str) {
        self.stdout.push_str(s);
        self.ops.push(CaptureOp::Stdout(s.to_string()));
    }

    fn write_stderr(&mut self, s: &str) {
        self.stderr.push_str(s);
        self.ops.push(CaptureOp::Stderr(s.to_string()));
    }

    fn flush_all(&mut self) {}

    fn clear_current_line(&mut self) {
        self.ops.push(CaptureOp::ClearCurrentLine);
    }

    fn move_cursor_up(&mut self, n: usize) {
        if n > 0 {
            self.ops.push(CaptureOp::MoveUp(n));
        }
    }

    fn move_to_column0(&mut self) {
        self.ops.push(CaptureOp::MoveToColumn0);
    }

    fn is_interactive(&self) -> bool {
        false
    }
}

struct RealBackend {
    stdout: Stdout,
    stderr: Stderr,
}

impl RealBackend {
    fn new() -> Self {
        Self {
            stdout: io::stdout(),
            stderr: io::stderr(),
        }
    }
}

impl TerminalBackend for RealBackend {
    fn write_stdout(&mut self, s: &str) {
        let _ = self.stdout.write_all(s.as_bytes());
    }

    fn write_stderr(&mut self, s: &str) {
        let _ = queue!(self.stderr, Print(s));
    }

    fn flush_all(&mut self) {
        let _ = self.stdout.flush();
        let _ = self.stderr.flush();
    }

    fn clear_current_line(&mut self) {
        let _ = queue!(self.stderr, MoveToColumn(0), Clear(ClearType::CurrentLine));
    }

    fn move_cursor_up(&mut self, n: usize) {
        if n > 0 {
            let _ = queue!(self.stderr, MoveUp(u16::try_from(n).unwrap_or(u16::MAX)));
        }
    }

    fn move_to_column0(&mut self) {
        let _ = queue!(self.stderr, MoveToColumn(0));
    }

    fn is_interactive(&self) -> bool {
        terminal::is_raw_mode_enabled().unwrap_or(false)
            || std::io::IsTerminal::is_terminal(&io::stdin())
    }
}

impl TerminalUi {
    /// Install the process-wide UI used by the tracing layer and REPL.
    pub fn init_global() -> Self {
        GLOBAL
            .get_or_init(|| Self::with_backend(Box::new(RealBackend::new())))
            .clone()
    }

    #[must_use]
    pub fn global() -> Self {
        Self::init_global()
    }

    #[must_use]
    pub fn with_backend(backend: Box<dyn TerminalBackend>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TerminalUiInner {
                mode: UiMode::Idle,
                backend,
            })),
        }
    }

    /// Print a full log line on stderr, always above an active prompt.
    pub fn writeln(&self, line: &str) {
        let mut inner = self.inner.lock();
        let prompting = matches!(inner.mode, UiMode::Prompting { .. });
        if prompting {
            inner.backend.clear_current_line();
            inner.backend.move_to_column0();
        }
        inner.backend.write_stderr(line);
        if !line.ends_with('\n') {
            inner.backend.write_stderr("\n");
        }
        if prompting {
            inner.redraw_prompt();
        }
        inner.backend.flush_all();
    }

    /// Replace a previously drawn multi-line block (open tree) in place.
    ///
    /// `old_line_count` is how many lines are currently on screen for the block.
    /// Returns the new line count.
    pub fn replace_block(&self, old_line_count: usize, new_lines: &[String]) -> usize {
        let mut inner = self.inner.lock();
        let prompting = matches!(inner.mode, UiMode::Prompting { .. });
        if prompting {
            inner.backend.clear_current_line();
            inner.backend.move_to_column0();
        }

        if old_line_count > 0 {
            inner.backend.move_cursor_up(old_line_count);
        }

        for line in new_lines {
            inner.backend.clear_current_line();
            inner.backend.move_to_column0();
            inner.backend.write_stderr(line);
            inner.backend.write_stderr("\n");
        }

        if new_lines.len() < old_line_count {
            let extra = old_line_count.saturating_sub(new_lines.len());
            for _ in 0..extra {
                inner.backend.clear_current_line();
                inner.backend.move_to_column0();
                inner.backend.write_stderr("\n");
            }
            inner.backend.move_cursor_up(extra);
        }

        if prompting {
            inner.redraw_prompt();
        }
        inner.backend.flush_all();
        new_lines.len()
    }

    /// Stream LLM text to stdout.
    pub fn write_stream(&self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        let mut inner = self.inner.lock();
        match &inner.mode {
            UiMode::Streaming { .. } => {}
            UiMode::Prompting { .. } => {
                inner.backend.clear_current_line();
                inner.backend.move_to_column0();
                inner.mode = UiMode::Streaming {
                    at_line_start: true,
                };
            }
            UiMode::Idle => {
                inner.mode = UiMode::Streaming {
                    at_line_start: true,
                };
            }
        }
        inner.backend.write_stdout(delta);
        if let UiMode::Streaming { at_line_start } = &mut inner.mode {
            *at_line_start = delta.ends_with('\n');
        }
        inner.backend.flush_all();
    }

    /// Ensure the stream ends with a newline before the next prompt/logs.
    pub fn end_stream(&self) {
        let mut inner = self.inner.lock();
        if let UiMode::Streaming {
            at_line_start: false,
        } = inner.mode
        {
            inner.backend.write_stdout("\n");
        }
        if matches!(inner.mode, UiMode::Streaming { .. }) {
            inner.mode = UiMode::Idle;
            inner.backend.flush_all();
        }
    }

    /// Async REPL line read (blocking editor on a worker thread).
    pub async fn read_line(&self) -> Option<String> {
        let ui = self.clone();
        tokio::task::spawn_blocking(move || ui.read_line_blocking())
            .await
            .unwrap_or(None)
    }

    fn read_line_blocking(&self) -> Option<String> {
        let interactive = {
            let inner = self.inner.lock();
            inner.backend.is_interactive() || std::io::IsTerminal::is_terminal(&io::stdin())
        };

        if !interactive {
            return self.read_line_stdio();
        }

        if terminal::enable_raw_mode().is_err() {
            return self.read_line_stdio();
        }

        self.begin_prompt();
        let result = self.edit_line_raw();
        let _ = terminal::disable_raw_mode();

        if let Some(line) = result {
            self.finish_prompt_with_line(&line);
            Some(line)
        } else {
            self.cancel_prompt();
            None
        }
    }

    fn read_line_stdio(&self) -> Option<String> {
        self.begin_prompt();
        let mut line = String::new();
        match io::stdin().read_line(&mut line) {
            Ok(0) | Err(_) => {
                self.cancel_prompt();
                None
            }
            Ok(_) => {
                while line.ends_with('\n') || line.ends_with('\r') {
                    line.pop();
                }
                self.finish_prompt_with_line(&line);
                Some(line)
            }
        }
    }

    fn begin_prompt(&self) {
        let mut inner = self.inner.lock();
        if let UiMode::Streaming {
            at_line_start: false,
        } = inner.mode
        {
            inner.backend.write_stdout("\n");
        }
        inner.mode = UiMode::Prompting {
            buffer: String::new(),
        };
        inner.redraw_prompt();
        inner.backend.flush_all();
    }

    fn finish_prompt_with_line(&self, line: &str) {
        let mut inner = self.inner.lock();
        inner.mode = UiMode::Prompting {
            buffer: line.to_string(),
        };
        inner.backend.clear_current_line();
        inner.backend.move_to_column0();
        inner.redraw_prompt();
        inner.backend.write_stderr("\n");
        inner.mode = UiMode::Idle;
        inner.backend.flush_all();
    }

    fn cancel_prompt(&self) {
        let mut inner = self.inner.lock();
        if matches!(inner.mode, UiMode::Prompting { .. }) {
            inner.backend.clear_current_line();
            inner.backend.move_to_column0();
            inner.backend.write_stderr("\n");
            inner.mode = UiMode::Idle;
            inner.backend.flush_all();
        }
    }

    fn edit_line_raw(&self) -> Option<String> {
        loop {
            let Ok(ev) = event::read() else {
                return None;
            };
            let Event::Key(KeyEvent {
                code,
                modifiers,
                kind,
                ..
            }) = ev
            else {
                continue;
            };
            // Only handle key presses (ignore repeats / releases).
            if kind != KeyEventKind::Press {
                continue;
            }

            match code {
                KeyCode::Enter => {
                    let buffer = {
                        let inner = self.inner.lock();
                        match &inner.mode {
                            UiMode::Prompting { buffer } => buffer.clone(),
                            _ => String::new(),
                        }
                    };
                    return Some(buffer);
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    return None;
                }
                KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                    let empty = {
                        let inner = self.inner.lock();
                        match &inner.mode {
                            UiMode::Prompting { buffer } => buffer.is_empty(),
                            _ => true,
                        }
                    };
                    if empty {
                        return None;
                    }
                }
                KeyCode::Backspace => {
                    let mut inner = self.inner.lock();
                    if let UiMode::Prompting { buffer } = &mut inner.mode {
                        buffer.pop();
                        inner.redraw_prompt();
                        inner.backend.flush_all();
                    }
                }
                KeyCode::Char(c) if !modifiers.contains(KeyModifiers::CONTROL) => {
                    let mut inner = self.inner.lock();
                    if let UiMode::Prompting { buffer } = &mut inner.mode {
                        buffer.push(c);
                        inner.redraw_prompt();
                        inner.backend.flush_all();
                    }
                }
                _ => {}
            }
        }
    }

    #[cfg(test)]
    pub fn with_capture() -> (Self, Arc<Mutex<CaptureBackend>>) {
        let capture = Arc::new(Mutex::new(CaptureBackend::default()));
        let backend = SharedCaptureBackend {
            inner: Arc::clone(&capture),
        };
        (Self::with_backend(Box::new(backend)), capture)
    }

    #[cfg(test)]
    pub fn set_prompt_buffer_for_test(&self, buffer: &str) {
        let mut inner = self.inner.lock();
        inner.mode = UiMode::Prompting {
            buffer: buffer.to_string(),
        };
        inner.redraw_prompt();
        inner.backend.flush_all();
    }

    #[cfg(test)]
    pub fn mode_is_prompting(&self) -> bool {
        matches!(self.inner.lock().mode, UiMode::Prompting { .. })
    }
}

impl TerminalUiInner {
    fn redraw_prompt(&mut self) {
        let UiMode::Prompting { buffer } = &self.mode else {
            return;
        };
        self.backend.clear_current_line();
        self.backend.move_to_column0();
        self.backend
            .write_stderr(&format!("{PROMPT_PREFIX}{buffer}"));
    }
}

#[cfg(test)]
struct SharedCaptureBackend {
    inner: Arc<Mutex<CaptureBackend>>,
}

#[cfg(test)]
impl TerminalBackend for SharedCaptureBackend {
    fn write_stdout(&mut self, s: &str) {
        self.inner.lock().write_stdout(s);
    }

    fn write_stderr(&mut self, s: &str) {
        self.inner.lock().write_stderr(s);
    }

    fn flush_all(&mut self) {
        self.inner.lock().flush_all();
    }

    fn clear_current_line(&mut self) {
        self.inner.lock().clear_current_line();
    }

    fn move_cursor_up(&mut self, n: usize) {
        self.inner.lock().move_cursor_up(n);
    }

    fn move_to_column0(&mut self) {
        self.inner.lock().move_to_column0();
    }

    fn is_interactive(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writeln_during_prompt_inserts_above_and_keeps_buffer() {
        let (ui, capture) = TerminalUi::with_capture();
        ui.set_prompt_buffer_for_test("hello");
        ui.writeln("post-turn log");

        let cap = capture.lock();
        assert!(cap.stderr.contains("post-turn log"));
        // Final prompt redraw must still show the in-progress input.
        assert!(
            cap.stderr.ends_with(">: hello") || cap.stderr.contains(">: hello"),
            "stderr={:?}",
            cap.stderr
        );
        let clear_before_log = cap
            .ops
            .iter()
            .position(|op| matches!(op, CaptureOp::ClearCurrentLine))
            .expect("clear");
        let log_pos = cap
            .ops
            .iter()
            .position(|op| matches!(op, CaptureOp::Stderr(s) if s.contains("post-turn log")))
            .expect("log");
        assert!(clear_before_log < log_pos);
    }

    #[test]
    fn replace_block_rewrites_in_place() {
        let (ui, capture) = TerminalUi::with_capture();
        let n = ui.replace_block(0, &["|- A".into(), "|- B".into()]);
        assert_eq!(n, 2);
        let n = ui.replace_block(2, &["|- A".into(), "| |- A1".into(), "└ B".into()]);
        assert_eq!(n, 3);
        let cap = capture.lock();
        assert!(cap.ops.iter().any(|op| matches!(op, CaptureOp::MoveUp(2))));
    }
}

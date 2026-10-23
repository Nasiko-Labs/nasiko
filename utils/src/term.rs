//! Live one-line terminal status animation, one-shot success/info/warning/error lines, and
//! color helpers.
//! Shared by `nasiko` (oss/cli) and `nasiko-ee` so both CLIs get the
//! same look and feel.
//!
//! ```ignore
//! let _handle = status::start_status("working");
//! // … do work …
//! // handle is dropped here → line is cleared
//! ```
//!
//! Animations go to stderr so piped/scripted stdout stays clean. When stderr
//! is not a TTY nothing animates (a single static line is printed).

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Overwrites the current terminal line with `msg` (no newline).
pub fn print_status(msg: &str) {
    // \x1b[0m resets any color left from a previous animation frame before we
    // erase the line, so a partial escape sequence can never taint future output.
    eprint!("\r\x1b[0m\x1b[2K{msg}");
    let _ = std::io::stderr().flush();
}

/// Erases the current terminal status line written by [`print_status`].
pub fn clear_status() {
    eprint!("\r\x1b[0m\x1b[2K");
    let _ = std::io::stderr().flush();
}

/// RAII guard returned by [`start_status`]. The status line is cleared
/// automatically when this is dropped, so a following `print_success`/
/// `print_warning` line lands cleanly.
pub struct StatusHandle {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for StatusHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // No thread → non-TTY mode where nothing was drawn; emitting the
        // clear sequence would leak escape codes into piped output.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
            clear_status();
        }
    }
}

/// Whether ANSI color/SGR codes should be emitted, per the `NO_COLOR` convention
/// (<https://no-color.org>). Shared so every caller that colors terminal output —
/// this module's own status/box helpers and `nasiko chat`'s HITL prompts — honors
/// the same env var consistently instead of each hardcoding its own check.
pub fn use_color() -> bool {
    std::env::var_os("NO_COLOR").is_none()
}

/// [`use_color`], and stdout is a real terminal — so output redirected to a file or piped into
/// another program stays plain text instead of carrying raw escape codes.
fn use_color_stdout() -> bool {
    use std::io::IsTerminal;
    use_color() && std::io::stdout().is_terminal()
}

/// Same as [`use_color_stdout`], for output written to stderr.
fn use_color_stderr() -> bool {
    use std::io::IsTerminal;
    use_color() && std::io::stderr().is_terminal()
}

/// Wraps `s` in the ANSI SGR `code` when `enabled`, otherwise returns it unchanged.
fn paint(s: &str, code: &str, enabled: bool) -> String {
    if enabled {
        format!("\x1b[{code}m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}

/// `s` in green, for text printed to stdout.
pub fn green(s: &str) -> String {
    paint(s, "32", use_color_stdout())
}

/// `s` in red, for text printed to stdout.
pub fn red(s: &str) -> String {
    paint(s, "31", use_color_stdout())
}

/// `s` in yellow, for text printed to stdout.
pub fn yellow(s: &str) -> String {
    paint(s, "33", use_color_stdout())
}

/// `s` dimmed, for secondary text printed to stdout.
pub fn dim(s: &str) -> String {
    paint(s, "2", use_color_stdout())
}

/// `s` in bold, for a heading printed to stdout.
pub fn bold(s: &str) -> String {
    paint(s, "1", use_color_stdout())
}

/// `s` in cyan, for text printed to stdout — used for next-step command hints.
pub fn cyan(s: &str) -> String {
    paint(s, "36", use_color_stdout())
}

/// Like [`cyan`], for text that ends up on stderr — e.g. inside an `anyhow::Error` message,
/// which the runtime prints to stderr.
pub fn cyan_stderr(s: &str) -> String {
    paint(s, "36", use_color_stderr())
}

/// Prints a green `✓ msg` line to stdout.
pub fn print_success(msg: &str) {
    println!("{} {msg}", paint("✓", "32", use_color_stdout()));
}

/// Prints a cyan `ℹ msg` line to stdout.
pub fn print_info(msg: &str) {
    println!("{} {msg}", paint("ℹ", "36", use_color_stdout()));
}

/// Like [`print_info`], on stderr — for a side note that must not mix into a command's output.
pub fn print_note(msg: &str) {
    eprintln!("{} {msg}", paint("ℹ", "36", use_color_stderr()));
}

/// Prints a yellow `⚠ msg` warning line to stderr.
pub fn print_warning(msg: &str) {
    eprintln!("{} {msg}", paint("⚠", "33", use_color_stderr()));
}

/// Prints a red `✗ msg` error line to stderr.
pub fn print_error(msg: &str) {
    eprintln!("{} {msg}", paint("✗", "31", use_color_stderr()));
}

/// Shows a live one-line status with a colored shimmer bar and elapsed time.
/// The line is cleared automatically when the returned handle is dropped.
pub fn start_status(msg: impl Into<String>) -> StatusHandle {
    use std::io::IsTerminal;

    let msg = msg.into();
    let stop = Arc::new(AtomicBool::new(false));

    if !std::io::stderr().is_terminal() {
        return StatusHandle { stop, thread: None };
    }

    let thread_stop = Arc::clone(&stop);
    let thread = std::thread::spawn(move || {
        let started = Instant::now();
        let mut frame = 0usize;
        let use_color = use_color();

        while !thread_stop.load(Ordering::Relaxed) {
            print_status(&render_frame(frame, &msg, started.elapsed(), use_color));
            frame += 1;
            std::thread::sleep(Duration::from_millis(70));
        }
    });

    StatusHandle {
        stop,
        thread: Some(thread),
    }
}

fn render_frame(frame: usize, msg: &str, elapsed: Duration, use_color: bool) -> String {
    let elapsed = format_elapsed(elapsed);
    let width = shimmer_width();
    // overhead: bar(width) + " "(1) + " ("(2) + ")"(1)
    format!(
        "{} {} \x1b[2m({elapsed})\x1b[0m",
        shimmer_bar(frame, width, use_color),
        fit_msg(msg, width + 4 + elapsed.len())
    )
}

/// Shimmer bar width scaled to the terminal: ~1/8 of the columns, clamped to
/// 8–16 so it stays compact on wide terminals and usable on narrow ones.
fn shimmer_width() -> usize {
    (terminal_cols() / 8).clamp(8, 16)
}

/// Bright peak with a block-gradient halo sweeping left-to-right, in one cyan
/// hue. Block-drawing glyphs (unlike braille dot-patterns) fill the full cell
/// height uniformly, so the bar sits cleanly on the text baseline in every
/// terminal font.
fn shimmer_bar(frame: usize, width: usize, use_color: bool) -> String {
    let pos = frame % width;
    let mut bar = String::new();
    for idx in 0..width {
        // wrap-around distance so the peak re-enters smoothly from the left edge
        let dist = {
            let d = (idx as isize - pos as isize).unsigned_abs();
            d.min(width - d)
        };
        let ch = match dist {
            0 => '█',
            1 => '▓',
            2 => '▒',
            3 => '░',
            _ => '·',
        };
        if use_color {
            let code = match dist {
                0 => "\x1b[96;1m",     // bright cyan, bold
                1 => "\x1b[36m",       // cyan
                2 | 3 => "\x1b[36;2m", // dim cyan
                _ => "\x1b[2m",        // dim default
            };
            bar.push_str(code);
            bar.push(ch);
            bar.push_str("\x1b[0m");
        } else {
            bar.push(ch);
        }
    }
    bar
}

fn format_elapsed(duration: Duration) -> String {
    let deciseconds = duration.as_millis() / 100;
    let minutes = deciseconds / 600;
    let seconds = deciseconds % 600;
    if minutes == 0 {
        format!("{}.{:01}s", seconds / 10, seconds % 10)
    } else {
        format!("{minutes}m {}.{:01}s", seconds / 10, seconds % 10)
    }
}

/// Visible width of `s`, skipping ANSI SGR (`\x1b[...m`) and OSC 8 hyperlink
/// (`\x1b]8;;url\x1b\\...\x1b]8;;\x1b\\`) escape sequences — both appear in box content (colored
/// labels, `nasiko chat`'s linkified replies) and would otherwise throw off [`print_box`]'s
/// padding and word-wrap width, which must size by what's actually on screen, not byte length.
/// Counts by `char`, not real display width, so CJK/fullwidth/emoji (which render 2 columns
/// wide in most terminals) undercount and can throw off box alignment — not worth pulling in
/// `unicode-width` for; known ceiling, not a bug to chase.
pub fn visible_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            width += 1;
            continue;
        }
        match chars.peek() {
            Some('[') => {
                chars.next();
                for c2 in chars.by_ref() {
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                chars.next();
                // OSC sequence: consume through its terminator, BEL or ST (`\x1b\\`).
                while let Some(c2) = chars.next() {
                    if c2 == '\x07' {
                        break;
                    }
                    if c2 == '\x1b' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    width
}

/// Word-wraps `text` to `width` visible columns ([`visible_width`]), splitting on spaces and
/// treating existing newlines as hard breaks. [`print_box`]'s only caller.
fn wrap_to_width(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut current = String::new();
        let mut current_width = 0;
        for word in paragraph.split(' ') {
            let word_width = visible_width(word);
            if !current.is_empty() && current_width + 1 + word_width > width {
                lines.push(std::mem::take(&mut current));
                current_width = 0;
            }
            if !current.is_empty() {
                current.push(' ');
                current_width += 1;
            }
            current.push_str(word);
            current_width += word_width;
        }
        lines.push(current);
    }
    lines
}

/// Draws a bordered box around `body` to stderr, with an optional `title` set into the top
/// border — the visual anchor for anything that should stand out from the surrounding stream:
/// `nasiko chat`'s sub-agent tool-call announcements/replies, and its HITL pause prompts.
/// `color` is the ANSI SGR code (e.g. `"36"` cyan, `"33"` yellow) applied to the border and
/// title; `body` may already carry its own ANSI colors or OSC 8 hyperlinks — sizing and
/// wrapping account for that via [`visible_width`]. Width comes from the real terminal (same
/// source [`start_status`] uses), clamped so a huge terminal doesn't produce an absurdly wide
/// box and a narrow one still fits a full sentence. Respects `NO_COLOR` like the rest of this
/// module, and draws an uncolored border when stderr is not a terminal.
pub fn print_box(title: Option<&str>, body: &str, color: &str) {
    let line_width = terminal_cols().clamp(40, 92).saturating_sub(2);
    let content_width = line_width.saturating_sub(4).max(10);

    let (open, close) = if use_color_stderr() {
        (format!("\x1b[{color}m"), "\x1b[0m".to_string())
    } else {
        (String::new(), String::new())
    };

    let top = match title {
        Some(t) => {
            let dashes = line_width.saturating_sub(visible_width(t) + 5).max(1);
            format!("┌─ {t} {}┐", "─".repeat(dashes))
        }
        None => format!("┌{}┐", "─".repeat(line_width.saturating_sub(2))),
    };
    eprintln!("{open}{top}{close}");

    for raw_line in body.split('\n') {
        for line in wrap_to_width(raw_line, content_width) {
            let pad = content_width.saturating_sub(visible_width(&line));
            eprintln!("{open}│{close} {line}{} {open}│{close}", " ".repeat(pad));
        }
    }
    eprintln!(
        "{open}└{}┘{close}",
        "─".repeat(line_width.saturating_sub(2))
    );
}

/// Reads one line of free-text input from a raw-mode terminal, with a full-redraw scheme
/// (relative cursor movement, `prev_rows`-tracked, same technique as `nasiko`'s HITL combo-select
/// widget) that stays correct once the buffer wraps past one physical terminal row — unlike
/// `dialoguer::Input`, whose own incremental redraw desyncs at that point (confirmed live:
/// typing a message long enough to wrap showed the prompt rendered twice, one frame truncated).
/// `prompt` may carry ANSI color; `": "` is appended to match `dialoguer`'s own default
/// formatting. Returns `Err` on Ctrl+C, Esc, or a non-interactive terminal — the same contract as
/// `dialoguer::Input::interact_text()` — so callers written as `while let Ok(input) = ...` keep
/// exiting their loop gracefully.
pub fn read_text_line(prompt: &str) -> anyhow::Result<String> {
    use anyhow::Context;
    use crossterm::cursor::{MoveToColumn, MoveUp};
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    use crossterm::execute;
    use crossterm::terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode};
    use std::io::stderr;

    enable_raw_mode().context("this prompt needs an interactive terminal to answer")?;
    struct RawGuard;
    impl Drop for RawGuard {
        fn drop(&mut self) {
            let _ = disable_raw_mode();
        }
    }
    let _guard = RawGuard;

    let mut buffer = String::new();
    let mut out = stderr();
    let mut prev_rows = 0usize;

    loop {
        let cols = terminal_cols();
        // `prev_rows - 1` is how many rows to climb back to this line's first physical row
        // (no trailing newline is printed, so the cursor sits on the *same* row as the last
        // char written, unlike the combo-select widget's per-row-newline scheme). Only
        // execute a `MoveUp` when that's actually positive: `MoveUp(0)` is not a no-op —
        // ANSI's CUU defaults a 0 parameter to 1 (confirmed against both a real terminal and
        // `pyte`), so emitting it here silently walked the cursor up one extra row per
        // keystroke and clobbered whatever was already on screen above the prompt.
        let rows_up = prev_rows.saturating_sub(1);
        if rows_up > 0 {
            execute!(
                out,
                MoveUp(rows_up.min(u16::MAX as usize) as u16),
                MoveToColumn(0),
                Clear(ClearType::FromCursorDown)
            )?;
        } else {
            // `prev_rows` is 0 (first frame) or 1 (single row so far) — either way the cursor
            // is already on the line's only row, so no vertical move is needed at all.
            execute!(out, MoveToColumn(0), Clear(ClearType::FromCursorDown))?;
        }

        let line = format!("{prompt}: {buffer}");
        write!(out, "{line}")?;
        out.flush()?;
        prev_rows = visible_width(&line).max(1).div_ceil(cols);

        match read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    write!(out, "\r\n")?;
                    out.flush()?;
                    anyhow::bail!("cancelled");
                }
                KeyCode::Enter => {
                    write!(out, "\r\n")?;
                    out.flush()?;
                    return Ok(buffer);
                }
                KeyCode::Backspace => {
                    buffer.pop();
                }
                KeyCode::Esc => anyhow::bail!("cancelled"),
                KeyCode::Char(c) => buffer.push(c),
                _ => {}
            },
            _ => {}
        }
    }
}

/// Returns the terminal column count, falling back to `$COLUMNS`, then 80.
pub fn terminal_cols() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(w, _)| w as usize)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|s| s.parse::<usize>().ok())
        })
        .filter(|&n| n >= 20)
        .unwrap_or(80)
}

/// Truncates `msg` so the full rendered line stays within [`terminal_cols`].
/// `fixed_overhead` is the total visible column count of everything *except*
/// the message. Truncated messages get a `…` suffix.
fn fit_msg(msg: &str, fixed_overhead: usize) -> String {
    let cols = terminal_cols().saturating_sub(1);
    if fixed_overhead >= cols {
        return String::new();
    }
    let budget = cols - fixed_overhead;
    let char_count = msg.chars().count();
    if char_count <= budget {
        return msg.to_owned();
    }
    if budget <= 1 {
        return "…".to_string();
    }

    let visible_chars = budget - 1;
    let prefix: String = msg.chars().take(visible_chars).collect();
    format!("{prefix}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_formats_seconds_and_minutes() {
        assert_eq!(format_elapsed(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_elapsed(Duration::from_secs(75)), "1m 15.0s");
    }

    #[test]
    fn fit_msg_truncates_with_ellipsis() {
        // terminal_cols falls back to >= 20, so a huge message always truncates
        let long = "x".repeat(500);
        let fitted = fit_msg(&long, 5);
        assert!(fitted.ends_with('…'));
        assert!(fitted.chars().count() < 500);
    }

    #[test]
    fn visible_width_ignores_sgr_codes() {
        assert_eq!(visible_width("\x1b[1;36mhello\x1b[0m"), 5);
    }

    #[test]
    fn visible_width_ignores_osc8_hyperlinks() {
        let hyperlink = "\x1b]8;;https://example.com\x1b\\\x1b[34;4mhere\x1b[0m\x1b]8;;\x1b\\";
        assert_eq!(visible_width(hyperlink), 4);
    }

    #[test]
    fn visible_width_plain_text_is_char_count() {
        assert_eq!(visible_width("plain text"), 10);
    }

    #[test]
    fn wrap_to_width_breaks_on_spaces_within_budget() {
        let wrapped = wrap_to_width("one two three four", 8);
        assert_eq!(wrapped, vec!["one two", "three", "four"]);
    }

    #[test]
    fn wrap_to_width_preserves_existing_newlines_as_hard_breaks() {
        let wrapped = wrap_to_width("first line\nsecond line", 80);
        assert_eq!(wrapped, vec!["first line", "second line"]);
    }

    #[test]
    fn paint_wraps_text_in_the_sgr_code_when_enabled() {
        assert_eq!(paint("ok", "32", true), "\x1b[32mok\x1b[0m");
    }

    #[test]
    fn paint_leaves_text_plain_when_disabled() {
        assert_eq!(paint("ok", "32", false), "ok");
    }

    #[test]
    fn wrap_to_width_never_splits_a_single_long_word() {
        let word = "x".repeat(20);
        let wrapped = wrap_to_width(&word, 8);
        assert_eq!(wrapped, vec![word]);
    }
}

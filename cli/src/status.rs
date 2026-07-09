//! Live one-line terminal status animations.
//!
//! ```ignore
//! let _handle = status::start_status("working");
//! // … do work …
//! // handle is dropped here → line is cleared
//! ```
//!
//! All output goes to stderr so piped/scripted stdout stays clean. When
//! stderr is not a TTY nothing animates (a single static line is printed).

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

/// RAII guard returned by the `start_status*` functions.
/// The status line is cleared automatically when this is dropped.
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

#[derive(Debug, Clone, Copy)]
pub enum StatusAnimation {
    /// Unicode braille spinner: `⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏`
    Braille,
    /// Bright peak with a block-gradient halo sweeping left-to-right.
    Shimmer,
}

/// Shows a live one-line status with a braille spinner and elapsed time.
/// The line is cleared automatically when the returned handle is dropped.
pub fn start_status(msg: impl Into<String>) -> StatusHandle {
    start_status_with_animation(msg, StatusAnimation::Braille)
}

/// Shows a live one-line status using the selected animation style.
pub fn start_status_with_animation(
    msg: impl Into<String>,
    animation: StatusAnimation,
) -> StatusHandle {
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
        let use_color = std::env::var_os("NO_COLOR").is_none();

        while !thread_stop.load(Ordering::Relaxed) {
            print_status(&render_frame(
                animation,
                frame,
                &msg,
                started.elapsed(),
                use_color,
            ));
            frame += 1;
            std::thread::sleep(Duration::from_millis(frame_ms(animation)));
        }
    });

    StatusHandle {
        stop,
        thread: Some(thread),
    }
}

fn render_frame(
    animation: StatusAnimation,
    frame: usize,
    msg: &str,
    elapsed: Duration,
    use_color: bool,
) -> String {
    let elapsed = format_elapsed(elapsed);
    // Each arm pre-computes the visual overhead of its fixed decoration and
    // clamps the message so the total never wraps past the terminal width.
    match animation {
        StatusAnimation::Braille => {
            let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let pfx = frames[frame % frames.len()];
            let pfx = if use_color {
                format!("\x1b[36m{pfx}\x1b[0m")
            } else {
                pfx.to_string()
            };
            // overhead: spinner(1) + " "(1) + " ("(2) + ")"(1) = 5
            format!(
                "{pfx} {} \x1b[2m({elapsed})\x1b[0m",
                fit_msg(msg, 5 + elapsed.len())
            )
        }
        StatusAnimation::Shimmer => {
            // overhead: bar(28) + " "(1) + " ("(2) + ")"(1) = 32
            format!(
                "{} {} \x1b[2m({elapsed})\x1b[0m",
                shimmer_bar(frame, use_color),
                fit_msg(msg, 32 + elapsed.len())
            )
        }
    }
}

fn frame_ms(animation: StatusAnimation) -> u64 {
    match animation {
        StatusAnimation::Shimmer => 70,
        StatusAnimation::Braille => 90,
    }
}

/// Bright peak with a block-gradient halo sweeping left-to-right, in one cyan hue.
fn shimmer_bar(frame: usize, use_color: bool) -> String {
    const WIDTH: usize = 28;
    let pos = frame % WIDTH;
    let mut bar = String::new();
    for idx in 0..WIDTH {
        // wrap-around distance so the peak re-enters smoothly from the left edge
        let dist = {
            let d = (idx as isize - pos as isize).unsigned_abs();
            d.min(WIDTH - d)
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

/// Returns the terminal column count, falling back to `$COLUMNS`, then 80.
fn terminal_cols() -> usize {
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
}

mod app;
mod event;
pub mod session;
mod ui;

use std::io;

use anyhow::{Context, Result};
use crossterm::{
    event::{KeyCode, KeyModifiers},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::CrosstermBackend;

use app::{App, AppMode};
use event::{AppEvent, EventLoop};

pub fn run_tui(endpoint: &str, resume_id: Option<&str>) -> Result<()> {
    let (session, history) = if let Some(id) = resume_id {
        session::resume_session(id, endpoint)?
    } else {
        (session::start_session(endpoint)?, Vec::new())
    };

    terminal::enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = ratatui::Terminal::new(backend)?;

    let (event_loop, event_tx) = EventLoop::new();
    let mut app = App::new(session, event_tx);

    if !history.is_empty() {
        app.load_history(history);
    }

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        if let Some(ev) = event_loop.next() {
            match ev {
                AppEvent::Key(key) => {
                    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                        break;
                    }
                    match app.mode {
                        AppMode::Input => handle_input_key(&mut app, key),
                        AppMode::Scroll => handle_scroll_key(&mut app, key),
                    }
                }
                AppEvent::StreamToken(token) => app.handle_stream_token(token),
                AppEvent::StreamStatus(status) => app.handle_stream_status(status),
                AppEvent::StreamDone => app.handle_stream_done(),
                AppEvent::StreamError(err) => app.handle_stream_error(err),
                AppEvent::Tick => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    terminal::disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    println!("Session: {}", app.session.id);
    Ok(())
}

fn handle_input_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::Enter => app.submit_message(),
        KeyCode::Char(c) => app.insert_char(c),
        KeyCode::Backspace => app.delete_char_before(),
        KeyCode::Delete => app.delete_char_after(),
        KeyCode::Left => app.move_cursor_left(),
        KeyCode::Right => app.move_cursor_right(),
        KeyCode::Home => app.move_cursor_home(),
        KeyCode::End => app.move_cursor_end(),
        KeyCode::PageUp => {
            app.mode = AppMode::Scroll;
            app.scroll_up();
        }
        KeyCode::PageDown => app.scroll_down(),
        KeyCode::Esc => {
            if app.streaming {
                // do nothing while streaming
            } else {
                app.should_quit = true;
            }
        }
        _ => {}
    }
}

fn handle_scroll_key(app: &mut App, key: crossterm::event::KeyEvent) {
    match key.code {
        KeyCode::PageUp | KeyCode::Up => app.scroll_up(),
        KeyCode::PageDown | KeyCode::Down => app.scroll_down(),
        KeyCode::Esc | KeyCode::Char('i') | KeyCode::Char('q') => {
            app.mode = AppMode::Input;
            app.scroll_offset = 0;
        }
        _ => {
            app.mode = AppMode::Input;
            app.scroll_offset = 0;
        }
    }
}

// ─── Session management commands ────────────────────────────────────────────

pub fn create_session(agent_url: Option<&str>) -> Result<()> {
    let base_url = crate::config::active_url().context("no active cluster — run `nasiko connect <url>`")?;
    let token = crate::config::active_token()
        .context("config error")?
        .ok_or_else(|| anyhow::anyhow!("not logged in — run `nasiko auth login`"))?;
    let s = session::create_cp_session(&base_url, &token, agent_url.unwrap_or(""), "New session")?;
    println!("{}", s.session_id);
    Ok(())
}

pub fn session_history(session_id: &str) -> Result<()> {
    let base_url = crate::config::active_url().context("no active cluster")?;
    let token = crate::config::active_token()
        .context("config error")?
        .ok_or_else(|| anyhow::anyhow!("not logged in — run `nasiko auth login`"))?;
    let messages = session::fetch_cp_messages(&base_url, &token, session_id)?;
    if messages.is_empty() {
        println!("No messages in session '{session_id}'.");
        return Ok(());
    }
    for msg in messages {
        println!("[{}] {}", msg.role, msg.content);
    }
    Ok(())
}

pub fn delete_session(session_id: &str, yes: bool) -> Result<()> {
    if !yes {
        let confirmed = dialoguer::Confirm::new()
            .with_prompt(format!("Delete session '{session_id}'?"))
            .default(false)
            .interact()?;
        if !confirmed {
            println!("Cancelled.");
            return Ok(());
        }
    }
    let base_url = crate::config::active_url().context("no active cluster")?;
    let token = crate::config::active_token()
        .context("config error")?
        .ok_or_else(|| anyhow::anyhow!("not logged in — run `nasiko auth login`"))?;
    session::delete_cp_session(&base_url, &token, session_id)?;
    println!("Session '{session_id}' deleted.");
    Ok(())
}

// ─── Sessions list command ──────────────────────────────────────────────────

pub fn list_sessions(endpoint: Option<&str>) -> Result<()> {
    if let Some(endpoint) = endpoint
        && let Some((base_url, token)) = session::cp_credentials(endpoint) {
            let sessions = session::list_cp_sessions(&base_url, &token)?;
            if sessions.is_empty() {
                println!("No sessions found.");
                return Ok(());
            }
            println!("{:<36} {:<20} {:<24} TITLE", "ID", "AGENT", "UPDATED");
            for s in sessions {
                println!(
                    "{:<36} {:<20} {:<24} {}",
                    s.session_id,
                    s.agent_name.as_deref().unwrap_or("-"),
                    s.updated_at.get(..19).unwrap_or(&s.updated_at),
                    s.title,
                );
            }
            return Ok(());
        }

    let sessions = session::list_local_sessions()?;
    if sessions.is_empty() {
        println!("No local sessions found.");
        return Ok(());
    }
    println!("{:<36} {:<40} {:<24} TITLE", "ID", "ENDPOINT", "CREATED");
    for s in sessions {
        println!(
            "{:<36} {:<40} {:<24} {}",
            s.id,
            if s.endpoint.len() > 38 {
                let n = s.endpoint.floor_char_boundary(35);
                format!("{}...", &s.endpoint[..n])
            } else {
                s.endpoint.clone()
            },
            s.created_at.get(..19).unwrap_or(&s.created_at),
            s.title,
        );
    }
    Ok(())
}

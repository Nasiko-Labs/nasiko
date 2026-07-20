use std::sync::mpsc;

use super::event::{self, AppEvent, StatusEvent, StatusKind};
use super::session::{self, LocalMessage, Session, SessionBackend};

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    User,
    Agent,
    Status,
}

#[derive(Debug, PartialEq)]
pub enum AppMode {
    Input,
    Scroll,
}

pub struct App {
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub cursor_pos: usize,
    pub scroll_offset: u16,
    pub mode: AppMode,
    pub session: Session,
    pub streaming: bool,
    pub current_agent_text: String,
    pub status_line: Option<StatusEvent>,
    pub should_quit: bool,
    event_tx: mpsc::Sender<AppEvent>,
}

impl App {
    pub fn new(session: Session, event_tx: mpsc::Sender<AppEvent>) -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            mode: AppMode::Input,
            session,
            streaming: false,
            current_agent_text: String::new(),
            status_line: None,
            should_quit: false,
            event_tx,
        }
    }

    pub fn load_history(&mut self, history: Vec<(String, String)>) {
        for (role, text) in history {
            let role = match role.as_str() {
                "user" => Role::User,
                _ => Role::Agent,
            };
            self.messages.push(ChatMessage { role, text });
        }
    }

    pub fn submit_message(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.streaming {
            return;
        }

        if text == "/quit" || text == "/exit" {
            self.should_quit = true;
            return;
        }

        self.messages.push(ChatMessage {
            role: Role::User,
            text: text.clone(),
        });
        self.input.clear();
        self.cursor_pos = 0;
        self.streaming = true;
        self.current_agent_text.clear();
        self.status_line = None;
        self.scroll_offset = 0;

        self.persist_user_message(&text);

        // W3C trace_id for the exchange (hex, 32 chars) — becomes the
        // traceparent header so the whole exchange lands in one Tempo trace.
        let trace_id = uuid::Uuid::new_v4().to_string().replace('-', "");
        event::send_message_streaming(&self.session, &text, trace_id, self.event_tx.clone());
    }

    pub fn handle_stream_token(&mut self, token: String) {
        self.current_agent_text.push_str(&token);
    }

    pub fn handle_stream_status(&mut self, status: StatusEvent) {
        self.status_line = Some(status);
    }

    pub fn handle_stream_done(&mut self) {
        self.streaming = false;
        if !self.current_agent_text.is_empty() {
            let text = std::mem::take(&mut self.current_agent_text);
            self.persist_agent_message(&text);
            self.messages.push(ChatMessage {
                role: Role::Agent,
                text,
            });
        }
        self.status_line = None;
    }

    pub fn handle_stream_error(&mut self, err: String) {
        self.messages.push(ChatMessage {
            role: Role::Status,
            text: format!("error: {err}"),
        });
    }

    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    pub fn delete_char_before(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.input[..self.cursor_pos]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.input.remove(prev);
            self.cursor_pos = prev;
        }
    }

    pub fn delete_char_after(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.input.remove(self.cursor_pos);
        }
    }

    pub fn move_cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos = self.input[..self.cursor_pos]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn move_cursor_right(&mut self) {
        if self.cursor_pos < self.input.len() {
            self.cursor_pos = self.input[self.cursor_pos..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor_pos + i)
                .unwrap_or(self.input.len());
        }
    }

    pub fn move_cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    pub fn move_cursor_end(&mut self) {
        self.cursor_pos = self.input.len();
    }

    pub fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(3);
    }

    pub fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(3);
    }

    fn persist_user_message(&self, text: &str) {
        match &self.session.backend {
            SessionBackend::Cp {
                base_url,
                token,
                session_id,
            } => {
                let _ = session::post_cp_message(base_url, token, session_id, "user", text);
            }
            SessionBackend::Local => {
                if let Ok(mut local) = session::load_local_session(&self.session.id) {
                    local.messages.push(LocalMessage {
                        role: "user".to_string(),
                        text: text.to_string(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    });
                    let _ = session::save_local_session(&local);
                }
            }
        }
    }

    fn persist_agent_message(&self, text: &str) {
        match &self.session.backend {
            SessionBackend::Cp {
                base_url,
                token,
                session_id,
            } => {
                let _ = session::post_cp_message(base_url, token, session_id, "assistant", text);
            }
            SessionBackend::Local => {
                if let Ok(mut local) = session::load_local_session(&self.session.id) {
                    local.messages.push(LocalMessage {
                        role: "agent".to_string(),
                        text: text.to_string(),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    });
                    let _ = session::save_local_session(&local);
                }
            }
        }
    }

    pub fn status_text(&self) -> String {
        if self.streaming {
            if let Some(ref status) = self.status_line {
                match &status.kind {
                    StatusKind::Thinking => format!("thinking: {}", status.text),
                    StatusKind::ToolCall => format!("-> {}", status.text),
                    StatusKind::ToolResult { success } => {
                        let icon = if *success { "ok" } else { "err" };
                        format!("[{icon}] {}", status.text)
                    }
                    StatusKind::Working => status.text.clone(),
                    StatusKind::Failed => format!("FAILED: {}", status.text),
                }
            } else {
                "receiving...".to_string()
            }
        } else {
            format!(
                "{} ({}) | /quit to exit | PageUp/Down to scroll",
                self.session.title,
                &self.session.id[..8]
            )
        }
    }
}

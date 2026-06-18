use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::app::{App, AppMode, ChatMessage, Role};

pub fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([
        Constraint::Min(6),
        Constraint::Length(3),
        Constraint::Length(3),
    ])
    .split(frame.area());

    draw_messages(frame, app, chunks[0]);
    draw_status_bar(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
}

fn draw_messages(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    for msg in &app.messages {
        render_message(&mut lines, msg);
        lines.push(Line::raw(""));
    }

    if app.streaming && !app.current_agent_text.is_empty() {
        let streaming_msg = ChatMessage {
            role: Role::Agent,
            text: app.current_agent_text.clone(),
        };
        render_message(&mut lines, &streaming_msg);
        lines.push(Line::raw(""));
    }

    let total_lines = lines.len() as u16;
    let visible_height = area.height.saturating_sub(2);
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = if app.scroll_offset == 0 {
        max_scroll
    } else {
        max_scroll.saturating_sub(app.scroll_offset)
    };

    let text = Text::from(lines);
    let messages_widget = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Chat ")
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(messages_widget, area);
}

fn render_message<'a>(lines: &mut Vec<Line<'a>>, msg: &ChatMessage) {
    match msg.role {
        Role::User => {
            lines.push(Line::from(vec![
                Span::styled("you", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(": "),
            ]));
            for text_line in msg.text.lines() {
                lines.push(Line::from(Span::raw(text_line.to_string())));
            }
        }
        Role::Agent => {
            lines.push(Line::from(vec![
                Span::styled(
                    "agent",
                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                ),
                Span::raw(": "),
            ]));
            for text_line in msg.text.lines() {
                lines.push(Line::from(Span::raw(text_line.to_string())));
            }
        }
        Role::Status => {
            lines.push(Line::from(Span::styled(
                msg.text.clone(),
                Style::default().fg(Color::Red),
            )));
        }
    }
}

fn draw_status_bar(frame: &mut Frame, app: &App, area: Rect) {
    let status_text = app.status_text();
    let style = if app.streaming {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let status = Paragraph::new(Line::from(Span::styled(status_text, style)))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );

    frame.render_widget(status, area);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = match app.mode {
        AppMode::Input if !app.streaming => Color::Cyan,
        _ => Color::DarkGray,
    };

    let input_widget = Paragraph::new(Line::from(app.input.as_str())).block(
        Block::default()
            .borders(Borders::ALL)
            .title(if app.streaming { " ... " } else { " > " })
            .border_style(Style::default().fg(border_color)),
    );

    frame.render_widget(input_widget, area);

    if app.mode == AppMode::Input && !app.streaming {
        let cursor_x = area.x + 1 + app.cursor_pos as u16;
        let cursor_y = area.y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }
}

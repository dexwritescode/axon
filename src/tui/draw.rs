use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

use crate::app::{App, ChatMessage};

pub fn draw(frame: &mut Frame, app: &App) {
    let [conv_area, input_area, status_area] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let lines: Vec<Line> = app
        .messages
        .iter()
        .map(|msg| match msg {
            ChatMessage::User(text) => Line::from(vec![
                Span::styled("You  ", Style::default().fg(Color::Green)),
                Span::raw(text.as_str()),
            ]),
            ChatMessage::Agent(text) => Line::from(vec![
                Span::styled("Agent", Style::default().fg(Color::Cyan)),
                Span::raw(format!(" {text}")),
            ]),
            ChatMessage::ToolCall { name, args } => Line::from(vec![
                Span::styled("  →  ", Style::default().fg(Color::Yellow)),
                Span::styled(name.as_str(), Style::default().fg(Color::Yellow)),
                Span::raw(format!("  {args}")),
            ]),
            ChatMessage::ToolResult { name, content } => Line::from(vec![
                Span::styled("  ←  ", Style::default().fg(Color::Magenta)),
                Span::styled(name.as_str(), Style::default().fg(Color::Magenta)),
                Span::raw(format!("  {content}")),
            ]),
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(" Axon "))
            .wrap(Wrap { trim: false }),
        conv_area,
    );

    frame.render_widget(
        Paragraph::new(format!(" > {}", app.input)).block(Block::bordered()),
        input_area,
    );

    let backend = &app.config.backend;
    let model = if backend.model.is_empty() {
        "(no model set)".to_string()
    } else {
        backend.model.clone()
    };
    frame.render_widget(
        Paragraph::new(format!(
            "  {}  │  {}  │  tools: {}",
            backend.base_url,
            model,
            app.config.tool_approval.as_str()
        )),
        status_area,
    );
}

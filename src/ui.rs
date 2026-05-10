use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    text::Line,
    widgets::{Block, Paragraph},
};

use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App) {
    let [main_area, status_area] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(frame.area());

    let [chat_area, tree_area] =
        Layout::horizontal([Constraint::Percentage(75), Constraint::Percentage(25)])
            .areas(main_area);

    frame.render_widget(Block::bordered().title(" Chat "), chat_area);
    frame.render_widget(Block::bordered().title(" Files "), tree_area);

    let backend = &app.config.backend;
    let model = if backend.model.is_empty() {
        "(no model set)".to_string()
    } else {
        backend.model.clone()
    };
    let approval = app.config.tool_approval.as_str();
    let status_text = format!(
        "  {}  │  {}  │  tools: {}",
        backend.base_url, model, approval
    );
    frame.render_widget(
        Paragraph::new(Line::from(status_text)).block(Block::bordered().title(" Status ")),
        status_area,
    );
}

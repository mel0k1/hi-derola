use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, Kind, Phase};

pub const ACCENT: Color = Color::Rgb(122, 162, 247);
pub const DIM: Color = Color::Rgb(106, 115, 125);

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let rows = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .split(area);
    draw_messages(f, app, rows[0]);
    draw_input(f, app, rows[1]);
    draw_status(f, app, rows[2]);
}

fn draw_messages(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if app.entries.is_empty() {
        lines.push(Line::from(Span::styled(
            "hi-derola · type a message, /help for commands",
            Style::new().fg(DIM),
        )));
    }
    for e in &app.entries {
        match e.kind {
            Kind::You => {
                lines.push(Line::from(Span::styled(
                    "you",
                    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                )));
                push_body(&mut lines, &e.text);
            }
            Kind::Bot => {
                lines.push(Line::from(Span::styled(
                    "bot",
                    Style::new().fg(DIM).add_modifier(Modifier::BOLD),
                )));
                push_body(&mut lines, &e.text);
            }
            Kind::Info => {
                for l in e.text.lines() {
                    lines.push(Line::from(Span::styled(
                        format!("· {l}"),
                        Style::new().fg(DIM),
                    )));
                }
            }
        }
        lines.push(Line::from(""));
    }
    let total = wrapped_height(&lines, area.width);
    let view_h = area.height as usize;
    let offset = total.saturating_sub(view_h).saturating_sub(app.scroll_up);
    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((offset.min(u16::MAX as usize) as u16, 0));
    f.render_widget(p, area);
}

fn push_body(lines: &mut Vec<Line>, text: &str) {
    for l in text.lines() {
        lines.push(Line::from(format!("  {l}")));
    }
}

fn wrapped_height(lines: &[Line], width: u16) -> usize {
    let w = width.max(1) as usize;
    lines
        .iter()
        .map(|l| l.width().div_ceil(w).max(1))
        .sum()
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    match app.phase {
        Phase::Confirm => {
            let block = Block::new()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(ACCENT))
                .title(Span::styled("confirm", Style::new().fg(ACCENT)));
            let inner = block.inner(area);
            f.render_widget(block, area);
            let text = match &app.confirm {
                Some(c) => {
                    let args = match serde_json::from_str::<serde_json::Value>(&c.args) {
                        Ok(v) => v["command"]
                            .as_str()
                            .or_else(|| v["path"].as_str())
                            .unwrap_or("")
                            .to_string(),
                        Err(_) => c.args.clone(),
                    };
                    let args = args.lines().next().unwrap_or("");
                    let args = if args.chars().count() > 60 {
                        let t: String = args.chars().take(57).collect();
                        format!("{t}...")
                    } else {
                        args.to_string()
                    };
                    format!("{} {}\n[y] run  [n] skip  [a] allow all", c.name, args)
                }
                None => "...".into(),
            };
            f.render_widget(Paragraph::new(text), inner);
        }
        _ => {
            let title = match app.phase {
                Phase::Waiting => "input · waiting",
                _ => "input",
            };
            let block = Block::new()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(DIM))
                .title(Span::styled(title, Style::new().fg(ACCENT)));
            let inner = block.inner(area);
            f.render_widget(block, area);
            f.render_widget(Paragraph::new(app.input.as_str()), inner);
            let cursor_x = inner.x
                + app
                    .input
                    .chars()
                    .count()
                    .min(inner.width.saturating_sub(1) as usize)
                    as u16;
            f.set_cursor_position(ratatui::layout::Position::new(cursor_x, inner.y));
        }
    }
}

fn draw_status(f: &mut Frame, app: &App, area: Rect) {
    let hints = match app.phase {
        Phase::Waiting => "esc cancel",
        _ => "enter send · esc quit · /help",
    };
    let cols = Layout::horizontal([
        Constraint::Min(10),
        Constraint::Length(hints.len() as u16),
    ])
    .split(area);
    let left = Line::from(vec![
        Span::styled(
            " hi-derola",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {}", app.status), Style::new().fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(left), cols[0]);
    let right = Paragraph::new(Span::styled(hints, Style::new().fg(DIM)))
        .alignment(Alignment::Right);
    f.render_widget(right, cols[1]);
}

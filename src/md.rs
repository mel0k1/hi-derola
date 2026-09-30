use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::ui::{ACCENT, DIM};

const CODE_FG: Color = Color::Rgb(180, 190, 200);
const CODE_BG: Color = Color::Rgb(26, 30, 38);

fn code_style() -> Style {
    Style::new().fg(CODE_FG).bg(CODE_BG)
}

fn inline_code_style() -> Style {
    Style::new().fg(ACCENT)
}

fn flush(out: &mut Vec<Line<'static>>, cur: &mut Vec<Span<'static>>, quote: usize) {
    let prefix = "> ".repeat(quote);
    if cur.is_empty() {
        if prefix.is_empty() {
            out.push(Line::from(""));
        } else {
            out.push(Line::from(Span::styled(prefix, Style::new().fg(DIM))));
        }
        return;
    }
    let mut spans = Vec::new();
    if !prefix.is_empty() {
        spans.push(Span::styled(prefix, Style::new().fg(DIM)));
    }
    spans.append(cur);
    out.push(Line::from(spans));
}

pub fn render(src: &str) -> Vec<Line<'static>> {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TASKLISTS);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut style = Style::new();
    let mut in_code = false;
    let mut quote = 0usize;
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut link: Option<String> = None;

    for ev in Parser::new_ext(src, opts) {
        match ev {
            Event::Start(tag) => match tag {
                Tag::Heading { level, .. } => {
                    style = Style::new()
                        .fg(if matches!(level, HeadingLevel::H1 | HeadingLevel::H2) {
                            ACCENT
                        } else {
                            Color::Gray
                        })
                        .add_modifier(Modifier::BOLD);
                }
                Tag::BlockQuote(_) => quote += 1,
                Tag::CodeBlock(_) => in_code = true,
                Tag::List(start) => lists.push(start),
                Tag::Item => {
                    let mut marker = "- ".to_string();
                    if let Some(Some(c)) = lists.last_mut() {
                        marker = format!("{c}. ");
                        *c += 1;
                    }
                    let indent = "  ".repeat(lists.len().saturating_sub(1));
                    cur.push(Span::styled(format!("{indent}{marker}"), Style::new().fg(ACCENT)));
                }
                Tag::Strong => style = style.add_modifier(Modifier::BOLD),
                Tag::Emphasis => style = style.add_modifier(Modifier::ITALIC),
                Tag::Strikethrough => style = style.add_modifier(Modifier::CROSSED_OUT),
                Tag::Link { dest_url, .. } => link = Some(dest_url.to_string()),
                Tag::Image { dest_url, .. } => link = Some(dest_url.to_string()),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => flush(&mut out, &mut cur, quote),
                TagEnd::Heading(_) => {
                    flush(&mut out, &mut cur, quote);
                    style = Style::new();
                    out.push(Line::from(""));
                }
                TagEnd::BlockQuote(_) => quote = quote.saturating_sub(1),
                TagEnd::CodeBlock => {
                    in_code = false;
                    out.push(Line::from(""));
                }
                TagEnd::List(_) => {
                    lists.pop();
                }
                TagEnd::Item => flush(&mut out, &mut cur, quote),
                TagEnd::Strong => style = style.remove_modifier(Modifier::BOLD),
                TagEnd::Emphasis => style = style.remove_modifier(Modifier::ITALIC),
                TagEnd::Strikethrough => style = style.remove_modifier(Modifier::CROSSED_OUT),
                TagEnd::Link { .. } => {
                    if let Some(d) = link.take() {
                        if !d.starts_with('#') {
                            cur.push(Span::styled(format!(" ({d})"), Style::new().fg(DIM)));
                        }
                    }
                }
                TagEnd::Image { .. } => {
                    link = None;
                }
                _ => {}
            },
            Event::Text(t) => {
                if in_code {
                    for l in t.lines() {
                        out.push(Line::from(Span::styled(l.to_string(), code_style())));
                    }
                } else {
                    cur.push(Span::styled(t.to_string(), style));
                }
            }
            Event::Code(c) => cur.push(Span::styled(c.to_string(), inline_code_style())),
            Event::SoftBreak | Event::HardBreak => flush(&mut out, &mut cur, quote),
            Event::Rule => {
                flush(&mut out, &mut cur, quote);
                out.push(Line::from(Span::styled("────────────", Style::new().fg(DIM))));
            }
            Event::TaskListMarker(done) => {
                cur.push(Span::styled(
                    if done { "[x] " } else { "[ ] " },
                    Style::new().fg(if done { ACCENT } else { DIM }),
                ));
            }
            _ => {}
        }
    }
    flush(&mut out, &mut cur, quote);
    out
}

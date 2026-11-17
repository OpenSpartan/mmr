//! The status bar and the key caps along the bottom.

use super::ink::{
    bold, fg, on, paint, put, put_right, spans_width, width, BRIGHT, CAP, CAP_DIM, CAP_DIM_FG, CAP_LIT, DOWN, FAINT,
    GAP, GRAPHITE, GRAPHITE_FG, GRAPHITE_HI, INK, LABEL,
};
use super::keys::Pane;
use super::Cx;
use crate::{clock, store};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// A status pill: bold `color` text on a dark `tint`.
fn pill(text: &str, color: Color, tint: Color) -> Span<'static> {
    Span::styled(format!(" {text} "), Style::new().fg(color).bg(tint).add_modifier(Modifier::BOLD))
}

/// A band across the screen: whether the game is connected, Pinned, Names hidden,
/// the problem count, the counters, and at the right end a notice or the time. The
/// right end gets its room first, and a counter that doesn't fit is left off whole.
pub(super) fn status_bar(buf: &mut Buffer, area: Rect, cx: &Cx) {
    let (state, ui) = (cx.state, cx.ui);
    paint(buf, area, area, on(LABEL, GRAPHITE));
    let inner = Rect { x: area.x + 2.min(area.width), width: area.width.saturating_sub(4), ..area };
    let mut spans = vec![if state.lobby_open > 0 {
        pill("Connected to lobby", Color::Rgb(150, 230, 170), Color::Rgb(28, 64, 44))
    } else {
        pill("Waiting for game", GRAPHITE_FG, GRAPHITE_HI)
    }];
    if ui.pinned.is_some() && !cx.list.is_empty() {
        spans.extend([Span::raw(GAP), pill("Pinned", Color::Rgb(240, 200, 110), Color::Rgb(72, 56, 22))]);
    }
    if ui.mask {
        spans.extend([Span::raw(GAP), pill("Names hidden", Color::Rgb(205, 170, 255), Color::Rgb(56, 42, 84))]);
    }
    let n = state.problem_count();
    if n > 0 {
        spans.extend([Span::raw(GAP), Span::raw("Problems "), Span::styled(n.to_string(), bold(DOWN))]);
    }
    let right = match &ui.notice {
        Some(n) if n.at.elapsed() < n.lasts => {
            Span::styled(n.text.clone(), bold(if n.problem { Color::Rgb(240, 200, 110) } else { BRIGHT }))
        }
        _ => Span::styled(clock::local_clock(), fg(BRIGHT)),
    };
    let value = |s: String| Span::styled(s, fg(BRIGHT));
    let counters = [
        ("Rosters", value(state.p.rosters.to_string())),
        ("Xbox documents", value(state.p.documents.to_string())),
        ("Proxy", value(format!("127.0.0.1:{}", state.o.port))),
    ];
    for (label, v) in counters {
        let need = spans_width(&spans) + 3 + width(label) + 1 + width(&v.content) + 3 + width(&right.content);
        if need > inner.width {
            break;
        }
        spans.extend([Span::raw(GAP), Span::raw(label), Span::raw(" "), v]);
    }
    let room = inner.width.saturating_sub(width(&right.content) + 2);
    let mut x = inner.x;
    for s in &spans {
        if x + width(&s.content) > inner.x + room {
            break;
        }
        x = put(buf, inner, x, inner.y, &s.content, s.style);
    }
    put_right(buf, inner, inner.right(), inner.y, &right.content, right.style);
}

/// How a key cap looks: lit when what it toggles is on, dim when it would do nothing.
#[derive(Clone, Copy, PartialEq)]
enum Cap {
    Plain,
    Lit,
    Dim,
}

fn cap(k: &str, how: Cap) -> Span<'static> {
    let style = match how {
        Cap::Plain => Style::new().fg(BRIGHT).bg(CAP),
        Cap::Lit => Style::new().fg(INK).bg(CAP_LIT),
        Cap::Dim => Style::new().fg(CAP_DIM_FG).bg(CAP_DIM),
    };
    Span::styled(format!(" {k} "), style.add_modifier(Modifier::BOLD))
}

/// The keys along the bottom in groups, wrapped onto more lines rather than left
/// off. With `?`, every key.
pub(super) fn key_lines(cx: &Cx, w: u16) -> Vec<Line<'static>> {
    type Item = (Vec<(&'static str, Cap)>, &'static str);
    let (state, ui) = (cx.state, cx.ui);
    let any = !cx.list.is_empty();
    let nav = if any { Cap::Plain } else { Cap::Dim };
    let lit = |on_: bool| if on_ { Cap::Lit } else { Cap::Plain };
    let folder = if state.o.save || store::matches_dir(&state.o.store).exists() { Cap::Plain } else { Cap::Dim };
    // The arrows choose a match on the list and scroll anywhere else.
    let arrows = if ui.focus == Pane::Matches { "Match" } else { "Scroll" };
    let follow = if any && ui.pinned.is_some() { Cap::Plain } else { Cap::Dim };
    let mut groups: Vec<Vec<Item>> = vec![
        vec![(vec![("Up", nav), ("Down", nav)], arrows), (vec![("Tab", Cap::Plain)], "Pane")],
        vec![(vec![("f", follow)], "Follow"), (vec![("d", if any { lit(ui.details) } else { Cap::Dim })], "Details")],
        vec![(vec![("m", lit(ui.mask))], "Hide names"), (vec![("o", folder)], "Folder")],
        vec![(vec![("?", lit(ui.all_keys))], "Keys"), (vec![("q", Cap::Plain)], "Quit")],
    ];
    if ui.all_keys {
        groups.push(vec![
            (vec![("Home", nav)], "Top"),
            (vec![("End", nav)], "Bottom"),
            (vec![("PgUp", nav), ("PgDn", nav)], "Page"),
        ]);
        groups.push(vec![
            (vec![("e", Cap::Plain)], "Events"),
            (vec![("h", Cap::Plain)], "History"),
            (vec![("p", Cap::Plain)], "Problems"),
        ]);
        groups.push(vec![(vec![("Esc", Cap::Plain)], "Back"), (vec![("Ctrl+C", Cap::Plain)], "Quit now")]);
    }
    let (inside, between) = if w >= 120 { ("  ", "    ") } else { ("  ", "   ") };
    let mut lines: Vec<Vec<Span<'static>>> = vec![vec![Span::raw("  ")]];
    for group in groups {
        for (n, (keys, what)) in group.into_iter().enumerate() {
            let dim = keys.iter().all(|k| k.1 == Cap::Dim);
            let mut item: Vec<Span<'static>> = Vec::new();
            for (i, (k, how)) in keys.iter().enumerate() {
                if i > 0 {
                    item.push(Span::raw(" "));
                }
                item.push(cap(k, *how));
            }
            item.push(Span::styled(format!(" {what}"), fg(if dim { FAINT } else { LABEL })));
            let line = lines.last_mut().unwrap();
            let sep = if line.len() == 1 {
                ""
            } else if n == 0 {
                between
            } else {
                inside
            };
            if line.len() > 1 && spans_width(line) + width(sep) + spans_width(&item) > w {
                lines.push(vec![Span::raw("  ")]);
            } else {
                line.push(Span::raw(sep));
            }
            lines.last_mut().unwrap().extend(item);
        }
    }
    lines.into_iter().map(Line::from).collect()
}

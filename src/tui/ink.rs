//! The colors, and the text, rules and headers drawn into the buffer.

use super::keys::Ui;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

pub(super) const MUTED: Color = Color::Rgb(128, 137, 156);
pub(super) const FAINT: Color = Color::Rgb(84, 92, 110);
pub(super) const LABEL: Color = Color::Rgb(150, 160, 180);
pub(super) const BRIGHT: Color = Color::Rgb(236, 240, 246);
/// Cyan marks you and the seam, and nothing else.
pub(super) const SEAM: Color = Color::Rgb(96, 216, 240);
pub(super) const SEAM_INK: Color = Color::Rgb(10, 22, 30);
pub(super) const SEAM_UNLIT: Color = Color::Rgb(36, 78, 90);
pub(super) const DOWN: Color = Color::Rgb(250, 120, 110);
pub(super) const LIVE: Color = Color::Rgb(110, 214, 140);
pub(super) const INK: Color = Color::Rgb(16, 20, 28);
pub(super) const GLASS_INK: Color = Color::Rgb(240, 244, 250);
pub(super) const GRAPHITE: Color = Color::Rgb(30, 36, 48);
pub(super) const GRAPHITE_HI: Color = Color::Rgb(44, 52, 68);
pub(super) const GRAPHITE_LO: Color = Color::Rgb(24, 28, 38);
pub(super) const GRAPHITE_FG: Color = Color::Rgb(200, 208, 222);
pub(super) const RAIL_LINE: Color = Color::Rgb(70, 78, 96);
pub(super) const PICKED: Color = Color::Rgb(44, 54, 72);
pub(super) const CAP: Color = Color::Rgb(64, 74, 94);
pub(super) const CAP_LIT: Color = Color::Rgb(196, 204, 218);
pub(super) const CAP_DIM: Color = Color::Rgb(40, 46, 60);
pub(super) const CAP_DIM_FG: Color = Color::Rgb(96, 104, 122);

/// The game's team names and colors (the multiplayer globals, block 2), in team
/// order: a color for text, a soft one for labels on the glass, and light, base and
/// dark glass.
pub(super) const TEAMS: [(&str, Color, Color, [Color; 3]); 8] = [
    (
        "Eagle",
        Color::Rgb(255, 104, 104),
        Color::Rgb(250, 200, 205),
        [Color::Rgb(190, 58, 66), Color::Rgb(150, 28, 38), Color::Rgb(96, 14, 22)],
    ),
    (
        "Cobra",
        Color::Rgb(96, 152, 255),
        Color::Rgb(200, 215, 250),
        [Color::Rgb(52, 92, 180), Color::Rgb(28, 62, 150), Color::Rgb(16, 38, 100)],
    ),
    (
        "Hades",
        Color::Rgb(255, 214, 64),
        Color::Rgb(250, 236, 190),
        [Color::Rgb(170, 140, 30), Color::Rgb(135, 108, 16), Color::Rgb(84, 66, 8)],
    ),
    (
        "Valkyrie",
        Color::Rgb(90, 204, 100),
        Color::Rgb(200, 240, 205),
        [Color::Rgb(40, 130, 50), Color::Rgb(20, 100, 30), Color::Rgb(12, 64, 18)],
    ),
    (
        "Rampart",
        Color::Rgb(194, 128, 255),
        Color::Rgb(225, 205, 250),
        [Color::Rgb(110, 60, 170), Color::Rgb(80, 30, 130), Color::Rgb(50, 18, 84)],
    ),
    (
        "Cutlass",
        Color::Rgb(255, 110, 190),
        Color::Rgb(250, 205, 230),
        [Color::Rgb(160, 40, 110), Color::Rgb(120, 20, 80), Color::Rgb(76, 12, 50)],
    ),
    (
        "Valor",
        Color::Rgb(236, 150, 60),
        Color::Rgb(250, 220, 190),
        [Color::Rgb(170, 95, 25), Color::Rgb(130, 70, 15), Color::Rgb(84, 44, 8)],
    ),
    (
        "Hazard",
        Color::Rgb(120, 222, 200),
        Color::Rgb(205, 245, 235),
        [Color::Rgb(30, 130, 120), Color::Rgb(20, 100, 92), Color::Rgb(12, 64, 58)],
    ),
];

/// Party mark colors, from a family no team uses.
pub(super) const PARTY_COLORS: [Color; 4] =
    [Color::Rgb(232, 232, 238), Color::Rgb(172, 188, 214), Color::Rgb(214, 190, 150), Color::Rgb(190, 174, 214)];

/// Space between items on a line.
pub(super) const GAP: &str = "   ";
pub(super) fn team(t: u16) -> &'static (&'static str, Color, Color, [Color; 3]) {
    &TEAMS[t as usize % TEAMS.len()]
}

pub(super) fn team_name(t: u16) -> String {
    match TEAMS.get(t as usize) {
        Some(x) => x.0.to_string(),
        None => format!("Team {}", u32::from(t) + 1),
    }
}

pub(super) fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

pub(super) fn bold(c: Color) -> Style {
    Style::new().fg(c).add_modifier(Modifier::BOLD)
}

pub(super) fn on(fg: Color, bg: Color) -> Style {
    Style::new().fg(fg).bg(bg)
}

pub(super) fn width(s: &str) -> u16 {
    s.chars().count() as u16
}

pub(super) fn cut(s: &str, n: u16) -> String {
    s.chars().take(n as usize).collect()
}

/// Writes `text` at `x`, `y`, cut at the right edge of `area`, and returns the x after it.
pub(super) fn put(buf: &mut Buffer, area: Rect, x: u16, y: u16, text: &str, style: Style) -> u16 {
    if y < area.y || y >= area.bottom() || x < area.x || x >= area.right() {
        return x;
    }
    buf.set_stringn(x, y, text, (area.right() - x) as usize, style).0
}

pub(super) fn put_spans(buf: &mut Buffer, area: Rect, mut x: u16, y: u16, spans: &[Span]) -> u16 {
    for s in spans {
        x = put(buf, area, x, y, &s.content, s.style);
    }
    x
}

/// Writes `text` so it ends just before `right`.
pub(super) fn put_right(buf: &mut Buffer, area: Rect, right: u16, y: u16, text: &str, style: Style) {
    put(buf, area, right.saturating_sub(width(text)), y, text, style);
}

pub(super) fn paint(buf: &mut Buffer, area: Rect, r: Rect, style: Style) {
    let r = r.intersection(area);
    if !r.is_empty() {
        buf.set_style(r, style);
    }
}

pub(super) fn row(area: Rect, y: u16) -> Rect {
    Rect { y, height: 1, ..area }
}

pub(super) fn spans_width(spans: &[Span]) -> u16 {
    spans.iter().map(|s| width(&s.content)).sum()
}

/// How a component's title looks: lit like a key cap while it has focus, bright
/// while it's on screen, and muted for a tab that isn't.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Title {
    Focused,
    Shown,
    Hidden,
}

pub(super) fn title(name: &str, how: Title) -> Span<'static> {
    let text = format!(" {name} ");
    match how {
        Title::Focused => Span::styled(text, Style::new().fg(INK).bg(CAP_LIT).add_modifier(Modifier::BOLD)),
        Title::Shown => Span::styled(text, bold(BRIGHT)),
        Title::Hidden => Span::styled(text, fg(MUTED)),
    }
}

/// The header every component shares: a rule with the titles at its left end, then
/// `left`, and at its right end `right`, which holds the rows out of view and the
/// component's context. The rule is lit while the component has focus.
pub(super) fn header(
    buf: &mut Buffer,
    area: Rect,
    titles: &[Span<'static>],
    left: &[Span<'static>],
    right: &[Span<'static>],
    lit: bool,
) {
    put(buf, area, area.x, area.y, &"\u{2500}".repeat(area.width as usize), fg(if lit { LABEL } else { FAINT }));
    let x = put_spans(buf, area, area.x + 2, area.y, titles);
    if !left.is_empty() {
        put_spans(buf, area, x + 2, area.y, &[&[Span::raw(" ")], left, &[Span::raw(" ")]].concat());
    }
    if !right.is_empty() && spans_width(right) <= right_room(area, titles, left) {
        let right = [&[Span::raw(" ")], right, &[Span::raw(" ")]].concat();
        put_spans(buf, area, area.right() - 2 - spans_width(&right), area.y, &right);
    }
}

/// How wide `header`'s right end can be after `titles` and `left`. Each part has a
/// space on either side, with two cells of rule before the titles, between the parts
/// and after the right end.
pub(super) fn right_room(area: Rect, titles: &[Span], left: &[Span]) -> u16 {
    let left = if left.is_empty() { 0 } else { spans_width(left) + 4 };
    area.width.saturating_sub(spans_width(titles) + left + 8)
}

/// What a scrolled pane showed: how many rows, and how many are out of view above
/// and below.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Shown {
    pub(super) rows: usize,
    pub(super) above: usize,
    pub(super) below: usize,
}

impl Shown {
    /// The rows out of view, for the right end of a header.
    pub(super) fn spans(self) -> Vec<Span<'static>> {
        let mut v = Vec::new();
        for (n, word) in [(self.above, "above"), (self.below, "below")] {
            if n > 0 {
                if !v.is_empty() {
                    v.push(Span::raw(GAP));
                }
                v.push(Span::styled(format!("{n} {word}"), fg(MUTED)));
            }
        }
        v
    }

    /// The rows out of view, joined with a component's context.
    pub(super) fn with(self, context: Vec<Span<'static>>) -> Vec<Span<'static>> {
        let mut v = self.spans();
        if !v.is_empty() && !context.is_empty() {
            v.push(Span::raw(GAP));
        }
        v.extend(context);
        v
    }

    /// What `room` leaves for the context after the rows out of view.
    pub(super) fn room(self, room: u16) -> u16 {
        let w = spans_width(&self.spans());
        room.saturating_sub(if w > 0 { w + width(GAP) } else { 0 })
    }

    /// Tells the key handling how far the focused pane pages and scrolls.
    pub(super) fn report(self, ui: &Ui) {
        ui.page.set(self.rows.max(1));
        ui.most.set(self.above + self.below);
    }
}

/// A window of `rows` over `total` rows, scrolled `from` the top as far as it goes:
/// its first row, and what's out of view.
pub(super) fn window(total: usize, rows: usize, from: usize) -> (usize, Shown) {
    let start = from.min(total.saturating_sub(rows));
    let end = (start + rows).min(total);
    (start, Shown { rows, above: start, below: total - end })
}

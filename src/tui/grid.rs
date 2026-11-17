//! The players: each team's band and rows, in mirrored columns about the center line.

use super::facts::{mean, one, Facts, Kind, Seat, Side};
use super::ink::{
    bold, cut, fg, on, paint, put, put_right, put_spans, spans_width, team, team_name, FAINT, GLASS_INK, GRAPHITE,
    GRAPHITE_FG, GRAPHITE_HI, GRAPHITE_LO, MUTED, PARTY_COLORS, SEAM,
};
use super::Cx;
use crate::view;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use std::time::Duration;

/// How long a changed player stays marked.
const FRESH: Duration = Duration::from_secs(6);
/// One status word per player, the same everywhere: Left, Late or You.
pub(super) fn status(s: &Seat) -> (&'static str, Style) {
    if !s.present {
        ("Left", fg(FAINT))
    } else if s.late {
        ("Late", fg(MUTED))
    } else if s.you {
        ("You", bold(SEAM))
    } else {
        ("", Style::new())
    }
}

/// How a player's cells look. A player who left is faint, and nothing else is.
pub(super) fn looks(s: &Seat) -> (Style, Style, Style) {
    let base = if s.present { Style::new() } else { fg(FAINT) };
    let name = if s.you { base.add_modifier(Modifier::BOLD) } else { base };
    let mode = if s.present { base.add_modifier(Modifier::BOLD) } else { base };
    (name, mode, base)
}

/// The party mark beside each row of `rows` (seat indexes in table order). A party
/// sits together under one bracket, heavy beside its leader. A party split across
/// teams shows its number instead, so its members can be matched up. With `left`,
/// the brackets open to the left, toward the names.
fn marks(fa: &Facts, rows: &[usize], left: bool) -> Vec<Option<(char, Color)>> {
    let party: Vec<Option<usize>> = rows.iter().map(|&i| fa.party(i)).collect();
    let apart = |p: usize| fa.teams.iter().filter(|t| t.seats.iter().any(|&i| fa.party(i) == Some(p))).count() > 1;
    (0..rows.len())
        .map(|r| {
            let p = party[r]?;
            let color = PARTY_COLORS[p % PARTY_COLORS.len()];
            if apart(p) {
                return Some((char::from_digit((p as u32 + 1) % 10, 10).unwrap_or('?'), color));
            }
            let up = r > 0 && party[r - 1] == Some(p);
            let down = r + 1 < rows.len() && party[r + 1] == Some(p);
            let lead = fa.seats[rows[r]].p.leader;
            let c = match (up, down, left, lead) {
                (false, false, _, _) => return None,
                (false, true, false, false) => '\u{250c}',
                (false, true, true, false) => '\u{2510}',
                (true, true, false, false) => '\u{251c}',
                (true, true, true, false) => '\u{2524}',
                (true, false, false, false) => '\u{2514}',
                (true, false, true, false) => '\u{2518}',
                (false, true, false, true) => '\u{250f}',
                (false, true, true, true) => '\u{2513}',
                (true, true, false, true) => '\u{2523}',
                (true, true, true, true) => '\u{252b}',
                (true, false, false, true) => '\u{2517}',
                (true, false, true, true) => '\u{251b}',
            };
            Some((c, color))
        })
        .collect()
}

/// A rating's width as printed, like 10.1.
const NUMBER: u16 = 4;

/// What a band shows: its name, whether it's yours, its means (Xbox MMR, then
/// Mode MMR), its glass from light to dark, and a soft color for its labels.
pub(super) struct Face {
    name: String,
    you: bool,
    means: (Option<f64>, Option<f64>),
    pub(super) glass: [Color; 3],
    pub(super) soft: Color,
}

impl Face {
    /// A team's band; `mine` is your team.
    pub(super) fn of(t: &Side, mine: Option<u16>) -> Face {
        let &(_, _, soft, glass) = team(t.id);
        Face { name: team_name(t.id), you: mine == Some(t.id), means: (t.xbox, t.mean), glass, soft }
    }
}

/// One team's columns: where its status words, names, party marks, Xbox MMR and
/// Mode MMR go. Every row of the team uses them, the band of means as well as the
/// players, so each kind of number always sits in one column.
#[derive(Clone, Copy)]
pub(super) struct Half {
    /// Facing right, toward the center line: names right-aligned, status on the left.
    left: bool,
    /// The half's cells, for bands and tints.
    span: (u16, u16),
    status: (u16, u16),
    /// A place number before the names, in a free for all.
    rank: Option<(u16, u16)>,
    names: (u16, u16),
    lane: u16,
    /// Each number column's title slot; the numbers sit at its inner edge.
    xbox: (u16, u16),
    pub(super) mode: (u16, u16),
}

impl Half {
    /// A number ending at its column's inner edge.
    fn number(&self, buf: &mut Buffer, area: Rect, y: u16, slot: (u16, u16), text: &str, style: Style) {
        let right = if self.left { slot.1 } else { slot.0 + NUMBER };
        put_right(buf, area, right, y, text, style);
    }

    /// A column title, aligned to the inner edge like its numbers.
    pub(super) fn title(&self, buf: &mut Buffer, area: Rect, y: u16, slot: (u16, u16), text: &str, style: Style) {
        if self.left {
            put_right(buf, area, slot.1, y, text, style);
        } else {
            put(buf, area, slot.0, y, text, style);
        }
    }

    /// Text in the name column, aligned as the names are.
    fn name_text(&self, buf: &mut Buffer, area: Rect, y: u16, text: &str, style: Style) {
        let w = self.names.1.saturating_sub(self.names.0);
        if self.left {
            put_right(buf, area, self.names.1, y, &cut(text, w), style);
        } else {
            put(buf, area, self.names.0, y, &cut(text, w), style);
        }
    }

    /// Text in the status slot, at the outer edge.
    fn outer(&self, buf: &mut Buffer, area: Rect, y: u16, spans: &[Span]) {
        let x = if self.left { self.status.0 } else { self.status.1.saturating_sub(spans_width(spans)) };
        put_spans(buf, area, x, y, spans);
    }

    fn cells(&self, y: u16) -> Rect {
        Rect { x: self.span.0, y, width: self.span.1.saturating_sub(self.span.0), height: 1 }
    }

    /// A band `rows` high from `y`: the name and column titles, then the means in the
    /// players' columns. A band one row high has only the name and the means.
    pub(super) fn band(&self, buf: &mut Buffer, area: Rect, y: u16, rows: u16, face: &Face) {
        for r in 0..rows {
            let shade = if rows == 1 { 1 } else { r as usize };
            paint(buf, area, self.cells(y + r), on(GLASS_INK, face.glass[shade]));
        }
        let mut label = vec![Span::styled(face.name.clone(), bold(GLASS_INK))];
        if face.you {
            label.push(Span::styled("  You", bold(SEAM)));
        }
        if !self.left {
            label.reverse();
            if face.you {
                label[0] = Span::styled("You  ", bold(SEAM));
            }
        }
        self.outer(buf, area, y, &label);
        let means_y = y + rows.saturating_sub(1);
        if rows > 1 {
            let (xh, mh) = if self.xbox.1 - self.xbox.0 >= 8 { ("Xbox MMR", "Mode MMR") } else { ("Xbox", "Mode") };
            self.title(buf, area, y, self.xbox, xh, fg(face.soft));
            self.title(buf, area, y, self.mode, mh, fg(face.soft));
            self.name_text(buf, area, means_y, "Mean", fg(face.soft));
        }
        let (xbox, mode) = face.means;
        if xbox.is_some() || mode.is_some() {
            self.number(buf, area, means_y, self.xbox, &one(xbox), fg(GLASS_INK));
            self.number(buf, area, means_y, self.mode, &one(mode), bold(GLASS_INK));
        }
    }

    /// Players in `rows`, one per row. When some don't fit, the last row says how many
    /// more. With `places`, each player's place goes before the name.
    fn players(&self, buf: &mut Buffer, rows: Rect, seats: &[usize], places: bool, cx: &Cx, fa: &Facts) {
        let marks = marks(fa, seats, self.left);
        let fits = seats.len() <= usize::from(rows.height);
        let show = if fits { seats.len() } else { usize::from(rows.height.saturating_sub(1)) };
        // One lookup key for every player, so the match key is cloned once.
        let mut touched = (fa.m.key.clone(), 0);
        for (n, &i) in seats.iter().take(show).enumerate() {
            let s = &fa.seats[i];
            let y = rows.y + n as u16;
            touched.1 = s.x;
            let changed = s.present && cx.state.touched.get(&touched).is_some_and(|t| t.elapsed() < FRESH);
            // A row only has a background when it's chosen, so a change is underlined.
            let (name_style, mode_style, xbox_style) = looks(s);
            let name_style =
                if changed { name_style.add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { name_style };
            let (word, word_style) = status(s);
            self.outer(buf, rows, y, &[Span::styled(word, word_style)]);
            let place = s.mode.filter(|_| places && s.present).map(|v| fa.place(v));
            if let (Some(r), Some(p)) = (self.rank, place) {
                put_right(buf, rows, r.1, y, &p.to_string(), fg(MUTED));
            }
            self.name_text(buf, rows, y, &view::who(fa.m, s.x, cx.ui.mask), name_style);
            if let Some((c, color)) = marks[n] {
                put(buf, rows, self.lane, y, &c.to_string(), fg(color));
            }
            self.number(buf, rows, y, self.xbox, &one(s.xbox), xbox_style);
            self.number(buf, rows, y, self.mode, &one(s.mode), mode_style);
        }
        if !fits && rows.height > 0 {
            self.name_text(buf, rows, rows.y + show as u16, &format!("{} more", seats.len() - show), fg(MUTED));
        }
    }
}

/// The two facing halves. The ratings sit against the center line on both sides,
/// the party marks against the names, and the status words at the outer edges, one
/// cell in.
pub(super) struct Mirror {
    pub(super) mid: u16,
    pub(super) l: Half,
    pub(super) r: Half,
}

impl Mirror {
    pub(super) fn new(area: Rect, ranked: bool) -> Mirror {
        let mid = area.x + area.width / 2;
        let (lx, rx) = (area.x, area.right());
        let st = 4;
        let rank = if ranked { 4 } else { 0 };
        // The number columns are as wide as their titles; when that leaves too little
        // room for names, the titles shorten to Xbox and Mode.
        let room = (mid - lx).saturating_sub(st + rank + 8);
        let s = if room.saturating_sub(2 * 8 + 6) >= 14 { 8 } else { 5 };
        let l_mode = (mid.saturating_sub(2 + s), mid.saturating_sub(2));
        let l_xbox = (l_mode.0.saturating_sub(2 + s), l_mode.0.saturating_sub(2));
        let l_lane = l_xbox.0.saturating_sub(2);
        let r_mode = (mid + 3, mid + 3 + s);
        let r_xbox = (r_mode.1 + 2, r_mode.1 + 2 + s);
        let r_lane = r_xbox.1 + 1;
        let l_status = (lx + 1, lx + 1 + st);
        let r_status = (rx.saturating_sub(1 + st), rx.saturating_sub(1));
        Mirror {
            mid,
            l: Half {
                left: true,
                span: (lx, mid.saturating_sub(1)),
                status: l_status,
                rank: ranked.then(|| (l_status.1 + 1, l_status.1 + 1 + rank)),
                names: (l_status.1 + 1 + rank + 1, l_lane.saturating_sub(1)),
                lane: l_lane,
                xbox: l_xbox,
                mode: l_mode,
            },
            r: Half {
                left: false,
                span: (mid + 2, rx),
                status: r_status,
                rank: ranked.then(|| (r_status.0.saturating_sub(1 + rank), r_status.0.saturating_sub(1))),
                names: (r_lane + 2, r_status.0.saturating_sub(2 + rank)),
                lane: r_lane,
                xbox: r_xbox,
                mode: r_mode,
            },
        }
    }

    fn divider(&self, buf: &mut Buffer, area: Rect, from: u16, to: u16) {
        for y in from..to {
            put(buf, area, self.mid, y, "\u{2502}", fg(FAINT));
        }
    }
}

/// Rows the players need. Two teams: the longer side, under the glass. More teams:
/// each facing pair's bands and players, with a gap between pairs. One list: its
/// band and every player.
pub(super) fn roster_height(fa: &Facts) -> u16 {
    match fa.kind {
        Kind::Two => fa.live().iter().map(|t| t.seats.len()).max().unwrap_or(0) as u16,
        Kind::Many => {
            let pairs: Vec<u16> =
                fa.teams.chunks(2).map(|p| 2 + p.iter().map(|t| t.seats.len()).max().unwrap_or(0) as u16).collect();
            pairs.iter().sum::<u16>() + pairs.len().saturating_sub(1) as u16
        }
        Kind::Ffa | Kind::Lobby => 2 + fa.seats.len() as u16,
    }
}

/// Two teams facing each other across the seam's line, yours on the left; their
/// bands are the glass above.
pub(super) fn two(buf: &mut Buffer, area: Rect, cx: &Cx, fa: &Facts, mir: &Mirror) {
    let live = fa.live();
    mir.divider(buf, area, area.y, area.bottom());
    mir.l.players(buf, area, &live[0].seats, false, cx, fa);
    mir.r.players(buf, area, &live[1].seats, false, cx, fa);
}

/// Three or more teams in pairs, yours first and then the best, each with its band.
/// Pairs aren't matchups, so no line runs between them.
pub(super) fn pairs(buf: &mut Buffer, area: Rect, cx: &Cx, fa: &Facts, mir: &Mirror) {
    let mine = fa.you().and_then(|s| s.team);
    let mut y = area.y;
    for pair in fa.teams.chunks(2) {
        if y >= area.bottom() {
            break;
        }
        let need = pair.iter().map(|t| t.seats.len()).max().unwrap_or(0) as u16;
        let rows = need.min(area.bottom().saturating_sub(y + 2));
        for (n, t) in pair.iter().enumerate() {
            let half = if n == 0 { &mir.l } else { &mir.r };
            half.band(buf, area, y, 2, &Face::of(t, mine));
            half.players(buf, Rect { y: y + 2, height: rows, ..area }, &t.seats, false, cx, fa);
        }
        y += 2 + need + 1;
    }
}

/// A free for all, or a lobby before teams: one ranked list in a team's columns,
/// under the lobby's band, continuing in the right half when it's long.
pub(super) fn ranked(buf: &mut Buffer, area: Rect, cx: &Cx, fa: &Facts) {
    let mir = Mirror::new(area, true);
    let order: Vec<usize> = (0..fa.seats.len()).collect();
    let rows = Rect { y: area.y + 2, height: area.height.saturating_sub(2), ..area };
    let face = Face {
        name: if fa.kind == Kind::Ffa { "FFA" } else { "Lobby" }.to_string(),
        you: fa.you().is_some_and(|s| s.present),
        means: (fa.xbox_mean(), mean(fa.counted().filter_map(|s| s.mode))),
        glass: [GRAPHITE_HI, GRAPHITE, GRAPHITE_LO],
        soft: GRAPHITE_FG,
    };
    mir.l.band(buf, area, area.y, 2, &face);
    let split = order.len() > usize::from(rows.height);
    let (first, rest) =
        if split { order.split_at(usize::from(rows.height).min(order.len())) } else { (&order[..], &[][..]) };
    mir.l.players(buf, rows, first, true, cx, fa);
    if split {
        mir.r.band(buf, area, area.y, 2, &Face { name: String::new(), you: false, means: (None, None), ..face });
        mir.r.players(buf, rows, rest, true, cx, fa);
    }
}

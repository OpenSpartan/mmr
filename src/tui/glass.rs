//! The glass: the seam between two teams, a scale for more teams or for players without
//! teams, and the crystal before any match.

use super::facts::{bounds, Facts, Verdict};
use super::grid::{Face, Mirror};
use super::ink::{
    bold, fg, on, paint, put, put_right, row, team, team_name, width, GLASS_INK, GRAPHITE, GRAPHITE_FG, GRAPHITE_HI,
    GRAPHITE_LO, RAIL_LINE, SEAM, SEAM_INK, SEAM_UNLIT,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use std::sync::OnceLock;

fn rail(buf: &mut Buffer, area: Rect) {
    for r in 0..area.height {
        let shade = match (area.height, r) {
            (1, _) => GRAPHITE,
            (_, 0) => GRAPHITE_HI,
            (_, 1) => GRAPHITE,
            _ => GRAPHITE_LO,
        };
        paint(buf, area, row(area, area.y + r), on(GRAPHITE_FG, shade));
    }
}

/// The glass before there's a match: graphite, with the seam unlit where it will be.
pub(super) fn unlit(buf: &mut Buffer, area: Rect, mir: &Mirror) {
    rail(buf, area);
    paint(
        buf,
        area,
        Rect { x: mir.mid.saturating_sub(1), width: 3, height: area.height.min(2), ..area },
        on(GRAPHITE_FG, SEAM_UNLIT),
    );
}

/// The lean reaches its limit at `LEAN_CAP` of the players' spread, and stops
/// `LEAN_END` cells from the glass's end.
const LEAN_CAP: f64 = 1.5;
pub(super) const LEAN_END: u16 = 12;

/// Two teams as glass: each team's band (its name, then its means in its players'
/// columns), yours on the left, with the cyan seam between them carrying the gap.
/// The bottom row shows the balance. Where the two colors meet moves toward the
/// weaker team by the gap as a share of the players' spread, and the verdict sits
/// under the favored team's Mode MMR, or under the seam when it's even.
pub(super) fn seam(buf: &mut Buffer, area: Rect, fa: &Facts, mir: &Mirror) {
    if area.is_empty() {
        return;
    }
    let live = fa.live();
    let (l, r) = (live[0], live[1]);
    let mine = fa.you().and_then(|s| s.team);
    let bands = area.height.min(2);
    let (lf, rf) = (Face::of(l, mine), Face::of(r, mine));
    mir.l.band(buf, area, area.y, bands, &lf);
    mir.r.band(buf, area, area.y, bands, &rf);
    let seam = Rect { x: mir.mid.saturating_sub(1), y: area.y, width: 3, height: bands };
    paint(buf, area, seam, on(SEAM_INK, SEAM));
    let g = fa.gap().map(|g| format!("{g:.1}")).unwrap_or_default();
    let center = |s: &str| (mir.mid + 1).saturating_sub(width(s) / 2 + width(s) % 2);
    if bands > 1 {
        put(buf, area, center("Gap"), area.y, "Gap", fg(SEAM_INK));
    }
    put(buf, area, center(&g), area.y + bands - 1, &g, bold(SEAM_INK));
    if area.height < 3 {
        return;
    }
    let y = area.y + 2;
    let verdict = fa.verdict().unwrap_or(Verdict::Even);
    let room = f64::from((mir.mid - area.x).saturating_sub(LEAN_END));
    let at = match (verdict, fa.lean()) {
        (Verdict::Even, _) | (_, None) => mir.mid,
        (v, Some((_, share))) => {
            let n = u16::try_from((share.min(LEAN_CAP) / LEAN_CAP * room).round() as i32).unwrap_or(0);
            if v.team() == Some(l.id) {
                mir.mid.saturating_add(n)
            } else {
                mir.mid.saturating_sub(n)
            }
        }
    }
    .clamp(area.x, area.right() - 1);
    paint(buf, area, Rect { x: area.x, y, width: at - area.x, height: 1 }, on(lf.soft, lf.glass[2]));
    let right = Rect { x: at + 1, y, width: area.right().saturating_sub(at + 1), height: 1 };
    paint(buf, area, right, on(rf.soft, rf.glass[2]));
    paint(buf, area, Rect { x: at, y, width: 1, height: 1 }, on(SEAM_INK, SEAM));
    match verdict.team() {
        // When it's even, the marker under the seam is the word itself.
        None => {
            paint(buf, area, Rect { x: center("Even").saturating_sub(1), y, width: 6, height: 1 }, on(SEAM_INK, SEAM));
            put(buf, area, center("Even"), y, "Even", bold(SEAM_INK));
        }
        Some(t) if t == l.id => mir.l.title(buf, area, y, mir.l.mode, verdict.words(), fg(lf.soft)),
        Some(_) => mir.r.title(buf, area, y, mir.r.mode, verdict.words(), fg(rf.soft)),
    }
}

/// Where a scale starts on the glass, after its Mode MMR label.
const SCALE_FROM: u16 = 12;

/// A value on a scale, with its mark, the mark's style and a label for above it.
type Tick<'a> = (f64, &'a str, Style, Option<String>);

/// A scale from the lowest to the highest value on the middle row, with each
/// tick marked and labeled above it where there's room.
fn scale(buf: &mut Buffer, area: Rect, x0: u16, x1: u16, lo: f64, hi: f64, ticks: &[Tick]) {
    let y = area.y + area.height / 2;
    let (a, b) = (format!("{lo:.1}"), format!("{hi:.1}"));
    put(buf, area, x0, y, &a, fg(GRAPHITE_FG));
    put_right(buf, area, x1, y, &b, fg(GRAPHITE_FG));
    let (t0, t1) = (x0 + width(&a) + 1, x1.saturating_sub(width(&b) + 1));
    if t1 <= t0 + 1 {
        return;
    }
    put(buf, area, t0, y, &"\u{2500}".repeat((t1 - t0) as usize), fg(RAIL_LINE));
    let span = f64::from(t1 - t0 - 1);
    #[expect(clippy::cast_sign_loss, reason = "every tick lies between lo and hi")]
    let at = |v: f64| t0 + if hi > lo { ((v - lo) / (hi - lo) * span).round() as u16 } else { 0 };
    let mut taken = t0;
    let mut order: Vec<&Tick> = ticks.iter().collect();
    order.sort_by(|p, q| p.0.total_cmp(&q.0));
    for (v, mark, style, label) in order {
        let x = at(*v);
        put(buf, area, x, y, mark, *style);
        if let (Some(label), true) = (label, area.height >= 3) {
            let w = width(label);
            let lx = x.saturating_sub(w / 2).max(taken).min(t1.saturating_sub(w));
            if lx >= taken {
                put(buf, area, lx, area.y, label, *style);
                taken = lx + w + 1;
            }
        }
    }
}

/// The scale's bottom row: your standing on the left and the spread on the right,
/// in the same places for teams and players.
fn standing(buf: &mut Buffer, area: Rect, left: Option<String>, right: &str) {
    if area.height < 3 {
        return;
    }
    let y = area.bottom() - 1;
    if let Some(l) = left {
        put(buf, area, area.x + 1, y, &l, bold(SEAM));
    }
    put_right(buf, area, area.right().saturating_sub(1), y, right, fg(GRAPHITE_FG));
}

/// Three or more teams: a scale of the team means with each team's tick and name.
pub(super) fn team_scale(buf: &mut Buffer, area: Rect, fa: &Facts) {
    rail(buf, area);
    let y = area.y + area.height / 2;
    put(buf, area, area.x + 1, y, "Mode MMR", fg(GRAPHITE_FG));
    let live = fa.live();
    let Some((lo, hi)) = bounds(&live.iter().filter_map(|t| t.mean).collect::<Vec<_>>()) else { return };
    let mine = fa.you().and_then(|s| s.team);
    let ticks: Vec<Tick> = live
        .iter()
        .filter_map(|t| {
            let style = if Some(t.id) == mine { bold(SEAM) } else { bold(team(t.id).1) };
            Some((t.mean?, "\u{2503}", style.bg(GRAPHITE), Some(team_name(t.id))))
        })
        .collect();
    scale(buf, area, area.x + SCALE_FROM, area.right().saturating_sub(2), lo, hi, &ticks);
    let you = fa.team_rank().map(|(n, of)| format!("Team rank {n} of {of}"));
    standing(buf, area, you, &fa.gap().map(|g| format!("Gap {g:.1}")).unwrap_or_default());
}

/// A free for all, or a lobby before teams: a scale of every player's Mode MMR, yours in cyan.
pub(super) fn player_scale(buf: &mut Buffer, area: Rect, fa: &Facts) {
    rail(buf, area);
    let y = area.y + area.height / 2;
    put(buf, area, area.x + 1, y, "Mode MMR", fg(GRAPHITE_FG));
    let Some((lo, hi)) = fa.range() else { return };
    let ticks: Vec<Tick> = fa
        .counted()
        .filter_map(|s| {
            let v = s.mode?;
            Some(if s.you {
                (v, "\u{2503}", bold(SEAM).bg(GRAPHITE), Some("You".to_string()))
            } else {
                (v, "\u{2502}", fg(GLASS_INK).bg(GRAPHITE), None)
            })
        })
        .collect();
    scale(buf, area, area.x + SCALE_FROM, area.right().saturating_sub(2), lo, hi, &ticks);
    let you = fa.rank().map(|(n, of)| format!("Rank {n} of {of}"));
    standing(buf, area, you, &format!("Range {:.1}", hi - lo));
}

/// Before any match: the crystal where the players will be.
pub(super) fn waiting(buf: &mut Buffer, area: Rect) {
    let rows = logo();
    let wide = rows.first().map_or(0, Vec::len) as u16;
    let x = (area.x + area.width / 2).saturating_sub(wide / 2);
    for (n, pair) in rows.chunks(2).enumerate() {
        let y = area.y + n as u16;
        for c in 0..wide as usize {
            let s = pixels(pair[0][c], pair.get(1).and_then(|r| r[c]));
            put(buf, area, x + c as u16, y, &s.content, s.style);
        }
    }
}

/// The logo's pixels, row by row, from `assets/logo.txt`.
pub(super) fn logo() -> &'static [Vec<Option<Color>>] {
    static LOGO: OnceLock<Vec<Vec<Option<Color>>>> = OnceLock::new();
    LOGO.get_or_init(|| {
        include_str!("../../assets/logo.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| {
                l.split_whitespace()
                    .map(|p| {
                        u32::from_str_radix(p, 16).ok().map(|v| Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
                    })
                    .collect()
            })
            .collect()
    })
}

/// Two pixels, one above the other, in one cell.
fn pixels(top: Option<Color>, bottom: Option<Color>) -> Span<'static> {
    match (top, bottom) {
        (Some(t), Some(b)) => Span::styled("\u{2580}", Style::new().fg(t).bg(b)),
        (Some(t), None) => Span::styled("\u{2580}", Style::new().fg(t)),
        (None, Some(b)) => Span::styled("\u{2584}", Style::new().fg(b)),
        (None, None) => Span::raw(" "),
    }
}

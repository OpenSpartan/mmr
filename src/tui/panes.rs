//! The panes below the players: the match list, and the chosen match's events, your
//! history and the problems. Also the details table, which takes the players' place.

use super::facts::{one, plural, tenths, Facts, Seat, Verdict};
use super::grid::{looks, status};
use super::ink::{
    bold, cut, fg, header, on, paint, put, put_right, put_spans, right_room, row, spans_width, team, team_name, title,
    width, window, Shown, Title, BRIGHT, DOWN, FAINT, GAP, GRAPHITE, LIVE, MUTED, PICKED, SEAM,
};
use super::keys::{Pane, Ui};
use super::{is_live, Cx};
use crate::tracker::Mark;
use crate::watch::State;
use crate::{clock, names, store, view};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use std::collections::HashMap;

/// From this width up, the match list and the pane beside it sit side by side.
const SPLIT: u16 = 110;
/// The narrowest the pane beside the match list gets.
const SIDE_MIN: u16 = 34;
/// What the match list says while it's empty.
pub(super) const NONE_YET: &str = "No match yet";

/// Every player in one table with the raw facts: the party and its leader, the
/// first roster value when it differs from the Xbox MMR, and the team the matchmaker
/// planned. Your team comes first, as in the main view, and the table scrolls when
/// it's long.
pub(super) fn details(buf: &mut Buffer, area: Rect, fa: &Facts, ui: &Ui) -> Shown {
    let name_w = fa.seats.iter().map(|s| width(&view::who(fa.m, s.x, ui.mask))).max().unwrap_or(0).clamp(8, 22);
    let first = |s: &Seat| s.p.mmr.iter().find_map(|m| m.value);
    let differs = fa.seats.iter().any(|s| first(s).map(tenths) != s.xbox.map(tenths));
    let mut cols: Vec<(&str, u16, bool)> = vec![
        ("Team", 9, false),
        ("Player", name_w, false),
        ("Party", 6, false),
        ("Status", 6, false),
        ("Xbox MMR", 8, true),
        ("Mode MMR", 8, true),
        ("Planned", 8, false),
    ];
    if differs {
        cols.insert(5, ("First MMR", 9, true));
    }
    let table = Table { area, cols: &cols, gap: if area.width < 100 { 1 } else { 2 } };
    let rank: HashMap<u16, usize> = fa.teams.iter().enumerate().map(|(n, t)| (t.id, n)).collect();
    let mut order: Vec<usize> = (0..fa.seats.len()).collect();
    order.sort_by_key(|&i| {
        (fa.seats[i].team.and_then(|t| rank.get(&t).copied()).unwrap_or(usize::MAX), !fa.seats[i].present)
    });
    // Where a column starts, and where the numbers in it end.
    let col = |name: &str| cols.iter().position(|c| c.0 == name).unwrap_or(0);
    let at = |name: &str| table.x(col(name));
    let end = |name: &str| table.end(col(name));
    table.titles(buf, |_| fg(MUTED));
    let rows = area.height.saturating_sub(1) as usize;
    let (start, shown) = window(order.len(), rows, ui.details_from);
    for (n, &i) in order.iter().skip(start).take(rows).enumerate() {
        let s = &fa.seats[i];
        let y = area.y + 1 + n as u16;
        let (name_style, mode_style, xbox_style) = looks(s);
        let team_style = |t: u16| fg(if s.present { team(t).1 } else { FAINT });
        if let Some(t) = s.team {
            put(buf, area, at("Team"), y, &team_name(t), team_style(t));
        }
        put(buf, area, at("Player"), y, &cut(&view::who(fa.m, s.x, ui.mask), name_w), name_style);
        if let Some(p) = fa.party(i) {
            let lead = if s.p.leader { " Lead" } else { "" };
            put(buf, area, at("Party"), y, &format!("{}{lead}", p + 1), name_style);
        }
        let (word, style) = status(s);
        put(buf, area, at("Status"), y, word, style);
        put_right(buf, area, end("Xbox MMR"), y, &one(s.xbox), xbox_style);
        if differs {
            put_right(buf, area, end("First MMR"), y, &one(first(s)), xbox_style);
        }
        put_right(buf, area, end("Mode MMR"), y, &one(s.mode), mode_style);
        let planned =
            s.p.xbox
                .as_ref()
                .and_then(|x| x.initial_team.as_deref())
                .and_then(|t| t.trim_matches('"').parse::<u16>().ok());
        if let Some(t) = planned {
            put(buf, area, at("Planned"), y, &team_name(t), team_style(t));
        }
    }
    shown
}

/// The folder, with `%LOCALAPPDATA%` standing in for that path so the user name doesn't show.
fn shown_path(path: &std::path::Path) -> String {
    let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    match base.and_then(|b| path.strip_prefix(b).ok().map(|rest| rest.display().to_string())) {
        Some(rest) => format!("%LOCALAPPDATA%\\{rest}"),
        None => path.display().to_string(),
    }
}

/// `Saving to:` and the folder, dropping whole folders from the left to fit `room`,
/// or nothing when even the last one doesn't fit.
pub(super) fn saving(state: &State, room: u16) -> Vec<Span<'static>> {
    if !state.o.save {
        return vec![Span::styled("Not saving", fg(MUTED))];
    }
    let mut path = shown_path(&store::matches_dir(&state.o.store));
    let label = "Saving to: ";
    while width(label) + width(&path) > room {
        match path.split_once('\\') {
            Some((_, rest)) => path = rest.to_string(),
            None => return Vec::new(),
        }
    }
    vec![Span::styled(label, fg(MUTED)), Span::styled(path, fg(BRIGHT))]
}

/// The match list's columns: title, width, and whether it's right-aligned.
const MATCH_COLUMNS: [(&str, u16, bool); 9] = [
    ("#", 3, true),
    ("Start", 5, false),
    ("Playlist", 15, false),
    ("Teams", 7, false),
    ("Gap", 4, true),
    ("Favored", 8, false),
    ("Rank", 8, true),
    ("Xbox MMR", 8, true),
    ("State", 5, false),
];

/// The match list's width: its columns, with two cells between each.
fn list_width() -> u16 {
    MATCH_COLUMNS.iter().map(|c| c.1).sum::<u16>() + 2 * (MATCH_COLUMNS.len() as u16 - 1)
}

/// A table's columns, each a title, a width and whether it's right-aligned, from
/// the left of `area` with `gap` cells between them.
struct Table<'a> {
    area: Rect,
    cols: &'a [(&'a str, u16, bool)],
    gap: u16,
}

impl Table<'_> {
    /// Where column `n` starts.
    fn x(&self, n: usize) -> u16 {
        self.area.x + self.cols[..n].iter().map(|c| c.1 + self.gap).sum::<u16>()
    }

    /// Where column `n` ends, and so where its right-aligned text ends.
    fn end(&self, n: usize) -> u16 {
        self.x(n) + self.cols[n].1
    }

    /// The titles along the top row, each aligned as its column is.
    fn titles(&self, buf: &mut Buffer, style: impl Fn(&str) -> Style) {
        for (n, &(title, _, right)) in self.cols.iter().enumerate() {
            if right {
                put_right(buf, self.area, self.end(n), self.area.y, title, style(title));
            } else {
                put(buf, self.area, self.x(n), self.area.y, title, style(title));
            }
        }
    }
}

/// A list's lines from its rows' times, with a day line before each day's first row.
/// Each line has the index of the row it shows, or `None` for a day line, and its day.
fn day_lines<'a>(times: impl Iterator<Item = &'a str>) -> Vec<(Option<usize>, String)> {
    let mut lines: Vec<(Option<usize>, String)> = Vec::new();
    for (i, at) in times.enumerate() {
        let d = clock::local_day(at).unwrap_or_default();
        if lines.last().is_none_or(|l| l.1 != d) {
            lines.push((None, d.clone()));
        }
        lines.push((Some(i), d));
    }
    lines
}

/// Below the players. When wide, the match list sits beside Events, History or
/// Problems; when narrow, only the pane last focused shows. Each has its header.
pub(super) fn lower(buf: &mut Buffer, area: Rect, cx: &Cx, fa: Option<&Facts>) {
    let (state, ui) = (cx.state, cx.ui);
    let n = state.problem_count();
    let look = |p: Pane, shown: bool| {
        if ui.focus == p {
            Title::Focused
        } else if shown {
            Title::Shown
        } else {
            Title::Hidden
        }
    };
    // The tabs, with `shown` the one on screen, and the problem count.
    let tabs = |panes: &[Pane], shown: Pane| {
        let mut v = Vec::new();
        for (i, &p) in panes.iter().enumerate() {
            if i > 0 {
                v.push(Span::raw(" "));
            }
            v.push(title(p.name(), look(p, p == shown)));
            if p == Pane::Problems && n > 0 {
                v.push(Span::styled(n.to_string(), bold(DOWN)));
            }
        }
        v
    };
    let body = |r: Rect| Rect { y: r.y + 1, height: r.height.saturating_sub(1), ..r };
    let side = if ui.lower == Pane::Matches { Pane::Events } else { ui.lower };
    // Each pane draws first, so its header can say what's out of view.
    let draw_pane = |buf: &mut Buffer, p: Pane, r: Rect| -> Shown {
        let shown = match (p, fa) {
            (Pane::Matches, _) => match_list(buf, r, cx),
            (Pane::Problems, _) => problems(buf, r, cx),
            (Pane::History, Some(_)) => history(buf, r, cx),
            (_, Some(fa)) => events(buf, r, fa, ui),
            (_, None) => Shown::default(),
        };
        if ui.focus == p {
            shown.report(ui);
        }
        shown
    };
    if area.width >= SPLIT {
        let left_w = list_width().min(area.width.saturating_sub(SIDE_MIN));
        let left = Rect { width: left_w, ..area };
        let right = Rect { x: area.x + left_w + 3, width: area.width.saturating_sub(left_w + 3), ..area };
        let m = draw_pane(buf, Pane::Matches, body(left));
        let s = draw_pane(buf, side, body(right));
        let head = Rect { width: left_w + 1, height: 1, ..area };
        let titles = tabs(&[Pane::Matches], Pane::Matches);
        let saving = saving(state, m.room(right_room(head, &titles, &[])));
        header(buf, head, &titles, &[], &m.with(saving), ui.focus == Pane::Matches);
        let tabs = tabs(&[Pane::Events, Pane::History, Pane::Problems], side);
        let head = Rect { x: area.x + left_w + 2, width: area.width.saturating_sub(left_w + 2), height: 1, ..area };
        header(buf, head, &tabs, &[], &s.spans(), ui.focus == side);
        put(buf, area, area.x + left_w + 1, area.y, "\u{252c}", fg(FAINT));
        for y in area.y + 1..area.bottom() {
            put(buf, area, area.x + left_w + 1, y, "\u{2502}", fg(FAINT));
        }
        return;
    }
    let shown = draw_pane(buf, ui.lower, body(area));
    let tabs = tabs(&[Pane::Matches, Pane::Events, Pane::History, Pane::Problems], ui.lower);
    let head = row(area, area.y);
    let context =
        if ui.lower == Pane::Matches { saving(state, shown.room(right_room(head, &tabs, &[]))) } else { Vec::new() };
    header(buf, head, &tabs, &[], &shown.with(context), ui.focus == ui.lower);
}

/// The saved matches and this run's, oldest first, with a line for each day. It
/// scrolls to keep the chosen match in view, and the first row's day always shows.
/// The columns are the same at every size: the match's fields as in its header, who
/// was favored, your rank, and the lobby's Xbox MMR, the one number that compares
/// across modes. The chosen row is lit while the list has focus.
fn match_list(buf: &mut Buffer, area: Rect, cx: &Cx) -> Shown {
    let (state, list, at) = (cx.state, &cx.list, cx.at);
    let t = &state.p.tracker;
    // Two cells between columns, or one when two don't fit.
    let table = Table { area, cols: &MATCH_COLUMNS, gap: if list_width() <= area.width { 2 } else { 1 } };
    table.titles(buf, |title| fg(if title == "Rank" { SEAM } else { MUTED }));
    if list.is_empty() {
        put(buf, area, table.x(1), area.y + 1, NONE_YET, fg(MUTED));
        return Shown::default();
    }
    let lines = day_lines(list.iter().map(|k| t.matches[*k].first_seen.as_str()));
    let rows = area.height.saturating_sub(1) as usize;
    if rows == 0 {
        let before = at.min(list.len());
        return Shown { rows: 0, above: before, below: list.len() - before };
    }
    let at_line = lines.iter().position(|l| l.0 == Some(at)).unwrap_or(lines.len() - 1);
    let start_for = |rows: usize| (at_line + 2).saturating_sub(rows).min(lines.len().saturating_sub(rows));
    let mut start = start_for(rows);
    // Once a day line scrolls out of view, the first row repeats the day, if
    // there's room for a match row as well.
    let sticky = rows > 1 && start > 0 && lines[start].0.is_some();
    if sticky {
        start = start_for(rows.saturating_sub(1));
    }
    let take = rows.saturating_sub(usize::from(sticky));
    let mut shown: Vec<(Option<usize>, String)> = lines.iter().skip(start).take(take).cloned().collect();
    if sticky && shown.first().is_some_and(|l| l.0.is_some()) {
        shown.insert(0, (None, lines[start].1.clone()));
    }
    let first = shown.iter().find_map(|l| l.0).unwrap_or(0);
    let last = shown.iter().rev().find_map(|l| l.0).unwrap_or(0);
    let lit = cx.ui.focus == Pane::Matches;
    for (r, (i, d)) in shown.iter().enumerate() {
        let y = area.y + 1 + r as u16;
        let Some(i) = i else {
            put(buf, area, table.x(1), y, d, fg(FAINT));
            continue;
        };
        let fa = Facts::of(&t.matches[list[*i]]);
        let picked = *i == at;
        // The chosen row is lit while the list has focus, and a quiet band otherwise.
        let band = if lit { PICKED } else { GRAPHITE };
        let tint = |s: Style| if picked { s.bg(band) } else { s };
        let text = tint(match (picked, lit) {
            (true, true) => bold(BRIGHT),
            (true, false) => fg(BRIGHT),
            _ => fg(MUTED),
        });
        if picked {
            paint(buf, area, row(area, y), on(BRIGHT, band));
        }
        let cell = |buf: &mut Buffer, n: usize, s: String, style: Style| {
            if MATCH_COLUMNS[n].2 {
                put_right(buf, area, table.end(n), y, &s, style);
            } else {
                put(buf, area, table.x(n), y, &cut(&s, MATCH_COLUMNS[n].1), style);
            }
        };
        cell(buf, 0, (i + 1).to_string(), text);
        cell(buf, 1, clock::local_minute(&fa.m.first_seen).unwrap_or_default(), text);
        cell(buf, 2, names::playlist(&fa.m.setup), text);
        cell(buf, 3, fa.teams_text(), text);
        cell(buf, 4, fa.gap().map(|g| format!("{g:.1}")).unwrap_or_default(), text);
        let mine = fa.you().and_then(|s| s.team);
        let (favored, style) = match fa.verdict() {
            Some(Verdict::Even) => ("Even".to_string(), text),
            Some(v) => match v.team() {
                Some(t) if Some(t) == mine => ("You".to_string(), tint(bold(SEAM))),
                Some(t) => (team_name(t), tint(fg(team(t).1))),
                None => (String::new(), text),
            },
            None => (String::new(), text),
        };
        cell(buf, 5, favored, style);
        cell(buf, 6, fa.rank().map(|(a, b)| format!("{a} of {b}")).unwrap_or_default(), text);
        cell(buf, 7, one(fa.xbox_mean()), text);
        if is_live(state, list[*i], *i + 1 == list.len()) {
            cell(buf, 8, "Live".into(), tint(bold(LIVE)));
        }
    }
    Shown { rows: take, above: first, below: list.len().saturating_sub(last + 1) }
}

/// What happened in the chosen match, from its own record: when it was found, the
/// split into teams with the gap at the time, who joined late, who left, and real
/// rating and team changes. The matchmaker's first values aren't events, and
/// neither are the starting players one by one. The list follows the newest event
/// unless scrolled up.
fn events(buf: &mut Buffer, area: Rect, fa: &Facts, ui: &Ui) -> Shown {
    struct Ev {
        at: String,
        what: &'static str,
        player: String,
        team: Option<u16>,
        value: String,
    }
    let mut ev: Vec<Ev> = Vec::new();
    let who = |s: &Seat| view::who(fa.m, s.x, ui.mask);
    let starters = fa.seats.iter().filter(|s| !s.late).count();
    ev.push(Ev {
        at: fa.m.first_seen.clone(),
        what: "Found",
        player: plural(starters, "player"),
        team: None,
        value: String::new(),
    });
    let teams_at =
        fa.seats.iter().filter_map(|s| s.p.team.iter().find(|t| t.value.is_some()).map(|t| t.at.clone())).min();
    if let Some(at) = teams_at {
        let (sizes, gap) = fa.at_start();
        let player = gap.map_or(sizes.clone(), |g| format!("{sizes}  Gap {g:.1}"));
        ev.push(Ev { at, what: "Split", player, team: None, value: String::new() });
    }
    for s in &fa.seats {
        if s.late {
            if let Some(first) = s.p.mmr.first() {
                ev.push(Ev { at: first.at.clone(), what: "Late", player: who(s), team: s.team, value: one(s.mode) });
            }
        }
        for w in s.rated.windows(2) {
            let value = format!("{} to {}", one(w[0].value), one(w[1].value));
            ev.push(Ev { at: w[1].at.clone(), what: "Rated", player: who(s), team: s.team, value });
        }
        let mut teams: Vec<&Mark<Option<u16>>> = Vec::new();
        for t in s.p.team.iter().filter(|t| t.value.is_some()) {
            if teams.last().is_none_or(|l| l.value != t.value) {
                teams.push(t);
            }
        }
        for w in teams.windows(2) {
            ev.push(Ev { at: w[1].at.clone(), what: "Moved", player: who(s), team: w[1].value, value: one(s.mode) });
        }
        if let (false, Some(at)) = (s.present, &s.p.left_at) {
            ev.push(Ev { at: at.clone(), what: "Left", player: who(s), team: s.team, value: one(s.mode) });
        }
    }
    ev.sort_by(|a, b| a.at.cmp(&b.at));
    // Times are to the minute, as in the match list. A name takes its team's color,
    // as everywhere else, and numbers end at the right edge.
    let xe = area.x + 7;
    let xp = xe + 7;
    // The column title can be wider than the numbers, since it has a row of its own.
    let value_w = ev.iter().map(|e| width(&e.value)).max().unwrap_or(0).max(3);
    let name_w = area.right().saturating_sub(value_w + 2).saturating_sub(xp);
    put(buf, area, area.x, area.y, "Time", fg(MUTED));
    put(buf, area, xe, area.y, "Event", fg(MUTED));
    put(buf, area, xp, area.y, "Player", fg(MUTED));
    put_right(buf, area, area.right(), area.y, "Mode MMR", fg(MUTED));
    let rows = area.height.saturating_sub(1) as usize;
    let most = ev.len().saturating_sub(rows);
    let (start, shown) = window(ev.len(), rows, most - ui.events_up.min(most));
    for (n, e) in ev.iter().skip(start).take(rows).enumerate() {
        let y = area.y + 1 + n as u16;
        put(buf, area, area.x, y, &clock::local_minute(&e.at).unwrap_or_default(), fg(MUTED));
        put(buf, area, xe, y, e.what, Style::new());
        let style = e.team.map_or(fg(MUTED), |t| fg(team(t).1));
        let room = if e.value.is_empty() { area.right().saturating_sub(xp) } else { name_w };
        put(buf, area, xp, y, &cut(&e.player, room), style);
        put_right(buf, area, area.right(), y, &e.value, Style::new());
    }
    shown
}

/// Your Mode MMR in the chosen match's playlist and mode, match by match up to the
/// chosen one, with a line for each day as in the match list. Mode MMR only compares
/// within a mode, so this is where to follow yours.
fn history(buf: &mut Buffer, area: Rect, cx: &Cx) -> Shown {
    struct Row {
        at: String,
        map: Option<String>,
        mode: f64,
        rank: Option<(usize, usize)>,
        chosen: bool,
    }
    let (list, at) = (&cx.list, cx.at);
    let t = &cx.state.p.tracker;
    let chosen = &t.matches[list[at]];
    let Some(key) = names::mode_key(&chosen.setup) else {
        put(buf, area, area.x, area.y, "No mode known for this match", fg(MUTED));
        return Shown::default();
    };
    // Every match up to the chosen one is read again each frame. Nothing is cached,
    // since a few hundred matches read quickly.
    let mut rows: Vec<Row> = Vec::new();
    for (n, k) in list.iter().take(at + 1).enumerate() {
        let m = &t.matches[*k];
        if names::mode_key(&m.setup).as_ref() != Some(&key) {
            continue;
        }
        let fa = Facts::of(m);
        if let Some(mode) = fa.you().and_then(|s| s.mode) {
            let (map, rank) = (names::of(&m.setup).map, fa.rank());
            rows.push(Row { at: m.first_seen.clone(), map, mode, rank, chosen: n == at });
        }
    }
    // The title: the playlist and the mode, as in the match header.
    let mut what = vec![Span::raw(names::playlist(&chosen.setup))];
    if let Some(mode) = names::of(&chosen.setup).mode {
        what.extend([Span::raw(GAP), Span::raw(mode)]);
    }
    put_spans(buf, area, area.x, area.y, &what);
    let count = if rows.len() == 1 { "1 match".to_string() } else { format!("{} matches", rows.len()) };
    if spans_width(&what) + 3 + width(&count) <= area.width {
        put_right(buf, area, area.right(), area.y, &count, fg(MUTED));
    }
    // Columns from the right: Change, Mode MMR, Rank. Start is on the left, with the
    // map between when there's room.
    let change_r = area.right();
    let mode_r = change_r.saturating_sub(8);
    let rank_r = mode_r.saturating_sub(10);
    let map_x = area.x + 7;
    let map_w = rank_r.saturating_sub(8 + 2).saturating_sub(map_x);
    let y0 = area.y + 1;
    put(buf, area, area.x, y0, "Start", fg(MUTED));
    if map_w >= 4 {
        put(buf, area, map_x, y0, "Map", fg(MUTED));
    }
    put_right(buf, area, rank_r, y0, "Rank", fg(SEAM));
    put_right(buf, area, mode_r, y0, "Mode MMR", fg(MUTED));
    put_right(buf, area, change_r, y0, "Change", fg(MUTED));
    let lines = day_lines(rows.iter().map(|r| r.at.as_str()));
    let height = area.height.saturating_sub(2) as usize;
    let most = lines.len().saturating_sub(height);
    let (start, view) = window(lines.len(), height, most - cx.ui.history_up.min(most));
    for (n, (line, day)) in lines.iter().skip(start).take(height).enumerate() {
        let y = y0 + 1 + n as u16;
        let Some(i) = *line else {
            put(buf, area, area.x, y, day, fg(FAINT));
            continue;
        };
        let r = &rows[i];
        // The chosen match is banded as it is in the list, when you have a Mode MMR in it.
        let text = if r.chosen { fg(BRIGHT).bg(GRAPHITE) } else { fg(MUTED) };
        if r.chosen {
            paint(buf, area, row(area, y), on(BRIGHT, GRAPHITE));
        }
        put(buf, area, area.x, y, &clock::local_minute(&r.at).unwrap_or_default(), text);
        if let (true, Some(map)) = (map_w >= 4, &r.map) {
            put(buf, area, map_x, y, &cut(map, map_w), text);
        }
        if let Some((a, b)) = r.rank {
            put_right(buf, area, rank_r, y, &format!("{a} of {b}"), text);
        }
        put_right(buf, area, mode_r, y, &one(Some(r.mode)), text.add_modifier(Modifier::BOLD));
        if let Some(prev) = i.checked_sub(1).map(|p| rows[p].mode) {
            let d = tenths(r.mode) - tenths(prev);
            let (s, c) = if d.abs() < 0.05 {
                ("0.0".to_string(), MUTED)
            } else {
                (format!("{d:+.1}"), if d > 0.0 { LIVE } else { DOWN })
            };
            put_right(buf, area, change_r, y, &s, text.fg(c));
        }
    }
    view
}

/// What went wrong reading the traffic, each with a count.
fn problems(buf: &mut Buffer, area: Rect, cx: &Cx) -> Shown {
    let state = cx.state;
    let all: Vec<(&String, &u64)> = state.p.decoder.problems.iter().chain(&state.problems).collect();
    if all.is_empty() {
        put(buf, area, area.x, area.y, "No problems yet", fg(MUTED));
        return Shown::default();
    }
    put_right(buf, area, area.x + 5, area.y, "Count", fg(MUTED));
    put(buf, area, area.x + 7, area.y, "Problem", fg(MUTED));
    let rows = area.height.saturating_sub(1) as usize;
    let (start, shown) = window(all.len(), rows, cx.ui.problems_from);
    for (n, (what, count)) in all.iter().skip(start).take(rows).enumerate() {
        let y = area.y + 1 + n as u16;
        put_right(buf, area, area.x + 5, y, &count.to_string(), bold(DOWN));
        put(buf, area, area.x + 7, y, what, Style::new());
    }
    shown
}

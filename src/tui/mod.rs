//! The full-screen view of `mmr watch`. As in the icon, the two teams are red and
//! blue glass, and the cyan seam between them carries the gap. From the top: the
//! chosen match's header, the glass (or a scale, for more teams or a free for all),
//! the players, the match list beside the Events, History or Problems pane, a status
//! bar, and the keys.
//!
//! Everything drawn is ASCII apart from box lines and the logo's half blocks, so
//! columns line up in any console font. Some fonts draw symbols like bullets and
//! arrows two cells wide.
//!
//! Mode MMR is the roster's rating once the lobby service has set it. When a player
//! first appears, the roster carries the matchmaker's value (the Xbox MMR, to the
//! last digit), and a few milliseconds later the mode's own value, which only
//! compares within one playlist and mode. Xbox MMR is the matchmaker's rating and
//! compares across matches. A player who backfills has no Xbox document, so their
//! Xbox MMR is that first roster value. Counts and means include only the players
//! still in the match.
//!
//! The layout keeps to a few rules so the numbers can be read at a glance:
//! - One grid: a team's means sit in the same columns as its players' numbers, from
//!   the glass down, and a free for all uses the same columns.
//! - One order: Xbox MMR, then Mode MMR, toward the center line.
//! - One name for each number and each state, everywhere: Mode MMR, Xbox MMR, Mean,
//!   Gap, Rank, Left, Late, You, Even, Favored.
//! - One shape for a team: a band with its name and means, then its players, best
//!   first.
//! - One measure of balance: the gap between the top two team means as a share of
//!   the spread of the players' Mode MMR, which reads the same in every mode.
//! - Cyan marks you and the seam, and nothing else.
//! - One header for every part: a rule with its name on the left, and on the right
//!   what's out of view and its context. The focused part's name is lit like a key
//!   cap. Tab moves the focus, and the arrows act on the focused part.
//! - A row background marks the chosen row, and nothing else.
//! - Positions read N of M, and words keep sentence case; game names keep theirs.

mod bars;
mod facts;
mod glass;
mod grid;
mod ink;
mod keys;
mod panes;
#[cfg(test)]
mod tests;

use crate::tap::Out;
use crate::watch::State;
use crate::{clock, names, store};
use bars::{key_lines, status_bar};
use facts::{one, Facts, Kind};
use glass::{player_scale, seam, team_scale, unlit, waiting};
use grid::{pairs, ranked, roster_height, two, Mirror};
use ink::{bold, fg, header, right_room, row, spans_width, title, Shown, Title, GAP, LIVE, MUTED};
use keys::{key, Act, Notice, Pane, Ui};
use panes::{details, lower};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::text::Span;
use ratatui::Frame;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Duration;

/// The glass's height, or one row when the screen is short.
const GLASS: u16 = 3;
/// The matches to list, oldest first. A session whose roster never arrived isn't listed.
fn matches(state: &State) -> Vec<&String> {
    let t = &state.p.tracker;
    t.order.iter().filter(|k| t.matches[*k].has_roster()).collect()
}

/// Whether the newest match was seen this run, not only loaded from the saved matches.
fn is_ours(state: &State, list: &[&String]) -> bool {
    list.last().is_some_and(|k| state.changed.contains_key(*k))
}

/// Before a frame, brings the UI state up to date with the matches and returns the list.
fn catch_up<'a>(state: &'a State, ui: &mut Ui) -> Vec<&'a String> {
    // A match keeps its place and its scroll when its session gets its match id.
    for k in [&mut ui.pinned, &mut ui.seen] {
        if let Some(m) = k.as_deref().and_then(|k| state.match_of(k)) {
            *k = Some(m.key.clone());
        }
    }
    let list = matches(state);
    ui.ours = is_ours(state, &list);
    ui.track(&list);
    if list.len() > ui.count && ui.count > 0 && ui.pinned.is_some() {
        ui.notice = Some(Notice::new("New match", false));
    }
    ui.count = list.len();
    list
}

/// How long to wait for a key before drawing again.
const POLL: Duration = Duration::from_millis(250);

/// Runs until the user quits, and restores the terminal however it ends.
pub fn run(state: &mut State, rx: &Receiver<Out>) -> std::io::Result<()> {
    state.load_saved();
    let mut terminal = ratatui::try_init()?;
    let mut ui = Ui::new(state.o.mask);
    let result = (|| -> std::io::Result<()> {
        loop {
            loop {
                match rx.try_recv() {
                    Ok(out) => state.take(out),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            }
            let list = catch_up(state, &mut ui);
            terminal.draw(|f| draw(f, state, &ui))?;
            if event::poll(POLL)? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        match key(&mut ui, k, &list, state.lobby_open > 0) {
                            Act::Stay => {}
                            Act::Open => ui.notice = Some(open(state, &ui, &list)),
                            Act::Quit => return Ok(()),
                        }
                    }
                }
            }
        }
    })();
    ratatui::restore();
    result
}

/// Opens the folder of saved matches in Explorer, with the chosen match's file selected.
fn open(state: &State, ui: &Ui, list: &[&String]) -> Notice {
    use std::os::windows::process::CommandExt;
    let dir = store::matches_dir(&state.o.store);
    if !state.o.save && !dir.exists() {
        return Notice::new("Not saving", true);
    }
    if std::fs::create_dir_all(&dir).is_err() {
        return Notice::new("Folder not created", true);
    }
    let file = list.get(ui.index(list)).map(|k| store::path_of(&state.o.store, k)).filter(|p| p.exists());
    let mut explorer = std::process::Command::new("explorer.exe");
    match &file {
        Some(f) => explorer.raw_arg(format!("/select,\"{}\"", f.display())),
        None => explorer.arg(&dir),
    };
    match explorer.spawn() {
        Ok(_) => Notice::new("Folder opened", false),
        Err(_) => Notice::new("Folder not opened", true),
    }
}

/// What a frame is drawn from: the state, the UI state, the listed matches and the
/// chosen match's place among them.
struct Cx<'a> {
    state: &'a State,
    ui: &'a Ui,
    list: Vec<&'a String>,
    at: usize,
}

fn draw(f: &mut Frame, state: &State, ui: &Ui) {
    let list = matches(state);
    let cx = Cx { state, ui, at: ui.index(&list), list };
    let area = f.area();
    let keys = key_lines(&cx, area.width);
    let bars = 1 + keys.len() as u16;
    let buf = f.buffer_mut();
    if area.height <= bars {
        status_bar(buf, row(area, area.y), &cx);
        return;
    }
    let top = Rect { height: area.height - bars, ..area };
    status_bar(buf, row(area, top.bottom()), &cx);
    for (i, l) in keys.iter().enumerate() {
        buf.set_line(area.x, top.bottom() + 1 + i as u16, l, area.width);
    }

    let chosen = cx.list.get(cx.at).map(|k| &state.p.tracker.matches[*k]);
    let facts = chosen.map(Facts::of);
    let inner = Rect { x: top.x + 2.min(top.width), width: top.width.saturating_sub(4), ..top };
    let players = match &facts {
        None => 5,
        Some(fa) if ui.details => 1 + fa.seats.len() as u16,
        Some(fa) => roster_height(fa),
    };
    // With two teams, the glass is the team bands, so the players follow it with no gap.
    let joined = facts.as_ref().is_some_and(|f| f.kind == Kind::Two) && !ui.details;
    // Room goes to the players first: the glass thins, the gaps go, then the lower
    // area shrinks to its rule and a row, then goes.
    let need =
        |glass: u16, air: u16, lower: u16| 1 + air + glass + if joined { 0 } else { air } + players + air + lower;
    let (glass, air, lower_min) = [(GLASS, 1, 4), (1, 1, 4), (1, 0, 4), (1, 0, 2), (1, 0, 0)]
        .into_iter()
        .find(|&(g, a, l)| need(g, a, l) <= top.height)
        .unwrap_or((1, 0, 0));
    let head = row(inner, top.y);
    let mut y = top.y + 1 + air;
    let glass_area = Rect { y, height: glass.min(top.bottom().saturating_sub(y)), ..inner };
    let mir = Mirror::new(inner, false);
    match &facts {
        None => unlit(buf, glass_area, &mir),
        Some(fa) if fa.kind == Kind::Two => seam(buf, glass_area, fa, &mir),
        Some(fa) if fa.kind == Kind::Many => team_scale(buf, glass_area, fa),
        Some(fa) => player_scale(buf, glass_area, fa),
    }
    y += glass + if joined { 0 } else { air };
    let room = top.bottom().saturating_sub(y + if lower_min > 0 { air + lower_min } else { 0 });
    let body = Rect { y, height: players.min(room), ..inner };
    let mut shown = Shown::default();
    match &facts {
        None => waiting(buf, body),
        Some(fa) if ui.details => {
            shown = details(buf, body, fa, ui);
            if ui.focus == Pane::Details {
                shown.report(ui);
            }
        }
        Some(fa) => match fa.kind {
            Kind::Two => two(buf, body, &cx, fa, &mir),
            Kind::Many => pairs(buf, body, &cx, fa, &mir),
            Kind::Ffa | Kind::Lobby => ranked(buf, body, &cx, fa),
        },
    }
    match_header(buf, head, &cx, facts.as_ref(), shown);
    y += body.height + air;
    if lower_min > 0 && y < top.bottom() {
        lower(buf, Rect { y, height: top.bottom() - y, ..inner }, &cx, facts.as_ref());
    }
}

/// Live: the newest match, while the game is connected, once this run has seen it
/// change. A saved match is never live.
fn is_live(state: &State, key: &str, newest: bool) -> bool {
    newest && state.lobby_open > 0 && state.changed.contains_key(key)
}

/// The chosen match's header: its place, then the fields of its row in the match
/// list, in the same order and under the same names. The least important fields drop
/// first when the line is short. At the right end: what the details have out of view,
/// and Live.
fn match_header(buf: &mut Buffer, area: Rect, cx: &Cx, facts: Option<&Facts>, shown: Shown) {
    let Some(fa) = facts else {
        header(buf, area, &[title("Match", Title::Hidden)], &[], &[], false);
        return;
    };
    let (ui, at, count) = (cx.ui, cx.at, cx.list.len());
    let lit = ui.focus == Pane::Details;
    let name = [title(&format!("Match {} of {count}", at + 1), if lit { Title::Focused } else { Title::Shown })];
    let pair = |l: &str, v: String| vec![Span::styled(format!("{l} "), fg(MUTED)), Span::raw(v)];
    // Each fact with its priority: the lowest drops first.
    let mut facts: Vec<(u8, Vec<Span<'static>>)> = Vec::new();
    if let Some(t) = clock::local_minute(&fa.m.first_seen) {
        facts.push((3, pair("Start", t)));
    }
    let list = names::playlist(&fa.m.setup);
    if !list.is_empty() {
        facts.push((6, vec![Span::raw(list)]));
    }
    let named = names::of(&fa.m.setup);
    let game = match (named.mode, named.map) {
        (Some(mode), Some(map)) => vec![Span::raw(mode), Span::styled(" on ", fg(MUTED)), Span::raw(map)],
        (Some(one), None) | (None, Some(one)) => vec![Span::raw(one)],
        (None, None) => Vec::new(),
    };
    if !game.is_empty() {
        facts.push((7, game));
    }
    facts.push((9, vec![Span::raw(fa.teams_text())]));
    if let Some((a, b)) = fa.rank() {
        facts.push((2, pair("Rank", format!("{a} of {b}"))));
    }
    if let Some(x) = fa.xbox_mean() {
        facts.push((1, pair("Xbox MMR", one(Some(x)))));
    }
    // Left and Late also show beside each player, so these drop before the playlist
    // and game names do.
    if fa.left() > 0 {
        facts.push((4, pair("Left", fa.left().to_string())));
    }
    if fa.late() > 0 {
        facts.push((4, pair("Late", fa.late().to_string())));
    }
    if let (true, Some(id), false) = (ui.details, &fa.m.match_id, ui.mask) {
        facts.push((5, vec![Span::styled(id.clone(), fg(MUTED))]));
    }
    let live = is_live(cx.state, &fa.m.key, at + 1 == count);
    let right = shown.with(if live { vec![Span::styled("Live", bold(LIVE))] } else { Vec::new() });
    // The facts get the room the right end leaves, less a space on each side.
    let room = right_room(area, &name, &[]).saturating_sub(spans_width(&right) + 4);
    let joined = |facts: &[(u8, Vec<Span<'static>>)]| -> Vec<Span<'static>> {
        let mut v = Vec::new();
        for (n, (_, f)) in facts.iter().enumerate() {
            if n > 0 {
                v.push(Span::raw(GAP));
            }
            v.extend(f.iter().cloned());
        }
        v
    };
    while spans_width(&joined(&facts)) > room && facts.len() > 1 {
        let least = facts.iter().enumerate().min_by_key(|(n, f)| (f.0, usize::MAX - n)).map_or(0, |(n, _)| n);
        facts.remove(least);
    }
    header(buf, area, &name, &joined(&facts), &right, lit);
}

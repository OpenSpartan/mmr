//! Key handling: which pane has focus, what's pinned and scrolled, and what each key does.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::cell::Cell;
use std::time::{Duration, Instant};

/// How long a notice stays in the status bar.
const NOTICE: Duration = Duration::from_secs(4);
/// Two presses of q within this quit while the game is connected.
const QUIT_AGAIN: Duration = Duration::from_secs(2);
pub(super) struct Notice {
    pub(super) text: String,
    pub(super) problem: bool,
    pub(super) at: Instant,
    /// How long it stays in the status bar.
    pub(super) lasts: Duration,
}

impl Notice {
    pub(super) fn new(text: &str, problem: bool) -> Notice {
        Notice { text: text.to_string(), problem, at: Instant::now(), lasts: NOTICE }
    }
}

/// A pane that can take focus. One has focus at a time, and Tab moves through them
/// in reading order.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Pane {
    /// Every player in one table, in place of the teams while it's open.
    Details,
    Matches,
    Events,
    /// Your Mode MMR in the chosen match's playlist and mode, up to that match.
    History,
    Problems,
}

impl Pane {
    pub(super) fn name(self) -> &'static str {
        match self {
            Pane::Details => "Details",
            Pane::Matches => "Matches",
            Pane::Events => "Events",
            Pane::History => "History",
            Pane::Problems => "Problems",
        }
    }
}

pub(super) struct Ui {
    pub(super) mask: bool,
    /// The match kept on screen, or `None` to follow the newest.
    pub(super) pinned: Option<String>,
    /// The chosen match at the last check, pinned or followed.
    pub(super) seen: Option<String>,
    /// Whether the newest match is from this run, so following shows it. Until one is,
    /// following shows no match rather than the last saved one as if it were current.
    pub(super) ours: bool,
    pub(super) details: bool,
    pub(super) focus: Pane,
    /// The lower pane last focused: the one shown when only one fits, and which of
    /// Events, History or Problems sits beside the match list.
    pub(super) lower: Pane,
    /// Rows scrolled: the events and the history up from the newest, which they
    /// follow; the problems and the details down from the top.
    pub(super) events_up: usize,
    pub(super) history_up: usize,
    pub(super) problems_from: usize,
    pub(super) details_from: usize,
    /// The focused pane's rows and how far it can scroll, as last drawn.
    pub(super) page: Cell<usize>,
    pub(super) most: Cell<usize>,
    /// Whether every key shows, in place of the key bar.
    pub(super) all_keys: bool,
    pub(super) notice: Option<Notice>,
    /// When q was pressed once while connected.
    quit: Option<Instant>,
    /// How many matches there were, to say when another arrives while one is pinned.
    pub(super) count: usize,
}

impl Ui {
    pub(super) fn new(mask: bool) -> Ui {
        Ui {
            mask,
            pinned: None,
            seen: None,
            ours: true,
            details: false,
            focus: Pane::Matches,
            lower: Pane::Matches,
            events_up: 0,
            history_up: 0,
            problems_from: 0,
            details_from: 0,
            page: Cell::new(10),
            most: Cell::new(0),
            all_keys: false,
            notice: None,
            quit: None,
            count: 0,
        }
    }

    /// The chosen match's place in `list`, or `list.len()` for none.
    pub(super) fn index(&self, list: &[&String]) -> usize {
        let newest = if self.ours { list.len().saturating_sub(1) } else { list.len() };
        self.pinned.as_ref().and_then(|p| list.iter().position(|k| *k == p)).unwrap_or(newest)
    }

    /// Notes the chosen match. A newly chosen match starts its events and history at
    /// the newest and its details at the top.
    pub(super) fn track(&mut self, list: &[&String]) {
        let chosen = list.get(self.index(list)).map(|k| k.as_str());
        if chosen != self.seen.as_deref() {
            self.seen = chosen.map(str::to_string);
            self.events_up = 0;
            self.history_up = 0;
            self.details_from = 0;
        }
    }

    pub(super) fn focus(&mut self, p: Pane) {
        self.focus = p;
        if p != Pane::Details {
            self.lower = p;
        }
    }

    /// The panes Tab moves along, in reading order.
    fn panes(&self) -> Vec<Pane> {
        let all = [Pane::Details, Pane::Matches, Pane::Events, Pane::History, Pane::Problems];
        all.into_iter().filter(|p| *p != Pane::Details || self.details).collect()
    }

    /// Scrolls the focused pane `n` rows toward its top, or its bottom when not `top`.
    fn scroll(&mut self, top: bool, n: usize) {
        let most = self.most.get();
        let (v, up) = match self.focus {
            Pane::Events => (&mut self.events_up, top),
            Pane::History => (&mut self.history_up, top),
            Pane::Problems => (&mut self.problems_from, !top),
            Pane::Details => (&mut self.details_from, !top),
            Pane::Matches => return,
        };
        *v = if up { v.saturating_add(n) } else { v.saturating_sub(n) }.min(most);
    }
}

pub(super) enum Act {
    Stay,
    Open,
    Quit,
}

/// What a key does. The arrows, the page keys, Home and End act on the focused pane:
/// on the match list they choose a match, and elsewhere they scroll.
pub(super) fn key(ui: &mut Ui, k: KeyEvent, list: &[&String], connected: bool) -> Act {
    if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
        return Act::Quit;
    }
    let last = list.len().saturating_sub(1);
    let at = ui.index(list);
    let page = ui.page.get().max(1);
    let on_matches = ui.focus == Pane::Matches;
    if list.is_empty() && matches!(k.code, KeyCode::Char('f' | 'd') | KeyCode::Enter) {
        return Act::Stay;
    }
    // Choosing the newest match follows it again, if it's from this run.
    let go = |ui: &mut Ui, to: usize| {
        if !list.is_empty() {
            ui.pinned = (to < last || !ui.ours).then(|| list[to].clone());
        }
    };
    match k.code {
        KeyCode::Char('q') => {
            if !connected || ui.quit.is_some_and(|t| t.elapsed() < QUIT_AGAIN) {
                return Act::Quit;
            }
            ui.quit = Some(Instant::now());
            // The prompt clears when another press would no longer quit.
            ui.notice = Some(Notice { lasts: QUIT_AGAIN, ..Notice::new("Press q again to quit", true) });
        }
        KeyCode::Char('o') => return Act::Open,
        KeyCode::Up | KeyCode::Char('k') if on_matches => go(ui, at.saturating_sub(1)),
        KeyCode::Down | KeyCode::Char('j') if on_matches => go(ui, (at + 1).min(last)),
        KeyCode::PageUp if on_matches => go(ui, at.saturating_sub(page)),
        KeyCode::PageDown if on_matches => go(ui, (at + page).min(last)),
        KeyCode::Home if on_matches => go(ui, 0),
        KeyCode::End if on_matches => go(ui, last),
        KeyCode::Up | KeyCode::Char('k') => ui.scroll(true, 1),
        KeyCode::Down | KeyCode::Char('j') => ui.scroll(false, 1),
        KeyCode::PageUp => ui.scroll(true, page),
        KeyCode::PageDown => ui.scroll(false, page),
        KeyCode::Home => ui.scroll(true, usize::MAX),
        KeyCode::End => ui.scroll(false, usize::MAX),
        KeyCode::Char('f') => ui.pinned = None,
        KeyCode::Char('e') => ui.focus(Pane::Events),
        KeyCode::Char('h') => ui.focus(Pane::History),
        KeyCode::Char('p') => ui.focus(Pane::Problems),
        KeyCode::Tab | KeyCode::BackTab => {
            let panes = ui.panes();
            let n = panes.iter().position(|p| *p == ui.focus).unwrap_or(0);
            let step = if k.code == KeyCode::Tab { 1 } else { panes.len() - 1 };
            ui.focus(panes[(n + step) % panes.len()]);
        }
        KeyCode::Esc => {
            if ui.all_keys {
                ui.all_keys = false;
            } else if ui.details {
                ui.details = false;
                ui.focus(ui.lower);
            } else {
                ui.focus(Pane::Matches);
            }
        }
        KeyCode::Char('m') => ui.mask = !ui.mask,
        KeyCode::Char('d') | KeyCode::Enter if k.code != KeyCode::Enter || on_matches => {
            ui.details = !ui.details;
            ui.details_from = 0;
            ui.focus(if ui.details { Pane::Details } else { ui.lower });
        }
        KeyCode::Char('?') => ui.all_keys = !ui.all_keys,
        _ => {}
    }
    ui.track(list);
    Act::Stay
}

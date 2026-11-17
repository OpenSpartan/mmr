//! `mmr watch`: points the system proxy at the local proxy, follows the rosters as
//! the game receives them, and puts the system proxy back on exit.

use crate::feed::Pipeline;
use crate::tap::{Host, Out};
use crate::tracker::{Change, Event, Match};
use crate::{cert, clock, gamertags, names, proxy, store, sysproxy, view};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;
use windows_sys::core::BOOL;
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

pub struct Options {
    pub store: PathBuf,
    pub port: u16,
    pub mask: bool,
    pub save: bool,
    pub plain: bool,
    /// The game's folder, for the names of playlists, modes and maps; found when not given.
    pub game: Option<PathBuf>,
}

/// Handles Ctrl+C, Ctrl+Break and closing the window: puts the system proxy back, then
/// lets Windows end the process as usual. Windows doesn't call it on sign-out or shutdown,
/// because WinINet loads user32 into mmr, so after those the next run's recovery puts the
/// settings back.
///
/// Windows runs it on a new thread while the rest of mmr keeps going, so it must not
/// panic, which would abort, or wait on a lock held for long. The only one it takes,
/// sysproxy's, is held only while settings are applied.
extern "system" fn on_console(_: u32) -> BOOL {
    use std::io::Write;
    let said = match sysproxy::restore() {
        Some(Ok(())) => "Put the system proxy settings back.".to_string(),
        Some(Err(e)) => e,
        None => return 0,
    };
    let _ = writeln!(std::io::stderr(), "\n{said}");
    0
}

fn describe(s: &sysproxy::Settings) -> String {
    use windows_sys::Win32::Networking::WinInet::{
        PROXY_TYPE_AUTO_DETECT, PROXY_TYPE_AUTO_PROXY_URL, PROXY_TYPE_PROXY,
    };
    let mut parts = Vec::new();
    if s.flags & PROXY_TYPE_PROXY != 0 {
        parts.push(format!("proxy {}", s.server.as_deref().unwrap_or("?")));
    }
    if s.flags & PROXY_TYPE_AUTO_PROXY_URL != 0 {
        parts.push(format!("setup script {}", s.autoconfig.as_deref().unwrap_or("?")));
    }
    if s.flags & PROXY_TYPE_AUTO_DETECT != 0 {
        parts.push("detect automatically".to_string());
    }
    if parts.is_empty() {
        "no proxy".to_string()
    } else {
        parts.join(", ")
    }
}

pub fn run(o: Options) -> Result<(), String> {
    let _running = sysproxy::hold()?;
    if let Some(said) = sysproxy::recover() {
        println!("{said}");
    }
    let root = cert::Root::load(&o.store)?
        .ok_or_else(|| format!("No root saved in {}: run `mmr cert new` first.", cert::cert_dir(&o.store).display()))?;
    if root.days_left() < 1 {
        return Err("The root has expired: run `mmr cert new`.".into());
    }
    if !cert::store::has(&root.der)? {
        return Err("The saved root isn't in your Root store: run `mmr cert new`.".into());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let proxy = Arc::new(proxy::Proxy::new(&root, o.port, tx)?);
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| e.to_string())?;
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(("127.0.0.1", o.port)))
        .map_err(|e| format!("can't listen on 127.0.0.1:{}: {e} (another program may have it: try --port)", o.port))?;
    // SAFETY: on_console is a 'static fn with the handler's signature.
    if unsafe { SetConsoleCtrlHandler(Some(on_console), 1) } == 0 {
        return Err("can't watch for Ctrl+C, so the system proxy wouldn't be put back: not starting".into());
    }
    let guard = sysproxy::set(o.port)?;
    runtime.spawn(proxy.serve(listener));
    names::follow(o.game.clone());
    let tui = !o.plain && std::io::stdout().is_terminal();
    let mut state = State::new(o, describe(&guard.before));
    if tui {
        if let Err(e) = crate::tui::run(&mut state, &rx) {
            eprintln!("The full-screen view stopped ({e}); switching to one line per change.");
            plain(&mut state, &rx);
        }
    } else {
        plain(&mut state, &rx);
    }
    // Put back here rather than when the guard drops, to say whether it worked. None
    // means the console handler got there first, and said so.
    if let Some(r) = sysproxy::restore() {
        r?;
        println!("Put the system proxy settings back ({}).", state.before);
    }
    drop(guard);
    Ok(())
}

/// One line per change, and a match's table when its id becomes known.
fn plain(state: &mut State, rx: &Receiver<Out>) {
    println!("mmr watch: proxy on 127.0.0.1:{}. Ctrl+C puts back the system proxy ({}).", state.o.port, state.before);
    // Load only the names, so saved matches don't count in the match numbers.
    if state.o.save {
        gamertags::load(&state.o.store);
    }
    let mut printed = state.logged;
    // The proxy holds a sender for as long as it runs, so this only ends on exit.
    for out in rx {
        state.take(out);
        let fresh = (state.logged - printed) as usize;
        for entry in state.log.iter().skip(state.log.len().saturating_sub(fresh)) {
            println!("{}", state.text(entry, state.o.mask));
            if let Entry::Event { e, .. } = entry {
                if matches!(e.change, Change::Named { .. }) {
                    if let Some(m) = state.match_of(&e.key) {
                        print!("{}", view::table(m, state.number(&m.key), state.o.mask));
                    }
                }
            }
        }
        printed = state.logged;
    }
}

/// A line of the change log.
pub enum Entry {
    Event { at: String, e: Event },
    Note { at: String, text: String, problem: bool },
}

const LOG: usize = 1000;

/// What `watch` knows, whichever way it's shown.
pub struct State {
    pub o: Options,
    /// A description of the system proxy settings that go back on exit.
    pub before: String,
    pub p: Pipeline,
    pub lobby_open: u32,
    pub sessions_open: u32,
    pub problems: BTreeMap<String, u64>,
    pub log: VecDeque<Entry>,
    /// Entries ever logged, including those dropped from the front.
    pub logged: u64,
    /// The match that changed last.
    pub current: Option<String>,
    /// When each match last changed, by key: as an instant and as local time.
    pub changed: HashMap<String, (Instant, String)>,
    /// When each player in each match last changed.
    pub touched: HashMap<(String, u64), Instant>,
}

impl State {
    pub fn new(o: Options, before: String) -> State {
        let mut p = Pipeline::default();
        if o.save {
            p.resume_from = Some(o.store.clone());
        }
        State {
            o,
            before,
            p,
            lobby_open: 0,
            sessions_open: 0,
            problems: BTreeMap::new(),
            log: VecDeque::new(),
            logged: 0,
            current: None,
            changed: HashMap::new(),
            touched: HashMap::new(),
        }
    }

    /// The match an event belongs to now. Rosters kept under a game session move to a
    /// match once it's named, possibly later in the same message, so a session's key
    /// finds the session's current match.
    pub fn match_of(&self, key: &str) -> Option<&Match> {
        let t = &self.p.tracker;
        t.matches.get(key).or_else(|| t.session(key))
    }

    /// The match's number, counting from 1 in the order matches were seen.
    pub fn number(&self, key: &str) -> usize {
        self.p.tracker.order.iter().position(|k| k == key).map_or(0, |i| i + 1)
    }

    /// Puts the matches saved by earlier runs in front of this run's, oldest first,
    /// so they can be compared. Nothing is loaded with --no-save.
    pub fn load_saved(&mut self) {
        if !self.o.save {
            return;
        }
        match store::load_all(&self.o.store) {
            Ok(saved) => saved.into_iter().for_each(|m| {
                gamertags::learn(&m);
                self.p.tracker.resume(m);
            }),
            Err(e) => *self.problems.entry(format!("can't read the saved matches: {e}")).or_default() += 1,
        }
    }

    pub fn problem_count(&self) -> u64 {
        self.p.decoder.problems.values().chain(self.problems.values()).sum()
    }

    /// An entry as text, with its local time first.
    pub fn text(&self, entry: &Entry, mask: bool) -> String {
        match entry {
            Entry::Event { at, e } => match self.match_of(&e.key) {
                Some(m) => format!("{at}  {}", view::trace(m, self.number(&m.key), e, mask)),
                None => format!("{at}  a match that's gone"),
            },
            Entry::Note { at, text, problem: true } => format!("{at}  Problem: {text}"),
            Entry::Note { at, text, .. } => format!("{at}  {text}"),
        }
    }

    fn push(&mut self, entry: Entry) {
        self.log.push_back(entry);
        self.logged += 1;
        while self.log.len() > LOG {
            self.log.pop_front();
        }
    }

    fn note(&mut self, text: &str, problem: bool) {
        self.push(Entry::Note { at: clock::local_clock(), text: text.to_string(), problem });
    }

    fn save(&mut self, key: &str) {
        if !self.o.save {
            return;
        }
        let Some(k) = self.match_of(key).filter(|m| m.has_roster()).map(|m| m.key.clone()) else { return };
        // Save the names in the game's cache now, since it may not have them later.
        if let Some(m) = self.p.tracker.matches.get_mut(&k) {
            m.setup.named = names::of(&m.setup);
        }
        if let Some(m) = self.p.tracker.matches.get(&k) {
            if let Err(e) = store::save(&self.o.store, m) {
                *self.problems.entry(format!("can't save a match: {e}")).or_default() += 1;
            }
        }
    }

    pub fn take(&mut self, out: Out) {
        match out {
            Out::Item(it) => {
                let events = self.p.item(&it);
                // A document that only adds gamertags makes no events, so nothing
                // else saves what it added.
                if let Some(k) = self.p.doc_match.take() {
                    if !events.iter().any(|e| e.key == k) {
                        self.save(&k);
                    }
                }
                self.events(events);
            }
            Out::Problem(what) => {
                self.note(&what, true);
                *self.problems.entry(what).or_default() += 1;
            }
            Out::Opened(Host::Lobby) => {
                self.lobby_open += 1;
                self.note("Lobby connection opened", false);
            }
            Out::Closed(Host::Lobby, conn) => {
                self.lobby_open = self.lobby_open.saturating_sub(1);
                self.p.decoder.close(conn);
                self.note("Lobby connection closed", false);
            }
            Out::Opened(Host::SessionDirectory) => self.sessions_open += 1,
            Out::Closed(Host::SessionDirectory, _) => self.sessions_open = self.sessions_open.saturating_sub(1),
        }
    }

    pub fn events(&mut self, events: Vec<Event>) {
        let now = Instant::now();
        let mut keys: Vec<String> = Vec::new();
        for mut e in events {
            let Some(m) = self.match_of(&e.key) else { continue };
            e.key.clone_from(&m.key);
            let xuid = match e.change {
                Change::Joined { xuid, .. }
                | Change::Mmr { xuid, .. }
                | Change::Team { xuid, .. }
                | Change::Xbox { xuid, .. }
                | Change::Left { xuid }
                | Change::Back { xuid } => Some(xuid),
                Change::Named { .. } => None,
            };
            if let Some(x) = xuid {
                self.touched.insert((e.key.clone(), x), now);
            }
            if !keys.contains(&e.key) {
                keys.push(e.key.clone());
            }
            self.push(Entry::Event { at: clock::local_clock(), e });
        }
        for k in &keys {
            self.save(k);
            self.changed.insert(k.clone(), (now, clock::local_clock()));
        }
        if let Some(k) = keys.pop() {
            self.current = Some(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::{Item, Kind};
    use crate::roster::{Entry as Row, Roster};

    fn state(dir: &std::path::Path) -> State {
        let o = Options { store: dir.to_path_buf(), port: 0, mask: false, save: true, plain: true, game: None };
        State::new(o, String::new())
    }

    fn doc(session: &str, xuid: u64, gamertag: &str) -> Out {
        let d = serde_json::json!({
            "constants": {"custom": {"gameSessionId": session}},
            "members": {"0": {"gamertag": gamertag, "constants": {"system": {"xuid": xuid.to_string()}}}}
        });
        let at = "2026-11-11T01:00:00.000Z".to_string();
        Out::Item(Item { kind: Kind::Document, conn: 0, at, payload: serde_json::to_vec(&d).unwrap() })
    }

    #[test]
    fn a_document_that_only_names_players_saves_the_match_it_went_to() {
        let dir = store::scratch("named");
        let mut s = state(&dir);
        let x = 9_100_000_000_000_101;
        let roster = |id: &str| Roster {
            match_id: Some(id.into()),
            entries: vec![Row { xuid: x, mmr: Some(1.0), team: Some(0), party: None, leader: false }],
            ..Roster::default()
        };
        // Two matches in one game session, as in a custom game; the second is current.
        s.p.tracker.apply("s", None, "2026-11-11T00:00:00.000Z", &roster("m1"));
        s.p.tracker.apply("s", None, "2026-11-11T00:30:00.000Z", &roster("m2"));
        s.take(doc("S", x, "Named"));
        let saved = store::load(&dir, "m2").unwrap().expect("m2 saved");
        assert_eq!(saved.players[&x].gamertag.as_deref(), Some("Named"));
        assert!(store::load(&dir, "m1").unwrap().is_none(), "m1 didn't change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_match_that_cant_be_read_is_a_problem() {
        let dir = store::scratch("problem");
        std::fs::create_dir_all(store::matches_dir(&dir)).unwrap();
        std::fs::write(store::path_of(&dir, "s"), b"not json").unwrap();
        let mut s = state(&dir);
        s.take(doc("s", 9_100_000_000_000_102, "G"));
        assert_eq!(s.problem_count(), 1);
        assert!(store::matches_dir(&dir).join("s.json.bad").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

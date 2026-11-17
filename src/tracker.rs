//! Each match's players: every change to their rating and team, in order.
//!
//! A match is keyed by its match id, the one in `/hi/matches/{id}/stats`. Rosters
//! seen before the match id is known are kept under their game session, and move
//! to the match once a roster names it.

use crate::clock;
use crate::roster::{Roster, Setup};
use serde::{Deserialize, Serialize};
use std::collections::{btree_map, BTreeMap, HashMap, HashSet};

/// Without Xbox documents, a player who arrives more than this many seconds after
/// teams were set joined in progress; sooner, they were still loading in.
const LOADING: i64 = 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mark<T> {
    pub at: String,
    pub state: u32,
    pub value: T,
}

/// What the matchmaker wrote in the Xbox session document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Xbox {
    pub mmr: Option<f64>,
    pub team_balance_mmr: Option<f64>,
    pub initial_team: Option<String>,
}

impl Xbox {
    /// Fills each field this one lacks from `o`, and returns whether any was filled.
    fn fill(&mut self, o: Xbox) -> bool {
        let a = fill(&mut self.mmr, o.mmr);
        let b = fill(&mut self.team_balance_mmr, o.team_balance_mmr);
        let c = fill(&mut self.initial_team, o.initial_team);
        a || b || c
    }
}

/// Sets `to` from `from` when `to` is empty, and returns whether it did.
fn fill<T>(to: &mut Option<T>, from: Option<T>) -> bool {
    let gave = to.is_none() && from.is_some();
    if gave {
        *to = from;
    }
    gave
}

/// Appends `from`'s marks to `to`, dropping any that repeat the last value.
fn extend<T: PartialEq>(to: &mut Vec<Mark<T>>, from: Vec<Mark<T>>) {
    for m in from {
        if to.last().is_none_or(|l| l.value != m.value) {
            to.push(m);
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub gamertag: Option<String>,
    /// The roster's `mmr`, one mark per change.
    pub mmr: Vec<Mark<Option<f64>>>,
    pub team: Vec<Mark<Option<u16>>>,
    /// The rating in the first state that gave this player a team.
    pub mmr_when_teamed: Option<f64>,
    pub party: Option<String>,
    pub leader: bool,
    pub first_state: Option<u32>,
    pub last_state: Option<u32>,
    pub states: u32,
    pub xbox: Option<Xbox>,
    /// When the player was first missing from the roster, while they still are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_at: Option<String>,
}

impl Player {
    pub fn latest_mmr(&self) -> Option<f64> {
        self.mmr.last().and_then(|m| m.value)
    }

    pub fn latest_team(&self) -> Option<u16> {
        self.team.last().and_then(|m| m.value)
    }

    /// The matchmaker's MMR for this player, from the Xbox document.
    pub fn xbox_mmr(&self) -> Option<f64> {
        self.xbox.as_ref().and_then(|x| x.mmr)
    }

    /// Moves every state this record names `by` later.
    fn shift(&mut self, by: u32) {
        for s in [&mut self.first_state, &mut self.last_state].into_iter().flatten() {
            *s += by;
        }
        self.mmr.iter_mut().for_each(|m| m.state += by);
        self.team.iter_mut().for_each(|m| m.state += by);
    }

    /// Merges `later`, this player's record from later states. Values set once, like
    /// the gamertag and first state, keep this record's and fill gaps from `later`.
    /// The latest state's party, leader and leave time come from `later` if it saw them.
    fn merge(&mut self, later: Player) {
        fill(&mut self.gamertag, later.gamertag);
        match (&mut self.xbox, later.xbox) {
            (Some(x), Some(l)) => {
                x.fill(l);
            }
            (x, l) => {
                fill(x, l);
            }
        }
        if !self.team.iter().any(|m| m.value.is_some()) {
            self.mmr_when_teamed = later.mmr_when_teamed;
        }
        fill(&mut self.first_state, later.first_state);
        if later.last_state.is_some() {
            self.last_state = later.last_state;
            self.party = later.party;
            self.leader = later.leader;
            self.left_at = later.left_at;
        }
        self.states += later.states;
        extend(&mut self.mmr, later.mmr);
        extend(&mut self.team, later.team);
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Match {
    /// The match id once known, else the game session's id.
    pub key: String,
    pub match_id: Option<String>,
    pub game_session: Option<String>,
    pub owner: Option<u64>,
    pub first_seen: String,
    pub last_seen: String,
    pub states: u32,
    pub players: BTreeMap<u64, Player>,
    /// The playlist, mode and map, from the roster's settings.
    #[serde(default, skip_serializing_if = "Setup::is_empty")]
    pub setup: Setup,
}

/// How a match reads, the same in the table and the full-screen view.
impl Match {
    /// Whether a roster arrived. A game session with no roster, as when the match was
    /// canceled, has only its documents and isn't a match.
    pub fn has_roster(&self) -> bool {
        self.players.values().any(|p| p.first_state.is_some())
    }

    /// Whether `p` is in the latest roster state.
    pub fn present(&self, p: &Player) -> bool {
        p.last_state == Some(self.states.saturating_sub(1))
    }

    /// Whether `p` joined in progress. With Xbox documents, a player the matchmaker
    /// didn't rate joined late if they came after teams were first set. Without them,
    /// a later player joined late only if they came over `LOADING` seconds after;
    /// sooner, they were still loading in.
    pub fn late(&self, p: &Player) -> bool {
        let Some(first) = p.first_state else { return false };
        // The first state that gave anyone a team, and when.
        let start = self
            .players
            .values()
            .filter_map(|p| p.team.iter().find(|t| t.value.is_some()).map(|t| (t.state, t.at.as_str())))
            .min();
        let documented = self.players.values().any(|p| p.xbox_mmr().is_some());
        match (documented, p.xbox_mmr(), start) {
            (true, Some(_), _) => false,
            (true, None, s) => first > s.map_or(0, |s| s.0),
            (false, _, Some((s, at))) => {
                let came = p.mmr.first().map(|m| m.at.as_str()).and_then(clock::seconds);
                first > s && came.zip(clock::seconds(at)).is_some_and(|(a, b)| a - b > LOADING)
            }
            (false, _, None) => false,
        }
    }

    /// Each party with two or more players still in.
    pub fn parties(&self) -> HashSet<&str> {
        let mut sizes: HashMap<&str, usize> = HashMap::new();
        for p in self.players.values().filter(|p| self.present(p)) {
            if let Some(k) = p.party.as_deref() {
                *sizes.entry(k).or_default() += 1;
            }
        }
        sizes.into_iter().filter(|(_, n)| *n > 1).map(|(k, _)| k).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The match id became known; the match was kept under `from` until now.
    Named {
        from: Option<String>,
    },
    Joined {
        xuid: u64,
        mmr: Option<f64>,
        team: Option<u16>,
    },
    Mmr {
        xuid: u64,
        from: Option<f64>,
        to: Option<f64>,
    },
    Team {
        xuid: u64,
        from: Option<u16>,
        to: Option<u16>,
    },
    /// The Xbox result gave a new field; `mmr` is its MMR now.
    Xbox {
        xuid: u64,
        mmr: Option<f64>,
    },
    /// In the last roster state, missing from this one.
    Left {
        xuid: u64,
    },
    /// Left, and in this roster state again.
    Back {
        xuid: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub key: String,
    pub at: String,
    pub change: Change,
}

#[derive(Default)]
pub struct Tracker {
    pub matches: BTreeMap<String, Match>,
    /// Matches in the order they were first seen.
    pub order: Vec<String>,
    /// Each game session's current match.
    current: HashMap<String, String>,
}

impl Tracker {
    fn entry(&mut self, key: &str) -> &mut Match {
        match self.matches.entry(key.to_string()) {
            btree_map::Entry::Occupied(e) => e.into_mut(),
            btree_map::Entry::Vacant(e) => {
                self.order.push(key.to_string());
                e.insert(Match { key: key.to_string(), ..Default::default() })
            }
        }
    }

    /// Picks up a match saved by an earlier run, so a restart continues it. A game
    /// session continues in the match seen last.
    pub fn resume(&mut self, m: Match) {
        if self.matches.contains_key(&m.key) {
            return;
        }
        if let Some(s) = &m.game_session {
            let newer = self.session(s).is_none_or(|c| c.last_seen <= m.last_seen);
            if newer {
                self.current.insert(s.clone(), m.key.clone());
            }
        }
        self.order.push(m.key.clone());
        self.matches.insert(m.key.clone(), m);
    }

    pub fn has(&self, key: &str) -> bool {
        self.matches.contains_key(key)
    }

    /// Game session `session`'s current match.
    pub fn session(&self, session: &str) -> Option<&Match> {
        self.current.get(session).and_then(|k| self.matches.get(k))
    }

    /// Where a roster from game session `session` belongs. `session` is empty when
    /// the link it came on was opened before mmr was watching.
    pub fn key_for(&self, session: &str, r: &Roster) -> String {
        if let Some(m) = &r.match_id {
            return m.clone();
        }
        if let Some(k) = self.current.get(session) {
            return k.clone();
        }
        if session.is_empty() {
            "unknown".to_string()
        } else {
            session.to_string()
        }
    }

    /// Moves what was kept under `from` into `to`, its states after any `to` has. A
    /// player in both keeps what `to` has and gains what only `from` had.
    fn rename(&mut self, from: &str, to: &str) -> bool {
        // Only rosters still waiting for their match id move; a finished match stays.
        if self.matches.get(from).is_none_or(|m| m.match_id.is_some()) {
            return false;
        }
        let Some(old) = self.matches.remove(from) else { return false };
        self.order.retain(|k| k != from);
        let m = self.entry(to);
        m.game_session = m.game_session.take().or(old.game_session);
        m.owner = m.owner.or(old.owner);
        if m.first_seen.is_empty() || (!old.first_seen.is_empty() && old.first_seen < m.first_seen) {
            m.first_seen = old.first_seen;
        }
        if old.last_seen > m.last_seen {
            m.last_seen = old.last_seen;
        }
        // States count from each match's first, so the moved ones follow `to`'s.
        let before = m.states;
        m.states += old.states;
        adopt(&mut m.setup, &old.setup);
        for (x, mut p) in old.players {
            p.shift(before);
            match m.players.entry(x) {
                btree_map::Entry::Occupied(mut e) => e.get_mut().merge(p),
                btree_map::Entry::Vacant(e) => {
                    e.insert(p);
                }
            }
        }
        true
    }

    /// Applies one roster state. A player missing from it is marked as left, not removed.
    pub fn apply(&mut self, session: &str, owner: Option<u64>, at: &str, r: &Roster) -> Vec<Event> {
        let key = self.key_for(session, r);
        let mut changes = Vec::new();
        if !session.is_empty() {
            let before = self.current.insert(session.to_string(), key.clone());
            if r.match_id.is_some() {
                // Rosters before the match id were kept under the session.
                let from = before.filter(|b| *b != key).or_else(|| Some(session.to_string()).filter(|s| *s != key));
                if let Some(from) = from {
                    if self.rename(&from, &key) {
                        changes.push(Change::Named { from: Some(from) });
                    }
                }
            }
        }
        let fresh = !self.matches.contains_key(&key);
        let m = self.entry(&key);
        if fresh && r.match_id.is_some() {
            changes.push(Change::Named { from: None });
        }
        let state = m.states;
        m.states += 1;
        if m.first_seen.is_empty() {
            m.first_seen = at.to_string();
        }
        m.last_seen = at.to_string();
        m.owner = m.owner.or(owner);
        if m.match_id.is_none() {
            m.match_id.clone_from(&r.match_id);
        }
        adopt(&mut m.setup, &r.setup);
        if !session.is_empty() && m.game_session.is_none() {
            m.game_session = Some(session.to_string());
        }
        for e in &r.entries {
            let p = m.players.entry(e.xuid).or_default();
            let joined = p.first_state.is_none();
            if joined {
                p.first_state = Some(state);
                changes.push(Change::Joined { xuid: e.xuid, mmr: e.mmr, team: e.team });
            }
            p.last_state = Some(state);
            if p.left_at.take().is_some() {
                changes.push(Change::Back { xuid: e.xuid });
            }
            p.states += 1;
            let last_mmr = p.mmr.last().map(|m| m.value);
            if last_mmr != Some(e.mmr) {
                p.mmr.push(Mark { at: at.to_string(), state, value: e.mmr });
                if !joined {
                    changes.push(Change::Mmr { xuid: e.xuid, from: last_mmr.flatten(), to: e.mmr });
                }
            }
            let teamed_before = p.team.iter().any(|m| m.value.is_some());
            if !teamed_before && e.team.is_some() {
                p.mmr_when_teamed = e.mmr;
            }
            let last_team = p.team.last().map(|m| m.value);
            if last_team != Some(e.team) {
                p.team.push(Mark { at: at.to_string(), state, value: e.team });
                if !joined {
                    changes.push(Change::Team { xuid: e.xuid, from: last_team.flatten(), to: e.team });
                }
            }
            p.party.clone_from(&e.party);
            p.leader = e.leader;
        }
        if state > 0 {
            for (x, p) in &mut m.players {
                if p.last_state == Some(state - 1) {
                    p.left_at = Some(at.to_string());
                    changes.push(Change::Left { xuid: *x });
                }
            }
        }
        changes.into_iter().map(|change| Event { key: key.clone(), at: at.to_string(), change }).collect()
    }

    /// An Xbox session document. A field already known keeps its first value: the
    /// matchmaking result is written once, and later documents can lack it or part
    /// of it. Lobby and party documents name no game session and give `None`.
    pub fn apply_document(&mut self, at: &str, doc: &serde_json::Value) -> Option<(String, Vec<Event>)> {
        let session = document_session(doc)?;
        let members = doc.get("members")?.as_object()?;
        let key = self.current.get(&session).cloned().unwrap_or_else(|| session.clone());
        self.current.insert(session.clone(), key.clone());
        let m = self.entry(&key);
        m.game_session.get_or_insert(session);
        let mut changes = Vec::new();
        for member in members.values() {
            let Some(xuid) =
                member.pointer("/constants/system/xuid").and_then(|x| x.as_str()).and_then(|x| x.parse::<u64>().ok())
            else {
                continue;
            };
            let p = m.players.entry(xuid).or_default();
            let gamertag = member.get("gamertag").and_then(|g| g.as_str());
            if let Some(g) = gamertag {
                crate::gamertags::saw(xuid, g, at);
            }
            if p.gamertag.is_none() {
                p.gamertag = gamertag.map(str::to_string);
            }
            let Some(result) = member.pointer("/constants/custom/matchmakingResult") else { continue };
            let attr = |k: &str| result.get("playerAttrs").and_then(|a| a.get(k)).and_then(serde_json::Value::as_f64);
            let x = Xbox {
                mmr: attr("MMR"),
                team_balance_mmr: attr("trueMatchTeamBalanceMmr"),
                initial_team: result
                    .get("initialTeam")
                    .filter(|t| !t.is_null())
                    .map(|t| t.as_str().map_or_else(|| t.to_string(), str::to_string)),
            };
            let xbox = p.xbox.get_or_insert_default();
            if xbox.fill(x) {
                changes.push(Change::Xbox { xuid, mmr: xbox.mmr });
            }
        }
        let events = changes.into_iter().map(|change| Event { key: key.clone(), at: at.to_string(), change }).collect();
        Some((key, events))
    }
}

/// Overwrites each kept setting that `s` also gives.
fn adopt(keep: &mut Setup, s: &Setup) {
    fn over(keep: &mut Option<String>, s: Option<&String>) {
        if let Some(v) = s {
            keep.get_or_insert_default().clone_from(v);
        }
    }
    over(&mut keep.experience, s.experience.as_ref());
    keep.ranked = s.ranked.or(keep.ranked);
    over(&mut keep.playlist, s.playlist.as_ref());
    over(&mut keep.mode, s.mode.as_ref());
    over(&mut keep.map, s.map.as_ref());
    over(&mut keep.pair, s.pair.as_ref());
}

/// The game session an Xbox session document is for, lowercased. An empty id is none.
pub fn document_session(doc: &serde_json::Value) -> Option<String> {
    let s = doc.pointer("/constants/custom/gameSessionId")?.as_str()?;
    (!s.is_empty()).then(|| s.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{Entry, Roster};

    fn e(xuid: u64, mmr: Option<f64>, team: Option<u16>) -> Entry {
        Entry { xuid, mmr, team, party: None, leader: false }
    }

    fn r(entries: Vec<Entry>) -> Roster {
        Roster { match_id: None, entries, ..Default::default() }
    }

    fn named(id: &str, entries: Vec<Entry>) -> Roster {
        Roster { match_id: Some(id.to_string()), entries, ..Default::default() }
    }

    #[test]
    fn a_player_missing_from_the_next_state_left() {
        let mut t = Tracker::default();
        t.apply("s", None, "a", &r(vec![e(1, Some(1.0), Some(0)), e(2, Some(2.0), Some(1))]));
        let ev = t.apply("s", None, "b", &r(vec![e(1, Some(1.0), Some(0))]));
        assert_eq!(ev, vec![Event { key: "s".into(), at: "b".into(), change: Change::Left { xuid: 2 } }]);
        assert!(t.apply("s", None, "c", &r(vec![e(1, Some(1.0), Some(0))])).is_empty());
    }

    #[test]
    fn only_changes_are_marked() {
        let mut t = Tracker::default();
        assert_eq!(t.apply("s", None, "a", &r(vec![e(1, Some(1.0), None)])).len(), 1);
        assert!(t.apply("s", None, "b", &r(vec![e(1, Some(1.0), None)])).is_empty());
        let ev = t.apply("s", None, "c", &r(vec![e(1, Some(2.0), None)]));
        assert_eq!(ev[0].change, Change::Mmr { xuid: 1, from: Some(1.0), to: Some(2.0) });
        let p = &t.matches["s"].players[&1];
        assert_eq!(p.mmr.iter().map(|m| m.value).collect::<Vec<_>>(), vec![Some(1.0), Some(2.0)]);
        assert_eq!(p.states, 3);
    }

    #[test]
    fn the_rating_when_teamed_is_kept_when_it_later_moves() {
        let mut t = Tracker::default();
        t.apply("s", None, "a", &r(vec![e(1, Some(1.0), None)]));
        t.apply("s", None, "b", &r(vec![e(1, Some(2.0), Some(0))]));
        t.apply("s", None, "c", &r(vec![e(1, Some(3.0), Some(1))]));
        let p = &t.matches["s"].players[&1];
        assert_eq!(p.mmr_when_teamed, Some(2.0));
        assert_eq!(p.latest_mmr(), Some(3.0));
    }

    #[test]
    fn a_joiner_and_a_gap() {
        let mut t = Tracker::default();
        t.apply("s", None, "a", &r(vec![e(1, Some(1.0), None)]));
        t.apply("s", None, "b", &r(vec![e(1, Some(1.0), None), e(2, None, None)]));
        t.apply("s", None, "c", &r(vec![e(2, Some(5.0), None)]));
        let s = &t.matches["s"];
        assert_eq!(s.players[&2].first_state, Some(1));
        assert_eq!(s.players[&1].last_state, Some(1));
        assert_eq!(s.players[&1].latest_mmr(), Some(1.0));
        assert_eq!(s.players[&2].mmr.len(), 2);
    }

    #[test]
    fn rosters_before_the_match_id_move_to_the_match() {
        let mut t = Tracker::default();
        t.apply("s", None, "a", &r(vec![e(1, Some(1.0), None)]));
        let ev = t.apply("s", None, "b", &named("m", vec![e(1, Some(1.0), Some(0)), e(2, Some(2.0), Some(1))]));
        assert_eq!(ev[0].change, Change::Named { from: Some("s".into()) });
        assert_eq!(t.order, vec!["m".to_string()]);
        let m = &t.matches["m"];
        assert_eq!((m.states, m.game_session.as_deref(), m.match_id.as_deref()), (2, Some("s"), Some("m")));
        assert_eq!(m.players[&1].mmr.len(), 1);
        // A later roster without the id stays with the match.
        t.apply("s", None, "c", &r(vec![e(1, Some(1.5), Some(0))]));
        assert_eq!(t.matches["m"].states, 3);
    }

    #[test]
    fn a_second_match_in_one_session_is_its_own() {
        let mut t = Tracker::default();
        t.apply("s", None, "a", &named("m1", vec![e(1, Some(1.0), None)]));
        t.apply("s", None, "b", &named("m2", vec![e(1, Some(1.1), None)]));
        assert_eq!(t.order, vec!["m1".to_string(), "m2".to_string()]);
        assert_eq!(t.matches["m1"].players[&1].latest_mmr(), Some(1.0));
    }

    #[test]
    fn a_link_opened_before_watching_still_finds_its_match() {
        let mut t = Tracker::default();
        t.apply("s", Some(9), "a", &named("m", vec![e(1, Some(1.0), None)]));
        t.apply("", None, "b", &named("m", vec![e(1, Some(1.2), None)]));
        assert_eq!(t.order.len(), 1);
        assert_eq!(t.matches["m"].players[&1].latest_mmr(), Some(1.2));
    }

    #[test]
    fn a_document_merges_with_the_roster() {
        let mut t = Tracker::default();
        t.apply("abc", None, "a", &named("m", vec![e(7, Some(1.0), None)]));
        let doc = serde_json::json!({
            "constants": {"custom": {"gameSessionId": "ABC"}},
            "members": {"0": {"gamertag": "G", "constants": {
                "system": {"xuid": "7"},
                "custom": {"matchmakingResult": {"initialTeam": 1, "playerAttrs": {"MMR": 2.5}}}}}}
        });
        let (key, ev) = t.apply_document("t", &doc).unwrap();
        assert_eq!(key, "m");
        assert_eq!(ev[0].change, Change::Xbox { xuid: 7, mmr: Some(2.5) });
        let p = &t.matches["m"].players[&7];
        assert_eq!(p.gamertag.as_deref(), Some("G"));
        let x = p.xbox.as_ref().unwrap();
        assert_eq!((x.mmr, x.initial_team.as_deref()), (Some(2.5), Some("1")));
        assert_eq!(p.latest_mmr(), Some(1.0));
    }

    #[test]
    fn the_newest_saved_match_is_its_sessions_current() {
        let saved = |key: &str, last: &str| Match {
            key: key.into(),
            match_id: Some(key.into()),
            game_session: Some("s".into()),
            last_seen: last.into(),
            ..Match::default()
        };
        for newest_first in [false, true] {
            let mut t = Tracker::default();
            let (old, new) = (saved("old", "2026-11-11T00:00:00.000Z"), saved("new", "2026-11-11T01:00:00.000Z"));
            if newest_first {
                t.resume(new);
                t.resume(old);
            } else {
                t.resume(old);
                t.resume(new);
            }
            assert_eq!(t.key_for("s", &r(vec![])), "new");
        }
    }

    fn result(session: &str, xuid: u64, gamertag: Option<&str>, r: serde_json::Value) -> serde_json::Value {
        let mut member = serde_json::json!({"constants": {"system": {"xuid": xuid.to_string()}}});
        if !r.is_null() {
            member["constants"]["custom"]["matchmakingResult"] = r;
        }
        if let Some(g) = gamertag {
            member["gamertag"] = g.into();
        }
        serde_json::json!({"constants": {"custom": {"gameSessionId": session}}, "members": {"0": member}})
    }

    #[test]
    fn a_player_who_left_and_comes_back_is_back() {
        let mut t = Tracker::default();
        t.apply("s", None, "a", &r(vec![e(1, Some(1.0), Some(0)), e(2, Some(2.0), Some(1))]));
        t.apply("s", None, "b", &r(vec![e(1, Some(1.0), Some(0))]));
        let ev = t.apply("s", None, "c", &r(vec![e(1, Some(1.0), Some(0)), e(2, Some(2.0), Some(1))]));
        assert_eq!(ev, vec![Event { key: "s".into(), at: "c".into(), change: Change::Back { xuid: 2 } }]);
        assert_eq!(t.matches["s"].players[&2].left_at, None);
    }

    #[test]
    fn an_xbox_result_fills_in_field_by_field() {
        let mut t = Tracker::default();
        let x = 9_100_000_000_000_302;
        let partial = serde_json::json!({"initialTeam": null, "playerAttrs": {"trueMatchTeamBalanceMmr": 1.5}});
        let (_, ev) = t.apply_document("a", &result("s", x, None, partial)).unwrap();
        assert_eq!(ev[0].change, Change::Xbox { xuid: x, mmr: None });
        assert_eq!(t.matches["s"].players[&x].xbox.as_ref().unwrap().initial_team, None);
        let full = result("s", x, None, serde_json::json!({"initialTeam": 1, "playerAttrs": {"MMR": 2.5}}));
        let (_, ev) = t.apply_document("b", &full).unwrap();
        assert_eq!(ev[0].change, Change::Xbox { xuid: x, mmr: Some(2.5) });
        let other = serde_json::json!({"initialTeam": 0, "playerAttrs": {"MMR": 3.0, "trueMatchTeamBalanceMmr": 9.0}});
        let (_, ev) = t.apply_document("c", &result("s", x, None, other)).unwrap();
        assert!(ev.is_empty(), "{ev:?}");
        let want = Xbox { mmr: Some(2.5), team_balance_mmr: Some(1.5), initial_team: Some("1".into()) };
        assert_eq!(t.matches["s"].players[&x].xbox, Some(want));
    }

    #[test]
    fn a_player_moved_into_a_match_keeps_what_only_the_session_had() {
        let mut t = Tracker::default();
        let x = 9_100_000_000_000_301;
        t.apply("s2", None, "a", &named("m", vec![e(x, Some(1.0), Some(0))]));
        let doc = result("s", x, Some("Moved"), serde_json::json!({"initialTeam": 0, "playerAttrs": {"MMR": 2.5}}));
        assert_eq!(t.apply_document("b", &doc).unwrap().0, "s");
        t.apply("s", None, "c", &named("m", vec![e(x, Some(1.0), Some(0))]));
        let p = &t.matches["m"].players[&x];
        assert_eq!(p.gamertag.as_deref(), Some("Moved"));
        assert_eq!(p.xbox.as_ref().and_then(|x| x.mmr), Some(2.5));
        assert_eq!((p.first_state, p.last_state, p.states, p.mmr.len()), (Some(0), Some(1), 2, 1));
    }

    #[test]
    fn rosters_moved_into_a_match_with_states_come_after_them() {
        let mut t = Tracker::default();
        t.apply("s2", None, "a", &named("m", vec![e(1, Some(1.0), Some(0))]));
        t.apply("s", None, "b", &r(vec![e(2, Some(2.0), Some(1))]));
        let ev = t.apply("s", None, "c", &named("m", vec![e(1, Some(1.0), Some(0))]));
        let m = &t.matches["m"];
        assert_eq!(m.states, 3);
        let p = &m.players[&2];
        assert_eq!((p.first_state, p.last_state, p.mmr[0].state, p.team[0].state), (Some(1), Some(1), 1, 1));
        assert!(ev.iter().any(|e| e.change == Change::Left { xuid: 2 }), "{ev:?}");
    }

    #[test]
    fn a_match_saved_in_the_first_schema_still_loads() {
        // As the first version wrote it: no settings, and no time a player left.
        let first = r#"{
            "key": "m", "match_id": "m", "game_session": "s", "owner": 1,
            "first_seen": "2026-11-01T00:00:00.000Z", "last_seen": "2026-11-01T00:10:00.000Z", "states": 2,
            "players": {"1": {
                "gamertag": "G",
                "mmr": [{"at": "2026-11-01T00:00:00.000Z", "state": 0, "value": 1.5}],
                "team": [{"at": "2026-11-01T00:00:00.000Z", "state": 0, "value": null}],
                "mmr_when_teamed": null, "party": null, "leader": false,
                "first_state": 0, "last_state": 1, "states": 2, "xbox": null
            }}
        }"#;
        let m: Match = serde_json::from_str(first).unwrap();
        assert_eq!((m.players[&1].latest_mmr(), m.players[&1].left_at.as_deref()), (Some(1.5), None));
        assert!(m.setup.is_empty() && m.players[&1].first_state == Some(0));
        // Settings without a pair or names, which are left out when empty.
        let mut v: serde_json::Value = serde_json::from_str(first).unwrap();
        v["setup"] = serde_json::json!({"experience": "Arena", "ranked": true, "playlist": "p", "mode": "o"});
        let m: Match = serde_json::from_value(v).unwrap();
        assert_eq!((m.setup.mode.as_deref(), m.setup.pair.as_deref()), (Some("o"), None));
    }

    #[test]
    fn only_a_roster_makes_a_match() {
        let mut t = Tracker::default();
        t.apply_document("a", &result("s", 9_100_000_000_000_303, Some("Doc"), serde_json::Value::Null)).unwrap();
        assert!(!t.matches["s"].has_roster());
        t.apply("s", None, "b", &r(vec![e(1, None, None)]));
        assert!(t.matches["s"].has_roster());
    }

    #[test]
    fn with_documents_a_player_the_matchmaker_didnt_rate_joined_late() {
        let mut t = Tracker::default();
        let x = 9_100_000_000_000_304;
        t.apply("s", None, "2026-11-11T00:00:00.000Z", &r(vec![e(x, Some(1.0), Some(0)), e(2, Some(1.0), Some(1))]));
        let rated = serde_json::json!({"initialTeam": 0, "playerAttrs": {"MMR": 1.0}});
        t.apply_document("a", &result("s", x, None, rated)).unwrap();
        let all = vec![e(x, Some(1.0), Some(0)), e(2, Some(1.0), Some(1)), e(3, Some(1.0), Some(1))];
        t.apply("s", None, "2026-11-11T00:00:05.000Z", &r(all));
        let m = &t.matches["s"];
        let late: Vec<bool> = [x, 2, 3].iter().map(|k| m.late(&m.players[k])).collect();
        assert_eq!(late, [false, false, true]);
        assert!([x, 2, 3].iter().all(|k| m.present(&m.players[k])));
    }

    #[test]
    fn a_document_with_an_empty_game_session_is_not_a_match() {
        let mut t = Tracker::default();
        let doc = serde_json::json!({
            "constants": {"custom": {"gameSessionId": ""}},
            "members": {"0": {"gamertag": "G", "constants": {"system": {"xuid": "9100000000000201"}}}}
        });
        assert!(t.apply_document("t", &doc).is_none());
        assert!(t.matches.is_empty() && t.order.is_empty());
    }
}

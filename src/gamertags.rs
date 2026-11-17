//! Gamertags for players a match's own Xbox documents don't name. The session
//! document has a member's gamertag only once they've accepted the match, so a
//! player who accepted after the game last read it, or who joined late, has none
//! there. Any other match that named them, from this run or saved, gives the latest
//! gamertag they had. This module writes nothing: the saved matches keep the names.

use crate::tracker::Match;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

/// Each player's latest gamertag, with when it was seen. Tests run in parallel and
/// share it, so each test uses XUIDs no other test does.
static KNOWN: Mutex<BTreeMap<u64, (String, String)>> = Mutex::new(BTreeMap::new());

/// Keeps `gamertag` for `xuid`, unless a later one is already kept.
pub fn saw(xuid: u64, gamertag: &str, at: &str) {
    let mut known = KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if known.get(&xuid).is_none_or(|(when, _)| when.as_str() <= at) {
        known.insert(xuid, (at.to_string(), gamertag.to_string()));
    }
}

/// Keeps every gamertag `m` names.
pub fn learn(m: &Match) {
    for (x, p) in &m.players {
        if let Some(g) = &p.gamertag {
            saw(*x, g, &m.last_seen);
        }
    }
}

/// Keeps every gamertag in the matches saved in `store`.
pub fn load(store: &Path) {
    if let Ok(all) = crate::store::load_all(store) {
        all.iter().for_each(learn);
    }
}

/// The latest gamertag any match gave `xuid`.
pub fn of(xuid: u64) -> Option<String> {
    KNOWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&xuid).map(|(_, g)| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_gamertag_wins() {
        let x = 9_100_000_000_000_001;
        assert_eq!(of(x), None);
        saw(x, "Later", "2026-11-11T02:00:00.000Z");
        saw(x, "Earlier", "2026-11-11T01:00:00.000Z");
        assert_eq!(of(x).as_deref(), Some("Later"));
        saw(x, "Latest", "2026-11-11T03:00:00.000Z");
        assert_eq!(of(x).as_deref(), Some("Latest"));
    }
}

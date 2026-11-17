//! Text output, and the pieces of it the live view shares. With `mask`, players are
//! letters and ids are left out.

use crate::roster::Setup;
use crate::tracker::{Change, Event, Match, Player};
use crate::{gamertags, names};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;

fn letter(i: usize) -> String {
    let c = (b'A' + (i % 26) as u8) as char;
    if i < 26 {
        c.to_string()
    } else {
        format!("{c}{}", i / 26)
    }
}

pub fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

fn num(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.4}")).unwrap_or_default()
}

/// A player as shown: a letter by XUID order with `mask`, else the gamertag this
/// match or another gave them, or their XUID.
pub fn who(s: &Match, x: u64, mask: bool) -> String {
    if mask {
        return letter(s.players.keys().position(|k| *k == x).unwrap_or(s.players.len()));
    }
    s.players.get(&x).and_then(|p| p.gamertag.clone()).or_else(|| gamertags::of(x)).unwrap_or_else(|| x.to_string())
}

/// Team order, then the highest rating first.
fn ordered(s: &Match) -> Vec<(u64, &Player)> {
    let mut rows: Vec<(u64, &Player)> = s.players.iter().map(|(x, p)| (*x, p)).collect();
    rows.sort_by(|a, b| {
        let t = |p: &Player| p.latest_team().map_or(u32::MAX, u32::from);
        t(a.1)
            .cmp(&t(b.1))
            .then(b.1.latest_mmr().unwrap_or(f64::MIN).total_cmp(&a.1.latest_mmr().unwrap_or(f64::MIN)))
            .then(a.0.cmp(&b.0))
    });
    rows
}

pub fn table(s: &Match, number: usize, mask: bool) -> String {
    let mut o = String::new();
    let head = if number > 0 { format!("Match {number}") } else { "Match".to_string() };
    let id = match (&s.match_id, mask) {
        (_, true) => String::new(),
        (Some(m), false) => m.clone(),
        (None, false) => format!("id not known yet, game session {}", s.game_session.as_deref().unwrap_or(&s.key)),
    };
    let _ = writeln!(o, "\n{head} {id}");
    let _ = writeln!(o, "Roster states {}, players {}", s.states, s.players.len());
    let _ = writeln!(
        o,
        "{:<20} {:>4} {:>9} {:>9} {:>7} {:>5} {:>9}  Note",
        "Player", "Team", "MMR", "At team", "Updates", "Party", "Xbox MMR"
    );
    let rows = ordered(s);
    let parties = parties(s, &rows);
    for &(x, p) in &rows {
        let mut name = who(s, x, mask);
        if s.owner == Some(x) {
            name.push_str(" (you)");
        }
        let party = p
            .party
            .as_deref()
            .filter(|_| s.present(p))
            .and_then(|k| parties.get(k))
            .map(ToString::to_string)
            .unwrap_or_default();
        let team = p.latest_team();
        let _ = writeln!(
            o,
            "{:<20} {:>4} {:>9} {:>9} {:>7} {:>5} {:>9}  {}",
            name,
            team.map(|t| t.to_string()).unwrap_or_default(),
            num(p.latest_mmr()),
            num(p.mmr_when_teamed),
            p.mmr.len().saturating_sub(1),
            party,
            num(p.xbox.as_ref().and_then(|x| x.mmr)),
            notes(s, p).join(", ")
        );
    }
    for (t, (mean, n)) in team_means(s) {
        let _ = writeln!(o, "Team {t}: mean {mean:.4} over {}", plural(n, "player"));
    }
    o
}

/// Each party of two or more still in, numbered from 1 in row order.
fn parties<'a>(s: &'a Match, rows: &[(u64, &'a Player)]) -> HashMap<&'a str, usize> {
    let big = s.parties();
    let mut out = HashMap::new();
    for (_, p) in rows.iter().filter(|(_, p)| s.present(p)) {
        if let Some(k) = p.party.as_deref().filter(|k| big.contains(k)) {
            let n = out.len() + 1;
            out.entry(k).or_insert(n);
        }
    }
    out
}

/// What's unusual about a player in this match.
fn notes(s: &Match, p: &Player) -> Vec<&'static str> {
    let mut notes = Vec::new();
    if p.first_state.is_none() {
        notes.push("not in roster");
    } else if s.late(p) {
        notes.push("joined late");
    }
    if p.first_state.is_some() && !s.present(p) {
        notes.push("left");
    }
    if p.first_state.is_some() && p.latest_mmr().is_none() {
        notes.push("no rating");
    }
    notes
}

/// Each team's mean rating over its rated players still in, and how many.
fn team_means(s: &Match) -> BTreeMap<u16, (f64, usize)> {
    let mut teams: BTreeMap<u16, Vec<f64>> = BTreeMap::new();
    for p in s.players.values().filter(|p| s.present(p)) {
        if let (Some(t), Some(m)) = (p.latest_team(), p.latest_mmr()) {
            teams.entry(t).or_default().push(m);
        }
    }
    teams.into_iter().map(|(t, v)| (t, (v.iter().sum::<f64>() / v.len() as f64, v.len()))).collect()
}

/// One line of the live trace: what changed in match `number`.
pub fn trace(s: &Match, number: usize, e: &Event, mask: bool) -> String {
    let opt = |v: Option<f64>| v.map_or_else(|| "none".to_string(), |x| format!("{x:.4}"));
    let team = |t: Option<u16>| t.map_or_else(|| "none".to_string(), |t| t.to_string());
    let what = match &e.change {
        Change::Named { .. } if mask => "match id known".to_string(),
        Change::Named { .. } => format!("match id {}", s.match_id.as_deref().unwrap_or("?")),
        Change::Joined { xuid, mmr, team: t } => {
            let mut w = format!("{} joined", who(s, *xuid, mask));
            if let Some(t) = t {
                let _ = write!(w, ", team {t}");
            }
            if let Some(m) = mmr {
                let _ = write!(w, ", MMR {m:.4}");
            }
            w
        }
        Change::Mmr { xuid, from, to } => format!("{} MMR {} -> {}", who(s, *xuid, mask), opt(*from), opt(*to)),
        Change::Team { xuid, from, to } => format!("{} team {} -> {}", who(s, *xuid, mask), team(*from), team(*to)),
        Change::Xbox { xuid, mmr } => format!("{} Xbox MMR {}", who(s, *xuid, mask), opt(*mmr)),
        Change::Left { xuid } => format!("{} left", who(s, *xuid, mask)),
        Change::Back { xuid } => format!("{} back", who(s, *xuid, mask)),
    };
    format!("Match {number}: {what}")
}

/// One table for each playlist and mode, latest played first, since Mode MMR only
/// compares within one. Each row has a player's match count there and their latest,
/// lowest and highest Mode MMR. Matches saved before the mode was recorded come
/// last, together.
pub fn history(matches: &[Match], only: Option<u64>, mask: bool) -> String {
    #[derive(Default)]
    struct Row {
        name: Option<String>,
        values: Vec<f64>,
        matches: u32,
        last_seen: String,
    }
    #[derive(Default)]
    struct Group {
        title: String,
        matches: u32,
        last_seen: String,
        rows: BTreeMap<u64, Row>,
    }
    let mut groups: Vec<(Option<(String, String)>, Group)> = Vec::new();
    for s in matches {
        let key = names::mode_key(&s.setup);
        let at = match groups.iter().position(|g| g.0 == key) {
            Some(i) => i,
            None => {
                let title = title(&s.setup, key.as_ref().map(|k| k.1.as_str()));
                groups.push((key, Group { title, ..Group::default() }));
                groups.len() - 1
            }
        };
        let g = &mut groups[at].1;
        let mut here = false;
        for (x, p) in &s.players {
            if only.is_some_and(|o| o != *x) || p.first_state.is_none() {
                continue;
            }
            here = true;
            let r = g.rows.entry(*x).or_default();
            r.matches += 1;
            r.last_seen.clone_from(&s.last_seen);
            if p.gamertag.is_some() {
                r.name.clone_from(&p.gamertag);
            }
            r.values.extend(p.latest_mmr());
        }
        if here {
            g.matches += 1;
            g.last_seen = g.last_seen.clone().max(s.last_seen.clone());
        }
    }
    groups.retain(|g| g.1.matches > 0);
    groups.sort_by(|a, b| a.0.is_none().cmp(&b.0.is_none()).then(b.1.last_seen.cmp(&a.1.last_seen)));
    let mut o = String::new();
    for (n, (_, g)) in groups.iter().enumerate() {
        if n > 0 {
            o.push('\n');
        }
        let count = if g.matches == 1 { "1 match".to_string() } else { format!("{} matches", g.matches) };
        let _ = writeln!(o, "{}   {count}", g.title);
        let _ = writeln!(
            o,
            "{:<20} {:>7} {:>9} {:>9} {:>9}  Last seen",
            "Player", "Matches", "Latest", "Lowest", "Highest"
        );
        let mut list: Vec<(&u64, &Row)> = g.rows.iter().collect();
        list.sort_by(|a, b| b.1.last_seen.cmp(&a.1.last_seen).then(a.0.cmp(b.0)));
        for (i, (x, r)) in list.iter().enumerate() {
            let who = if mask {
                letter(i)
            } else {
                r.name.clone().or_else(|| gamertags::of(**x)).unwrap_or_else(|| x.to_string())
            };
            let low = r.values.iter().copied().reduce(f64::min);
            let high = r.values.iter().copied().reduce(f64::max);
            let _ = writeln!(
                o,
                "{:<20} {:>7} {:>9} {:>9} {:>9}  {}",
                who,
                r.matches,
                num(r.values.last().copied()),
                num(low),
                num(high),
                r.last_seen
            );
        }
    }
    o
}

/// A history table's title: the playlist and the mode, as the game names them.
fn title(s: &Setup, mode: Option<&str>) -> String {
    let Some(mode) = mode else { return "Mode not recorded".to_string() };
    let parts = [Some(names::playlist(s)), names::of(s).mode];
    let parts: Vec<String> = parts.into_iter().flatten().filter(|p| !p.is_empty()).collect();
    // A mode the cache hasn't named goes by the start of its id.
    if parts.is_empty() {
        format!("Mode {}", mode.chars().take(8).collect::<String>())
    } else {
        parts.join("   ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::roster::{Entry, Named, Roster, Setup};
    use crate::tracker::Tracker;

    #[test]
    fn history_keeps_each_playlist_and_mode_to_itself() {
        let mut t = Tracker::default();
        let x = 9_100_000_000_000_501;
        let modes = [Some("strongholds"), Some("slayer"), Some("strongholds"), None];
        for (n, mode) in modes.into_iter().enumerate() {
            let setup = Setup {
                playlist: mode.map(|_| "quick-play".to_string()),
                mode: mode.map(str::to_string),
                ..Setup::default()
            };
            let r = Roster {
                match_id: Some(format!("m{n}")),
                entries: vec![Entry { xuid: x, mmr: Some(10.0 + n as f64), team: Some(0), party: None, leader: false }],
                setup,
            };
            t.apply(&format!("s{n}"), Some(x), &format!("2026-11-11T0{n}:00:00.000Z"), &r);
            // Names are kept as `watch` saves them; a roster never carries any.
            if n < 2 {
                t.matches.get_mut(&format!("m{n}")).unwrap().setup.named =
                    Named { playlist: Some("Quick Play".into()), mode: mode.map(|m| format!("Arena:{m}")), map: None };
            }
        }
        let matches: Vec<Match> = t.order.iter().map(|k| t.matches[k].clone()).collect();
        let out = history(&matches, None, true);
        let titles: Vec<&str> = out.lines().filter(|l| l.contains("match")).collect();
        assert_eq!(
            titles,
            [
                "Quick Play   Arena:strongholds   2 matches",
                "Quick Play   Arena:slayer   1 match",
                "Mode not recorded   1 match"
            ],
            "{out}"
        );
        let strongholds = out.lines().nth(2).unwrap();
        assert!(strongholds.contains("12.0000") && strongholds.contains("10.0000"), "{out}");
        assert!(!out.lines().take(3).any(|l| l.contains("11.0000")), "the slayer value stays out\n{out}");
    }

    #[test]
    fn the_table_counts_and_marks_players_as_the_live_view_does() {
        // Players A to E, masked, by their XUIDs: each with a rating, a team and a party.
        let all = [(10.0, 0, "p"), (20.0, 0, "p"), (15.0, 1, "q"), (12.0, 1, "r"), (9.0, 1, "r")];
        let roster = |who: &[usize]| Roster {
            match_id: Some("m".into()),
            entries: who
                .iter()
                .map(|&i| {
                    let (mmr, team, party) = all[i];
                    let xuid = 9_100_000_000_000_401 + i as u64;
                    Entry { xuid, mmr: Some(mmr), team: Some(team), party: Some(party.into()), leader: false }
                })
                .collect(),
            ..Roster::default()
        };
        let mut t = Tracker::default();
        t.apply("s", None, "2026-11-11T00:00:00.000Z", &roster(&[0, 1, 2]));
        // B leaves; D comes ten seconds after teams were set, still loading in.
        t.apply("s", None, "2026-11-11T00:00:10.000Z", &roster(&[0, 2, 3]));
        // E comes two minutes in.
        t.apply("s", None, "2026-11-11T00:02:10.000Z", &roster(&[0, 2, 3, 4]));
        let out = table(&t.matches["m"], 1, true);
        assert!(out.contains("Team 0: mean 10.0000 over 1 player"), "{out}");
        assert!(out.contains("Team 1: mean 12.0000 over 3 players"), "{out}");
        let line = |who: &str| out.lines().find(|l| l.starts_with(&format!("{who} "))).unwrap();
        let col = out.lines().find(|l| l.starts_with("Player")).unwrap().find("Party").unwrap();
        let party = |who: &str| line(who)[col..col + 5].trim().to_string();
        // A's party has no one else still in; D and E's does.
        assert_eq!(
            (party("A"), party("B"), party("D"), party("E")),
            (String::new(), String::new(), "1".into(), "1".into()),
            "{out}"
        );
        assert!(line("B").contains("left") && !line("A").contains("left"), "{out}");
        assert!(!line("D").contains("joined late") && line("E").contains("joined late"), "{out}");
    }

    #[test]
    fn a_player_this_match_doesnt_name_takes_a_name_from_another() {
        let x = 9_100_000_000_000_002;
        let mut m = Match::default();
        m.players.insert(x, Player::default());
        assert_eq!(who(&m, x, false), x.to_string());
        gamertags::saw(x, "Elsewhere", "2026-11-11T01:00:00.000Z");
        assert_eq!(who(&m, x, false), "Elsewhere");
        m.players.get_mut(&x).unwrap().gamertag = Some("Here".into());
        assert_eq!(who(&m, x, false), "Here");
        assert_eq!(who(&m, x, true), "A");
    }
}

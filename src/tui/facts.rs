//! A match as the screen reads it: who's in, their numbers, the teams and how even they are.

use crate::tracker::{Mark, Match, Player};
use std::cmp::Ordering;
use std::collections::HashMap;

/// Team means of one-decimal ratings carry about this much noise: a smaller gap is even.
const EVEN_GAP: f64 = 0.15;
/// The gap as a share of the players' spread (their standard deviation): under
/// `EVEN` it's even, and from `STRONG` up it's strongly favored.
const EVEN: f64 = 0.2;
const STRONG: f64 = 0.6;

/// Rounded as printed, so a mean of printed numbers can be checked by eye.
pub(super) fn tenths(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub(super) fn one(v: Option<f64>) -> String {
    v.map(|x| format!("{:.1}", tenths(x))).unwrap_or_default()
}

pub(super) use crate::view::plural;

fn desc(a: Option<f64>, b: Option<f64>) -> Ordering {
    b.unwrap_or(f64::MIN).total_cmp(&a.unwrap_or(f64::MIN))
}

/// The mean of the values as printed.
pub(super) fn mean(v: impl Iterator<Item = f64>) -> Option<f64> {
    let (n, sum) = v.fold((0usize, 0.0), |(n, s), x| (n + 1, s + tenths(x)));
    (n > 0).then(|| sum / n as f64)
}

/// A player as the screen counts them.
pub(super) struct Seat<'a> {
    pub(super) x: u64,
    pub(super) p: &'a Player,
    pub(super) team: Option<u16>,
    /// In the latest roster state.
    pub(super) present: bool,
    /// Joined in progress.
    pub(super) late: bool,
    pub(super) mode: Option<f64>,
    pub(super) xbox: Option<f64>,
    pub(super) you: bool,
    /// The mode's own values, in order, with repeats dropped.
    pub(super) rated: Vec<&'a Mark<Option<f64>>>,
}

/// A team: its seats in table order (best first, parties together, then those who left), and the
/// means over the players still in.
pub(super) struct Side {
    pub(super) id: u16,
    pub(super) seats: Vec<usize>,
    present: usize,
    pub(super) mean: Option<f64>,
    pub(super) xbox: Option<f64>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Kind {
    /// No teams yet.
    Lobby,
    /// Every team is one player.
    Ffa,
    Two,
    Many,
}

/// How even a match is, from the gap as a share of the players' spread.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Verdict {
    Even,
    Favored(u16),
    Strongly(u16),
}

impl Verdict {
    pub(super) fn words(self) -> &'static str {
        match self {
            Verdict::Even => "Even",
            Verdict::Favored(_) => "Favored",
            Verdict::Strongly(_) => "Strongly favored",
        }
    }

    pub(super) fn team(self) -> Option<u16> {
        match self {
            Verdict::Even => None,
            Verdict::Favored(t) | Verdict::Strongly(t) => Some(t),
        }
    }
}

/// A match as the screen reads it.
pub(super) struct Facts<'a> {
    pub(super) m: &'a Match,
    /// Players still in, best first with each party together, then those who left.
    pub(super) seats: Vec<Seat<'a>>,
    /// Your team first, then the rest, best first.
    pub(super) teams: Vec<Side>,
    pub(super) kind: Kind,
    /// Each party of two or more still in, numbered in seat order.
    parties: HashMap<&'a str, usize>,
}

/// Player `x` of match `m` as the screen counts them. `documented` is whether anyone
/// in the match has an Xbox document. Who's in and who joined late follow the match's
/// own rules, as in the plain table.
pub(super) fn seat<'a>(m: &'a Match, x: u64, p: &'a Player, documented: bool) -> Seat<'a> {
    let doc = p.xbox_mmr();
    let late = m.late(p);
    let values: Vec<&Mark<Option<f64>>> = p.mmr.iter().filter(|m| m.value.is_some()).collect();
    let is_doc = |v: Option<f64>| doc.zip(v).is_some_and(|(d, v)| (d - v).abs() < 1e-9);
    // The first value is the matchmaker's when it equals the document's. For a
    // player with no document entry, it's also the matchmaker's when a mode value
    // replaced it, or still could.
    let first_seed = values.first().is_some_and(|f| {
        is_doc(f.value)
            || (doc.is_none()
                && (late || !documented)
                && (values.iter().skip(1).any(|m| m.value != f.value) || (late && p.first_state == p.last_state)))
    });
    let mut rated: Vec<&Mark<Option<f64>>> = Vec::new();
    for (i, mk) in values.iter().enumerate() {
        if (i == 0 && first_seed) || is_doc(mk.value) || rated.last().is_some_and(|r| r.value == mk.value) {
            continue;
        }
        rated.push(mk);
    }
    let seed = values.first().filter(|_| first_seed).and_then(|m| m.value);
    Seat {
        x,
        p,
        team: p.latest_team(),
        present: m.present(p),
        late,
        mode: rated.last().and_then(|m| m.value),
        xbox: doc.or(seed),
        you: m.owner == Some(x),
        rated,
    }
}

/// The lowest and highest of `v`.
pub(super) fn bounds(v: &[f64]) -> Option<(f64, f64)> {
    let first = *v.first()?;
    Some(v.iter().fold((first, first), |(lo, hi), &x| (lo.min(x), hi.max(x))))
}

impl<'a> Facts<'a> {
    pub(super) fn of(m: &'a Match) -> Facts<'a> {
        let documented = m.players.values().any(|p| p.xbox_mmr().is_some());
        let mut seats: Vec<Seat> = m
            .players
            .iter()
            .filter(|(_, p)| p.first_state.is_some())
            .map(|(&x, p)| seat(m, x, p, documented))
            .collect();
        seats.sort_by(|a, b| b.present.cmp(&a.present).then(desc(a.mode, b.mode)).then(a.x.cmp(&b.x)));
        // Numbered in seat order, best first.
        let big = m.parties();
        let mut parties = HashMap::new();
        for s in seats.iter().filter(|s| s.present) {
            if let Some(k) = s.p.party.as_deref().filter(|k| big.contains(k)) {
                let n = parties.len();
                parties.entry(k).or_insert(n);
            }
        }
        // A party sits together at its best player's place, best first, so one
        // bracket always joins it.
        let party: Vec<Option<&str>> =
            seats.iter().map(|s| s.p.party.as_deref().filter(|k| s.present && parties.contains_key(k))).collect();
        let mut order: Vec<usize> = Vec::new();
        for i in 0..seats.len() {
            if order.contains(&i) {
                continue;
            }
            order.push(i);
            if let Some(k) = party[i] {
                order.extend((i + 1..seats.len()).filter(|&j| party[j] == Some(k)));
            }
        }
        let mut slots: Vec<Option<Seat>> = seats.into_iter().map(Some).collect();
        let seats: Vec<Seat> = order.iter().filter_map(|&i| slots[i].take()).collect();
        let mut ids: Vec<u16> = seats.iter().filter_map(|s| s.team).collect();
        ids.sort_unstable();
        ids.dedup();
        let mine = seats.iter().find(|s| s.you).and_then(|s| s.team);
        let mut teams: Vec<Side> = ids
            .iter()
            .map(|&id| {
                let idx: Vec<usize> = (0..seats.len()).filter(|&i| seats[i].team == Some(id)).collect();
                let here = || idx.iter().map(|&i| &seats[i]).filter(|s| s.present);
                Side {
                    id,
                    present: here().count(),
                    mean: mean(here().filter_map(|s| s.mode)),
                    xbox: mean(here().filter_map(|s| s.xbox)),
                    seats: idx,
                }
            })
            .collect();
        teams.sort_by(|a, b| {
            (Some(b.id) == mine).cmp(&(Some(a.id) == mine)).then(desc(a.mean, b.mean)).then(a.id.cmp(&b.id))
        });
        let live: Vec<&Side> = teams.iter().filter(|t| t.present > 0).collect();
        let kind = if live.is_empty() {
            Kind::Lobby
        } else if live.len() >= 3 && live.iter().all(|t| t.present == 1) {
            Kind::Ffa
        } else if live.len() == 2 {
            Kind::Two
        } else {
            Kind::Many
        };
        Facts { m, seats, teams, kind, parties }
    }

    pub(super) fn party(&self, i: usize) -> Option<usize> {
        let s = &self.seats[i];
        s.p.party.as_deref().and_then(|k| self.parties.get(k)).copied().filter(|_| s.present)
    }

    pub(super) fn live(&self) -> Vec<&Side> {
        self.teams.iter().filter(|t| t.present > 0).collect()
    }

    /// The players drawn and counted: everyone still in, on a team once there are teams.
    pub(super) fn counted(&self) -> impl Iterator<Item = &Seat<'a>> {
        let lobby = self.kind == Kind::Lobby;
        self.seats.iter().filter(move |s| s.present && (lobby || s.team.is_some()))
    }

    /// The team ahead and its lead over the next best team mean. No team leads a
    /// free for all.
    pub(super) fn lead(&self) -> Option<(u16, f64)> {
        if self.kind == Kind::Ffa {
            return None;
        }
        let mut by: Vec<(u16, f64)> = self.live().iter().filter_map(|t| Some((t.id, t.mean?))).collect();
        by.sort_by(|a, b| b.1.total_cmp(&a.1));
        match by[..] {
            [(t, a), (_, b), ..] => Some((t, a - b)),
            _ => None,
        }
    }

    /// The highest team mean minus the lowest. A free for all has no gap.
    pub(super) fn gap(&self) -> Option<f64> {
        if self.kind == Kind::Ffa {
            return None;
        }
        let means: Vec<f64> = self.live().iter().filter_map(|t| t.mean).collect();
        bounds(&means).filter(|_| means.len() >= 2).map(|(lo, hi)| hi - lo)
    }

    /// The standard deviation of the counted players' Mode MMR.
    fn spread(&self) -> Option<f64> {
        let v: Vec<f64> = self.counted().filter_map(|s| s.mode).collect();
        let m = mean(v.iter().copied())?;
        let var = v.iter().map(|x| (tenths(*x) - m).powi(2)).sum::<f64>() / v.len() as f64;
        (var > 0.0).then(|| var.sqrt())
    }

    /// The lead as a share of the spread, which the seam's lean and the verdict show.
    pub(super) fn lean(&self) -> Option<(u16, f64)> {
        let (t, gap) = self.lead()?;
        Some((t, gap / self.spread()?))
    }

    pub(super) fn verdict(&self) -> Option<Verdict> {
        let (t, gap) = self.lead()?;
        let share = self.lean().map_or(0.0, |l| l.1);
        Some(if gap < EVEN_GAP || share < EVEN {
            Verdict::Even
        } else if share < STRONG {
            Verdict::Favored(t)
        } else {
            Verdict::Strongly(t)
        })
    }

    /// The lowest and highest Mode MMR of the players counted.
    pub(super) fn range(&self) -> Option<(f64, f64)> {
        bounds(&self.counted().filter_map(|s| s.mode.map(tenths)).collect::<Vec<_>>())
    }

    /// The mean Xbox MMR of the players counted.
    pub(super) fn xbox_mean(&self) -> Option<f64> {
        mean(self.counted().filter_map(|s| s.xbox))
    }

    pub(super) fn you(&self) -> Option<&Seat<'a>> {
        self.seats.iter().find(|s| s.you)
    }

    /// A player's place by Mode MMR among the players counted, ties sharing a place.
    pub(super) fn place(&self, v: f64) -> usize {
        1 + self.counted().filter_map(|s| s.mode).filter(|o| tenths(*o) > tenths(v)).count()
    }

    /// Your place, and how many are rated.
    pub(super) fn rank(&self) -> Option<(usize, usize)> {
        let mine = self.you().filter(|s| s.present)?.mode?;
        Some((self.place(mine), self.counted().filter(|s| s.mode.is_some()).count()))
    }

    /// Your team's place by mean among the teams.
    pub(super) fn team_rank(&self) -> Option<(usize, usize)> {
        let mine = self.you()?.team?;
        let means: Vec<f64> = self.live().iter().filter_map(|t| t.mean).collect();
        let ours = self.live().iter().find(|t| t.id == mine)?.mean?;
        Some((1 + means.iter().filter(|m| **m > ours).count(), means.len()))
    }

    pub(super) fn sizes(sizes: &[usize]) -> String {
        let s = sizes.iter().map(ToString::to_string).collect::<Vec<_>>().join("v");
        if s.len() <= 7 {
            s
        } else {
            plural(sizes.len(), "team")
        }
    }

    /// Like `4v4`, `2v2v2v2`, `6 teams`, `FFA 8` or `Lobby 8`.
    pub(super) fn teams_text(&self) -> String {
        let n = self.counted().count();
        match self.kind {
            Kind::Lobby => format!("Lobby {n}"),
            Kind::Ffa => format!("FFA {n}"),
            _ => {
                let mut live = self.live();
                if live.len() < 2 {
                    return plural(n, "player");
                }
                live.sort_by_key(|t| t.id);
                Facts::sizes(&live.iter().map(|t| t.present).collect::<Vec<_>>())
            }
        }
    }

    /// The team sizes and the gap when teams were first set.
    pub(super) fn at_start(&self) -> (String, Option<f64>) {
        let firsts: Vec<(u32, u16, Option<f64>)> = self
            .seats
            .iter()
            .filter_map(|s| {
                s.p.team.iter().find(|t| t.value.is_some()).map(|t| (t.state, t.value.unwrap_or(0), s.mode))
            })
            .collect();
        let Some(start) = firsts.iter().map(|f| f.0).min() else { return (String::new(), None) };
        let mut by: Vec<(u16, Vec<f64>)> = Vec::new();
        for (_, t, v) in firsts.iter().filter(|f| f.0 == start) {
            match by.iter_mut().find(|b| b.0 == *t) {
                Some(b) => b.1.extend(*v),
                None => by.push((*t, v.iter().copied().collect())),
            }
        }
        by.sort_by_key(|b| b.0);
        let means: Vec<f64> = by.iter().filter_map(|b| mean(b.1.iter().copied())).collect();
        let gap = bounds(&means).filter(|_| means.len() >= 2).map(|(lo, hi)| hi - lo);
        if by.len() >= 3 && by.iter().all(|b| b.1.len() <= 1) {
            return (format!("FFA {}", by.len()), None);
        }
        (Facts::sizes(&by.iter().map(|b| b.1.len()).collect::<Vec<_>>()), gap)
    }

    pub(super) fn left(&self) -> usize {
        let lobby = self.kind == Kind::Lobby;
        self.seats.iter().filter(|s| !s.present && (lobby || s.team.is_some())).count()
    }

    pub(super) fn late(&self) -> usize {
        self.seats.iter().filter(|s| s.late && s.present).count()
    }

    #[cfg(test)]
    pub(super) fn side(&self, id: u16) -> Option<&Side> {
        self.teams.iter().find(|t| t.id == id)
    }
}

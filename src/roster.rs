//! The roster inside a game session message.
//!
//! The message's field 10 is the match settings (nullable); in it, field 8 is the
//! roster, a map of XUID to entry, and field 14 the match id (the one in
//! `/hi/matches/{id}/stats`). An entry leaves out its rating and team when they're
//! empty, so a missing value stays missing and never reads as 0.
//!
//! The same settings give the playlist, the game variant (the mode), the map and the
//! playlist's map and mode pair, all by asset id, plus the playlist's experience and
//! whether it's ranked. The names come from `names`.

use crate::bond::Value;
use serde::{Deserialize, Serialize};

const SETTINGS: u16 = 10;
const ROSTER: u16 = 8;
const MATCH_ID: u16 = 14;
const PARTY: u16 = 2;
const TEAM: u16 = 3;
const MMR: u16 = 4;
const LEADER: u16 = 7;
const MAP: u16 = 30;
const MODE: u16 = 31;
const PLAYLIST: u16 = 33;
const EXPERIENCE: u16 = 34;
const RANKED: u16 = 35;
const PAIR: u16 = 36;
/// An asset reference's asset id; 0 is its kind and 2 its version.
const ASSET_ID: u16 = 1;

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub xuid: u64,
    pub mmr: Option<f64>,
    pub team: Option<u16>,
    pub party: Option<String>,
    pub leader: bool,
}

/// What the match is: assets by id, the playlist's experience as the game names
/// it, and the assets' names once they're known.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Setup {
    pub experience: Option<String>,
    pub ranked: Option<bool>,
    pub playlist: Option<String>,
    pub mode: Option<String>,
    pub map: Option<String>,
    /// The playlist's map and mode pair.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair: Option<String>,
    #[serde(default, skip_serializing_if = "Named::is_empty")]
    pub named: Named,
}

/// The names of a match's playlist, mode and map, as the game shows them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Named {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
}

impl Named {
    pub fn is_empty(&self) -> bool {
        *self == Named::default()
    }

    /// Fills each name this one lacks from `other`.
    pub fn or(self, other: Named) -> Named {
        Named {
            playlist: self.playlist.or(other.playlist),
            mode: self.mode.or(other.mode),
            map: self.map.or(other.map),
        }
    }
}

impl Setup {
    pub fn is_empty(&self) -> bool {
        *self == Setup::default()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Roster {
    pub match_id: Option<String>,
    pub entries: Vec<Entry>,
    pub setup: Setup,
}

/// A Bond GUID struct: four fields, the last eight bytes a little-endian u64.
pub fn guid(v: &Value) -> Option<String> {
    let v = v.maybe()?;
    if !matches!(v, Value::Struct(_)) {
        return None;
    }
    let part = |i: u16| v.field(i).and_then(Value::uint).unwrap_or(0);
    let (a, b, c, d) = (part(0), part(1), part(2), part(3));
    if a == 0 && b == 0 && c == 0 && d == 0 {
        return None;
    }
    let t = d.to_le_bytes();
    Some(format!(
        "{a:08x}-{b:04x}-{c:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7]
    ))
}

/// The roster, if this message has one. `Err` means it has the shape of a roster
/// and something in it isn't the type the rosters seen so far have had.
pub fn read(root: &Value, warn: &mut dyn FnMut(&'static str)) -> Result<Option<Roster>, &'static str> {
    let Some(settings) = root.field(SETTINGS).and_then(Value::maybe) else { return Ok(None) };
    let Some(map) = settings.field(ROSTER) else { return Ok(None) };
    let Value::Map(rows) = map else { return Err("roster is not a map") };
    let mut entries = Vec::with_capacity(rows.len());
    for (k, e) in rows {
        let xuid = k.uint().ok_or("roster key is not a number")?;
        if !matches!(e, Value::Struct(_)) {
            return Err("roster entry is not a struct");
        }
        if xuid >> 48 != 9 {
            warn("roster key outside the XUID range");
        }
        let mmr = match e.field(MMR) {
            None => None,
            Some(Value::F64(v)) if v.is_finite() => Some(*v),
            Some(Value::F64(_)) => {
                warn("mmr is not a finite number");
                None
            }
            Some(_) => return Err("mmr is not a double"),
        };
        let team = match e.field(TEAM) {
            None => None,
            Some(v) => Some(u16::try_from(v.uint().ok_or("team is not a number")?).map_err(|_| "team out of range")?),
        };
        entries.push(Entry {
            xuid,
            mmr,
            team,
            party: e.field(PARTY).and_then(guid),
            leader: e.field(LEADER).and_then(Value::uint).unwrap_or(0) != 0,
        });
    }
    let asset = |id: u16| settings.field(id).and_then(Value::maybe).and_then(|r| r.field(ASSET_ID)).and_then(guid);
    let setup = Setup {
        experience: match settings.field(EXPERIENCE).and_then(Value::maybe) {
            Some(Value::Str(s)) if !s.is_empty() && s != "None" => Some(s.clone()),
            _ => None,
        },
        ranked: settings.field(RANKED).and_then(Value::maybe).and_then(Value::uint).map(|v| v != 0),
        playlist: asset(PLAYLIST),
        mode: asset(MODE),
        map: asset(MAP),
        pair: asset(PAIR),
        named: Named::default(),
    };
    Ok(Some(Roster { match_id: settings.field(MATCH_ID).and_then(guid), entries, setup }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(a: u64, d: u64) -> Value {
        Value::Struct(vec![
            (0, Value::Uint(a)),
            (1, Value::Uint(0x1234)),
            (2, Value::Uint(0x5678)),
            (3, Value::Uint(d)),
        ])
    }

    fn root(rows: Vec<(Value, Value)>) -> Value {
        let ms = Value::Struct(vec![(8, Value::Map(rows)), (14, Value::List(vec![g(0xdeadbeef, 0x0807060504030201)]))]);
        Value::Struct(vec![(10, Value::List(vec![ms]))])
    }

    fn entry(fields: Vec<(u16, Value)>) -> Value {
        Value::Struct(fields)
    }

    const X: u64 = 0x0009_0000_1234_5678;

    #[test]
    fn reads_values_and_the_match_id() {
        let r = root(vec![(
            Value::Uint(X),
            entry(vec![(2, g(7, 1)), (3, Value::Uint(1)), (4, Value::F64(2.5)), (7, Value::Bool(true))]),
        )]);
        let got = read(&r, &mut |w| panic!("{w}")).unwrap().unwrap();
        assert_eq!(got.match_id.as_deref(), Some("deadbeef-1234-5678-0102-030405060708"));
        assert_eq!(got.entries[0].mmr, Some(2.5));
        assert_eq!(got.entries[0].team, Some(1));
        assert!(got.entries[0].leader);
    }

    #[test]
    fn reads_the_playlist_mode_and_map() {
        let asset = |kind: u64, a: u64| Value::Struct(vec![(0, Value::Uint(kind)), (1, g(a, 9)), (2, g(1, 1))]);
        let ms = Value::Struct(vec![
            (8, Value::Map(vec![])),
            (30, asset(2, 0xaa)),
            (31, asset(6, 0xbb)),
            (33, asset(3, 0xcc)),
            (34, Value::Str("Arena".into())),
            (35, Value::Bool(true)),
            (36, asset(7, 0xdd)),
        ]);
        let got = read(&Value::Struct(vec![(10, Value::List(vec![ms]))]), &mut |w| panic!("{w}")).unwrap().unwrap();
        assert_eq!(got.setup.experience.as_deref(), Some("Arena"));
        assert_eq!(got.setup.ranked, Some(true));
        assert!(got.setup.playlist.as_deref().unwrap().starts_with("000000cc-"));
        assert!(got.setup.mode.as_deref().unwrap().starts_with("000000bb-"));
        assert!(got.setup.map.as_deref().unwrap().starts_with("000000aa-"));
        assert!(got.setup.pair.as_deref().unwrap().starts_with("000000dd-"));
        let plain = read(&root(vec![]), &mut |_| {}).unwrap().unwrap();
        assert!(plain.setup.is_empty());
    }

    #[test]
    fn a_missing_mmr_stays_missing_and_zero_stays_zero() {
        let r = root(vec![
            (Value::Uint(X), entry(vec![])),
            (Value::Uint(X + 1), entry(vec![(3, Value::Uint(0)), (4, Value::F64(0.0))])),
        ]);
        let got = read(&r, &mut |_| {}).unwrap().unwrap();
        assert_eq!((got.entries[0].mmr, got.entries[0].team), (None, None));
        assert_eq!((got.entries[1].mmr, got.entries[1].team), (Some(0.0), Some(0)));
    }

    #[test]
    fn no_settings_is_no_roster() {
        let r = Value::Struct(vec![(0, Value::Str("x".into()))]);
        assert_eq!(read(&r, &mut |_| {}).unwrap(), None);
    }

    #[test]
    fn a_wrong_mmr_type_is_an_error() {
        let r = root(vec![(Value::Uint(X), entry(vec![(4, Value::Uint(3))]))]);
        assert!(read(&r, &mut |_| {}).is_err());
    }

    #[test]
    fn an_odd_key_warns_but_keeps_the_entry() {
        let r = root(vec![(Value::Uint(5), entry(vec![]))]);
        let mut warned = 0;
        let got = read(&r, &mut |_| warned += 1).unwrap().unwrap();
        assert_eq!((warned, got.entries.len()), (1, 1));
    }
}

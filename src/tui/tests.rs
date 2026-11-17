//! Matches played as the game sends them, drawn into a test terminal.

use super::facts::{one, seat, Facts, Verdict};
use super::glass::{logo, LEAN_END};
use super::ink::{window, CAP, CAP_DIM, CAP_LIT, GRAPHITE, GRAPHITE_HI, SEAM, TEAMS};
use super::keys::{key, Act, Pane, Ui};
use super::panes::{saving, NONE_YET};
use super::*;
use crate::roster::{Entry as RosterEntry, Roster};
use crate::watch::Options;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use std::time::Instant;

fn options() -> Options {
    Options { store: "unused".into(), port: 47812, mask: false, save: false, plain: true, game: None }
}

const ME: u64 = 9_100_000_000_000_600;

struct P {
    x: u64,
    name: &'static str,
    team: Option<u16>,
    party: &'static str,
    leader: bool,
    mmr: f64,
    xbox: Option<f64>,
}

fn p(i: u64, name: &'static str, team: Option<u16>, party: &'static str, mmr: f64, xbox: Option<f64>) -> P {
    P { x: ME + i, name, team, party, leader: false, mmr, xbox }
}

/// A roster of `ps`, each at their Mode MMR, or at the value `first` gives them.
fn roster(id: &str, ps: &[&P], first: impl Fn(&P) -> Option<f64>) -> Roster {
    Roster {
        match_id: Some(id.to_string()),
        entries: ps
            .iter()
            .map(|p| RosterEntry {
                xuid: p.x,
                mmr: Some(first(p).unwrap_or(p.mmr)),
                team: p.team,
                party: Some(p.party.to_string()),
                leader: p.leader,
            })
            .collect(),
        ..Default::default()
    }
}

fn document(session: &str, ps: &[P]) -> serde_json::Value {
    let members: serde_json::Map<String, serde_json::Value> = ps
        .iter()
        .enumerate()
        .filter(|(_, p)| p.xbox.is_some())
        .map(|(i, p)| {
            let result = serde_json::json!({
                "initialTeam": p.team.map(|t| t.to_string()),
                "playerAttrs": {"MMR": p.xbox, "trueMatchTeamBalanceMmr": p.xbox.map(|v| v * 1.071143)}
            });
            let member = serde_json::json!({
                "gamertag": p.name,
                "constants": {"system": {"xuid": p.x.to_string()}, "custom": {"matchmakingResult": result}}
            });
            (i.to_string(), member)
        })
        .collect();
    serde_json::json!({"constants": {"custom": {"gameSessionId": session}}, "members": members})
}

/// A match as the game sends it: the Xbox document (unless `doc` is false), the
/// first roster (each starter at their Xbox MMR), the settled one, then `leave`
/// leaving, then `late` joining, each at a matchmaker value before the mode's.
#[expect(clippy::too_many_arguments, reason = "a fixture that spells out a whole match")]
fn play_with(s: &mut State, session: &str, id: &str, at: &str, ps: &[P], late: &[usize], leave: &[usize], doc: bool) {
    if doc {
        let (_, ev) = s.p.tracker.apply_document(at, &document(session, ps)).unwrap();
        s.events(ev);
    }
    let mut here: Vec<&P> = ps.iter().enumerate().filter(|(i, _)| !late.contains(i)).map(|(_, p)| p).collect();
    for first in [true, false] {
        let r = roster(id, &here, |p| if first { p.xbox } else { None });
        let ev = s.p.tracker.apply(session, Some(ME), at, &r);
        s.events(ev);
    }
    for &l in leave {
        here.retain(|p| p.x != ps[l].x);
        let ev = s.p.tracker.apply(session, Some(ME), at, &roster(id, &here, |_| None));
        s.events(ev);
    }
    for &j in late {
        here.push(&ps[j]);
        let joined = ps[j].x;
        for first in [true, false] {
            let r = roster(id, &here, |p| (first && p.x == joined).then_some(8.9));
            let ev = s.p.tracker.apply(session, Some(ME), at, &r);
            s.events(ev);
        }
    }
    let m = s.p.tracker.matches.get_mut(id).unwrap();
    for q in ps {
        if let Some(pl) = m.players.get_mut(&q.x) {
            pl.gamertag = Some(q.name.to_string());
        }
    }
}

fn play(s: &mut State, session: &str, id: &str, at: &str, ps: &[P], late: &[usize], leave: &[usize]) {
    play_with(s, session, id, at, ps, late, leave, true);
}

/// Arena Tactical Slayer: Carter leaves Eagle, Thom backfills, and you are
/// partied with Kilo Two, the leader, on Cobra.
fn arena() -> Vec<P> {
    let mut v = vec![
        p(0, "Spartan Nine", Some(1), "pA", 5.8, Some(9.43)),
        p(1, "Kilo Two", Some(1), "pA", 8.0, Some(8.65)),
        p(2, "Noble Six", Some(1), "s2", 4.3, Some(8.51)),
        p(3, "Jun A266", Some(1), "s3", 5.1, Some(8.81)),
        p(4, "Kat B320", Some(0), "s4", 7.3, Some(10.08)),
        p(5, "Emile A239", Some(0), "s5", 3.4, Some(8.49)),
        p(6, "Carter A259", Some(0), "s6", 3.2, Some(7.92)),
        p(7, "Jorge 052", Some(0), "s7", 5.9, Some(7.95)),
        p(8, "Thom A293", Some(0), "s8", 3.8, None),
    ];
    v[1].leader = true;
    v
}

const ID: &str = "1a2b3c4d-0000-4000-8000-000000000003";

fn sample() -> State {
    let mut s = State::new(options(), "no proxy".into());
    play(&mut s, "s-3", ID, "2026-11-11T00:27:40.000Z", &arena(), &[8], &[6]);
    s
}

const NAMES: [&str; 8] = ["Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel"];

fn lobby(team: fn(u64) -> Option<u16>) -> State {
    let mut s = State::new(options(), "no proxy".into());
    let ps: Vec<P> = (0..8u64)
        .map(|i| p(i, NAMES[i as usize], team(i), NAMES[i as usize], 3.0 + i as f64 * 0.4, Some(9.0)))
        .collect();
    play(&mut s, "s-4", "aaaa0000-0000-4000-8000-000000000004", "2026-11-11T01:00:00.000Z", &ps, &[], &[]);
    s
}

/// Matches saved by earlier runs, three on each of three days.
fn saved() -> State {
    let mut played = State::new(options(), "no proxy".into());
    for n in 0..9 {
        let id = format!("cccc0000-0000-4000-8000-00000000020{n}");
        let at = format!("2026-11-{:02}T1{}:00:00.000Z", 7 + n / 3, n % 3);
        play(&mut played, &format!("s-2{n}"), &id, &at, &arena(), &[8], &[6]);
    }
    let mut s = State::new(options(), "no proxy".into());
    for k in &played.p.tracker.order {
        s.p.tracker.resume(played.p.tracker.matches[k].clone());
    }
    s
}

fn facts(state: &State) -> Facts<'_> {
    Facts::of(state.p.tracker.matches.values().next().unwrap())
}

fn ui() -> Ui {
    Ui::new(false)
}

fn draw_buffer(state: &State, ui: &Ui, w: u16, h: u16) -> (String, Buffer) {
    let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
    t.draw(|f| draw(f, state, ui)).unwrap();
    let buf = t.backend().buffer().clone();
    let shown: String =
        (0..h).map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>() + "\n").collect();
    if std::env::var_os("MMR_SHOW_TUI").is_some() {
        println!("{shown}");
    }
    // Box lines and the logo's half blocks are the only symbols beyond ASCII.
    let odd: Vec<char> = shown
        .chars()
        .filter(|c| !c.is_ascii() && !('\u{2500}'..='\u{257f}').contains(c) && !['\u{2580}', '\u{2584}'].contains(c))
        .collect();
    assert!(odd.is_empty(), "drew {odd:?}\n{shown}");
    (shown, buf)
}

fn render(state: &State, ui: &Ui, w: u16, h: u16) -> String {
    draw_buffer(state, ui, w, h).0
}

fn line_with<'a>(shown: &'a str, needle: &str) -> &'a str {
    shown.lines().find(|l| l.contains(needle)).unwrap_or_else(|| panic!("no line with {needle:?}\n{shown}"))
}

/// The column, counted in cells, where `needle` starts or ends on the line.
fn start_of(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|at| line[..at].chars().count())
}

fn end_of(line: &str, needle: &str) -> Option<usize> {
    start_of(line, needle).map(|s| s + needle.chars().count())
}

#[test]
fn counts_and_means_take_only_the_players_still_in() {
    let state = sample();
    let fa = facts(&state);
    assert_eq!(fa.teams_text(), "4v4");
    assert_eq!((fa.left(), fa.late()), (1, 1));
    let eagle = fa.side(0).unwrap();
    // Means of the printed values, including the backfill's matchmaker value.
    assert!((eagle.mean.unwrap() - (7.3 + 5.9 + 3.8 + 3.4) / 4.0).abs() < 1e-9);
    assert!((eagle.xbox.unwrap() - (10.1 + 8.0 + 8.9 + 8.5) / 4.0).abs() < 1e-9);
    let (t, ahead) = fa.lead().unwrap();
    assert_eq!(t, 1);
    assert!((ahead - 0.7).abs() < 0.01, "{ahead}");
    assert_eq!(fa.rank(), Some((4, 8)));
    assert_eq!(fa.verdict(), Some(Verdict::Favored(1)));
}

#[test]
fn a_tiny_gap_is_even_in_any_mode() {
    let mut s = State::new(options(), "no proxy".into());
    let mut ps = arena();
    for (i, q) in ps.iter_mut().enumerate() {
        q.mmr = 14.6 + (i % 4) as f64 * 0.3 + if q.team == Some(1) { 0.05 } else { 0.0 };
    }
    play(&mut s, "s-6", ID, "2026-11-11T00:27:40.000Z", &ps[..8], &[], &[]);
    assert_eq!(facts(&s).verdict(), Some(Verdict::Even));
    let (shown, buf) = draw_buffer(&s, &ui(), 120, 30);
    let lean = shown.lines().nth(4).unwrap();
    assert!(lean.contains("Even") && !lean.contains("Favored"), "{shown}");
    // The marker is the word: every cyan cell on the row belongs to it.
    let even = start_of(lean, "Even").unwrap() as u16;
    let cyan: Vec<u16> = (0..120).filter(|&x| buf[(x, 4)].bg == SEAM).collect();
    assert_eq!(cyan, (even - 1..even + 5).collect::<Vec<_>>(), "{shown}");
}

#[test]
fn a_strong_favorite_on_the_right_pulls_the_lean_to_the_far_end() {
    let mut s = State::new(options(), "no proxy".into());
    let mut ps = arena();
    // Eagle, across from you, is far ahead.
    for (i, q) in ps.iter_mut().enumerate() {
        q.mmr = if q.team == Some(0) { 12.0 } else { 4.0 } + (i % 4) as f64 * 0.2;
    }
    play(&mut s, "s-8", ID, "2026-11-11T00:27:40.000Z", &ps[..8], &[], &[]);
    assert_eq!(facts(&s).verdict(), Some(Verdict::Strongly(0)));
    let (shown, buf) = draw_buffer(&s, &ui(), 120, 30);
    let lines: Vec<&str> = shown.lines().collect();
    let y = lines.iter().position(|l| l.contains("Strongly favored")).unwrap();
    let mid = start_of(line_with(&shown, "Kilo Two"), "\u{2502}").unwrap();
    assert!(start_of(lines[y], "Strongly favored").unwrap() > mid, "under the right-hand team\n{shown}");
    // Past the cap, the lean stops short of the glass's left end.
    let lean: Vec<u16> = (0..120).filter(|&x| buf[(x, y as u16)].bg == SEAM).collect();
    assert_eq!(lean, vec![2 + LEAN_END], "{shown}");
}

#[test]
fn team_sizes_give_way_to_a_count_past_seven_characters() {
    assert_eq!(Facts::sizes(&[4, 4]), "4v4");
    assert_eq!(Facts::sizes(&[12, 12]), "12v12");
    assert_eq!(Facts::sizes(&[2, 2, 2, 2]), "2v2v2v2");
    assert_eq!(Facts::sizes(&[10, 10, 10]), "3 teams");
    assert_eq!(Facts::sizes(&[2, 2, 2, 2, 2]), "5 teams");
}

#[test]
fn a_window_keeps_its_rows_in_range_and_counts_the_rest() {
    let w = |total, rows, from| {
        let (start, s) = window(total, rows, from);
        (start, s.rows, s.above, s.below)
    };
    assert_eq!(w(10, 4, 0), (0, 4, 0, 6));
    assert_eq!(w(10, 4, 3), (3, 4, 3, 3));
    assert_eq!(w(10, 4, 99), (6, 4, 6, 0), "scrolled past the end, the last rows");
    assert_eq!(w(3, 5, 2), (0, 5, 0, 0), "all of it fits");
    assert_eq!(w(5, 0, 2), (2, 0, 2, 3));
}

#[test]
fn without_documents_late_means_well_after_teams_were_set() {
    let mut s = State::new(options(), "no proxy".into());
    let ps: Vec<P> = (0..8u64)
        .map(|i| p(i, NAMES[i as usize], Some((i % 2) as u16), NAMES[i as usize], 5.0 + i as f64 * 0.1, None))
        .collect();
    let all: Vec<&P> = ps.iter().collect();
    // Six from the start, one 30 seconds in while still loading, and one two
    // minutes in.
    for (n, secs) in [(6, 0), (7, 30), (8, 120)] {
        let at = format!("2026-11-11T01:{:02}:{:02}.000Z", secs / 60, secs % 60);
        let ev = s.p.tracker.apply("s-3", Some(ME), &at, &roster(ID, &all[..n], |_| None));
        s.events(ev);
    }
    let fa = facts(&s);
    let late: Vec<u64> = fa.seats.iter().filter(|s| s.late).map(|s| s.x - ME).collect();
    assert_eq!(late, vec![7]);
    let m = &s.p.tracker.matches[ID];
    assert!(!m.late(&m.players[&(ME + 6)]));
    let seated = seat(m, ME + 7, &m.players[&(ME + 7)], false);
    assert!(seated.late && seated.present && !seated.you);
}

#[test]
fn the_matchmakers_first_value_is_never_a_mode_mmr() {
    let state = sample();
    let fa = facts(&state);
    let thom = fa.seats.iter().find(|s| s.p.gamertag.as_deref() == Some("Thom A293")).unwrap();
    assert!(thom.late);
    assert_eq!((one(thom.mode), one(thom.xbox)), ("3.8".to_string(), "8.9".to_string()));
    let you = fa.you().unwrap();
    assert_eq!((one(you.mode), one(you.xbox)), ("5.8".to_string(), "9.4".to_string()));
    assert!(!you.late && you.rated.len() == 1);
}

#[test]
fn without_an_xbox_document_the_first_values_are_still_the_matchmakers() {
    let mut s = State::new(options(), "no proxy".into());
    play_with(&mut s, "s-5", ID, "2026-11-11T00:27:40.000Z", &arena()[..8], &[], &[], false);
    let fa = facts(&s);
    let you = fa.you().unwrap();
    assert_eq!((one(you.mode), one(you.xbox)), ("5.8".to_string(), "9.4".to_string()));
    assert!(!you.late && you.rated.len() == 1);
    let mut u = ui();
    u.focus(Pane::Events);
    assert!(!render(&s, &u, 120, 30).contains("Rated"));
}

#[test]
fn your_team_faces_from_the_left_on_one_grid_from_the_glass_down() {
    let state = sample();
    let shown = render(&state, &ui(), 120, 30);
    assert!(
        line_with(&shown, "Match 1 of 1").contains("Start ") && shown.contains("Late 1") && shown.contains("Left 1")
    );
    let kilo = line_with(&shown, "Kilo Two");
    assert!(start_of(kilo, "Kilo Two") < start_of(kilo, "Kat B320"), "{shown}");
    let mid = start_of(kilo, "\u{2502}").unwrap();
    let titles = line_with(&shown, "Gap");
    let means = line_with(&shown, "Mean");
    assert_eq!(end_of(titles, "Mode MMR"), Some(mid - 2), "{shown}");
    assert_eq!(end_of(means, "5.8"), Some(mid - 2), "{shown}");
    assert_eq!(end_of(kilo, "8.0"), Some(mid - 2), "{kilo}");
    assert_eq!(end_of(titles, "Xbox MMR"), end_of(kilo, "8.7"));
    assert_eq!(start_of(means, "5.1"), start_of(kilo, "7.3"));
    assert_eq!(start_of(kilo, "7.3"), Some(mid + 4));
    let thom = line_with(&shown, "Thom A293");
    assert!(thom.contains("Late") && thom.contains("8.9"), "{thom}");
    assert!(line_with(&shown, "Carter A259").contains("Left"));
    let you = line_with(&shown, "Spartan Nine");
    assert_eq!(start_of(you, "You"), Some(3), "{you}");
    assert!(!shown.contains("-5.1") && !shown.contains("Change"));
}

#[test]
fn the_glass_carries_the_gap_and_leans_toward_the_weaker_team() {
    let state = sample();
    let (shown, buf) = draw_buffer(&state, &ui(), 120, 30);
    let lines: Vec<&str> = shown.lines().collect();
    let y = lines.iter().position(|l| l.contains("Mean")).unwrap() as u16;
    let mid = start_of(line_with(&shown, "Kilo Two"), "\u{2502}").unwrap() as u16;
    let seam: Vec<u16> = (0..120).filter(|&x| buf[(x, y)].bg == SEAM).collect();
    assert_eq!(seam, vec![mid - 1, mid, mid + 1], "{shown}");
    assert!(lines[y as usize].contains("0.7") || lines[y as usize].contains("0.8"));
    assert_eq!(buf[(5, y)].bg, TEAMS[1].3[1]);
    assert_eq!(buf[(115, y)].bg, TEAMS[0].3[1]);
    let lean: Vec<u16> = (0..120).filter(|&x| buf[(x, y + 1)].bg == SEAM).collect();
    assert!(lean.len() == 1 && lean[0] > mid, "{lean:?}\n{shown}");
    assert!(lines[y as usize + 1].contains("Favored"));
    assert!(line_with(&shown, "Gap").contains("Cobra  You"));
}

#[test]
fn a_party_sits_together_under_one_line() {
    let state = sample();
    let shown = render(&state, &ui(), 120, 30);
    let lines: Vec<&str> = shown.lines().collect();
    let kilo = lines.iter().position(|l| l.contains("Kilo Two")).unwrap();
    assert!(lines[kilo + 1].contains("Spartan Nine"), "{shown}");
    let col = end_of(lines[kilo], "Kilo Two").unwrap() + 1;
    // Kilo Two leads the party: the bracket is heavy beside them.
    assert_eq!(lines[kilo].chars().nth(col), Some('\u{2513}'), "{}", lines[kilo]);
    assert_eq!(lines[kilo + 1].chars().nth(col), Some('\u{2518}'));
    // Noble Six, the team's lowest, joins the party, so they sit inside its
    // bracket at its best player's place, ahead of Jun.
    let mut s = State::new(options(), "no proxy".into());
    let mut ps = arena();
    ps[2].party = "pA";
    play(&mut s, "s-7", ID, "2026-11-11T00:27:40.000Z", &ps[..8], &[], &[]);
    let shown = render(&s, &ui(), 120, 30);
    let lines: Vec<&str> = shown.lines().collect();
    let kilo = lines.iter().position(|l| l.contains("Kilo Two")).unwrap();
    let rows: Vec<&str> = lines[kilo..kilo + 4].to_vec();
    assert!(rows[1].contains("Spartan Nine") && rows[2].contains("Noble Six") && rows[3].contains("Jun A266"));
    let col = end_of(rows[0], "Kilo Two").unwrap() + 1;
    let marks: String = rows.iter().map(|r| r.chars().nth(col).unwrap_or(' ')).collect();
    assert_eq!(marks, "\u{2513}\u{2524}\u{2518} ", "{shown}");
}

#[test]
fn a_party_on_two_teams_is_numbered() {
    let mut s = State::new(options(), "no proxy".into());
    let mut ps = arena();
    ps[4].party = "pA";
    play(&mut s, "s-7", ID, "2026-11-11T00:27:40.000Z", &ps[..8], &[], &[]);
    let shown = render(&s, &ui(), 120, 30);
    let lines: Vec<&str> = shown.lines().collect();
    let row = lines.iter().position(|l| l.contains("Kilo Two")).unwrap();
    let mark = |r: usize, name: &str, left: bool| {
        let col = if left { end_of(lines[r], name).unwrap() + 1 } else { start_of(lines[r], name).unwrap() - 2 };
        lines[r].chars().nth(col)
    };
    // Kat sits across from Kilo Two, who leads the party.
    assert_eq!(mark(row, "Kat B320", false), Some('1'), "{shown}");
    assert_eq!(mark(row, "Kilo Two", true), Some('1'), "{shown}");
    assert_eq!(mark(row + 1, "Spartan Nine", true), Some('1'), "{shown}");
}

#[test]
fn lists_matches_with_the_same_columns_always() {
    let mut state = sample();
    state.lobby_open = 1;
    let shown = render(&state, &ui(), 120, 30);
    let head = line_with(&shown, " Playlist ");
    for title in ["Start", "Teams", "Gap", "Favored", "Rank", "Xbox MMR", "State"] {
        assert!(head.contains(title), "{title}\n{shown}");
    }
    let row = shown.lines().find(|l| l.contains("4 of 8") && !l.contains("Rank")).unwrap();
    assert!(row.contains("You") && row.contains("Live"), "{row}");
    assert_eq!(end_of(head, "Rank"), end_of(row, "4 of 8"));
    assert!(line_with(&shown, "Found").contains('\u{2502}'), "events beside the matches\n{shown}");
    let ffa = render(&lobby(|i| Some(i as u16)), &ui(), 120, 30);
    assert_eq!(start_of(line_with(&ffa, " Playlist "), "Rank"), start_of(head, "Rank"));
}

#[test]
fn a_long_saving_path_loses_whole_folders() {
    let mut s = State::new(options(), "no proxy".into());
    s.o.save = true;
    s.o.store = r"D:\games\halo\hi-mmr".into();
    let text = |room| saving(&s, room).iter().map(|x| x.content.to_string()).collect::<String>();
    assert_eq!(text(100), r"Saving to: D:\games\halo\hi-mmr\matches");
    assert_eq!(text(28), r"Saving to: hi-mmr\matches");
    assert_eq!(text(20), r"Saving to: matches");
    assert_eq!(text(10), "");
}

#[test]
fn a_header_keeps_what_is_out_of_view() {
    let mut s = saved();
    s.o.save = true;
    let mut ui = ui();
    ui.ours = false;
    // When narrow, the match list shares its header with the other tabs. When
    // wide, the header is its own, and the path drops its drive to fit beside the
    // rows above.
    let wide = r"D:\games\halo infinite\hi-mmr-data1";
    for (w, store, path) in [(90, r"D:\games\halo\hi-mmr", r"hi-mmr"), (120, wide, &wide[3..])] {
        s.o.store = store.into();
        let shown = render(&s, &ui, w, 20);
        let head = line_with(&shown, " Matches ");
        assert!(head.contains(" above") && head.contains(&format!("Saving to: {path}\\matches ")), "{shown}");
    }
}

#[test]
fn status_bar_is_a_band_and_keys_are_caps() {
    let state = sample();
    let (shown, buf) = draw_buffer(&state, &ui(), 120, 30);
    let band = [GRAPHITE, GRAPHITE_HI];
    assert!((0..120).all(|x| band.contains(&buf[(x, 28)].bg) || (2..22).contains(&x)), "{shown}");
    let keys = shown.lines().nth(29).unwrap();
    let d = start_of(keys, " d ").unwrap() as u16;
    assert_eq!(buf[(d, 29)].bg, CAP);
    let o = start_of(keys, " o ").unwrap() as u16;
    assert_eq!(buf[(o, 29)].bg, CAP_DIM, "nothing to open when not saving");
    let f = start_of(keys, " f ").unwrap() as u16;
    assert_eq!(buf[(f, 29)].bg, CAP_DIM, "nothing to follow while following");
    for k in ["Match", "Pane", "Follow", "Details", "Hide names", "Folder", "Keys", "Quit"] {
        assert!(keys.contains(k), "{k}\n{keys}");
    }
}

#[test]
fn following_pinning_and_the_newest() {
    let mut state = sample();
    play(&mut state, "s-9", "bbbb0000-0000-4000-8000-000000000009", "2026-11-11T00:50:00.000Z", &arena(), &[], &[]);
    let list = matches(&state);
    let press = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let mut ui = ui();
    key(&mut ui, press(KeyCode::Up), &list, true);
    assert_eq!(ui.pinned.as_deref(), Some(ID));
    let shown = render(&state, &ui, 120, 30);
    assert!(shown.contains("Pinned") && line_with(&shown, "Pinned").matches("of 2").count() == 0, "{shown}");
    key(&mut ui, press(KeyCode::Down), &list, true);
    assert!(ui.pinned.is_none(), "reaching the newest follows it");
    key(&mut ui, press(KeyCode::Home), &list, true);
    assert!(ui.pinned.is_some());
    key(&mut ui, press(KeyCode::Char('f')), &list, true);
    assert!(ui.pinned.is_none(), "f follows the newest");
    let mut blank = Ui::new(false);
    key(&mut blank, press(KeyCode::Up), &[], true);
    assert!(blank.pinned.is_none(), "keys do nothing before any match");
}

#[test]
fn another_match_opens_at_its_newest_however_it_was_chosen() {
    let mut state = sample();
    let mut ui = ui();
    let scrolled = |ui: &Ui| (ui.events_up, ui.history_up, ui.details_from);
    catch_up(&state, &mut ui);
    (ui.events_up, ui.history_up, ui.details_from) = (2, 1, 3);
    catch_up(&state, &mut ui);
    assert_eq!(scrolled(&ui), (2, 1, 3), "the same match keeps its place");
    // While following, the next match is chosen without a key press.
    play(&mut state, "s-9", "bbbb0000-0000-4000-8000-000000000009", "2026-11-11T00:50:00.000Z", &arena(), &[], &[]);
    catch_up(&state, &mut ui);
    assert_eq!(scrolled(&ui), (0, 0, 0), "{:?}", ui.pinned);
    (ui.events_up, ui.history_up, ui.details_from) = (2, 1, 3);
    let list = catch_up(&state, &mut ui);
    key(&mut ui, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &list, true);
    assert_eq!(scrolled(&ui), (0, 0, 0));
}

#[test]
fn quitting_while_connected_takes_two_presses() {
    let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    let mut ui = ui();
    assert!(matches!(key(&mut ui, q, &[], true), Act::Stay));
    assert!(matches!(key(&mut ui, q, &[], true), Act::Quit));
    let mut ui = Ui::new(false);
    assert!(matches!(key(&mut ui, q, &[], false), Act::Quit));
    let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
    assert!(matches!(key(&mut ui, ctrl_c, &[], true), Act::Quit));
}

#[test]
fn the_quit_prompt_lasts_while_a_second_press_quits() {
    let mut state = sample();
    let ago = |s| Instant::now().checked_sub(Duration::from_secs(s)).unwrap();
    let mut ui = ui();
    key(&mut ui, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE), &[], true);
    assert!(render(&state, &ui, 120, 30).contains("Press q again"));
    ui.notice.as_mut().unwrap().at = ago(3);
    assert!(!render(&state, &ui, 120, 30).contains("Press q again"), "a press now wouldn't quit");
    // Other notices stay longer.
    ui.pinned = Some(ID.into());
    catch_up(&state, &mut ui);
    play(&mut state, "s-9", "bbbb0000-0000-4000-8000-000000000009", "2026-11-11T00:50:00.000Z", &arena(), &[], &[]);
    catch_up(&state, &mut ui);
    ui.notice.as_mut().unwrap().at = ago(3);
    assert!(render(&state, &ui, 120, 30).contains("New match"));
}

#[test]
fn the_playlist_mode_and_map_go_by_the_games_names() {
    let mut state = sample();
    let m = state.p.tracker.matches.get_mut(ID).unwrap();
    m.setup.experience = Some("Arena".into());
    assert!(line_with(&render(&state, &ui(), 120, 30), "Match 1 of 1").contains("Arena"));
    let m = state.p.tracker.matches.get_mut(ID).unwrap();
    m.setup.named = crate::roster::Named {
        playlist: Some("Quick Play".into()),
        mode: Some("Arena:Strongholds".into()),
        map: Some("Live Fire".into()),
    };
    let shown = render(&state, &ui(), 120, 30);
    let head = line_with(&shown, "Match 1 of 1");
    assert!(head.contains("Quick Play   Arena:Strongholds on Live Fire   4v4"), "{head}");
    let row = shown.lines().find(|l| l.contains("4 of 8") && !l.contains("Rank")).unwrap();
    assert!(row.contains("Quick Play"), "{row}");
}

#[test]
fn history_follows_your_mode_mmr_within_one_playlist_and_mode() {
    let mut s = State::new(options(), "no proxy".into());
    let games = [(14.0, true), (5.0, false), (14.6, true), (14.1, true)];
    for (n, (you, strongholds)) in games.into_iter().enumerate() {
        let id = format!("aaaa0000-0000-4000-8000-00000000010{n}");
        let mut ps = arena();
        ps.truncate(8);
        ps[0].mmr = you;
        play(&mut s, &format!("s-{n}"), &id, &format!("2026-11-11T0{n}:00:00.000Z"), &ps, &[], &[]);
        let m = s.p.tracker.matches.get_mut(&id).unwrap();
        m.setup.playlist = Some("quick-play".into());
        m.setup.mode = Some(if strongholds { "strongholds" } else { "slayer" }.into());
        m.setup.named = crate::roster::Named {
            playlist: Some("Quick Play".into()),
            mode: Some(if strongholds { "Arena:Strongholds" } else { "Arena:Slayer" }.into()),
            map: Some("Live Fire".into()),
        };
    }
    let list = matches(&s);
    let mut ui = ui();
    key(&mut ui, KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE), &list, true);
    assert_eq!(ui.focus, Pane::History);
    let shown = render(&s, &ui, 80, 30);
    assert!(shown.contains("Quick Play   Arena:Strongholds") && shown.contains("3 matches"), "{shown}");
    let rows: Vec<&str> = shown.lines().skip(1).filter(|l| l.contains("Live Fire")).collect();
    assert_eq!(rows.len(), 3, "the slayer match isn't here\n{shown}");
    assert!(rows[0].trim_end().ends_with("14.0"), "{shown}");
    assert!(rows[1].trim_end().ends_with("14.6    +0.6"), "{shown}");
    assert!(rows[2].trim_end().ends_with("14.1    -0.5"), "{shown}");
    // It ends at the chosen match.
    key(&mut ui, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE), &list, true);
    key(&mut ui, KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE), &list, true);
    key(&mut ui, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &list, true);
    key(&mut ui, KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE), &list, true);
    assert!(render(&s, &ui, 80, 30).contains("2 matches"));
}

#[test]
fn history_bands_the_chosen_match_only() {
    let mut s = State::new(options(), "no proxy".into());
    for n in 0..3 {
        let id = format!("aaaa0000-0000-4000-8000-00000000010{n}");
        play(&mut s, &format!("s-{n}"), &id, &format!("2026-11-11T0{n}:00:00.000Z"), &arena()[..8], &[], &[]);
        let m = s.p.tracker.matches.get_mut(&id).unwrap();
        m.setup.playlist = Some("quick-play".into());
        m.setup.mode = Some("slayer".into());
        m.setup.named.map = Some("Live Fire".into());
    }
    // The chosen match, the newest, has no Mode MMR for you yet.
    let newest = s.p.tracker.matches.get_mut("aaaa0000-0000-4000-8000-000000000102").unwrap();
    newest.players.get_mut(&ME).unwrap().mmr.truncate(1);
    let mut ui = ui();
    ui.focus(Pane::History);
    let (shown, buf) = draw_buffer(&s, &ui, 80, 30);
    let rows: Vec<u16> =
        shown.lines().enumerate().skip(1).filter(|(_, l)| l.contains("Live Fire")).map(|(y, _)| y as u16).collect();
    assert_eq!(rows.len(), 2, "{shown}");
    assert!(rows.iter().all(|&y| buf[(3, y)].bg != GRAPHITE), "{shown}");
    ui.pinned = Some("aaaa0000-0000-4000-8000-000000000101".into());
    let (shown, buf) = draw_buffer(&s, &ui, 80, 30);
    let y = shown.lines().enumerate().skip(1).filter(|(_, l)| l.contains("Live Fire")).last().unwrap().0;
    assert_eq!(buf[(3, y as u16)].bg, GRAPHITE, "{shown}");
}

#[test]
fn events_are_the_real_ones() {
    let state = sample();
    let shown = render(&state, &ui(), 120, 30);
    assert!(line_with(&shown, "Found").contains("8 players"), "{shown}");
    assert!(shown.contains("Split  4v4  Gap"), "{shown}");
    let late = line_with(&shown, "Late   Thom A293");
    assert!(late.trim_end().ends_with("3.8"), "{late}");
    assert!(line_with(&shown, "Carter A2").contains("Left"));
    assert!(!shown.contains("Rated") && !shown.contains("8.9 to"), "{shown}");
}

#[test]
fn more_teams_come_in_pairs_under_a_scale_of_team_means() {
    let state = lobby(|i| Some((i % 4) as u16));
    let shown = render(&state, &ui(), 120, 34);
    for t in ["Eagle", "Cobra", "Hades", "Valkyrie"] {
        assert!(shown.matches(t).count() >= 2, "{t}\n{shown}");
    }
    assert!(shown.contains("Gap 1.2") && shown.contains("Team rank 4 of 4") && shown.contains("2v2v2v2"), "{shown}");
    assert!(shown.lines().any(|l| l.contains("Eagle  You") && l.contains("Mode MMR")), "{shown}");
    // Your team first, then the best: Valkyrie's mean is highest.
    let lines: Vec<&str> = shown.lines().collect();
    let band = lines.iter().position(|l| l.contains("Eagle  You") && l.contains("Mode MMR")).unwrap();
    assert!(lines[band].contains("Valkyrie"), "{}", lines[band]);
}

#[test]
fn one_player_teams_and_lobbies_are_one_ranked_list_on_the_same_grid() {
    let ffa = render(&lobby(|i| Some(i as u16)), &ui(), 120, 30);
    assert!(ffa.contains("FFA 8") && ffa.contains("Rank 8 of 8") && ffa.contains("Range 2.8"), "{ffa}");
    let at = |n: &str| ffa.lines().position(|l| l.contains(n)).unwrap();
    assert_eq!(at("Alpha") - at("Hotel"), 7);
    let two = render(&sample(), &ui(), 120, 30);
    let mid = start_of(line_with(&two, "Kilo Two"), "\u{2502}").unwrap();
    assert_eq!(end_of(line_with(&ffa, "Hotel"), "5.8"), Some(mid - 2), "{ffa}");
    assert_eq!(start_of(line_with(&ffa, "Alpha"), "You"), Some(3), "{ffa}");
    let before = render(&lobby(|_| None), &ui(), 120, 30);
    assert!(before.contains("Lobby 8"), "{before}");
}

#[test]
fn a_saved_match_is_listed_but_not_shown_until_chosen() {
    let saved = sample().p.tracker.matches[ID].clone();
    let mut state = State::new(options(), "no proxy".into());
    state.p.tracker.resume(saved);
    state.lobby_open = 1;
    let list = matches(&state);
    let press = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let mut ui = ui();
    ui.ours = is_ours(&state, &list);
    assert!(!ui.ours);
    let shown = render(&state, &ui, 120, 30);
    assert!(!shown.contains("Kilo Two") && !shown.contains("Match 1 of 1"), "{shown}");
    key(&mut ui, press(KeyCode::Up), &list, true);
    assert_eq!(ui.pinned.as_deref(), Some(ID), "the newest saved match can be chosen");
    let shown = render(&state, &ui, 120, 30);
    assert!(shown.contains("Kilo Two") && shown.contains("Match 1 of 1"), "{shown}");
    assert!(!is_live(&state, ID, true), "a saved match isn't live");
    key(&mut ui, press(KeyCode::Char('f')), &list, true);
    assert!(!render(&state, &ui, 120, 30).contains("Kilo Two"), "following waits for this run's match");
}

#[test]
fn waiting_shows_the_crystal_where_the_players_go() {
    let empty = State::new(options(), "no proxy".into());
    let shown = render(&empty, &ui(), 120, 30);
    assert!(shown.contains(NONE_YET) && shown.contains("\u{2580}") && !shown.contains("None yet"), "{shown}");
    assert!(line_with(&shown, "Waiting for game").contains("Rosters 0"));
    assert!(!shown.contains("Mode MMR"));
}

#[test]
fn masking_hides_names_and_ids() {
    let state = sample();
    let mut ui = ui();
    ui.mask = true;
    ui.details = true;
    let shown = render(&state, &ui, 120, 30);
    assert!(!shown.contains("Kilo Two") && !shown.contains(&ID[..8]) && shown.contains("Names hidden"));
}

#[test]
fn details_show_the_raw_facts() {
    let state = sample();
    let mut ui = ui();
    ui.details = true;
    let shown = render(&state, &ui, 120, 30);
    assert!(line_with(&shown, "Planned").contains("Xbox MMR") && shown.contains(ID));
    assert!(line_with(&shown, "Thom A293").contains("8.9"));
    assert!(line_with(&shown, "Kilo Two").contains("1 Lead"));
    let lines: Vec<&str> = shown.lines().collect();
    let first = lines.iter().position(|l| l.contains("Cobra") && !l.contains("Gap")).unwrap();
    let eagle = lines.iter().position(|l| l.starts_with("  Eagle")).unwrap();
    assert!(first < eagle, "your team first\n{shown}");
}

#[test]
fn every_player_shows_or_is_counted() {
    let mut s = State::new(options(), "no proxy".into());
    let names = [
        "Alpha", "Bravo", "Charlie", "Delta", "Echo", "Foxtrot", "Golf", "Hotel", "India", "Juliett", "Kilo", "Lima",
        "Mike", "November", "Oscar", "Papa", "Quebec", "Romeo", "Sierra", "Tango", "Uniform", "Victor", "Whiskey",
        "Xray",
    ];
    let ps: Vec<P> =
        (0..24u64).map(|i| p(i, names[i as usize], None, names[i as usize], 5.0 + i as f64 * 0.1, Some(9.0))).collect();
    play(&mut s, "s-8", "dddd0000-0000-4000-8000-000000000008", "2026-11-11T02:00:00.000Z", &ps, &[], &[]);
    let shown = render(&s, &ui(), 120, 30);
    for n in names {
        assert!(shown.contains(n), "{n}\n{shown}");
    }
    // The details have one row per player. The header counts the rows out of
    // view, and the arrows scroll to them.
    let list = matches(&s);
    let press = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let mut ui = ui();
    key(&mut ui, press(KeyCode::Char('d')), &list, true);
    assert_eq!(ui.focus, Pane::Details);
    let shown = render(&s, &ui, 120, 24);
    let head = shown.lines().next().unwrap();
    assert!(head.contains(" below") && !head.contains(" above"), "{shown}");
    assert!(!shown.contains("Alpha"), "the lowest is out of view\n{shown}");
    key(&mut ui, press(KeyCode::End), &list, true);
    let shown = render(&s, &ui, 120, 24);
    assert!(shown.contains("Alpha") && shown.lines().next().unwrap().contains(" above"), "{shown}");
}

#[test]
fn tab_moves_the_keys_along_the_panes_and_the_focused_title_is_lit() {
    let state = sample();
    let list = matches(&state);
    let press = |c| KeyEvent::new(c, KeyModifiers::NONE);
    let mut ui = ui();
    let lit = |ui: &Ui, name: &str| {
        let (shown, buf) = draw_buffer(&state, ui, 120, 30);
        let y = shown.lines().position(|l| l.contains(&format!(" {name} "))).unwrap();
        let x = start_of(shown.lines().nth(y).unwrap(), &format!(" {name} ")).unwrap() as u16;
        buf[(x, y as u16)].bg == CAP_LIT
    };
    assert!(lit(&ui, "Matches") && !lit(&ui, "Events"));
    key(&mut ui, press(KeyCode::Tab), &list, true);
    assert!(lit(&ui, "Events") && !lit(&ui, "Matches"));
    assert!(render(&state, &ui, 120, 30).contains("Scroll"), "the arrows scroll the events");
    key(&mut ui, press(KeyCode::Up), &list, true);
    assert!(ui.pinned.is_none(), "the arrows leave the match alone off the list");
    key(&mut ui, press(KeyCode::Tab), &list, true);
    assert!(lit(&ui, "History"));
    key(&mut ui, press(KeyCode::Tab), &list, true);
    assert!(lit(&ui, "Problems") && render(&state, &ui, 120, 30).contains("No problems yet"));
    key(&mut ui, press(KeyCode::Char('d')), &list, true);
    assert!(lit(&ui, "Match 1 of 1"));
    key(&mut ui, press(KeyCode::Esc), &list, true);
    assert!(!ui.details && ui.focus == Pane::Problems, "back where it was");
    key(&mut ui, press(KeyCode::Esc), &list, true);
    assert_eq!(ui.focus, Pane::Matches);
    key(&mut ui, press(KeyCode::BackTab), &list, true);
    assert_eq!(ui.focus, Pane::Problems);
}

#[test]
fn every_size_draws() {
    let empty = State::new(options(), "no proxy".into());
    let states = [sample(), empty, lobby(|i| Some((i % 4) as u16)), lobby(|i| Some(i as u16)), lobby(|_| None)];
    for s in &states {
        for pane in [Pane::Matches, Pane::Events, Pane::History, Pane::Problems, Pane::Details] {
            for (keys, details) in [(false, false), (true, false), (false, true)] {
                let mut ui = ui();
                ui.focus(pane);
                ui.all_keys = keys;
                ui.details = details || pane == Pane::Details;
                for (w, h) in [(160, 45), (120, 30), (80, 24), (60, 20), (20, 6), (1, 1)] {
                    render(s, &ui, w, h);
                }
            }
        }
    }
    let narrow = render(&sample(), &ui(), 80, 24);
    assert!(narrow.contains("Match 1 of 1") && narrow.contains("Spartan Nine") && narrow.contains("Mode"), "{narrow}");
    assert!(narrow.contains(" q  Quit"), "every key stays, on two lines if need be\n{narrow}");
}

#[test]
fn every_height_draws_at_every_width() {
    let empty = State::new(options(), "no proxy".into());
    let saved = saved();
    let middle = saved.p.tracker.order[4].clone();
    // Following this run's newest, waiting for one, and a saved match chosen.
    let states = [(&empty, true, None), (&sample(), true, None), (&saved, false, None), (&saved, false, Some(middle))];
    for (s, ours, pinned) in states {
        for pane in [Pane::Matches, Pane::Events, Pane::History] {
            let mut ui = ui();
            ui.ours = ours;
            ui.pinned.clone_from(&pinned);
            ui.focus(pane);
            for w in [1, 4, 80, 114, 120] {
                for h in 1..=46 {
                    render(s, &ui, w, h);
                }
            }
        }
    }
}

#[test]
fn the_logo_is_square() {
    let rows = logo();
    assert_eq!(rows.len(), 10);
    assert!(rows.iter().all(|r| r.len() == 10));
}

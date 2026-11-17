//! The names of the game's playlists, modes and maps. The match settings give only
//! asset ids, but the game's web cache on this machine keeps the content service's
//! document for each asset it has looked up, with its asset id and public name.
//! mmr only reads those files, and never asks a service.
//!
//! A cached entry is Bond: field 2 is the response body, itself Bond, whose field 0
//! is the asset id, 2 the public name and 4 where its files are stored (a path under
//! `/ugcstorage/<kind>/`). The name comes first, so a large entry is read only in
//! part.

use crate::bond::{self, Value};
use crate::roster::{self, Named, Setup};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, SystemTime};

/// The cache, under the game's folder.
const CACHE: &str = r"disk_cache\webcache";
/// How much of an entry is read: the name is near its start.
const HEAD: u64 = 64 * 1024;
/// How often the cache is read again for entries the game has added.
const AGAIN: Duration = Duration::from_secs(20);

/// An entry's length and when it was last written.
type Stamp = (u64, Option<SystemTime>);

#[derive(Default)]
struct Catalog {
    names: HashMap<String, String>,
    /// Each entry read, as it was then.
    seen: HashMap<PathBuf, Stamp>,
}

/// The one catalog. Tests run in parallel and share it, so each uses asset ids and
/// files of its own.
fn catalog() -> MutexGuard<'static, Catalog> {
    static CATALOG: OnceLock<Mutex<Catalog>> = OnceLock::new();
    CATALOG.get_or_init(Default::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// An asset's public name, if the game's cache has it.
pub fn name(id: &str) -> Option<String> {
    catalog().names.get(id).cloned()
}

/// Reads the game's cache, in `game` or wherever the game is installed, and again
/// every `AGAIN` while mmr runs.
pub fn follow(game: Option<PathBuf>) {
    std::thread::spawn(move || {
        let dirs = folders(game);
        loop {
            for d in &dirs {
                scan(d);
            }
            std::thread::sleep(AGAIN);
        }
    });
}

/// The web cache folders to read: under `game` when it's given (the game's folder
/// or the cache itself), otherwise those that exist under each Steam library and
/// the Xbox app's default folder.
fn folders(game: Option<PathBuf>) -> Vec<PathBuf> {
    let installs = match game {
        Some(g) if g.ends_with("webcache") => return vec![g],
        Some(g) => vec![g],
        None => {
            let steam = std::env::var_os("ProgramFiles(x86)")
                .map_or_else(|| PathBuf::from(r"C:\Program Files (x86)\Steam"), |p| PathBuf::from(p).join("Steam"));
            let vdf = std::fs::read_to_string(steam.join(r"steamapps\libraryfolders.vdf")).unwrap_or_default();
            let mut libraries = vec![steam];
            libraries.extend(steam_libraries(&vdf));
            let mut v: Vec<PathBuf> = libraries.iter().map(|l| l.join(r"steamapps\common\Halo Infinite")).collect();
            v.push(PathBuf::from(r"C:\XboxGames\Halo Infinite\Content"));
            v
        }
    };
    let mut out: Vec<PathBuf> = Vec::new();
    for d in installs.iter().map(|i| i.join(CACHE)).filter(|d| d.is_dir()) {
        if !out.contains(&d) {
            out.push(d);
        }
    }
    out
}

/// The library folders a Steam `libraryfolders.vdf` lists, from its `"path"` lines.
fn steam_libraries(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix("\"path\"")?.trim();
            let path = rest.strip_prefix('"')?.strip_suffix('"')?;
            Some(PathBuf::from(path.replace("\\\\", "\\")))
        })
        .collect()
}

/// Reads the entries of `dir` not read before, or changed since. An entry the game
/// holds open can't be read yet, and is tried again next time.
fn scan(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        let stamp = e.metadata().ok().map(|m| (m.len(), m.modified().ok()));
        if stamp.is_some() && catalog().seen.get(&path) == stamp.as_ref() {
            continue;
        }
        let mut head = Vec::new();
        if std::fs::File::open(&path).and_then(|f| f.take(HEAD).read_to_end(&mut head)).is_err() {
            continue;
        }
        let found = asset(&head);
        let mut c = catalog();
        if let Some(stamp) = stamp {
            c.seen.insert(path, stamp);
        }
        if let Some((id, name)) = found {
            c.names.insert(id, name);
        }
    }
}

/// The asset id and public name in the head of a cached entry, if it's an asset
/// document from the content service.
fn asset(entry: &[u8]) -> Option<(String, String)> {
    let outer = bond::head(entry);
    let body = outer.iter().find_map(|(i, v)| match (i, v) {
        (2, Value::Bytes(b)) => Some(b),
        _ => None,
    })?;
    let doc = bond::head(body);
    let field = |id: u16| doc.iter().find(|(i, _)| *i == id).map(|(_, v)| v);
    let stored = field(4).and_then(|f| f.field(0));
    if !matches!(stored, Some(Value::Str(s)) if s.contains("/ugcstorage/")) {
        return None;
    }
    let id = roster::guid(field(0)?)?;
    match field(2)? {
        Value::Str(name) if !name.trim().is_empty() => Some((id, name.trim().to_string())),
        _ => None,
    }
}

/// A map and mode pair's name, split into the mode and the map. The game names them
/// like `Arena:Strongholds on Live Fire` or `CTF:Arena on Unconveyed`, with the
/// mode's two parts in either order, so the mode is kept as written.
fn split_pair(name: &str) -> (Option<String>, Option<String>) {
    match name.rsplit_once(" on ") {
        Some((mode, map)) => (Some(mode.trim().to_string()), Some(map.trim().to_string())),
        None => (Some(name.trim().to_string()), None),
    }
}

/// The names of a match's playlist, mode and map: the ones it kept, then the
/// cache's. The mode and map come from the playlist's map and mode pair if it has one.
pub fn of(s: &Setup) -> Named {
    let look = |id: &Option<String>| id.as_deref().and_then(name);
    let (mode, map) = look(&s.pair).map_or((None, None), |p| split_pair(&p));
    s.named.clone().or(Named {
        playlist: look(&s.playlist),
        mode: mode.or_else(|| look(&s.mode)),
        map: map.or_else(|| look(&s.map)),
    })
}

/// The playlist as the game names it. Without a known name, it's built from the
/// settings' experience, with a space before each word (BigTeam becomes Big Team
/// Battle, FFA stays FFA) and Ranked in front when it's ranked, like Ranked Arena.
pub fn playlist(s: &Setup) -> String {
    if let Some(name) = of(s).playlist {
        return name;
    }
    let spaced = |s: &str| {
        let c: Vec<char> = s.chars().collect();
        let mut out = String::new();
        for (i, &ch) in c.iter().enumerate() {
            let lower = |j: usize| c.get(j).is_some_and(char::is_ascii_lowercase);
            let word = i > 0 && ch.is_ascii_uppercase() && (lower(i - 1) || lower(i + 1));
            if word && !out.ends_with(' ') {
                out.push(' ');
            }
            out.push(ch);
        }
        out
    };
    let name = |e: &str| if e == "BigTeam" { "Big Team Battle".to_string() } else { spaced(e) };
    match (&s.experience, s.ranked) {
        (Some(e), Some(true)) => format!("Ranked {}", name(e)),
        (Some(e), _) => name(e),
        (None, Some(true)) => "Ranked".into(),
        (None, _) => String::new(),
    }
}

/// The playlist and mode that a match's Mode MMR compares within, by asset id where
/// the settings gave one. `None` when the mode isn't known.
pub fn mode_key(s: &Setup) -> Option<(String, String)> {
    let mode = s.mode.clone().or_else(|| of(s).mode)?;
    Some((s.playlist.clone().unwrap_or_else(|| playlist(s)), mode))
}

/// Reads the game's cache once, for a command that runs and ends.
pub fn load(game: Option<PathBuf>) {
    for d in folders(game) {
        scan(&d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cached entry as the game writes it, cut at `keep` bytes: an ETag, the
    /// headers, then the body, an asset document stored under `kind`.
    fn entry(id: [u64; 4], name: &str, kind: &str, keep: usize) -> Vec<u8> {
        fn st(fields: &[u8]) -> Vec<u8> {
            let mut v = vec![fields.len() as u8 + 1];
            v.extend_from_slice(fields);
            v.push(0);
            v
        }
        fn var(mut n: u64, out: &mut Vec<u8>) {
            loop {
                let b = (n & 127) as u8;
                n >>= 7;
                if n == 0 {
                    out.push(b);
                    return;
                }
                out.push(b | 128);
            }
        }
        let mut guid = Vec::new();
        for (i, part) in id.iter().enumerate() {
            guid.push(
                (i as u8) << 5
                    | if i == 0 {
                        5
                    } else if i == 3 {
                        6
                    } else {
                        4
                    },
            );
            var(*part, &mut guid);
        }
        let mut doc = vec![0x0a];
        doc.extend(st(&guid));
        let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
        doc.push(0x52);
        var(name.encode_utf16().count() as u64, &mut doc);
        doc.extend(units);
        let url = format!("https://blobs/ugcstorage/{kind}/x/y/");
        let mut files = vec![0x09];
        var(url.len() as u64, &mut files);
        files.extend(url.bytes());
        doc.push(0x8a);
        doc.extend(st(&files));
        let mut body = Vec::new();
        var(doc.len() as u64 + 1, &mut body);
        body.extend(doc);
        body.push(0);
        let mut out = vec![0];
        out.extend([0x09, 2, b'e', b't']);
        out.push(0x4b);
        out.push(0x0e);
        var(body.len() as u64, &mut out);
        out.extend(body);
        out.push(0);
        let n = out.len() as u64 - 1;
        let mut whole = Vec::new();
        var(n, &mut whole);
        whole.extend(&out[1..]);
        whole.truncate(keep);
        whole
    }

    const ARENA: [u64; 4] = [0xedfef3ac, 0x9cbe, 0x4fa2, 0x83d4afde298f49b9];

    #[test]
    fn reads_an_assets_id_and_name_from_the_cache() {
        let e = entry(ARENA, "Ranked Arena", "playlist", usize::MAX);
        assert_eq!(asset(&e), Some(("edfef3ac-9cbe-4fa2-b949-8f29deafd483".to_string(), "Ranked Arena".to_string())));
        // Cut after the name and where its files are: still read.
        let cut = entry(ARENA, "Ranked Arena", "playlist", e.len() - 2);
        assert!(asset(&cut).is_some());
        assert_eq!(asset(&entry(ARENA, "Ranked Arena", "playlist", 12)), None);
    }

    #[test]
    fn an_entry_the_game_holds_open_is_read_once_its_free() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = crate::store::scratch("held");
        let e = entry([0x9100_0001, 1, 2, 3], "Held Playlist", "playlist", usize::MAX);
        let id = asset(&e).unwrap().0;
        let path = dir.join("held-entry");
        std::fs::write(&path, &e).unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&path).unwrap();
        scan(&dir);
        assert_eq!(name(&id), None);
        drop(held);
        scan(&dir);
        assert_eq!(name(&id).as_deref(), Some("Held Playlist"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_that_changes_is_read_again() {
        let dir = crate::store::scratch("changes");
        let e = entry([0x9100_0002, 1, 2, 3], "Changed Playlist", "playlist", usize::MAX);
        let id = asset(&e).unwrap().0;
        let path = dir.join("changing-entry");
        std::fs::write(&path, b"not yet").unwrap();
        scan(&dir);
        assert_eq!(name(&id), None);
        std::fs::write(&path, &e).unwrap();
        scan(&dir);
        assert_eq!(name(&id).as_deref(), Some("Changed Playlist"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pair_names_the_mode_and_the_map() {
        let p = |n: &str| split_pair(n);
        assert_eq!(p("Arena:Strongholds on Live Fire"), (Some("Arena:Strongholds".into()), Some("Live Fire".into())));
        assert_eq!(p("CTF:Arena on Unconveyed"), (Some("CTF:Arena".into()), Some("Unconveyed".into())));
        assert_eq!(p("Arena:VIP"), (Some("Arena:VIP".into()), None));
    }

    #[test]
    fn finds_every_steam_library() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(steam_libraries(vdf), vec![PathBuf::from(r"D:\SteamLibrary")]);
    }

    #[test]
    fn a_match_keeps_its_own_names_first() {
        let s = Setup {
            playlist: Some("no-such-asset".into()),
            named: Named { playlist: Some("Quick Play".into()), ..Named::default() },
            ..Setup::default()
        };
        assert_eq!(of(&s).playlist.as_deref(), Some("Quick Play"));
        assert_eq!(of(&s).map, None);
    }
}

//! Matches on disk, one JSON file each, named by match id. Only decoded facts are
//! written: never raw messages, tokens or keys.

use crate::tracker::Match;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map_or_else(|| PathBuf::from("."), PathBuf::from);
    base.join("hi-mmr")
}

pub fn matches_dir(dir: &Path) -> PathBuf {
    dir.join("matches")
}

fn file_name(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect::<String>() + ".json"
}

pub fn path_of(dir: &Path, key: &str) -> PathBuf {
    matches_dir(dir).join(file_name(key))
}

/// Writes to a temporary name, flushes it to disk and renames it, so a crash or a
/// power cut can't leave half a file. Once the match id is known, the file saved
/// under the game session is removed.
pub fn save(dir: &Path, m: &Match) -> io::Result<PathBuf> {
    fs::create_dir_all(matches_dir(dir))?;
    let path = path_of(dir, &m.key);
    let tmp = path.with_extension("tmp");
    let mut f = fs::File::create(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(m)?)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, &path)?;
    if let Some(s) = m.game_session.as_deref().filter(|s| *s != m.key) {
        let _ = fs::remove_file(path_of(dir, s));
    }
    Ok(path)
}

/// The match saved under `key`, or `None` when there's none. A file that isn't a
/// match is moved aside as `<name>.bad`, so the next save can't overwrite it. Both
/// that and a file that can't be read return an error.
pub fn load(dir: &Path, key: &str) -> io::Result<Option<Match>> {
    let path = path_of(dir, key);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match serde_json::from_slice(&bytes) {
        Ok(m) => Ok(Some(m)),
        Err(e) => Err(match aside(&path) {
            Ok(()) => io::Error::new(io::ErrorKind::InvalidData, format!("not a match ({e}), moved aside as .bad")),
            Err(moving) => {
                io::Error::new(moving.kind(), format!("not a match ({e}), and can't move it aside: {moving}"))
            }
        }),
    }
}

/// Renames `path` to the first free name of `<name>.bad`, `<name>.2.bad` and so on.
fn aside(path: &Path) -> io::Result<()> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    for n in 1..1000 {
        let to = path.with_file_name(if n == 1 { format!("{name}.bad") } else { format!("{name}.{n}.bad") });
        if !to.exists() {
            return fs::rename(path, to);
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "too many .bad files"))
}

/// Every saved match, oldest first. A file that can't be read or isn't a match is
/// skipped.
pub fn load_all(dir: &Path) -> io::Result<Vec<Match>> {
    let mut out = Vec::new();
    let matches = matches_dir(dir);
    if !matches.exists() {
        return Ok(out);
    }
    for e in fs::read_dir(matches)? {
        let path = e?.path();
        if path.extension().is_some_and(|x| x == "json") {
            let read = fs::read(&path).map_err(|e| e.to_string());
            match read.and_then(|b| serde_json::from_slice::<Match>(&b).map_err(|e| e.to_string())) {
                Ok(m) => out.push(m),
                Err(err) => eprintln!("skipping {}: {err}", path.display()),
            }
        }
    }
    out.sort_by(|a, b| a.first_seen.cmp(&b.first_seen).then_with(|| a.key.cmp(&b.key)));
    Ok(out)
}

/// A new empty folder for a test, under the system's temporary folder.
#[cfg(test)]
pub fn scratch(tag: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let d = std::env::temp_dir().join(format!("mmr-test-{}-{tag}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn saved(key: &str, first_seen: &str) -> Match {
        Match { key: key.into(), first_seen: first_seen.into(), ..Match::default() }
    }

    #[test]
    fn a_saved_match_reads_back_and_leaves_no_temporary_file() {
        let dir = scratch("save");
        let m = saved("m", "2026-11-11T00:00:00.000Z");
        let path = save(&dir, &m).unwrap();
        assert_eq!(load(&dir, "m").unwrap(), Some(m));
        assert!(!path.with_extension("tmp").exists());
        assert_eq!(load(&dir, "absent").unwrap(), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_is_moved_aside_not_overwritten() {
        let dir = scratch("corrupt");
        fs::create_dir_all(matches_dir(&dir)).unwrap();
        let path = path_of(&dir, "m");
        fs::write(&path, b"{\"key\": \"m\", tor").unwrap();
        assert!(load(&dir, "m").is_err());
        assert!(!path.exists());
        let bad = matches_dir(&dir).join("m.json.bad");
        assert_eq!(fs::read(&bad).unwrap(), b"{\"key\": \"m\", tor");
        // Corrupt again: the first copy stays.
        fs::write(&path, b"[").unwrap();
        assert!(load(&dir, "m").is_err());
        assert_eq!(fs::read(&bad).unwrap(), b"{\"key\": \"m\", tor");
        assert_eq!(fs::read(matches_dir(&dir).join("m.json.2.bad")).unwrap(), b"[");
        // Only .json files are matches.
        save(&dir, &saved("m", "2026-11-11T00:00:00.000Z")).unwrap();
        assert_eq!(load_all(&dir).unwrap().len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cant_be_read_is_an_error_and_is_skipped_by_load_all() {
        let dir = scratch("unreadable");
        save(&dir, &saved("good", "2026-11-11T00:00:00.000Z")).unwrap();
        // A folder by a match's name can't be read as a file.
        fs::create_dir_all(path_of(&dir, "held")).unwrap();
        assert!(load(&dir, "held").is_err());
        assert!(path_of(&dir, "held").exists(), "left where it is");
        let all = load_all(&dir).unwrap();
        assert_eq!(all.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["good"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn matches_seen_at_once_sort_by_key() {
        let dir = scratch("ties");
        fs::create_dir_all(matches_dir(&dir)).unwrap();
        // Named so the folder lists them the other way around.
        let at = "2026-11-11T00:00:00.000Z";
        fs::write(path_of(&dir, "a"), serde_json::to_vec(&saved("z", at)).unwrap()).unwrap();
        fs::write(path_of(&dir, "b"), serde_json::to_vec(&saved("y", at)).unwrap()).unwrap();
        fs::write(path_of(&dir, "c"), serde_json::to_vec(&saved("x", "2026-11-10T00:00:00.000Z")).unwrap()).unwrap();
        let keys: Vec<String> = load_all(&dir).unwrap().into_iter().map(|m| m.key).collect();
        assert_eq!(keys, ["x", "y", "z"]);
        let _ = fs::remove_dir_all(&dir);
    }
}

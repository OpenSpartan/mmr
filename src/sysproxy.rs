//! The Windows system proxy (the current user's WinINet LAN settings) while `mmr
//! watch` runs.
//!
//! Before changing anything, mmr writes the current settings to a file. It puts them
//! back on exit, on Ctrl+C or Ctrl+Break, and when the window closes. Windows doesn't
//! send sign-out or shutdown events to a program that loads user32, and WinINet loads
//! it, so after those, or a crash, the next run of mmr puts them back. Only HTTPS goes
//! to the proxy (`https=127.0.0.1:<port>`). Plain HTTP and the bypass list's hosts go
//! direct.

use crate::store;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use windows_sys::Win32::Foundation::{CloseHandle, GlobalFree, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::Networking::WinInet::{
    InternetQueryOptionW, InternetSetOptionW, INTERNET_OPTION_PER_CONNECTION_OPTION, INTERNET_OPTION_REFRESH,
    INTERNET_OPTION_SETTINGS_CHANGED, INTERNET_PER_CONN_AUTOCONFIG_URL, INTERNET_PER_CONN_FLAGS,
    INTERNET_PER_CONN_FLAGS_UI, INTERNET_PER_CONN_OPTIONW, INTERNET_PER_CONN_OPTIONW_0, INTERNET_PER_CONN_OPTION_LISTW,
    INTERNET_PER_CONN_PROXY_BYPASS, INTERNET_PER_CONN_PROXY_SERVER, PROXY_TYPE_DIRECT, PROXY_TYPE_PROXY,
};
use windows_sys::Win32::System::Threading::CreateMutexW;

/// The mutex `mmr watch` holds while it runs, and other commands hold while they recover.
/// The installer checks for it by name (its AppMutex) and asks for mmr to be closed first.
const RUNNING: &str = "OpenSpartan.mmr.watch";

/// Held while `mmr watch` runs, or another command recovers.
pub struct Running(HANDLE);

impl Drop for Running {
    fn drop(&mut self) {
        // SAFETY: the value owns the non-null handle, and closes it only here.
        unsafe { CloseHandle(self.0) };
    }
}

/// Marks `mmr watch` as running, unless another one already is.
pub fn hold() -> Result<Running, String> {
    hold_as(RUNNING)
}

fn hold_as(name: &str) -> Result<Running, String> {
    let w = crate::wide(name);
    // SAFETY: null security attributes are allowed, and the name is nul-terminated
    // and outlives the call.
    let h = unsafe { CreateMutexW(std::ptr::null(), 0, w.as_ptr()) };
    let e = io::Error::last_os_error();
    if h.is_null() {
        return Err(format!("can't make the {name} mutex: {e}"));
    }
    if e.raw_os_error() == Some(ERROR_ALREADY_EXISTS.cast_signed()) {
        // SAFETY: the handle was just returned, and isn't used after.
        unsafe { CloseHandle(h) };
        return Err("another `mmr watch` is running".into());
    }
    Ok(Running(h))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub flags: u32,
    pub server: Option<String>,
    pub bypass: Option<String>,
    pub autoconfig: Option<String>,
}

/// The string options, after the connection type.
const STRINGS: [u32; 3] =
    [INTERNET_PER_CONN_PROXY_SERVER, INTERNET_PER_CONN_PROXY_BYPASS, INTERNET_PER_CONN_AUTOCONFIG_URL];

/// Copies a string WinINet returned, then frees it.
///
/// # Safety
///
/// `p` is null, or a nul-terminated string from GlobalAlloc that isn't used after.
#[expect(clippy::maybe_infinite_iter, reason = "a wide C string ends at its nul")]
unsafe fn take_string(p: *mut u16) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the string is nul-terminated, so each unit up to the nul can be read.
    let n = (0..).take_while(|i| unsafe { *p.add(*i) } != 0).count();
    // SAFETY: the n units before the nul were just read.
    let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, n) });
    // SAFETY: from GlobalAlloc, and not used after.
    unsafe { GlobalFree(p.cast()) };
    Some(s).filter(|s| !s.is_empty())
}

fn list(options: &mut [INTERNET_PER_CONN_OPTIONW]) -> INTERNET_PER_CONN_OPTION_LISTW {
    INTERNET_PER_CONN_OPTION_LISTW {
        dwSize: size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32,
        pszConnection: std::ptr::null_mut(), // the LAN settings
        dwOptionCount: options.len() as u32,
        dwOptionError: 0,
        pOptions: options.as_mut_ptr(),
    }
}

/// The current settings. Microsoft advises reading the connection type with
/// INTERNET_PER_CONN_FLAGS_UI, since INTERNET_PER_CONN_FLAGS can leave out
/// "Automatically detect settings", and falling back where Windows lacks it.
pub fn query() -> Result<Settings, String> {
    query_as(INTERNET_PER_CONN_FLAGS_UI).or_else(|_| query_as(INTERNET_PER_CONN_FLAGS))
}

fn query_as(flags: u32) -> Result<Settings, String> {
    let [server, bypass, autoconfig] = STRINGS;
    let mut options =
        [flags, server, bypass, autoconfig].map(|o| INTERNET_PER_CONN_OPTIONW { dwOption: o, ..Default::default() });
    let mut l = list(&mut options);
    let mut size = size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32;
    // SAFETY: pOptions points at the live options array with dwOptionCount entries,
    // and the size is the list's size.
    let ok = unsafe {
        InternetQueryOptionW(
            std::ptr::null(),
            INTERNET_OPTION_PER_CONNECTION_OPTION,
            (&raw mut l).cast(),
            &raw mut size,
        )
    };
    if ok == 0 {
        return Err(format!("can't read the system proxy settings: {}", io::Error::last_os_error()));
    }
    // SAFETY: WinINet set dwValue for the flags, and a GlobalAlloc'd string or null for
    // each string option, and each is taken once.
    unsafe {
        Ok(Settings {
            flags: options[0].Value.dwValue,
            server: take_string(options[1].Value.pszValue),
            bypass: take_string(options[2].Value.pszValue),
            autoconfig: take_string(options[3].Value.pszValue),
        })
    }
}

/// Changes the settings. The connection type is always set with
/// INTERNET_PER_CONN_FLAGS, whichever way it was read.
fn apply(s: &Settings) -> Result<(), String> {
    let strings = [&s.server, &s.bypass, &s.autoconfig].map(|v| crate::wide(v.as_deref().unwrap_or("")));
    let mut options = vec![INTERNET_PER_CONN_OPTIONW {
        dwOption: INTERNET_PER_CONN_FLAGS,
        Value: INTERNET_PER_CONN_OPTIONW_0 { dwValue: s.flags },
    }];
    for (o, w) in STRINGS.iter().zip(&strings) {
        options.push(INTERNET_PER_CONN_OPTIONW {
            dwOption: *o,
            Value: INTERNET_PER_CONN_OPTIONW_0 { pszValue: w.as_ptr().cast_mut() },
        });
    }
    let l = list(&mut options);
    let size = size_of::<INTERNET_PER_CONN_OPTION_LISTW>() as u32;
    // SAFETY: the list and its strings live to the end of the function, and WinINet
    // only reads them.
    let ok = unsafe {
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_PER_CONNECTION_OPTION, (&raw const l).cast(), size)
    };
    if ok == 0 {
        return Err(format!("can't change the system proxy settings: {}", io::Error::last_os_error()));
    }
    // Tells running programs to read the settings again.
    // SAFETY: these two options take no buffer.
    unsafe {
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_SETTINGS_CHANGED, std::ptr::null(), 0);
        InternetSetOptionW(std::ptr::null(), INTERNET_OPTION_REFRESH, std::ptr::null(), 0);
    }
    Ok(())
}

/// Always in the default store, whatever `--store` says, so any later run finds it.
fn backup_path() -> PathBuf {
    store::default_dir().join("proxy-backup.json")
}

fn write_backup(path: &Path, s: &Settings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    }
    let json = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| format!("can't write {}: {e}", path.display()))
}

/// The backup a run left, or `None` if there isn't one.
fn read_backup(path: &Path) -> Result<Option<Settings>, String> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("can't read {}: {e}", path.display())),
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|e| format!("can't read {}: {e}", path.display()))
}

fn ours(port: u16) -> String {
    format!("https=127.0.0.1:{port}")
}

fn is_ours(s: &Settings) -> bool {
    s.flags & PROXY_TYPE_PROXY != 0 && s.server.as_deref().is_some_and(|v| v.starts_with("https=127.0.0.1:"))
}

/// `s` with the proxy turned off: direct, with no server, and the bypass list and
/// setup script as they were.
fn off(s: &Settings) -> Settings {
    Settings { flags: (s.flags & !PROXY_TYPE_PROXY) | PROXY_TYPE_DIRECT, server: None, ..s.clone() }
}

/// What to put back, while the proxy is set.
struct Active {
    previous: Settings,
    backup: PathBuf,
}

/// Held while settings are applied, so a `set` and a `restore` never interleave.
static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// Puts back the settings from before `set`, if they aren't back already. Safe to
/// call from the console control handler, and more than once.
pub fn restore() -> Option<Result<(), String>> {
    let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    let a = active.take()?;
    Some(match apply(&a.previous) {
        Ok(()) => {
            let _ = std::fs::remove_file(&a.backup);
            Ok(())
        }
        Err(e) => Err(format!("Restoring the system proxy settings failed ({e}). Running mmr again puts them back.")),
    })
}

/// Restores the system proxy on drop.
pub struct Guard {
    /// The settings `set` replaced.
    pub before: Settings,
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(Err(e)) = restore() {
            eprintln!("{e}");
        }
    }
}

/// Points the system proxy at 127.0.0.1:`port` until the guard drops. Call it
/// holding the watch mutex, after `recover`.
pub fn set(port: u16) -> Result<Guard, String> {
    let previous = query()?;
    if is_ours(&previous) {
        let e = "the system proxy still points at an earlier run of mmr: turn it off in the Windows proxy settings";
        return Err(e.into());
    }
    let backup = backup_path();
    write_backup(&backup, &previous)?;
    let mine = Settings {
        flags: PROXY_TYPE_DIRECT | PROXY_TYPE_PROXY,
        server: Some(ours(port)),
        bypass: previous.bypass.clone().or_else(|| Some("<local>".to_string())),
        autoconfig: previous.autoconfig.clone(),
    };
    // Held while applying, so a Ctrl+C waits and then puts the settings back.
    let mut active = ACTIVE.lock().unwrap_or_else(PoisonError::into_inner);
    *active = Some(Active { previous: previous.clone(), backup });
    if let Err(e) = apply(&mine) {
        drop(active);
        restore();
        return Err(e);
    }
    Ok(Guard { before: previous })
}

/// What recovery does.
#[derive(Debug, PartialEq)]
enum Fix {
    /// The proxy doesn't point at mmr, so any backup is out of date.
    Nothing,
    /// Put back the settings from the backup.
    Restore(Settings),
    /// Turn the proxy off, as there's no backup to use, and say why.
    Off(Settings, String),
}

/// What to do about the settings `now`, given the backup a run left.
fn plan(now: &Settings, saved: Result<Option<Settings>, String>) -> Fix {
    if !is_ours(now) {
        return Fix::Nothing;
    }
    match saved {
        Ok(Some(s)) if !is_ours(&s) => Fix::Restore(s),
        Ok(Some(_)) => Fix::Off(off(now), "its backup points at mmr too".into()),
        Ok(None) => Fix::Off(off(now), "no backup of the earlier settings was found".into()),
        Err(e) => Fix::Off(off(now), format!("its backup can't be used ({e})")),
    }
}

/// After a crash: if the proxy still points at mmr, puts back the backed-up settings, or
/// turns the proxy off if there's no backup to use, and says what it did. Call it holding
/// the watch mutex, so a running watch is never undone.
pub fn recover() -> Option<String> {
    let backup = backup_path();
    let now = query().ok()?;
    let (s, done) = match plan(&now, read_backup(&backup)) {
        Fix::Nothing => {
            let _ = std::fs::remove_file(&backup);
            return None;
        }
        Fix::Restore(s) => (s, "Restored the system proxy settings an earlier run of mmr left in place.".to_string()),
        Fix::Off(s, why) => {
            (s, format!("An earlier run of mmr left the system proxy pointing at it, and {why}, so mmr turned it off."))
        }
    };
    if let Err(e) = apply(&s) {
        return Some(format!("An earlier run of mmr left the system proxy set, and fixing it failed: {e}"));
    }
    let _ = std::fs::remove_file(&backup);
    Some(done)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Networking::WinInet::{PROXY_TYPE_AUTO_DETECT, PROXY_TYPE_AUTO_PROXY_URL};

    fn s(flags: u32, server: &str) -> Settings {
        Settings {
            flags,
            server: Some(server.to_string()),
            bypass: Some("<local>".to_string()),
            autoconfig: Some("http://wpad/wpad.dat".to_string()),
        }
    }

    #[test]
    fn one_watch_at_a_time() {
        let name = "OpenSpartan.mmr.test";
        let first = hold_as(name).unwrap();
        assert!(hold_as(name).is_err());
        drop(first);
        drop(hold_as(name).unwrap());
    }

    #[test]
    fn only_our_setting_counts_as_ours() {
        assert!(is_ours(&s(PROXY_TYPE_DIRECT | PROXY_TYPE_PROXY, &ours(47811))));
        assert!(!is_ours(&s(PROXY_TYPE_DIRECT, &ours(47811))));
        assert!(!is_ours(&s(PROXY_TYPE_DIRECT | PROXY_TYPE_PROXY, "127.0.0.1:8888")));
    }

    #[test]
    fn turning_the_proxy_off_keeps_the_rest() {
        let now = s(PROXY_TYPE_PROXY | PROXY_TYPE_AUTO_DETECT | PROXY_TYPE_AUTO_PROXY_URL, &ours(47811));
        let o = off(&now);
        assert_eq!(o.flags, PROXY_TYPE_DIRECT | PROXY_TYPE_AUTO_DETECT | PROXY_TYPE_AUTO_PROXY_URL);
        assert_eq!(o.server, None);
        assert_eq!((&o.bypass, &o.autoconfig), (&now.bypass, &now.autoconfig));
        assert!(!is_ours(&o));
    }

    #[test]
    fn recovery_restores_the_backup_or_turns_the_proxy_off() {
        let mine = s(PROXY_TYPE_DIRECT | PROXY_TYPE_PROXY, &ours(47811));
        let before = s(PROXY_TYPE_DIRECT | PROXY_TYPE_AUTO_DETECT, "");
        // Not ours: nothing to put back, whatever the backup says.
        assert_eq!(plan(&before, Ok(Some(mine.clone()))), Fix::Nothing);
        assert_eq!(plan(&before, Err("bad".into())), Fix::Nothing);
        assert_eq!(plan(&before, Ok(None)), Fix::Nothing);
        assert_eq!(plan(&mine, Ok(Some(before.clone()))), Fix::Restore(before));
        for saved in [Ok(None), Err("can't read it".to_string()), Ok(Some(mine.clone()))] {
            match plan(&mine, saved) {
                Fix::Off(o, _) => assert_eq!(o, off(&mine)),
                f => panic!("{f:?}"),
            }
        }
        // A corrupt backup is reported.
        let Fix::Off(_, why) = plan(&mine, Err("can't read it".into())) else { panic!() };
        assert!(why.contains("can't read it"), "{why}");
    }

    #[test]
    fn a_backup_is_missing_readable_or_reported() {
        let dir = std::env::temp_dir().join(format!("mmr-test-backup-{}", std::process::id()));
        let path = dir.join("proxy-backup.json");
        assert_eq!(read_backup(&path), Ok(None));
        let saved = s(PROXY_TYPE_DIRECT, "");
        write_backup(&path, &saved).unwrap();
        assert_eq!(read_backup(&path), Ok(Some(saved)));
        std::fs::write(&path, b"{\"flags\":").unwrap();
        let e = read_backup(&path).unwrap_err();
        assert!(e.contains(&path.display().to_string()), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

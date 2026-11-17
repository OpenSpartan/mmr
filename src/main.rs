//! Halo Infinite lobby rosters, by match, from Fiddler captures or a local proxy.

mod amqp;
mod bond;
mod cert;
mod clock;
mod feed;
mod gamertags;
mod names;
mod proxy;
mod roster;
mod saz;
mod store;
mod sysproxy;
mod tap;
mod tracker;
mod tui;
mod view;
mod watch;

use feed::Pipeline;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "mmr read <file.saz> [...] [--mask] [--json <file>] [--save] [--store <dir>]
mmr watch [--port <n>] [--mask] [--plain] [--no-save] [--store <dir>] [--game <dir>]
mmr history [<xuid>] [--mask] [--store <dir>] [--game <dir>]
mmr cert [new | remove] [--store <dir>]
mmr help";

#[derive(Debug, PartialEq)]
enum Command {
    Help,
    Read,
    Watch,
    /// Only this XUID's ratings, if one is given.
    History(Option<u64>),
    /// The saved root's state, unless asked to make or remove one.
    Cert(Option<Cert>),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Cert {
    New,
    Remove,
}

/// The options each command takes.
const OPTIONS: [(&str, &[&str]); 4] = [
    ("read", &["--mask", "--json", "--save", "--store"]),
    ("watch", &["--port", "--mask", "--plain", "--no-save", "--store", "--game"]),
    ("history", &["--mask", "--store", "--game"]),
    ("cert", &["--store"]),
];

#[derive(Debug, Default, PartialEq)]
struct Args {
    positional: Vec<String>,
    mask: bool,
    save: bool,
    no_save: bool,
    plain: bool,
    port: Option<u16>,
    json: Option<PathBuf>,
    store: Option<PathBuf>,
    game: Option<PathBuf>,
}

impl Args {
    fn store(&self) -> PathBuf {
        self.store.clone().unwrap_or_else(store::default_dir)
    }
}

/// The command and its arguments, or what's wrong with them. Each command takes only
/// its own options, and only as many other arguments as it uses.
fn parse(raw: &[String]) -> Result<(Command, Args), String> {
    if raw.first().is_some_and(|x| x == "help") || raw.iter().any(|x| x == "--help" || x == "-h") {
        return Ok((Command::Help, Args::default()));
    }
    let Some((name, rest)) = raw.split_first() else { return Err("no command given".into()) };
    let takes = OPTIONS.iter().find(|(n, _)| *n == name.as_str()).ok_or_else(|| format!("unknown command {name}"))?.1;
    let mut a = Args::default();
    let mut it = rest.iter();
    while let Some(x) = it.next() {
        if !x.starts_with("--") {
            a.positional.push(x.clone());
            continue;
        }
        if !takes.contains(&x.as_str()) && OPTIONS.iter().any(|(_, o)| o.contains(&x.as_str())) {
            return Err(format!("{name} doesn't take {x}"));
        }
        let mut value = || it.next().ok_or(format!("{x} needs a value"));
        match x.as_str() {
            "--mask" => a.mask = true,
            "--save" => a.save = true,
            "--no-save" => a.no_save = true,
            "--plain" => a.plain = true,
            "--port" => {
                a.port =
                    Some(value()?.parse().ok().filter(|p| *p != 0).ok_or("--port needs a number from 1 to 65535")?);
            }
            "--json" => a.json = Some(value()?.into()),
            "--store" => a.store = Some(value()?.into()),
            "--game" => a.game = Some(value()?.into()),
            _ => return Err(format!("unknown option {x}")),
        }
    }
    let command = match (name.as_str(), a.positional.as_slice()) {
        ("read", []) => return Err("no capture given".into()),
        ("read", _) => Command::Read,
        ("watch", []) => Command::Watch,
        ("history", []) => Command::History(None),
        ("history", [x]) => Command::History(Some(x.parse().map_err(|_| "the XUID must be a number")?)),
        ("cert", []) => Command::Cert(None),
        ("cert", [x]) if x == "new" => Command::Cert(Some(Cert::New)),
        ("cert", [x]) if x == "remove" => Command::Cert(Some(Cert::Remove)),
        ("cert", [x]) => return Err(format!("unknown cert command {x}")),
        ("watch", [x, ..]) | (_, [_, x, ..]) => return Err(format!("unexpected argument {x}")),
        _ => return Err(format!("unknown command {name}")),
    };
    Ok((command, a))
}

fn read(a: &Args) -> Result<(), String> {
    let mut all = Vec::new();
    for path in &a.positional {
        let mut p = Pipeline::default();
        for it in saz::read(path.as_ref())? {
            p.item(&it);
        }
        let name = Path::new(path).file_name().unwrap_or_default().to_string_lossy();
        let found: Vec<_> = p.tracker.order.iter().map(|k| &p.tracker.matches[k]).filter(|m| m.has_roster()).collect();
        println!(
            "\n{} rosters {}, Xbox documents {}, matches {}",
            if a.mask { "Capture:" } else { &name },
            p.rosters,
            p.documents,
            found.len()
        );
        report(&p);
        for (n, m) in found.iter().enumerate() {
            print!("{}", view::table(m, n + 1, a.mask));
        }
        all.extend(found.into_iter().cloned());
    }
    if a.save {
        let dir = a.store();
        for m in &all {
            store::save(&dir, m).map_err(|e| e.to_string())?;
        }
        let n = all.len();
        println!("\nSaved {n} {} in {}", if n == 1 { "match" } else { "matches" }, store::matches_dir(&dir).display());
    }
    if let Some(path) = &a.json {
        std::fs::write(path, serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn report(p: &Pipeline) {
    if p.bad_documents > 0 {
        println!("Problem: Xbox documents that aren't JSON ({})", p.bad_documents);
    }
    for (what, n) in &p.decoder.problems {
        println!("Problem: {what} ({n})");
    }
}

fn watch(a: &Args) -> Result<(), String> {
    watch::run(watch::Options {
        store: a.store(),
        port: a.port.unwrap_or(proxy::DEFAULT_PORT),
        mask: a.mask,
        save: !a.no_save,
        plain: a.plain,
        game: a.game.clone(),
    })
}

fn history(a: &Args, only: Option<u64>) -> Result<(), String> {
    let matches = store::load_all(&a.store()).map_err(|e| e.to_string())?;
    matches.iter().for_each(gamertags::learn);
    names::load(a.game.clone());
    print!("{}", view::history(&matches, only, a.mask));
    Ok(())
}

/// `mmr cert`: the saved root and whether it's trusted. `new` makes and installs a
/// root, then removes older ones; `remove` takes every mmr root out of the store and
/// deletes the saved files. Windows asks before each change to the Root store.
fn certificate(a: &Args, action: Option<Cert>) -> Result<(), String> {
    let dir = a.store();
    match action {
        None => cert_status(&dir),
        Some(Cert::New) => {
            let root = cert::Root::make()?;
            println!("Made {} ({}).", root.about.subject, root.about.sha1);
            println!("Adding it to your Root store: Windows will ask you to confirm.");
            cert::store::add(&root.der)?;
            root.save(&dir)?;
            println!("Installed. Expires {}. Saved in {}.", root.about.expires, cert::cert_dir(&dir).display());
            for old in cert::store::ours()?.iter().filter(|d| cert::sha1_hex(d) != root.about.sha1) {
                println!("Removing an older mmr root ({}): Windows will ask you to confirm.", cert::sha1_hex(old));
                if let Err(e) = cert::store::remove(old) {
                    println!("{e}");
                }
            }
            Ok(())
        }
        Some(Cert::Remove) => {
            let ours = cert::store::ours()?;
            for der in &ours {
                println!("Removing {} from your Root store: Windows will ask you to confirm.", cert::sha1_hex(der));
                cert::store::remove(der)?;
            }
            let files = cert::Root::forget(&dir);
            println!(
                "Removed {} from your Root store and {}.",
                view::plural(ours.len(), "mmr root"),
                view::plural(files, "saved file")
            );
            Ok(())
        }
    }
}

fn cert_status(dir: &Path) -> Result<(), String> {
    let installed: Vec<String> = cert::store::ours()?.iter().map(|d| cert::sha1_hex(d)).collect();
    let mut mine = None;
    match cert::Root::load(dir) {
        Ok(Some(root)) => {
            println!("Root:       {}", root.about.subject);
            println!("SHA-1:      {}", root.about.sha1);
            println!("Created:    {}", root.about.created);
            println!("Expires:    {} ({} days left)", root.about.expires, root.days_left());
            println!("Limited to: {} (no IP addresses)", cert::HOSTS.join(", "));
            let key = cert::cert_dir(dir).join("root.key");
            println!("Key:        {} (DPAPI, this user on this machine)", key.display());
            let trusted = installed.contains(&root.about.sha1);
            println!(
                "Trusted:    {}",
                if trusted { "yes, in your Root store" } else { "no, not in your Root store: run `mmr cert new`" }
            );
            if root.days_left() < 8 {
                println!("It expires soon: run `mmr cert new`.");
            }
            mine = Some(root.about.sha1);
        }
        Ok(None) => println!("No root saved in {}: run `mmr cert new`.", cert::cert_dir(dir).display()),
        Err(e) => println!("The saved root can't be used: {e}"),
    }
    let others = installed.iter().filter(|s| Some(*s) != mine.as_ref()).count();
    if others > 0 {
        println!("Other mmr roots in your Root store: {others} (`mmr cert remove` takes them all out).");
    }
    Ok(())
}

fn main() -> ExitCode {
    let mut raw: Vec<String> = std::env::args().skip(1).collect();
    // A double-click starts watch, the same as the Start menu shortcut.
    if raw.is_empty() && own_window() {
        raw.push("watch".into());
    }
    let parsed = parse(&raw);
    if !matches!(parsed, Ok((Command::Watch, _))) {
        recover();
    }
    let result = parsed.map_err(|e| format!("{e}\n{USAGE}")).and_then(|(command, a)| match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Read => read(&a),
        Command::Watch => watch(&a),
        Command::History(only) => history(&a, only),
        Command::Cert(action) => certificate(&a, action),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            // A window opened from the Start menu or Explorer closes on exit, so wait for Enter.
            if matches!(raw.first().map(String::as_str), None | Some("watch")) && own_window() {
                eprintln!("\nPress Enter to close.");
                let _ = std::io::stdin().read_line(&mut String::new());
            }
            ExitCode::FAILURE
        }
    }
}

/// Puts the system proxy back if a crashed `watch` left it pointing at mmr. Every other
/// command runs this, including the uninstaller's `cert remove`. It holds the watch mutex,
/// so it never runs while a watch does, and no watch starts partway through.
fn recover() {
    if let Ok(_running) = sysproxy::hold() {
        if let Some(said) = sysproxy::recover() {
            eprintln!("{said}");
        }
    }
}

/// `s` as a nul-terminated UTF-16 string, for Win32.
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Whether mmr is the only program in its console window, as when it was started
/// from a shortcut rather than a terminal.
fn own_window() -> bool {
    let mut ids = [0u32; 2];
    // SAFETY: the buffer's real length is passed.
    let n = unsafe { windows_sys::Win32::System::Console::GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) };
    n == 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use feed::{Item, Kind};

    fn parsed(args: &str) -> Result<(Command, Args), String> {
        parse(&args.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn help_is_asked_for_in_three_ways() {
        for args in ["help", "--help", "-h", "help watch", "watch --help", "cert new -h", "read a.saz --frob -h"] {
            assert_eq!(parsed(args), Ok((Command::Help, Args::default())), "{args}");
        }
    }

    #[test]
    fn each_command_takes_its_own_arguments() {
        let (c, a) = parsed("watch --port 5000 --mask --plain --no-save --store s --game g").unwrap();
        assert_eq!(c, Command::Watch);
        let want = Args {
            port: Some(5000),
            mask: true,
            plain: true,
            no_save: true,
            store: Some("s".into()),
            game: Some("g".into()),
            ..Args::default()
        };
        assert_eq!(a, want);
        let (c, a) = parsed("read a.saz b.saz --json out.json --save --mask").unwrap();
        assert_eq!(c, Command::Read);
        assert_eq!(a.positional, ["a.saz", "b.saz"]);
        assert_eq!((a.json, a.save, a.mask), (Some("out.json".into()), true, true));
        assert_eq!(parsed("history").unwrap().0, Command::History(None));
        assert_eq!(parsed("history 9100000000000001 --game g").unwrap().0, Command::History(Some(9100000000000001)));
        assert_eq!(parsed("cert").unwrap().0, Command::Cert(None));
        assert_eq!(parsed("cert new --store s").unwrap().0, Command::Cert(Some(Cert::New)));
        assert_eq!(parsed("cert remove").unwrap().0, Command::Cert(Some(Cert::Remove)));
    }

    #[test]
    fn wrong_arguments_say_what_is_wrong() {
        for (args, said) in [
            ("", "no command given"),
            ("frob", "unknown command frob"),
            ("frob --mask", "unknown command frob"),
            ("watch --json out.json", "watch doesn't take --json"),
            ("read a.saz --port 1", "read doesn't take --port"),
            ("cert --mask", "cert doesn't take --mask"),
            ("watch --frob", "unknown option --frob"),
            ("watch extra", "unexpected argument extra"),
            ("history 1 2", "unexpected argument 2"),
            ("cert new x", "unexpected argument x"),
            ("cert frob", "unknown cert command frob"),
            ("history abc", "the XUID must be a number"),
            ("read", "no capture given"),
            ("read --mask", "no capture given"),
            ("watch --port", "--port needs a value"),
            ("watch --port 0", "--port needs a number from 1 to 65535"),
            ("watch --port x", "--port needs a number from 1 to 65535"),
            ("read a.saz --store", "--store needs a value"),
        ] {
            assert_eq!(parsed(args), Err(said.to_string()), "{args}");
        }
    }

    /// Real captures can't live in the repository. Run with
    /// `MMR_TEST_SAZ=<file.saz> cargo test -- --ignored`.
    #[test]
    #[ignore = "needs a capture of your own in MMR_TEST_SAZ"]
    fn splitting_messages_anywhere_changes_nothing() {
        let path = std::env::var("MMR_TEST_SAZ").expect("MMR_TEST_SAZ");
        let items = saz::read(path.as_ref()).unwrap();
        let whole = run(items.iter().cloned());
        assert!(whole.rosters > 0);
        for size in [1usize, 2, 3, 7, 64, 1000] {
            let split = items.iter().flat_map(|it| -> Vec<Item> {
                if it.kind != Kind::WebSocket {
                    return vec![it.clone()];
                }
                it.payload.chunks(size).map(|c| Item { payload: c.to_vec(), ..it.clone() }).collect()
            });
            let p = run(split);
            assert_eq!(p.rosters, whole.rosters, "chunks of {size}");
            assert_eq!(p.tracker.matches, whole.tracker.matches, "chunks of {size}");
            assert!(p.decoder.problems.is_empty(), "{:?}", p.decoder.problems);
        }
    }

    fn run(items: impl Iterator<Item = Item>) -> Pipeline {
        let mut p = Pipeline::default();
        for it in items {
            p.item(&it);
        }
        p
    }
}

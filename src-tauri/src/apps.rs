//! The user's app list and how each app on it is found.
//!
//! The list starts empty. Apps show up by themselves when they write an announce file or are in
//! the catalog; the user can also add any app by name. For each app the hub looks, in order, at
//! the program the user picked, the app's announce file in `<config>/apps/<name>.json` (written
//! by apps that want to be found), then the app's launcher: a `.desktop` file in
//! ~/.local/share/applications on Linux, its uninstall entry under HKCU on Windows (read-only).
//! Paths under a blocked folder or drive (set by the user, none by default) are never used.
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// The hub's own name, used for its announce file.
pub const HUB: &str = "Apphub";

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct AppEntry {
    pub name: String,
    /// The program to open; empty means find it.
    pub program: String,
    /// Names of the apps it works with, as the user typed them.
    pub works_with: Vec<String>,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub apps: Vec<AppEntry>,
    /// Folders or drives the hub never opens or installs from, e.g. "/mnt/share" or "S:".
    pub blocked: Vec<String>,
    /// Folder with one subfolder per app, each with a latest.json; empty means none.
    pub catalog: String,
    /// Found-by-itself apps the user removed, so they stay off the list.
    pub hidden: Vec<String>,
}

/// The saved settings; a missing file is a fresh start. A damaged file is an error, so the
/// caller never saves over (and loses) a list it could not read.
pub fn load(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("{} cannot be read ({e}). Fix or delete it; nothing will be saved until then.", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes through a temporary file so a crash never leaves half a file.
pub fn save(path: &Path, settings: &Settings) -> Result<(), String> {
    write_json(path, &serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?)
}

fn write_json(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// An app name is also its announce file name, so it must be a plain name.
pub fn check_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    let bad = name.is_empty() || name.len() > 64 || name.starts_with('.') || name.chars().any(|c| "/\\:*?\"<>|".contains(c) || c.is_control());
    if bad {
        return Err(format!("\"{name}\" cannot be an app name: use up to 64 letters, digits, spaces or dashes."));
    }
    Ok(name.to_string())
}

fn norm(p: &str) -> String {
    p.trim().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// True when `path` is inside one of the blocked folders or drives ("S:" blocks S:\anything).
pub fn blocked(path: &Path, list: &[String]) -> bool {
    let p = norm(&path.to_string_lossy());
    list.iter().map(|b| norm(b)).filter(|b| !b.is_empty()).any(|b| p == b || p.starts_with(&format!("{b}/")) || (b.ends_with(':') && p.starts_with(&b)))
}

/// Adds the app, or replaces the one with the same name (any case).
pub fn upsert(settings: &mut Settings, name: &str, program: &str, works_with: &[String]) -> Result<(), String> {
    let name = check_name(name)?;
    let program = program.trim().trim_matches('"').to_string();
    if !program.is_empty() {
        let p = Path::new(&program);
        if !p.is_absolute() || !p.is_file() {
            return Err(format!("{program} is not a program file (give its full path)."));
        }
        if blocked(p, &settings.blocked) {
            return Err(format!("{program} is in a blocked folder."));
        }
    }
    let mut with: Vec<String> = vec![];
    for w in works_with.iter().map(|w| w.trim()).filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case(&name)) {
        if !with.iter().any(|x| x.eq_ignore_ascii_case(w)) {
            with.push(w.to_string());
        }
    }
    settings.hidden.retain(|h| !h.eq_ignore_ascii_case(&name));
    let entry = AppEntry { name, program, works_with: with };
    match settings.apps.iter_mut().find(|a| a.name.eq_ignore_ascii_case(&entry.name)) {
        Some(a) => *a = entry,
        None => settings.apps.push(entry),
    }
    Ok(())
}

/// Takes the app off the list and keeps it off, even if it would be found by itself again.
pub fn remove(settings: &mut Settings, name: &str) -> bool {
    let name = name.trim();
    let before = settings.apps.len();
    settings.apps.retain(|a| !a.name.eq_ignore_ascii_case(name));
    let was_hidden = settings.hidden.iter().any(|h| h.eq_ignore_ascii_case(name));
    if !was_hidden && !name.is_empty() {
        settings.hidden.push(name.to_string());
    }
    settings.apps.len() != before || !was_hidden
}

#[derive(Serialize, Deserialize, Default, Debug, PartialEq)]
#[serde(default)]
pub struct Announce {
    pub schema_version: u32,
    pub app: String,
    pub version: String,
    pub exe: String,
    pub seen_at: String,
    /// Optional: names of the apps it works with.
    pub works_with: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum State {
    /// The program the user picked: path.
    Chosen(PathBuf),
    /// Its announce file names a program that exists: version, when it last started, program.
    Announced(String, String, PathBuf),
    /// Found by its launcher or uninstall entry: version and (when known) program.
    Installed(String, Option<PathBuf>),
    NotInstalled,
    /// Its announce file is damaged and nothing else finds it.
    Unreadable(String),
}

impl State {
    pub fn program(&self) -> Option<&Path> {
        match self {
            State::Chosen(p) | State::Announced(_, _, p) => Some(p),
            State::Installed(_, p) => p.as_deref(),
            _ => None,
        }
    }
}

pub fn announce_file(folder: &Path, name: &str) -> PathBuf {
    folder.join(format!("{name}.json"))
}

/// UTC time as 2026-11-16T12:34:56Z.
fn iso(time: SystemTime) -> String {
    let s = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest / 60 % 60, rest % 60)
}

/// Writes (only) the hub's own announce file, so apps can see the hub is installed.
pub fn announce_self(folder: &Path, exe: &Path, now: SystemTime) -> Result<(), String> {
    let a = Announce { schema_version: 1, app: HUB.into(), version: env!("CARGO_PKG_VERSION").into(), exe: exe.display().to_string(), seen_at: iso(now), works_with: vec![] };
    write_json(&announce_file(folder, HUB), &serde_json::to_string_pretty(&a).map_err(|e| e.to_string())?)
}

/// One app's state from its announce file, then `registered` (version and program from its
/// launcher). A program that no longer exists means not installed.
pub fn look(folder: Option<&Path>, name: &str, registered: Option<(String, Option<PathBuf>)>, usable: impl Fn(&Path) -> bool) -> State {
    let mut damaged = None;
    if let Some(text) = folder.and_then(|f| std::fs::read_to_string(announce_file(f, name)).ok()) {
        match serde_json::from_str::<Announce>(text.trim_start_matches('\u{feff}')) {
            Ok(a) if !a.exe.is_empty() && usable(Path::new(&a.exe)) => return State::Announced(a.version, a.seen_at, PathBuf::from(a.exe)),
            Ok(_) => {}
            Err(e) => damaged = Some(format!("its announce file cannot be read ({e})")),
        }
    }
    match registered {
        Some((version, exe)) if exe.as_deref().is_none_or(&usable) => State::Installed(version, exe),
        _ => damaged.map_or(State::NotInstalled, State::Unreadable),
    }
}

/// The program in an uninstall entry's DisplayIcon: `"C:\x\app.exe",0` -> C:\x\app.exe.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn icon_exe(icon: &str) -> Option<PathBuf> {
    let icon = icon.trim();
    let path = match icon.strip_prefix('"') {
        Some(rest) => &rest[..rest.find('"')?],
        None => icon.rsplit_once(',').filter(|(_, n)| n.trim().parse::<i32>().is_ok()).map_or(icon, |(p, _)| p),
    };
    path.to_ascii_lowercase().ends_with(".exe").then(|| PathBuf::from(path))
}

/// An installed copy found by its launcher (Linux) or uninstall entry (Windows).
pub struct Entry {
    pub name: String,
    pub version: String,
    pub exe: Option<PathBuf>,
}

/// The launcher for `name`: its exact name first, else one that starts with the name and a
/// space ("Notes 2.1" for "Notes"), never a longer word ("Notebook").
pub fn launcher<'a>(entries: &'a [Entry], name: &str) -> Option<&'a Entry> {
    let n = name.trim().to_lowercase();
    entries.iter().find(|e| e.name.trim().to_lowercase() == n).or_else(|| entries.iter().find(|e| e.name.trim().to_lowercase().starts_with(&format!("{n} "))))
}

/// One .desktop launcher: Name=, the version from Comment= (its first word starting with a digit)
/// or X-AppImage-Version=, and the program in Exec=.
#[cfg_attr(windows, allow(dead_code))]
pub fn desktop_entry(text: &str) -> Option<Entry> {
    let mut lines = text.lines().map(str::trim).skip_while(|l| *l != "[Desktop Entry]").skip(1);
    let (mut name, mut comment, mut version, mut exec) = (None, "", None, "");
    for line in lines.by_ref().take_while(|l| !l.starts_with('[')) {
        match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
            Some(("Name", v)) => name = Some(v.to_string()),
            Some(("Comment", v)) => comment = v,
            Some(("X-AppImage-Version", v)) => version = Some(v.to_string()),
            Some(("Exec", v)) => exec = v,
            Some(("Hidden", "true")) => return None,
            _ => {}
        }
    }
    let version = comment.split_whitespace().find(|w| w.starts_with(|c: char| c.is_ascii_digit())).map(str::to_string).or(version).unwrap_or_default();
    let program = match exec.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or_default(),
        None => exec.split_whitespace().next().unwrap_or_default(),
    };
    // shortcut: only absolute Exec paths can be opened; a bare command shows as installed only. Pick the program by hand for those.
    let exe = Some(PathBuf::from(program)).filter(|p| p.is_absolute());
    Some(Entry { name: name?, version, exe })
}

fn read_announce(folder: &Path, name: &str) -> Option<Announce> {
    let text = std::fs::read_to_string(announce_file(folder, name)).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// The list as shown: the saved apps, then every app with an announce file, then every app in
/// `catalog` (names from the catalog), minus the hidden ones and the hub itself. An app with no
/// "works with" set by the user takes the one from its announce file. The bool is "found by itself".
pub fn list(settings: &Settings, folder: Option<&Path>, catalog: &[String]) -> Vec<(AppEntry, bool)> {
    let mut found: Vec<String> = folder
        .and_then(|f| std::fs::read_dir(f).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json").map(str::to_string))
        .collect();
    found.sort();
    found.extend(catalog.iter().cloned());
    let mut apps: Vec<(AppEntry, bool)> = settings.apps.iter().map(|a| (a.clone(), false)).collect();
    for name in found {
        let skip = check_name(&name).is_err()
            || name.eq_ignore_ascii_case(HUB)
            || settings.hidden.iter().any(|h| h.eq_ignore_ascii_case(&name))
            || apps.iter().any(|(a, _)| a.name.eq_ignore_ascii_case(&name));
        if !skip {
            apps.push((AppEntry { name, ..AppEntry::default() }, true));
        }
    }
    for (app, _) in apps.iter_mut().filter(|(a, _)| a.works_with.is_empty()) {
        if let Some(a) = folder.and_then(|f| read_announce(f, &app.name)) {
            app.works_with = a.works_with.into_iter().map(|w| w.trim().to_string()).filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case(&app.name)).collect();
        }
    }
    apps
}

/// Every app on the list and its state, read now (nothing is cached).
pub fn find(settings: &Settings, folder: Option<&Path>, catalog: &[String]) -> Vec<(AppEntry, bool, State)> {
    let entries = native::entries();
    let usable = |p: &Path| !blocked(p, &settings.blocked) && p.exists();
    list(settings, folder, catalog)
        .into_iter()
        .map(|(app, auto)| {
            let chosen = Some(PathBuf::from(&app.program)).filter(|p| !app.program.is_empty() && usable(p));
            let state = match chosen {
                Some(p) => State::Chosen(p),
                None => {
                    let registered = launcher(&entries, &app.name).map(|e| (e.version.clone(), e.exe.clone()));
                    look(folder, &app.name, registered, usable)
                }
            };
            (app, auto, state)
        })
        .collect()
}

#[cfg(not(windows))]
mod native {
    /// Linux: the user's own launchers in $XDG_DATA_HOME/applications (~/.local/share/applications).
    pub fn entries() -> Vec<super::Entry> {
        let dir = std::env::var_os("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".local/share")))
            .map(|d| d.join("applications"));
        let Some(Ok(files)) = dir.map(std::fs::read_dir) else { return vec![] };
        let mut paths: Vec<_> = files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        paths.sort();
        paths.iter().filter_map(|p| std::fs::read_to_string(p).ok()).filter_map(|t| super::desktop_entry(&t)).collect()
    }
}

#[cfg(windows)]
mod native {
    use super::Entry;
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Registry::*;

    const UNINSTALL: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    unsafe fn value(key: &str, name: &str) -> Option<String> {
        let (key, name) = (wide(key), wide(name));
        let mut text = vec![0u16; 2048];
        let mut bytes = (text.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, null_mut(), text.as_mut_ptr() as _, &mut bytes) != 0 {
            return None;
        }
        let end = text.iter().position(|c| *c == 0).unwrap_or(text.len());
        String::from_utf16(&text[..end]).ok()
    }
    /// Read-only: every HKCU uninstall entry that has a DisplayName.
    pub fn entries() -> Vec<Entry> {
        unsafe {
            let mut key: HKEY = null_mut();
            if RegOpenKeyExW(HKEY_CURRENT_USER, wide(UNINSTALL).as_ptr(), 0, KEY_READ, &mut key) != 0 {
                return vec![];
            }
            let mut found = vec![];
            for index in 0.. {
                let mut name = [0u16; 256];
                let mut length = name.len() as u32;
                if RegEnumKeyExW(key, index, name.as_mut_ptr(), &mut length, null_mut(), null_mut(), null_mut(), null_mut()) != 0 {
                    break;
                }
                let sub = format!("{UNINSTALL}\\{}", String::from_utf16_lossy(&name[..length as usize]));
                if let Some(display) = value(&sub, "DisplayName") {
                    found.push(Entry {
                        name: display,
                        version: value(&sub, "DisplayVersion").unwrap_or_default(),
                        exe: value(&sub, "DisplayIcon").as_deref().and_then(super::icon_exe),
                    });
                }
            }
            RegCloseKey(key);
            found
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apphub-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn fresh_start_is_empty_and_damage_is_not_overwritten() {
        let dir = temp("settings");
        let path = dir.join("settings.json");
        assert_eq!(load(&path).unwrap(), Settings::default());
        let mut s = Settings::default();
        upsert(&mut s, " Notes ", "", &["Paint".into(), "notes".into(), "paint".into(), " ".into()]).unwrap();
        save(&path, &s).unwrap();
        let back = load(&path).unwrap();
        assert_eq!(back.apps, [AppEntry { name: "Notes".into(), program: String::new(), works_with: vec!["Paint".into()] }]);
        std::fs::write(&path, "{oops").unwrap();
        assert!(load(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_and_remove() {
        let dir = temp("upsert");
        let program = dir.join("notes");
        std::fs::write(&program, "").unwrap();
        let mut s = Settings { blocked: vec![dir.display().to_string()], ..Settings::default() };
        assert!(upsert(&mut s, "Notes", &program.display().to_string(), &[]).is_err(), "blocked folder");
        s.blocked.clear();
        upsert(&mut s, "Notes", &program.display().to_string(), &[]).unwrap();
        upsert(&mut s, "notes", "", &["Paint".into()]).unwrap();
        assert_eq!(s.apps.len(), 1);
        assert_eq!(s.apps[0].works_with, ["Paint"]);
        for bad in ["", "../x", "a/b", "C:", ".hidden", &"x".repeat(65)] {
            assert!(upsert(&mut s, bad, "", &[]).is_err(), "{bad}");
        }
        assert!(upsert(&mut s, "Paint", "relative/path", &[]).is_err());
        assert!(remove(&mut s, " NOTES"));
        assert!(!remove(&mut s, "Notes"));
        assert_eq!(s.hidden, ["NOTES"]);
        upsert(&mut s, "notes", "", &[]).unwrap();
        assert!(s.hidden.is_empty(), "adding it again un-hides it");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocked_folders_and_drives() {
        let list = vec!["S:".to_string(), "/mnt/share/".to_string(), " ".to_string()];
        assert!(blocked(Path::new("S:\\apps\\x.exe"), &list));
        assert!(blocked(Path::new("s:/x"), &list));
        assert!(blocked(Path::new("/mnt/share"), &list));
        assert!(blocked(Path::new("/mnt/share/a/b"), &list));
        assert!(!blocked(Path::new("/mnt/shared/a"), &list));
        assert!(!blocked(Path::new("C:\\Users\\a\\x.exe"), &list));
        assert!(!blocked(Path::new("/home/a"), &[]));
    }

    #[test]
    fn linux_launchers() {
        let e = desktop_entry("[Desktop Entry]\nType=Application\nName=Notes\nName[de]=X\nComment=Notes 2.1.0 (abc123)\nExec=\"/opt/notes/Notes.AppImage\" %F\n[Desktop Action new]\nName=New\n").unwrap();
        assert_eq!((e.name.as_str(), e.version.as_str()), ("Notes", "2.1.0"));
        assert_eq!(e.exe, Some(PathBuf::from("/opt/notes/Notes.AppImage")));
        let bare = desktop_entry("[Desktop Entry]\nName=Paint 3\nX-AppImage-Version=1.4.1\nExec=paint %U\n").unwrap();
        assert_eq!((bare.version.as_str(), bare.exe.clone()), ("1.4.1", None));
        assert!(desktop_entry("[Desktop Entry]\nName=Old\nHidden=true\n").is_none());
        assert!(desktop_entry("Name=No group\n").is_none());
        let all = [bare, e, Entry { name: "Notebook".into(), version: String::new(), exe: None }];
        assert_eq!(launcher(&all, "notes").unwrap().name, "Notes");
        assert_eq!(launcher(&all, "Paint").unwrap().name, "Paint 3");
        assert!(launcher(&all, "Note").is_none());
    }

    #[test]
    fn iso_time() {
        assert_eq!(iso(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        assert_eq!(iso(UNIX_EPOCH + std::time::Duration::from_secs(1_795_000_000)), "2026-11-18T11:06:40Z");
    }

    #[test]
    fn look_prefers_the_announce_file_then_the_launcher() {
        let dir = temp("look");
        let yes = |_: &Path| true;
        let no = |_: &Path| false;
        let write = |name: &str, json: &str| std::fs::write(dir.join(name), json).unwrap();
        assert_eq!(look(Some(&dir), "Notes", None, yes), State::NotInstalled);
        // with a BOM, as Windows PowerShell writes it
        write("Notes.json", "\u{feff}{\"version\":\"2.0\",\"seen_at\":\"2026-11-18T10:00:00Z\",\"exe\":\"/opt/notes/notes\"}");
        assert_eq!(look(Some(&dir), "Notes", None, yes), State::Announced("2.0".into(), "2026-11-18T10:00:00Z".into(), PathBuf::from("/opt/notes/notes")));
        // its program is gone (or blocked): fall back to the launcher, else not installed
        let reg = Some(("1.9".to_string(), None));
        assert_eq!(look(Some(&dir), "Notes", reg.clone(), no), State::Installed("1.9".into(), None));
        assert_eq!(look(Some(&dir), "Notes", None, no), State::NotInstalled);
        write("Paint.json", "{not json");
        assert!(matches!(look(Some(&dir), "Paint", None, yes), State::Unreadable(_)));
        assert_eq!(look(Some(&dir), "Paint", reg, yes), State::Installed("1.9".into(), None));
        assert_eq!(look(None, "Paint", None, yes), State::NotInstalled);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_uses_the_chosen_program_unless_blocked() {
        let dir = temp("find");
        let program = dir.join("notes");
        std::fs::write(&program, "").unwrap();
        let mut s = Settings::default();
        upsert(&mut s, "Notes", &program.display().to_string(), &[]).unwrap();
        assert_eq!(find(&s, Some(&dir), &[])[0].2, State::Chosen(program.clone()));
        s.blocked = vec![dir.display().to_string()];
        assert_ne!(find(&s, Some(&dir), &[])[0].2, State::Chosen(program));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn announce_writes_only_the_hubs_own_file() {
        let dir = temp("announce");
        announce_self(&dir, Path::new("/opt/apphub/apphub"), UNIX_EPOCH).unwrap();
        let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names, ["Apphub.json"]);
        let back: Announce = serde_json::from_str(&std::fs::read_to_string(dir.join("Apphub.json")).unwrap()).unwrap();
        assert_eq!((back.app.as_str(), back.schema_version), ("Apphub", 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apps_show_up_by_themselves() {
        let dir = temp("list");
        announce_self(&dir, Path::new("/opt/apphub/apphub"), UNIX_EPOCH).unwrap();
        std::fs::write(dir.join("Notes.json"), r#"{"app":"Notes","exe":"/x","works_with":["Paint"," ","Notes"]}"#).unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap();
        let mut s = Settings::default();
        let names = |s: &Settings| list(s, Some(&dir), &["paint".into(), "NOTES".into(), "../bad".into()]).into_iter().map(|(a, auto)| (a.name, a.works_with, auto)).collect::<Vec<_>>();
        assert_eq!(names(&s), [("Notes".into(), vec!["Paint".to_string()], true), ("paint".into(), vec![], true)]);
        // a saved app keeps its own works-with; a removed one stays off
        upsert(&mut s, "Notes", "", &["Calendar".into()]).unwrap();
        remove(&mut s, "Paint");
        assert_eq!(names(&s), [("Notes".into(), vec!["Calendar".to_string()], false)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninstall_entry_helpers() {
        assert_eq!(icon_exe("\"C:\\x y\\notes.exe\",0"), Some(PathBuf::from("C:\\x y\\notes.exe")));
        assert_eq!(icon_exe("C:\\x\\a.exe,0"), Some(PathBuf::from("C:\\x\\a.exe")));
        assert_eq!(icon_exe("C:\\x\\a.ico"), None);
    }
}

//! Install and update from a catalog folder the user picks (none by default). Each app has a
//! subfolder with its setup and a latest.json naming the setup and its SHA256. Nothing runs
//! without a click, a confirm, and a SHA256 match; the setup opens with its own normal window.
use crate::apps::blocked;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::path::{Path, PathBuf};

/// One app's latest.json, e.g.
/// {"app":"Notes","version":"2.1.0","setup":"Notes_2.1.0_x64-setup.exe","sha256":"<64 hex>","released":"2026-11-22","notes":"…"}
#[derive(Deserialize, Default, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Latest {
    /// Matched against the app's name on the list (any case).
    pub app: String,
    pub version: String,
    /// A plain file name next to latest.json.
    pub setup: String,
    pub sha256: String,
    pub released: String,
    pub notes: String,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Offer {
    pub version: String,
    pub released: String,
    pub notes: String,
    /// "install" (not installed), "update" (newer), "older", "same"
    pub kind: &'static str,
    /// Why it cannot be installed; empty when it can.
    pub problem: String,
}

/// Checks a catalog folder before it is saved: an existing, absolute, unblocked folder.
pub fn check_folder(folder: &str, blocked_list: &[String]) -> Result<PathBuf, String> {
    let folder = PathBuf::from(folder.trim().trim_matches('"'));
    if blocked(&folder, blocked_list) {
        return Err(format!("{} is in a blocked folder.", folder.display()));
    }
    if !folder.is_absolute() || !folder.is_dir() {
        return Err(format!("{} is not a folder.", folder.display()));
    }
    Ok(folder)
}

/// Every `<folder>/<sub>/latest.json`, read now. Unreadable ones come back as notes for the user.
pub fn scan(folder: &Path) -> (Vec<(PathBuf, Latest)>, Vec<String>) {
    let (mut found, mut notes) = (vec![], vec![]);
    let Ok(subs) = std::fs::read_dir(folder) else {
        return (found, vec![format!("No catalog folder at {}.", folder.display())]);
    };
    let mut dirs: Vec<PathBuf> = subs.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs {
        let file = dir.join("latest.json");
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        match serde_json::from_str::<Latest>(text.trim_start_matches('\u{feff}')) {
            Ok(l) => found.push((dir, l)),
            Err(e) => notes.push(format!("{} cannot be read ({e}).", file.display())),
        }
    }
    (found, notes)
}

pub fn pick<'a>(name: &str, entries: &'a [(PathBuf, Latest)]) -> Option<&'a (PathBuf, Latest)> {
    entries.iter().find(|(_, l)| l.app.trim().eq_ignore_ascii_case(name.trim()))
}

/// The setup named in latest.json: a plain file name in the same folder, which must exist
/// (on Windows an .exe, the only kind that can be started directly).
pub fn setup_path(dir: &Path, latest: &Latest, blocked_list: &[String]) -> Result<PathBuf, String> {
    let name = latest.setup.trim();
    let plain = !name.is_empty() && !name.contains(['/', '\\', ':']) && name != ".." && name != ".";
    if !plain || (cfg!(windows) && !name.to_ascii_lowercase().ends_with(".exe")) {
        return Err(format!("latest.json names no usable setup (\"{name}\")."));
    }
    if latest.sha256.len() != 64 || !latest.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("latest.json has no SHA256 for the setup.".into());
    }
    let path = dir.join(name);
    if blocked(&path, blocked_list) {
        return Err(format!("{} is in a blocked folder.", path.display()));
    }
    if !path.is_file() {
        return Err(format!("The setup {name} is not in {}.", dir.display()));
    }
    Ok(path)
}

/// 0.5.0-beta.6 style versions: the x.y.z numbers first, then a plain release beats any
/// pre-release of it, then the pre-release numbers (beta.5 < beta.6).
pub fn compare(a: &str, b: &str) -> Ordering {
    fn key(v: &str) -> (Vec<u64>, bool, Vec<u64>) {
        let v = v.trim().trim_start_matches(['v', 'V']);
        let (core, pre) = v.split_once(['-', '+']).map_or((v, None), |(c, p)| (c, Some(p)));
        let core = core.split('.').map(|n| n.chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0)).collect();
        let pre_numbers = pre.unwrap_or("").split(|c: char| !c.is_ascii_digit()).filter(|n| !n.is_empty()).map(|n| n.parse().unwrap_or(u64::MAX)).collect();
        (core, pre.is_none(), pre_numbers)
    }
    key(a).cmp(&key(b))
}

/// What the card shows for this catalog entry, given the installed version (None = not installed).
pub fn offer(installed: Option<&str>, dir: &Path, latest: &Latest, blocked_list: &[String]) -> Offer {
    let kind = match installed.filter(|v| !v.is_empty()) {
        None => "install",
        Some(v) => match compare(&latest.version, v) {
            Ordering::Greater => "update",
            Ordering::Less => "older",
            Ordering::Equal => "same",
        },
    };
    Offer {
        version: latest.version.clone(),
        released: latest.released.clone(),
        notes: latest.notes.clone(),
        kind,
        problem: setup_path(dir, latest, blocked_list).err().unwrap_or_default(),
    }
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    std::io::copy(&mut std::fs::File::open(path)?, &mut hasher)?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Copies the setup off the catalog (so a pulled drive or a swapped file cannot change what runs),
/// then checks the copy's SHA256. Returns the checked copy; a mismatch deletes it.
pub fn fetch(setup: &Path, sha256: &str, into: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(into).map_err(|e| format!("{}: {e}", into.display()))?;
    let copy = into.join(setup.file_name().ok_or("no setup file name")?);
    std::fs::copy(setup, &copy).map_err(|e| format!("{}: {e}", setup.display()))?;
    let got = sha256_file(&copy).map_err(|e| format!("{}: {e}", copy.display()))?;
    if !got.eq_ignore_ascii_case(sha256.trim()) {
        let _ = std::fs::remove_file(&copy);
        return Err(format!(
            "This setup does not match its latest.json, so nothing was run. ({} is {got}, latest.json says {})",
            setup.display(),
            sha256.trim()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755)).map_err(|e| format!("{}: {e}", copy.display()))?;
    }
    Ok(copy)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apphub-catalog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn latest(app: &str, version: &str, setup: &str) -> Latest {
        Latest { app: app.into(), version: version.into(), setup: setup.into(), sha256: "a".repeat(64), ..Latest::default() }
    }

    const SETUP: &str = if cfg!(windows) { "setup.exe" } else { "setup.AppImage" };

    #[test]
    fn versions_order() {
        assert_eq!(compare("0.5.0-beta.6", "0.5.0-beta.5"), Ordering::Greater);
        assert_eq!(compare("0.5.0", "0.5.0-beta.9"), Ordering::Greater);
        assert_eq!(compare("0.4.12", "0.5.0-beta.1"), Ordering::Less);
        assert_eq!(compare("v1.4.1", "1.4.1"), Ordering::Equal);
        assert_eq!(compare("1.10.0", "1.9.9"), Ordering::Greater);
    }

    #[test]
    fn pick_matches_the_app_name() {
        let entries = [(PathBuf::from("a"), latest("Paint", "1", SETUP)), (PathBuf::from("b"), latest("Notes", "2", SETUP))];
        assert_eq!(pick(" notes", &entries).unwrap().0, PathBuf::from("b"));
        assert!(pick("Note", &entries).is_none());
    }

    #[test]
    fn offer_kinds_and_setup_checks() {
        let dir = temp("offer");
        std::fs::write(dir.join(SETUP), "x").unwrap();
        let l = latest("Notes", "1.4.2", SETUP);
        assert_eq!(offer(None, &dir, &l, &[]).kind, "install");
        assert_eq!(offer(Some("1.4.1"), &dir, &l, &[]).kind, "update");
        assert_eq!(offer(Some("1.4.2"), &dir, &l, &[]).kind, "same");
        assert_eq!(offer(Some("2.0"), &dir, &l, &[]).kind, "older");
        assert_eq!(offer(None, &dir, &l, &[]).problem, "");
        for bad in [latest("Notes", "1", "../x.exe"), latest("Notes", "1", "gone.exe"), Latest { sha256: String::new(), ..l.clone() }] {
            assert!(setup_path(&dir, &bad, &[]).is_err());
        }
        assert!(setup_path(&dir, &l, &[dir.display().to_string()]).is_err(), "blocked folder");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fetch_runs_nothing_on_a_mismatch() {
        let dir = temp("fetch");
        let setup = dir.join(SETUP);
        std::fs::write(&setup, "hello").unwrap();
        let good = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let into = dir.join("copies");
        assert!(fetch(&setup, &"0".repeat(64), &into).is_err());
        assert!(!into.join(SETUP).exists(), "a mismatched copy is deleted");
        assert_eq!(fetch(&setup, good, &into).unwrap(), into.join(SETUP));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scan_and_folder_checks() {
        let dir = temp("scan");
        std::fs::create_dir_all(dir.join("Notes")).unwrap();
        std::fs::write(dir.join("Notes").join("latest.json"), "\u{feff}{\"app\":\"Notes\",\"version\":\"2.0\"}").unwrap();
        std::fs::create_dir_all(dir.join("Paint")).unwrap();
        std::fs::write(dir.join("Paint").join("latest.json"), "{oops").unwrap();
        std::fs::create_dir_all(dir.join("Empty")).unwrap();
        let (found, notes) = scan(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.version, "2.0");
        assert_eq!(notes.len(), 1);
        assert!(!scan(&dir.join("missing")).1.is_empty());
        assert_eq!(check_folder(&dir.display().to_string(), &[]).unwrap(), dir);
        assert!(check_folder(&dir.display().to_string(), &[dir.display().to_string()]).is_err());
        assert!(check_folder("relative", &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

mod apps;
mod catalog;

use apps::{Settings, State};
use serde::Serialize;
use std::path::PathBuf;
use tauri::Manager;

#[derive(Serialize)]
struct Row {
    name: String,
    program: String,
    works_with: Vec<String>,
    /// "chosen" (program picked by the user), "announced" (announce file), "installed" (launcher), "missing", "damaged"
    status: &'static str,
    /// Found by itself (announce file or catalog), not added by the user.
    auto: bool,
    version: String,
    seen_at: String,
    note: String,
    can_open: bool,
    /// What the catalog has for this app; null when it has nothing.
    offer: Option<catalog::Offer>,
}

/// The hub's own folder: settings.json and apps/ (announce files) live here.
fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| format!("No settings folder: {e}"))
}

fn settings(app: &tauri::AppHandle) -> Result<(PathBuf, Settings), String> {
    let path = config_dir(app)?.join("settings.json");
    apps::load(&path).map(|s| (path, s))
}

/// Loads, changes and saves the settings. A damaged file stops here, so it is never overwritten.
fn change(app: &tauri::AppHandle, f: impl FnOnce(&mut Settings) -> Result<(), String>) -> Result<(), String> {
    let (path, mut s) = settings(app)?;
    f(&mut s)?;
    apps::save(&path, &s)
}

fn names(entries: &[(PathBuf, catalog::Latest)]) -> Vec<String> {
    entries.iter().map(|(_, l)| l.app.trim().to_string()).collect()
}

fn read_catalog(s: &Settings) -> (Vec<(PathBuf, catalog::Latest)>, Vec<String>) {
    match catalog::check_folder(&s.catalog, &s.blocked) {
        Ok(folder) => catalog::scan(&folder),
        Err(e) if !s.catalog.trim().is_empty() => (vec![], vec![e]),
        Err(_) => (vec![], vec![]),
    }
}

#[tauri::command]
fn hub(app: tauri::AppHandle) -> serde_json::Value {
    let (s, error) = match settings(&app) {
        Ok((_, s)) => (s, String::new()),
        Err(e) => (Settings::default(), e),
    };
    let (_, notes) = read_catalog(&s);
    let folder = config_dir(&app).map(|d| d.display().to_string()).unwrap_or_default();
    serde_json::json!({ "version": app.package_info().version.to_string(), "config": folder, "error": error,
        "catalog": s.catalog, "catalog_notes": notes, "blocked": s.blocked })
}

#[tauri::command]
fn list_apps(app: tauri::AppHandle) -> Result<Vec<Row>, String> {
    let (_, s) = settings(&app)?;
    let announce = config_dir(&app)?.join("apps");
    let (entries, _) = read_catalog(&s);
    Ok(apps::find(&s, Some(&announce), &names(&entries))
        .into_iter()
        .map(|(a, auto, state)| {
            let can_open = state.program().is_some();
            let (status, version, seen_at, note) = match state {
                State::Chosen(_) => ("chosen", String::new(), String::new(), String::new()),
                State::Announced(v, seen, _) => ("announced", v, seen, String::new()),
                State::Installed(v, _) => ("installed", v, String::new(), String::new()),
                State::NotInstalled => ("missing", String::new(), String::new(), String::new()),
                State::Unreadable(why) => ("damaged", String::new(), String::new(), why),
            };
            let installed = (status != "missing").then_some(version.as_str());
            let offer = catalog::pick(&a.name, &entries).map(|(dir, l)| catalog::offer(installed, dir, l, &s.blocked));
            Row { name: a.name, program: a.program, works_with: a.works_with, status, auto, version, seen_at, note, can_open, offer }
        })
        .collect())
}

/// Adds an app to the list, or changes the one with that name.
#[tauri::command]
fn save_app(app: tauri::AppHandle, name: String, program: String, works_with: Vec<String>) -> Result<(), String> {
    change(&app, |s| apps::upsert(s, &name, &program, &works_with))
}

/// Takes an app off the list and keeps it off (it stays installed).
#[tauri::command]
fn remove_app(app: tauri::AppHandle, name: String) -> Result<(), String> {
    change(&app, |s| if apps::remove(s, &name) { Ok(()) } else { Err(format!("{name} is not on the list.")) })
}

/// Starts the app as its own program. The program is looked up again here, never taken from the page.
#[tauri::command]
fn open_app(app: tauri::AppHandle, name: String) -> Result<(), String> {
    let (_, s) = settings(&app)?;
    let announce = config_dir(&app)?.join("apps");
    let (entries, _) = read_catalog(&s);
    let exe = apps::find(&s, Some(&announce), &names(&entries))
        .into_iter()
        .find(|(a, _, _)| a.name == name)
        .and_then(|(_, _, state)| state.program().map(PathBuf::from))
        .ok_or(format!("No program found for {name}. Set its program path."))?;
    let mut command = std::process::Command::new(&exe);
    if let Some(dir) = exe.parent() {
        command.current_dir(dir);
    }
    command.spawn().map(drop).map_err(|e| format!("{}: {e}", exe.display()))
}

/// Installs or updates one app from the catalog after the user clicked and confirmed.
/// The setup is looked up again here (never a path from the page), copied off the catalog,
/// SHA256-checked against its latest.json, then started with its normal window (never silent).
#[tauri::command]
fn install_app(app: tauri::AppHandle, name: String) -> Result<String, String> {
    let (_, s) = settings(&app)?;
    let (entries, _) = read_catalog(&s);
    let announce = config_dir(&app)?.join("apps");
    if !apps::list(&s, Some(&announce), &names(&entries)).iter().any(|(a, _)| a.name == name) {
        return Err(format!("{name} is not on the list."));
    }
    let (dir, latest) = catalog::pick(&name, &entries).ok_or(format!("{name} is not in the catalog."))?;
    let setup = catalog::setup_path(dir, latest, &s.blocked)?;
    let copy = catalog::fetch(&setup, &latest.sha256, &std::env::temp_dir().join("Apphub setups"))?;
    let mut command = std::process::Command::new(&copy);
    if let Some(d) = copy.parent() {
        command.current_dir(d);
    }
    match command.spawn() {
        Ok(_) => Ok(format!("{name} {} setup started. Follow its window, then click Refresh.", latest.version)),
        // ERROR_ELEVATION_REQUIRED: the setup asks for admin rights
        Err(e) if e.raw_os_error() == Some(740) => Err(format!("The {name} setup asks for admin rights, so it was not started. Run it yourself as an administrator.")),
        Err(e) => Err(format!("{}: {e}", copy.display())),
    }
}

/// Sets the catalog folder; empty clears it.
#[tauri::command]
fn set_catalog(app: tauri::AppHandle, folder: String) -> Result<(), String> {
    change(&app, |s| {
        s.catalog = if folder.trim().is_empty() { String::new() } else { catalog::check_folder(&folder, &s.blocked)?.display().to_string() };
        Ok(())
    })
}

/// Sets the blocked folders and drives, one per entry.
#[tauri::command]
fn set_blocked(app: tauri::AppHandle, blocked: Vec<String>) -> Result<(), String> {
    change(&app, |s| {
        s.blocked = blocked.iter().map(|b| b.trim().to_string()).filter(|b| !b.is_empty()).collect();
        Ok(())
    })
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Let apps see the hub is installed. Failing to is not a reason to stop.
            // In an AppImage, current_exe() is a temporary mount; $APPIMAGE is the real file.
            let exe = std::env::var_os("APPIMAGE").map(PathBuf::from).map_or_else(std::env::current_exe, Ok);
            if let (Ok(dir), Ok(exe)) = (config_dir(app.handle()), exe) {
                if let Err(e) = apps::announce_self(&dir.join("apps"), &exe, std::time::SystemTime::now()) {
                    eprintln!("announce: {e}");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![hub, list_apps, save_app, remove_app, open_app, install_app, set_catalog, set_blocked])
        .run(tauri::generate_context!())
        .expect("error while running Apphub");
}

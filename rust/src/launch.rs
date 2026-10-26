//! The desktop app's Setup page (game & folders) and the game it starts. TASurf is meant to be the only program a
//! user runs for TAS work: Setup finds Counter-Strike: Source (in Steam's own list of libraries) and SVR (in the
//! usual places), asks only for what it can't find, and keeps the project folder (timelines, videos).
//!
//! Launching starts CS:S through SVR's launcher (so runs can be recorded to video), on the chosen map, in a
//! window of the chosen size (the plugin runs tasurf_session.cfg on every map). The plugin is built and installed first when the copy in
//! the game is older. Metamod:Source (and with it SourceMod and TASurf) is switched on for that session only, by
//! its loader file in addons/ (`metamod_x64.vdf`, kept as `metamod_x64.vdf.tasurf-off` otherwise), and off again
//! when the game closes, so a game started from Steam is stock. SVR adds `-insecure` (no VAC) to the games it
//! starts.
//!
//! Settings live in tasurf.local.json next to cstrike_dir: svr_dir, project_dir and launch {map, width, height,
//! borderless, sourcemod_sessions_only}. Windows only (CS:S and SVR are), but it builds everywhere.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::config::{self, native, repo_root};

const LOADERS: [&str; 2] = ["metamod_x64.vdf", "metamod.vdf"];
const OFF: &str = ".tasurf-off";
const PROFILE: &str = "tasurf.ini"; // our game profile in SVR's data/games
const SVR_RELEASES: &str = "https://github.com/crashfort/SourceDemoRender/releases";

// ---------------------------------------------------------------- settings

fn local_path() -> PathBuf {
    repo_root().join("tasurf.local.json")
}

fn read_local() -> Map<String, Value> {
    std::fs::read_to_string(local_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| if let Value::Object(o) = v { Some(o) } else { None })
        .unwrap_or_default()
}

fn write_local(m: &Map<String, Value>) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&Value::Object(m.clone())).unwrap_or_default() + "\n";
    std::fs::write(local_path(), text).map_err(|e| format!("couldn't save {}: {}", local_path().display(), e))
}

fn get_str(m: &Map<String, Value>, k: &str) -> Option<String> {
    m.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string)
}

fn abs(s: &str) -> PathBuf {
    let p = native(s);
    if p.is_absolute() {
        p
    } else {
        repo_root().join(p)
    }
}

fn slashes(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// What the Setup page shows and launching uses.
#[derive(Clone, Debug)]
pub struct Settings {
    pub cstrike_dir: Option<PathBuf>,
    pub svr_dir: Option<PathBuf>,
    pub project_dir: PathBuf,
    pub map: String,
    pub width: u32,
    pub height: u32,
    pub borderless: bool,
    pub sessions_only: bool, // Metamod only in sessions TASurf starts
}

pub fn settings() -> Settings {
    let m = read_local();
    let l = m.get("launch").and_then(Value::as_object).cloned().unwrap_or_default();
    let num = |k: &str, d: u32| l.get(k).and_then(Value::as_u64).map(|v| v as u32).filter(|v| *v >= 320).unwrap_or(d);
    let env_game = std::env::var("TASURF_CSTRIKE").ok().filter(|v| !v.is_empty()); // as config::load
    Settings {
        cstrike_dir: env_game.or_else(|| get_str(&m, "cstrike_dir")).map(|s| abs(&s)),
        svr_dir: get_str(&m, "svr_dir").map(|s| abs(&s)),
        project_dir: get_str(&m, "project_dir").map(|s| abs(&s)).unwrap_or_else(default_project_dir),
        map: get_str(&l, "map").unwrap_or_default(),
        width: num("width", 1920),
        height: num("height", 1080),
        borderless: l.get("borderless").and_then(Value::as_bool).unwrap_or(true),
        sessions_only: l.get("sourcemod_sessions_only").and_then(Value::as_bool).unwrap_or(true),
    }
}

/// Where the project is until Setup picks another: an installed copy (the program in its TASurf folder, as the
/// installer puts it) keeps timelines and videos in Documents\TASurf, since its own folder is the app's and is
/// replaced by updates; a source checkout (the program under rust/target) in the TASurf folder itself.
fn default_project_dir() -> PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    let documents = reg_value(r"HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Shell Folders", "Personal")
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(|h| PathBuf::from(h).join("Documents")));
    project_default(exe_dir.as_deref(), repo_root(), documents)
}

fn project_default(exe_dir: Option<&Path>, root: &Path, documents: Option<PathBuf>) -> PathBuf {
    match documents {
        Some(d) if exe_dir == Some(root) => d.join("TASurf"),
        _ => root.to_path_buf(),
    }
}

/// The project folder ready to use: its timelines/ made, and in a new one the example timeline to start from.
pub fn ensure_project() {
    let dir = timelines_dir();
    if dir.is_dir() {
        return;
    }
    if std::fs::create_dir_all(&dir).is_ok() {
        let example = repo_root().join("timelines").join("example_surf.json");
        if example.is_file() && example.parent() != Some(dir.as_path()) {
            let _ = std::fs::copy(&example, dir.join("example_surf.json"));
        }
    }
}

/// The project's timelines.
pub fn timelines_dir() -> PathBuf {
    settings().project_dir.join("timelines")
}

/// Where finished videos go.
pub fn videos_dir() -> PathBuf {
    settings().project_dir.join("videos")
}

/// Save what the Setup page sent (only the keys it has; the rest of tasurf.local.json stays).
pub fn save(body: &Value) -> Result<Settings, String> {
    let mut m = read_local();
    let text = |k: &str| body.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    if let Some(v) = text("cstrike_dir") {
        let c = cstrike_of(&abs(v)).ok_or_else(|| format!("{} isn't Counter-Strike: Source's folder (no cstrike folder in it)", v))?;
        m.insert("cstrike_dir".into(), Value::String(slashes(&c)));
    }
    if let Some(v) = text("svr_dir") {
        let d = abs(v);
        if launcher_in(&d).is_none() {
            return Err(format!("{} has no svr_launcher64.exe: pick the folder SVR was extracted to", v));
        }
        m.insert("svr_dir".into(), Value::String(slashes(&d)));
    }
    if let Some(v) = text("project_dir") {
        let d = abs(v);
        std::fs::create_dir_all(d.join("timelines")).map_err(|e| format!("couldn't use {} as the project folder: {}", v, e))?;
        m.insert("project_dir".into(), Value::String(slashes(&d)));
    }
    let mut l = m.get("launch").and_then(Value::as_object).cloned().unwrap_or_default();
    if let Some(v) = text("map") {
        if !v.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
            return Err(format!("bad map name {:?}", v));
        }
        l.insert("map".into(), Value::String(v.to_string()));
    }
    for k in ["width", "height"] {
        if let Some(v) = body.get(k).and_then(Value::as_u64) {
            if !(320..=7680).contains(&v) {
                return Err(format!("the game window's {} must be 320 .. 7680", k));
            }
            l.insert(k.into(), json!(v));
        }
    }
    for (k, key) in [("borderless", "borderless"), ("sessions_only", "sourcemod_sessions_only")] {
        if let Some(v) = body.get(k).and_then(Value::as_bool) {
            l.insert(key.into(), Value::Bool(v));
        }
    }
    m.insert("launch".into(), Value::Object(l));
    write_local(&m)?;
    Ok(settings())
}

// ---------------------------------------------------------------- finding things

/// A console program run without a console window flashing up (the app is a windowed program).
fn quiet(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn reg_value(key: &str, value: &str) -> Option<String> {
    let out = quiet("reg").args(["query", key, "/v", value]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().find(|l| l.trim_start().starts_with(value))?;
    let at = line.find("REG_SZ")?;
    Some(line[at + "REG_SZ".len()..].trim().to_string()).filter(|s| !s.is_empty())
}

/// Steam's own folder.
pub fn steam_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    reg_value(r"HKCU\Software\Valve\Steam", "SteamPath")
        .or_else(|| reg_value(r"HKLM\SOFTWARE\WOW6432Node\Valve\Steam", "InstallPath"))
        .map(|s| native(&s))
        .filter(|p| p.is_dir())
        .or_else(|| Some(PathBuf::from(r"C:\Program Files (x86)\Steam")).filter(|p| p.is_dir()))
}

/// Steam's libraries (steamapps/libraryfolders.vdf), Steam's own folder first.
fn steam_libraries(steam: &Path) -> Vec<PathBuf> {
    let mut out = vec![steam.to_path_buf()];
    let vdf = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).unwrap_or_default();
    for line in vdf.lines() {
        let parts: Vec<&str> = line.split('"').collect(); // ["\t\t", "path", "\t\t", "D:\\Games", ""]
        if parts.len() >= 5 && parts[1] == "path" {
            let p = native(&parts[3].replace("\\\\", "\\"));
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// The cstrike folder of a folder the user picked: Counter-Strike Source's own, or its cstrike.
pub fn cstrike_of(dir: &Path) -> Option<PathBuf> {
    if dir.file_name().map_or(false, |n| n.eq_ignore_ascii_case("cstrike")) && dir.join("gameinfo.txt").is_file() {
        return Some(dir.to_path_buf());
    }
    Some(dir.join("cstrike")).filter(|c| c.join("gameinfo.txt").is_file())
}

/// Counter-Strike: Source in any of Steam's libraries.
pub fn find_cstrike() -> Option<PathBuf> {
    let steam = steam_dir()?;
    steam_libraries(&steam)
        .into_iter()
        .find_map(|lib| cstrike_of(&lib.join("steamapps").join("common").join("Counter-Strike Source")))
}

/// The game's program: the 64-bit one (what CS:S runs since 2024), else the old 32-bit hl2.exe.
fn game_exe(cstrike: &Path) -> Option<PathBuf> {
    let root = cstrike.parent()?;
    ["cstrike_win64.exe", "hl2.exe"].iter().map(|e| root.join(e)).find(|p| p.is_file())
}

fn is_64(exe: &Path) -> bool {
    exe.file_name().map_or(false, |n| n.to_string_lossy().contains("64"))
}

/// SVR's launcher in a folder (the 64-bit one; the 32-bit one only for an old 32-bit game).
fn launcher_in(dir: &Path) -> Option<PathBuf> {
    ["svr_launcher64.exe", "svr_launcher.exe"].iter().map(|e| dir.join(e)).find(|p| p.is_file())
}

/// SVR in the places people put it: the TASurf folder, the user's Desktop, Downloads and Documents, and the
/// drive roots and Program Files (a folder or two deep).
pub fn find_svr() -> Option<PathBuf> {
    let mut roots = vec![repo_root().join("third_party"), repo_root().to_path_buf()];
    if let Ok(home) = std::env::var("USERPROFILE") {
        for sub in ["Desktop", "Downloads", "Documents", "OneDrive\\Desktop", "OneDrive\\Documents", ""] {
            roots.push(PathBuf::from(&home).join(sub));
        }
    }
    for drive in ["C:\\", "D:\\", "E:\\"] {
        roots.push(PathBuf::from(drive));
        roots.push(PathBuf::from(drive).join("Program Files"));
        roots.push(PathBuf::from(drive).join("Games"));
    }
    let is_svr = |d: &Path| d.join("svr_launcher64.exe").is_file() && d.join("data").is_dir();
    for root in roots {
        if is_svr(&root) {
            return Some(root);
        }
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for e in entries.flatten() {
            let d = e.path();
            if !d.is_dir() {
                continue;
            }
            if is_svr(&d) {
                return Some(d);
            }
            let name = d.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if name.contains("svr") || name.contains("sourcedemorender") || name.contains("demorender") {
                if let Ok(inner) = std::fs::read_dir(&d) {
                    if let Some(found) = inner.flatten().map(|e| e.path()).find(|p| p.is_dir() && is_svr(p)) {
                        return Some(found);
                    }
                }
            }
        }
    }
    None
}

fn running(image: &str) -> bool {
    if !cfg!(windows) {
        return false;
    }
    let filter = format!("IMAGENAME eq {}", image);
    quiet("tasklist")
        .args(["/FI", filter.as_str(), "/NH"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains(&image.to_ascii_lowercase()))
        .unwrap_or(false)
}

pub fn game_running() -> bool {
    running("cstrike_win64.exe") || running("hl2.exe")
}

/// The maps the game has (maps/ and download/maps/), sorted.
pub fn maps(cstrike: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for dir in [cstrike.join("maps"), cstrike.join("download").join("maps")] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("bsp")) {
                    if let Some(s) = p.file_stem() {
                        let s = s.to_string_lossy().to_string();
                        if !out.contains(&s) {
                            out.push(s);
                        }
                    }
                }
            }
        }
    }
    out.sort_by_key(|s| s.to_ascii_lowercase());
    out
}

// ---------------------------------------------------------------- Metamod's loader, the plugin

/// "on", "off" (kept aside by TASurf) or "missing" (Metamod isn't installed).
fn loader_state(cstrike: &Path) -> &'static str {
    let addons = cstrike.join("addons");
    if LOADERS.iter().any(|l| addons.join(l).is_file()) {
        "on"
    } else if LOADERS.iter().any(|l| addons.join(format!("{}{}", l, OFF)).is_file()) {
        "off"
    } else {
        "missing"
    }
}

fn set_loader(cstrike: &Path, on: bool) -> Result<(), String> {
    let addons = cstrike.join("addons");
    for l in LOADERS {
        let (vdf, kept) = (addons.join(l), addons.join(format!("{}{}", l, OFF)));
        let (from, to) = if on { (kept, vdf) } else { (vdf, kept) };
        if from.is_file() {
            if to.is_file() {
                let _ = std::fs::remove_file(&to);
            }
            std::fs::rename(&from, &to).map_err(|e| format!("couldn't rename {}: {}", from.display(), e))?;
        }
    }
    Ok(())
}

/// When the app starts: a session that ended without the app (it was closed first, or crashed) leaves Metamod
/// on; with sourcemod_sessions_only it goes off again, unless the game is running.
pub fn tidy() {
    let s = settings();
    if let Some(c) = s.cstrike_dir.clone().or_else(find_cstrike) {
        if s.sessions_only && loader_state(&c) == "on" && !game_running() {
            let _ = set_loader(&c, false);
        }
    }
}

fn plugin_src_dir() -> PathBuf {
    repo_root().join("plugin").join("scripting")
}

fn plugin_out() -> PathBuf {
    repo_root().join("plugin").join("plugins").join("tasurf.smx")
}

fn modified(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// The newest change to the plugin's sources.
fn sources_changed() -> Option<SystemTime> {
    let mut newest = None;
    let mut dirs = vec![plugin_src_dir()];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().map_or(false, |x| x == "sp" || x == "inc") {
                newest = newest.max(modified(&p));
            }
        }
    }
    newest
}

fn find_spcomp(cstrike: &Path) -> Option<PathBuf> {
    let s = config::load();
    let mut c: Vec<PathBuf> = s.spcomp().into_iter().collect();
    for root in [repo_root().join("third_party").join("sourcemod"), cstrike.to_path_buf()] {
        for exe in ["spcomp64.exe", "spcomp.exe"] {
            c.push(root.join("addons").join("sourcemod").join("scripting").join(exe));
        }
    }
    c.into_iter().find(|p| p.is_file())
}

/// The plugin as the game has it: "up to date", "outdated" (launching installs the new one) or "missing".
fn plugin_state(cstrike: &Path) -> &'static str {
    let installed = cstrike.join("addons").join("sourcemod").join("plugins").join("tasurf.smx");
    if !installed.is_file() {
        return "missing";
    }
    let built = plugin_out();
    let stale = !built.is_file() || sources_changed() > modified(&built);
    let same = std::fs::read(&installed).ok() == std::fs::read(&built).ok();
    if same && !stale {
        "up to date"
    } else {
        "outdated"
    }
}

/// Build the plugin when its sources changed since, and put it and tasurf_session.cfg in the game.
fn install_plugin(cstrike: &Path, log: &dyn Fn(String)) -> Result<(), String> {
    let out = plugin_out();
    if !out.is_file() || sources_changed() > modified(&out) {
        match find_spcomp(cstrike) {
            Some(spcomp) => {
                log("building the plugin".into());
                let include = spcomp.parent().map(|p| p.join("include")).unwrap_or_default();
                let _ = std::fs::create_dir_all(out.parent().unwrap_or(Path::new(".")));
                let o = quiet(&spcomp.display().to_string())
                    .arg(plugin_src_dir().join("tasurf.sp"))
                    .arg(format!("-i{}", include.display()))
                    .arg(format!("-o{}", out.display()))
                    .current_dir(plugin_src_dir())
                    .output()
                    .map_err(|e| format!("couldn't run {}: {}", spcomp.display(), e))?;
                if !o.status.success() || !out.is_file() {
                    let text = String::from_utf8_lossy(&o.stdout).to_string() + &String::from_utf8_lossy(&o.stderr);
                    let errors: Vec<&str> = text.lines().filter(|l| l.contains("error")).take(5).collect();
                    return Err(format!("the plugin didn't build: {}", errors.join(" / ")));
                }
            }
            None if out.is_file() => log("the plugin's sources changed, but spcomp isn't here to build them".into()),
            None => return Err("no built plugin and no spcomp to build it (see README: third_party/sourcemod)".into()),
        }
    }
    let sm = cstrike.join("addons").join("sourcemod");
    let dest = sm.join("plugins").join("tasurf.smx");
    if std::fs::read(&dest).ok() != std::fs::read(&out).ok() {
        std::fs::copy(&out, &dest).map_err(|e| format!("couldn't install the plugin in {}: {}", dest.display(), e))?;
        log("installed the plugin".into());
    }
    let cfg = repo_root().join("plugin").join("cfg").join("tasurf_session.cfg");
    let cfg_dest = cstrike.join("cfg").join("tasurf_session.cfg");
    if std::fs::read(&cfg_dest).ok() != std::fs::read(&cfg).ok() {
        std::fs::copy(&cfg, &cfg_dest).map_err(|e| format!("couldn't install {}: {}", cfg_dest.display(), e))?;
    }
    let data = sm.join("data").join("tasurf");
    for sub in ["baked", "starts", "runs"] {
        let _ = std::fs::create_dir_all(data.join(sub));
    }
    Ok(())
}

// ---------------------------------------------------------------- what the Setup page shows

fn check(id: &str, ok: bool, label: &str, detail: String) -> Value {
    json!({"id": id, "ok": ok, "label": label, "detail": detail})
}

/// FFmpeg's libraries SVR's encoder loads (svr.zip has them next to it; a part-extracted zip, or an antivirus,
/// can leave one out, and then SVR fails in the game): the ones that aren't there.
fn svr_missing_libraries(svr: &Path) -> Vec<String> {
    let names: Vec<String> = std::fs::read_dir(svr)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_ascii_lowercase())
        .collect();
    ["avcodec", "avformat", "avutil", "swresample"]
        .iter()
        .filter(|lib| !names.iter().any(|n| n.starts_with(&format!("{}-", lib)) && n.ends_with(".dll")))
        .map(|lib| format!("{}-*.dll", lib))
        .collect()
}

/// What the searches found, once per run of the app (the Setup page asks again and again while a launch is
/// under way, and a Downloads folder can be big).
fn found() -> (Option<PathBuf>, Option<PathBuf>) {
    static FOUND: std::sync::OnceLock<(Option<PathBuf>, Option<PathBuf>)> = std::sync::OnceLock::new();
    FOUND.get_or_init(|| (find_cstrike(), find_svr())).clone()
}

/// SVR's folder: the one picked, else the one found.
pub fn svr_folder() -> Option<PathBuf> {
    settings().svr_dir.filter(|d| launcher_in(d).is_some()).or_else(|| found().1)
}

/// Settings, what was found, the checks, the maps and how a launch is going.
pub fn status() -> Value {
    let s = settings();
    let (found_game, found_svr) = found();
    let cstrike = s.cstrike_dir.clone().filter(|c| c.join("gameinfo.txt").is_file()).or_else(|| found_game.clone());
    let svr = s.svr_dir.clone().filter(|d| launcher_in(d).is_some()).or_else(|| found_svr.clone());
    let mut checks = Vec::new();
    match &cstrike {
        Some(c) => match game_exe(c) {
            Some(exe) => checks.push(check("game", true, "Counter-Strike: Source", slashes(&exe))),
            None => checks.push(check("game", false, "Counter-Strike: Source", format!("no cstrike_win64.exe next to {}", slashes(c)))),
        },
        None => checks.push(check("game", false, "Counter-Strike: Source", "not found: pick its folder".into())),
    }
    match &svr {
        Some(d) => {
            let missing = svr_missing_libraries(d);
            checks.push(if missing.is_empty() {
                check("svr", true, "SVR", slashes(&launcher_in(d).unwrap_or_default()))
            } else {
                check("svr", false, "SVR", format!(
                    "incomplete: {} missing from {}. Download svr.zip again ({}) and extract all of it there; if the file \
                     disappears again, your antivirus took it (Windows Security: Protection history)",
                    missing.join(", "), slashes(d), SVR_RELEASES))
            });
        }
        None => checks.push(check("svr", false, "SVR", format!("not found: pick its folder (download: {})", SVR_RELEASES))),
    }
    if let Some(c) = &cstrike {
        let mm = loader_state(c);
        let mm_detail = match mm {
            "missing" => "not installed: extract Metamod:Source into cstrike (see README)".to_string(),
            "on" if s.sessions_only => "installed (on now; off again when the game closes)".to_string(),
            "on" => "installed (always on)".to_string(),
            _ => "installed (off: on for TASurf's sessions only)".to_string(),
        };
        checks.push(check("metamod", mm != "missing", "Metamod:Source", mm_detail));
        let sm = c.join("addons").join("sourcemod").join("bin").is_dir();
        checks.push(check("sourcemod", sm, "SourceMod", if sm { "installed".into() } else { "not installed: extract it into cstrike (see README)".into() }));
        let p = plugin_state(c);
        let detail = match p {
            "missing" => "not installed yet: launching installs it",
            "outdated" => "older than this TASurf: launching installs the new one",
            _ => "up to date",
        };
        checks.push(check("plugin", true, "TASurf plugin", detail.into()));
    }
    let steam = running("steam.exe");
    checks.push(check("steam", true, "Steam", if steam { "running".into() } else { "not running: launching starts it".into() }));
    let maps = cstrike.as_deref().map(maps).unwrap_or_default();
    json!({
        "settings": {
            "cstrike_dir": cstrike.as_deref().map(slashes),
            "svr_dir": svr.as_deref().map(slashes),
            "project_dir": slashes(&s.project_dir),
            "map": s.map, "width": s.width, "height": s.height, "borderless": s.borderless,
            "sessions_only": s.sessions_only,
        },
        "found": {
            "cstrike_dir": s.cstrike_dir.is_none() && found_game.is_some(),
            "svr_dir": s.svr_dir.is_none() && found_svr.is_some(),
        },
        "checks": checks,
        "maps": maps,
        "game_running": game_running(),
        "launch": launch_state(),
    })
}

// ---------------------------------------------------------------- launching

#[derive(Default)]
struct Launch {
    state: String, // "" (never), "starting", "running", "closed", "failed"
    log: Vec<String>,
    error: Option<String>,
    started: f64,
}

static LAUNCH: Mutex<Option<Launch>> = Mutex::new(None);

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn with_launch<T>(f: impl FnOnce(&mut Launch) -> T) -> T {
    let mut g = LAUNCH.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Launch::default))
}

pub fn launch_state() -> Value {
    with_launch(|l| json!({"state": l.state, "log": l.log, "error": l.error, "started": l.started}))
}

/// The game a launch started is up (its process is there).
pub fn launched_running() -> bool {
    with_launch(|l| l.state == "running")
}

fn say(msg: String) {
    with_launch(|l| l.log.push(msg));
}

/// SVR's game profile for this launch: the game itself (by its path, in whichever library it is), the map and the
/// window (the plugin runs tasurf_session.cfg itself). SVR adds -steam -insecure +sv_lan 1 -console -novid -noip.
fn write_profile(svr: &Path, exe: &Path, s: &Settings) -> Result<(), String> {
    let mut args = format!("-game cstrike -windowed -w {} -h {}", s.width, s.height);
    if s.borderless {
        args += " -noborder -x 0 -y 0";
    }
    args += &format!(" +map {}", s.map);
    let text = format!(
        "name=Counter-Strike: Source (TASurf)\nother_path={}\ndemo_path=cstrike\nargs={}\n",
        exe.display(),
        args
    );
    let games = svr.join("data").join("games");
    std::fs::create_dir_all(&games).map_err(|e| e.to_string())?;
    std::fs::write(games.join(PROFILE), text).map_err(|e| format!("couldn't write SVR's game profile: {}", e))
}

fn wait_for(what: impl Fn() -> bool, secs: u64) -> bool {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if what() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    what()
}

fn svr_log_tail(svr: &Path) -> String {
    let text = std::fs::read_to_string(svr.join("data").join("svr_log.txt")).unwrap_or_default();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(4)..].join(" / ")
}

/// Start the game (in the background; launch_state says how it goes). Settings in body are saved first.
pub fn launch(body: &Value) -> Result<Value, String> {
    let s = save(body)?;
    if with_launch(|l| l.state == "starting") {
        return Err("the game is being started already".into());
    }
    if game_running() {
        return Err("CS:S is running already: close it first (a game started from Steam has no TASurf in it)".into());
    }
    let cstrike = s.cstrike_dir.clone().or_else(|| found().0).ok_or("Counter-Strike: Source wasn't found: pick its folder")?;
    let exe = game_exe(&cstrike).ok_or("no cstrike_win64.exe in the game's folder")?;
    let svr = s.svr_dir.clone().or_else(|| found().1).ok_or("SVR wasn't found: pick its folder")?;
    let missing = svr_missing_libraries(&svr);
    if !missing.is_empty() {
        return Err(format!("SVR is incomplete: {} missing from {}. Download svr.zip again ({}) and extract all of it there",
            missing.join(", "), slashes(&svr), SVR_RELEASES));
    }
    let launcher = svr.join(if is_64(&exe) { "svr_launcher64.exe" } else { "svr_launcher.exe" });
    if !launcher.is_file() {
        return Err(format!("no {} in {}", launcher.file_name().unwrap_or_default().to_string_lossy(), slashes(&svr)));
    }
    if s.map.is_empty() || !maps(&cstrike).contains(&s.map) {
        return Err(format!("the game has no map {:?}", s.map));
    }
    if loader_state(&cstrike) == "missing" || !cstrike.join("addons").join("sourcemod").join("bin").is_dir() {
        return Err("Metamod:Source and SourceMod aren't installed in the game (see README)".into());
    }
    with_launch(|l| *l = Launch { state: "starting".into(), log: Vec::new(), error: None, started: now() });
    std::thread::spawn(move || {
        let result = run_launch(&s, &cstrike, &exe, &svr, &launcher);
        if let Err(e) = result {
            if s.sessions_only && !game_running() {
                let _ = set_loader(&cstrike, false);
            }
            with_launch(|l| {
                l.state = "failed".into();
                l.error = Some(e);
            });
        }
    });
    Ok(launch_state())
}

fn run_launch(s: &Settings, cstrike: &Path, exe: &Path, svr: &Path, launcher: &Path) -> Result<(), String> {
    install_plugin(cstrike, &say)?;
    set_loader(cstrike, true)?;
    write_profile(svr, exe, s)?;
    if !running("steam.exe") {
        say("starting Steam".into());
        let steam = steam_dir().map(|d| d.join("steam.exe")).filter(|p| p.is_file()).ok_or("Steam isn't running and wasn't found")?;
        Command::new(&steam).arg("-silent").spawn().map_err(|e| format!("couldn't start Steam: {}", e))?;
        if !wait_for(|| running("steam.exe"), 90) {
            return Err("Steam didn't start".into());
        }
        std::thread::sleep(Duration::from_secs(8)); // signing in
    }
    say(format!("starting CS:S on {} through SVR", s.map));
    let status = Command::new(launcher)
        .arg(PROFILE)
        .current_dir(svr)
        .status()
        .map_err(|e| format!("couldn't run {}: {}", launcher.display(), e))?;
    if !status.success() {
        return Err(format!("SVR's launcher failed ({}): {}", status, svr_log_tail(svr)));
    }
    if !wait_for(game_running, 90) {
        return Err(format!("the game didn't start: {}", svr_log_tail(svr)));
    }
    say("the game is starting".into());
    with_launch(|l| l.state = "running".into());
    // the session lasts as long as the game: then Metamod goes off again
    while game_running() {
        std::thread::sleep(Duration::from_secs(2));
    }
    if s.sessions_only {
        set_loader(cstrike, false)?;
    }
    with_launch(|l| {
        l.state = "closed".into();
        l.log.push("the game closed".into());
    });
    Ok(())
}

// ---------------------------------------------------------------- picking folders

/// Windows' folder picker (through PowerShell, so no extra libraries); None when cancelled.
pub fn browse(title: &str, start: &str) -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let esc = |s: &str| s.replace('\'', "''");
    let script = format!(
        "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName System.Windows.Forms; \
         $f = New-Object System.Windows.Forms.Form -Property @{{TopMost = $true}}; \
         $d = New-Object System.Windows.Forms.FolderBrowserDialog; $d.Description = '{}'; $d.ShowNewFolderButton = $true; \
         $d.SelectedPath = '{}'; if ($d.ShowDialog($f) -eq 'OK') {{ $d.SelectedPath }}",
        esc(title),
        esc(&start.replace('/', "\\"))
    );
    let out = quiet("powershell").args(["-NoProfile", "-STA", "-Command", &script]).output().ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(path).filter(|p| !p.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("tasurf_launch_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn project_folder_of_an_installed_copy_and_a_checkout() {
        let root = Path::new("C:/Users/u/AppData/Local/TASurf");
        let docs = || Some(PathBuf::from("C:/Users/u/Documents"));
        assert_eq!(project_default(Some(root), root, docs()), PathBuf::from("C:/Users/u/Documents").join("TASurf"));
        assert_eq!(project_default(Some(&root.join("rust/target/release")), root, docs()), root);
        assert_eq!(project_default(Some(root), root, None), root);
        assert_eq!(project_default(None, root, docs()), root);
    }

    #[test]
    fn libraries_from_steams_list() {
        let steam = temp("steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps").join("libraryfolders.vdf"),
            "\"libraryfolders\"
{
	\"0\"
	{
		\"path\"		\"C:\\Program Files (x86)\\Steam\"
	}
	\"1\"
	{
		\"path\"		\"D:\\Games\\Steam Library\"
		\"label\"		\"\"
	}
}
",
        )
        .unwrap();
        let libs = steam_libraries(&steam);
        assert_eq!(libs[0], steam);
        assert!(libs.contains(&native("D:/Games/Steam Library")), "{:?}", libs);
        assert!(libs.contains(&native("C:/Program Files (x86)/Steam")), "{:?}", libs);
    }

    #[test]
    fn game_folder_either_way() {
        let root = temp("css").join("Counter-Strike Source");
        std::fs::create_dir_all(root.join("cstrike")).unwrap();
        std::fs::write(root.join("cstrike").join("gameinfo.txt"), "").unwrap();
        assert_eq!(cstrike_of(&root), Some(root.join("cstrike")));
        assert_eq!(cstrike_of(&root.join("cstrike")), Some(root.join("cstrike")));
        assert_eq!(cstrike_of(&root.join("nothing")), None);
    }

    #[test]
    fn metamod_on_and_off() {
        let c = temp("mm").join("cstrike");
        std::fs::create_dir_all(c.join("addons")).unwrap();
        assert_eq!(loader_state(&c), "missing");
        std::fs::write(c.join("addons").join("metamod_x64.vdf"), "\"Plugin\" { \"file\" \"x\" }").unwrap();
        assert_eq!(loader_state(&c), "on");
        set_loader(&c, false).unwrap();
        assert_eq!(loader_state(&c), "off");
        assert!(c.join("addons").join("metamod_x64.vdf.tasurf-off").is_file());
        set_loader(&c, false).unwrap(); // twice: still off
        set_loader(&c, true).unwrap();
        assert_eq!(loader_state(&c), "on");
        assert!(!c.join("addons").join("metamod_x64.vdf.tasurf-off").exists());
    }

    #[test]
    fn svr_libraries() {
        let svr = temp("svrlibs");
        for f in ["avformat-62.dll", "avutil-60.dll", "swresample-6.dll"] {
            std::fs::write(svr.join(f), "").unwrap();
        }
        assert_eq!(svr_missing_libraries(&svr), vec!["avcodec-*.dll".to_string()]);
        std::fs::write(svr.join("avcodec-62.dll"), "").unwrap();
        assert!(svr_missing_libraries(&svr).is_empty());
    }

    #[test]
    fn svr_profile() {
        let svr = temp("svr");
        let s = Settings {
            cstrike_dir: None, svr_dir: None, project_dir: svr.clone(), map: "surf_beginner".into(),
            width: 1280, height: 720, borderless: true, sessions_only: true,
        };
        let exe = native("C:/Games/Counter-Strike Source/cstrike_win64.exe");
        write_profile(&svr, &exe, &s).unwrap();
        let text = std::fs::read_to_string(svr.join("data").join("games").join(PROFILE)).unwrap();
        assert!(text.contains(&format!("other_path={}", exe.display())), "{}", text);
        assert!(text.contains("args=-game cstrike -windowed -w 1280 -h 720 -noborder -x 0 -y 0 +map surf_beginner"), "{}", text);
    }
}

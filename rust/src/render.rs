//! Rendering a run to video with SVR, for the desktop app (the plugin's side is plugin/scripting/tasurf/render.sp).
//!
//! The render settings are SVR's own recording options (its data/profiles/default.ini documents them), kept in
//! tasurf.local.json ("render") and written as SVR's profile data/profiles/tasurf.ini before each render; the
//! movie starts with `startmovie <file> profile=tasurf`. A render is a run request with `render <file>`: the
//! plugin plays it on the replay bot with you watching in first person and starts and ends the movie. Then the
//! finished file is moved from SVR's movies folder (data/movies, its README says) to the project's videos.
//!
//! Only a game started through SVR can render: without SVR, startmovie is the engine's own, which writes
//! thousands of image files into the game folder. So the game process must have SVR's module loaded.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::launch;

pub const ENCODERS: [&str; 3] = ["libx264", "libx264_444", "dnxhr"];
pub const CONTAINERS: [&str; 3] = ["mp4", "mkv", "mov"];
pub const PRESETS: [&str; 9] = ["ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower", "veryslow"];
pub const DNXHR: [&str; 3] = ["lb", "sq", "hq"];
const STYLES: [&str; 3] = ["normal", "italic", "extraitalic"];
const WEIGHTS: [&str; 11] =
    ["thin", "extralight", "light", "semilight", "normal", "medium", "semibold", "bold", "extrabold", "black", "extrablack"];
const ANCHORS: [&str; 3] = ["left", "center", "right"];
const LENGTHS: [&str; 3] = ["xy", "xyz", "z"];

/// The settings with their defaults: SVR's, except H.264 in mp4 (a file to share as it is; SVR's default, DNxHR in
/// mov, is for video editors and makes large files).
pub fn defaults() -> Map<String, Value> {
    let v = json!({
        "fps": 60, "encoder": "libx264", "container": "mp4", "crf": 15, "preset": "ultrafast", "intra": false,
        "dnxhr_profile": "hq", "audio": false,
        "motion_blur": false, "blur_mult": 60, "blur_exposure": 0.5,
        "velo": false, "velo_font": "Segoe UI", "velo_size": 48, "velo_color": "200 200 200 255",
        "velo_border_color": "0 0 0 255", "velo_border": 0, "velo_shadow": 0, "velo_style": "normal",
        "velo_weight": "bold", "velo_align": "0 90", "velo_anchor": "center", "velo_length": "xy", "velo_fps": 20,
        "lagcomp": 0.1, "viewmodel": false, "tail": 0.5,
    });
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn local_path() -> PathBuf {
    crate::config::repo_root().join("tasurf.local.json")
}

fn read_local() -> Map<String, Value> {
    std::fs::read_to_string(local_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| if let Value::Object(o) = v { Some(o) } else { None })
        .unwrap_or_default()
}

/// The saved settings over the defaults.
pub fn settings() -> Map<String, Value> {
    let mut out = defaults();
    if let Some(Value::Object(saved)) = read_local().get("render") {
        for (k, v) in saved {
            if out.contains_key(k) {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

fn color_ok(s: &str) -> bool {
    let parts: Vec<&str> = s.split_whitespace().collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u32>().map_or(false, |n| n <= 255))
}

/// One setting checked (and made canonical), or why it can't be.
fn check(k: &str, v: &Value) -> Result<Value, String> {
    let int = |lo: i64, hi: i64| -> Result<Value, String> {
        match v.as_f64() {
            Some(n) if n.fract() == 0.0 && n >= lo as f64 && n <= hi as f64 => Ok(json!(n as i64)),
            _ => Err(format!("{} must be a whole number from {} to {}", k, lo, hi)),
        }
    };
    let num = |lo: f64, hi: f64| -> Result<Value, String> {
        match v.as_f64() {
            Some(n) if n >= lo && n <= hi => Ok(json!(n)),
            _ => Err(format!("{} must be from {} to {}", k, lo, hi)),
        }
    };
    let one_of = |list: &[&str]| -> Result<Value, String> {
        match v.as_str() {
            Some(s) if list.contains(&s) => Ok(json!(s)),
            _ => Err(format!("{} must be one of {}", k, list.join(", "))),
        }
    };
    let flag = || v.as_bool().map(Value::Bool).ok_or_else(|| format!("{} must be true or false", k));
    match k {
        "fps" => int(1, 1000),
        "encoder" => one_of(&ENCODERS),
        "container" => one_of(&CONTAINERS),
        "crf" => int(0, 52),
        "preset" => one_of(&PRESETS),
        "dnxhr_profile" => one_of(&DNXHR),
        "intra" | "audio" | "motion_blur" | "velo" | "viewmodel" => flag(),
        "blur_mult" => int(2, 1000),
        "blur_exposure" => num(0.0, 1.0),
        "velo_font" => match v.as_str().map(str::trim) {
            Some(s) if !s.is_empty() && s.len() < 64 && !s.contains(['\n', '\r', '=']) => Ok(json!(s)),
            _ => Err("the overlay's font must be a font name".into()),
        },
        "velo_size" => int(4, 400),
        "velo_color" | "velo_border_color" => match v.as_str() {
            Some(s) if color_ok(s) => Ok(json!(s.split_whitespace().collect::<Vec<_>>().join(" "))),
            _ => Err(format!("{} must be four numbers 0..255 (red green blue alpha)", k)),
        },
        "velo_border" | "velo_shadow" => int(0, 50),
        "velo_style" => one_of(&STYLES),
        "velo_weight" => one_of(&WEIGHTS),
        "velo_align" => match v.as_str() {
            Some(s) if s.split_whitespace().count() == 2 && s.split_whitespace().all(|p| p.parse::<i32>().map_or(false, |n| n.abs() <= 100)) => {
                Ok(json!(s.split_whitespace().collect::<Vec<_>>().join(" ")))
            }
            _ => Err("the overlay's place must be two numbers -100..100 (across, down)".into()),
        },
        "velo_anchor" => one_of(&ANCHORS),
        "velo_length" => one_of(&LENGTHS),
        "velo_fps" => int(1, 1000),
        "lagcomp" => num(0.0, 2.0),
        "tail" => num(0.0, 10.0),
        _ => Err(format!("unknown render setting {:?}", k)),
    }
}

/// Save the settings the page sent (each checked; DNxHR can't go in mp4).
pub fn save(body: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    let mut cur = settings();
    for (k, v) in body {
        if cur.contains_key(k) {
            cur.insert(k.clone(), check(k, v)?);
        }
    }
    if let Some(f) = body.get("velo_font").and_then(Value::as_str) {
        let installed = fonts();
        if !installed.is_empty() && !installed.iter().any(|i| i.eq_ignore_ascii_case(f.trim())) {
            return Err(format!("the font {:?} isn't installed: pick one from the list", f));
        }
    }
    if cur["encoder"] == "dnxhr" && cur["container"] == "mp4" {
        return Err("DNxHR can't go in an mp4 file: pick mov or mkv".into());
    }
    let mut m = read_local();
    m.insert("render".into(), Value::Object(cur.clone()));
    let text = serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_default() + "\n";
    std::fs::write(local_path(), text).map_err(|e| format!("couldn't save the render settings: {}", e))?;
    Ok(cur)
}

fn text(v: &Value) -> String {
    match v {
        Value::Bool(b) => (if *b { "1" } else { "0" }).into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// SVR's profile from the settings (SVR's own key names; what isn't here comes from its default.ini).
pub fn profile_text(s: &Map<String, Value>) -> String {
    let keys = [
        ("video_fps", "fps"), ("video_encoder", "encoder"), ("video_x264_crf", "crf"), ("video_x264_preset", "preset"),
        ("video_x264_intra", "intra"), ("video_dnxhr_profile", "dnxhr_profile"), ("audio_enabled", "audio"),
        ("lagcomp_override", "lagcomp"),
        ("motion_blur_enabled", "motion_blur"), ("motion_blur_fps_mult", "blur_mult"), ("motion_blur_exposure", "blur_exposure"),
        ("velo_enabled", "velo"), ("velo_font", "velo_font"), ("velo_font_size", "velo_size"), ("velo_color", "velo_color"),
        ("velo_border_color", "velo_border_color"), ("velo_border_size", "velo_border"), ("velo_shadow_offset", "velo_shadow"),
        ("velo_font_style", "velo_style"), ("velo_font_weight", "velo_weight"), ("velo_align", "velo_align"),
        ("velo_anchor", "velo_anchor"), ("velo_length", "velo_length"), ("velo_fps", "velo_fps"),
    ];
    let mut out = String::from("# Written by TASurf from its render settings (TASurf's Export page); edit them there.\n");
    for (svr, ours) in keys {
        if let Some(v) = s.get(ours) {
            out += &format!("{}={}\n", svr, text(v));
        }
    }
    out
}

// ---------------------------------------------------------------- the render under way

#[derive(Default, Clone)]
struct Job {
    state: String, // "rendering", "encoding", "done", "failed"
    file: String,
    path: Option<PathBuf>,
    request: String,
    error: Option<String>,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);

fn set_job(f: impl FnOnce(&mut Job)) {
    let mut g = JOB.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Job::default));
}

fn job() -> Option<Job> {
    JOB.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The settings, the choices, where things are and how the last render went.
pub fn status() -> Value {
    let ls = launch::settings();
    let j = job();
    json!({
        "settings": settings(),
        "choices": {"encoder": ENCODERS, "container": CONTAINERS, "preset": PRESETS, "dnxhr_profile": DNXHR,
                    "velo_style": STYLES, "velo_weight": WEIGHTS, "velo_anchor": ANCHORS, "velo_length": LENGTHS},
        "svr_dir": svr_dir().map(|d| d.display().to_string().replace('\\', "/")),
        "size": format!("{} × {}", ls.width, ls.height),
        "videos": launch::videos_dir().display().to_string().replace('\\', "/"),
        "video_list": video_list(),
        "job": j.map(|j| json!({"state": j.state, "file": j.file, "request": j.request, "error": j.error,
                                "path": j.path.map(|p| p.display().to_string().replace('\\', "/"))})),
    })
}

fn svr_dir() -> Option<PathBuf> {
    launch::svr_folder()
}

/// Is SVR in the game that's running? (Its module in the process: tasklist /M.)
fn svr_in_game() -> bool {
    if !cfg!(windows) {
        return false;
    }
    ["svr_game64.dll", "svr_game.dll"].iter().any(|m| {
        let mut c = std::process::Command::new("tasklist");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c.args(["/M", m])
            .output()
            .map(|o| {
                let t = String::from_utf8_lossy(&o.stdout).to_ascii_lowercase();
                t.contains("cstrike_win64.exe") || t.contains("hl2.exe")
            })
            .unwrap_or(false)
    })
}

/// Where SVR puts movies: its movies folder (SVR 48 does), or data/movies (as its README says).
fn movie_dirs(svr: &Path) -> [PathBuf; 2] {
    [svr.join("movies"), svr.join("data").join("movies")]
}

/// A name for the movie that's free in SVR's movies folders and the project's videos.
fn movie_name(base: &str, ext: &str, svr_movies: &[PathBuf], videos: &Path) -> String {
    let free = |n: &str| svr_movies.iter().all(|d| !d.join(n).exists()) && !videos.join(n).exists();
    let first = format!("{}.{}", base, ext);
    if free(&first) {
        return first;
    }
    (2..10000).map(|i| format!("{}_{}.{}", base, i, ext)).find(|n| free(n)).unwrap_or(first)
}

/// The video's name as typed on the Export page, without its extension (one the format's own replaces): letters,
/// digits, '-', '_' and '.', as the game's startmovie takes it (one word). None when it's empty: the timeline's then.
pub fn video_base(typed: &str) -> Result<Option<String>, String> {
    let t = typed.trim();
    let lower = t.to_ascii_lowercase();
    let n = [".mp4", ".mkv", ".mov"].iter().find(|e| lower.ends_with(*e)).map_or(t, |e| &t[..t.len() - e.len()]);
    if n.is_empty() {
        return Ok(None);
    }
    if n.len() > 64 || n.starts_with('.') || !n.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        return Err(format!("the video's name: letters, digits, - _ and . only, at most 64 (got {})", crate::py::repr_str(typed)));
    }
    Ok(Some(n.to_string()))
}

/// What the render needs before it's asked for, and its request's extra lines; then (after the editor sent the
/// request, with its id) watch() follows it. video: the name typed on the Export page ("": the timeline's).
pub fn prepare(timeline: &str, video: &str) -> Result<(String, String), String> {
    let base = video_base(video)?.unwrap_or_else(|| timeline.to_string());
    if matches!(job().map(|j| j.state), Some(ref s) if s == "rendering" || s == "encoding") {
        return Err("a render is under way already".into());
    }
    let svr = svr_dir().ok_or("SVR wasn't found: pick its folder in Setup (Game & folders)")?;
    if !launch::game_running() {
        return Err("the game isn't running: start it from Setup (Game & folders: Launch)".into());
    }
    if !svr_in_game() {
        return Err("the game wasn't started through SVR, so it can't record: close it and start it from TASurf's Setup page (Launch)".into());
    }
    let s = settings();
    let profiles = svr.join("data").join("profiles");
    std::fs::create_dir_all(&profiles).map_err(|e| e.to_string())?;
    std::fs::write(profiles.join("tasurf.ini"), profile_text(&s)).map_err(|e| format!("couldn't write SVR's profile: {}", e))?;
    let videos = launch::videos_dir();
    let file = movie_name(&base, s["container"].as_str().unwrap_or("mp4"), &movie_dirs(&svr), &videos);
    let action = format!(
        "render {}\ntail {}\nviewmodel {}",
        file,
        s["tail"].as_f64().unwrap_or(0.5),
        if s["viewmodel"].as_bool().unwrap_or(false) { 1 } else { 0 }
    );
    Ok((file, action))
}

/// Follow a render the game was asked for: its run, then the movie file, moved to the project's videos.
pub fn watch(request: String, file: String, data_dir: PathBuf) {
    set_job(|j| *j = Job { state: "rendering".into(), file: file.clone(), path: None, request: request.clone(), error: None });
    std::thread::spawn(move || {
        let fail = |e: String| set_job(|j| {
            j.state = "failed".into();
            j.error = Some(e);
        });
        let svr = match svr_dir() {
            Some(s) => s,
            None => return fail("SVR wasn't found".into()),
        };
        // the run: its result (the plugin writes it after the last tick; a long run takes long, so no timeout
        // while the game runs)
        let result = data_dir.join("requests").join(format!("{}.result", request));
        loop {
            if result.is_file() {
                break;
            }
            if !launch::game_running() {
                return fail("the game closed during the render".into());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let text = std::fs::read_to_string(&result).unwrap_or_default();
        let status = text.lines().find_map(|l| l.strip_prefix("status ")).unwrap_or("").to_string();
        let detail = text.lines().find_map(|l| l.strip_prefix("detail ")).unwrap_or("").to_string();
        if !status.starts_with("complete") {
            return fail(format!("the run didn't complete ({}): {}", status, detail));
        }
        // the movie: SVR has it open until it's finished; then it can be moved
        set_job(|j| j.state = "encoding".into());
        let videos = launch::videos_dir();
        let _ = std::fs::create_dir_all(&videos);
        let dest = videos.join(&file);
        let start = Instant::now();
        loop {
            if let Some(src) = movie_dirs(&svr).iter().map(|d| d.join(&file)).find(|p| p.is_file()) {
                // only once SVR has finished it: an mp4's index is written last, so a file taken any earlier is cut
                // off and won't play
                let moved = finished(&src)
                    && (std::fs::rename(&src, &dest).is_ok()
                        || (std::fs::copy(&src, &dest).ok() == std::fs::metadata(&src).ok().map(|m| m.len())
                            && std::fs::remove_file(&src).is_ok()));
                if moved {
                    return set_job(|j| {
                        j.state = "done".into();
                        j.path = Some(dest.clone());
                    });
                }
            } else if start.elapsed() > Duration::from_secs(20) {
                return fail(format!(
                    "SVR didn't record (no {} in {}). The game's console says why; so does SVR's log: {}",
                    file,
                    movie_dirs(&svr)[0].display(),
                    svr.join("data").join("svr_log.txt").display()
                ));
            }
            if start.elapsed() > Duration::from_secs(600) {
                return fail(format!("the movie is still open in SVR after 10 minutes: {}", file));
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

/// SVR is done with a movie: nothing else has it open (it opens with no sharing at all, which fails while SVR's
/// encoder still writes it: being able to open it to write isn't enough, as FFmpeg lets others write too) and its
/// size has stayed the same for a second.
fn finished(p: &Path) -> bool {
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).ok();
    let before = size(p);
    std::thread::sleep(Duration::from_millis(1000));
    if before.is_none() || size(p) != before {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().read(true).share_mode(0).open(p).is_ok()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// The font families installed (as DirectWrite, which SVR draws the speed with, names them: WPF's list), sorted;
/// empty where they can't be listed. Listed once per run of the app (it takes a second or two).
pub fn fonts() -> Vec<String> {
    static FONTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    FONTS
        .get_or_init(|| {
            if !cfg!(windows) {
                return Vec::new();
            }
            let mut c = std::process::Command::new("powershell");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                c.creation_flags(0x0800_0000);
            }
            let script = "[Console]::OutputEncoding = [Text.Encoding]::UTF8; Add-Type -AssemblyName PresentationCore; \
                          [System.Windows.Media.Fonts]::SystemFontFamilies | ForEach-Object { $_.Source }";
            let out = match c.args(["-NoProfile", "-Command", script]).output() {
                Ok(o) => String::from_utf8_lossy(&o.stdout).to_string(),
                Err(_) => return Vec::new(),
            };
            let mut list: Vec<String> = out
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty() && !l.contains(['=', '\n', '\r']) && l.len() < 64)
                .collect();
            list.sort_by_key(|f| f.to_lowercase());
            list.dedup();
            list
        })
        .clone()
}

/// The project's videos, newest first: [{name, size, mtime}].
fn video_list() -> Vec<Value> {
    let mut out: Vec<(f64, Value)> = std::fs::read_dir(launch::videos_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let ext = p.extension()?.to_string_lossy().to_ascii_lowercase();
            if !CONTAINERS.contains(&ext.as_str()) {
                return None;
            }
            let meta = e.metadata().ok()?;
            let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
            Some((mtime, json!({"name": p.file_name()?.to_string_lossy(), "size": meta.len(), "mtime": mtime})))
        })
        .collect();
    out.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    out.into_iter().map(|(_, v)| v).collect()
}

/// Open a video of the project (by name; else the last render's), or show it in its folder.
pub fn open(what: &str, name: Option<&str>) -> Result<(), String> {
    let path = match name {
        Some(n) if !n.is_empty() => {
            if n.contains(['/', '\\', ':']) || n.starts_with('.') {
                return Err(format!("bad video name {:?}", n));
            }
            launch::videos_dir().join(n)
        }
        _ => job().and_then(|j| j.path).ok_or("no video yet")?,
    };
    if !path.is_file() {
        return Err(format!("{} isn't there any more", path.display()));
    }
    let mut c = if what == "folder" {
        let mut c = std::process::Command::new("explorer");
        c.arg(format!("/select,{}", path.display()));
        c
    } else {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]).arg(&path);
        c
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_has_svrs_names() {
        let mut s = defaults();
        s.insert("motion_blur".into(), json!(true));
        s.insert("velo".into(), json!(true));
        let t = profile_text(&s);
        for line in ["video_fps=60", "video_encoder=libx264", "video_x264_crf=15", "motion_blur_enabled=1", "motion_blur_fps_mult=60",
            "motion_blur_exposure=0.5", "velo_enabled=1", "velo_font=Segoe UI", "velo_color=200 200 200 255", "velo_align=0 90",
            "audio_enabled=0", "lagcomp_override=0.1"] {
            assert!(t.lines().any(|l| l == line), "{} not in\n{}", line, t);
        }
        assert!(!t.contains("tail") && !t.contains("viewmodel"), "{}", t);
    }

    #[test]
    fn settings_are_checked() {
        assert_eq!(check("fps", &json!(60)).unwrap(), json!(60));
        assert!(check("fps", &json!(59.5)).is_err());
        assert!(check("crf", &json!(53)).is_err());
        assert!(check("encoder", &json!("h265")).is_err());
        assert_eq!(check("velo_color", &json!(" 1  2 3 255 ")).unwrap(), json!("1 2 3 255"));
        assert!(check("velo_color", &json!("1 2 3")).is_err());
        assert!(check("velo_align", &json!("0 120")).is_err());
        assert!(check("velo_font", &json!("a=b")).is_err());
        assert!(check("blur_exposure", &json!(1.5)).is_err());
    }

    #[test]
    fn a_movie_still_open_isnt_finished() {
        let p = std::env::temp_dir().join(format!("tasurf_movie_{}.mp4", std::process::id()));
        std::fs::write(&p, b"ftypisom").unwrap();
        let writer = std::fs::OpenOptions::new().append(true).open(&p).unwrap();  // as SVR's encoder has it, sharing
        if cfg!(windows) {
            assert!(!finished(&p), "taken while still open");
        }
        drop(writer);
        assert!(finished(&p));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn video_names() {
        assert_eq!(video_base("").unwrap(), None);
        assert_eq!(video_base("  ").unwrap(), None);
        assert_eq!(video_base("year300 wr").unwrap_err(), "the video's name: letters, digits, - _ and . only, at most 64 (got 'year300 wr')");
        assert_eq!(video_base("year300_wr").unwrap(), Some("year300_wr".into()));
        assert_eq!(video_base("year300_wr.MP4").unwrap(), Some("year300_wr".into()));  // the format's own extension goes on
        assert_eq!(video_base("v1.2-final").unwrap(), Some("v1.2-final".into()));
        assert!(video_base("../x").is_err() && video_base(".hidden").is_err() && video_base(&"a".repeat(65)).is_err());
        assert!(video_base("a;quit").is_err());  // nothing that would end the game's command
    }

    #[test]
    fn names_never_overwrite() {
        let d = std::env::temp_dir().join(format!("tasurf_render_{}", std::process::id()));
        let (dirs, v) = (movie_dirs(&d), d.join("videos"));
        for m in dirs.iter().chain([&v]) {
            std::fs::create_dir_all(m).unwrap();
        }
        assert_eq!(movie_name("run", "mp4", &dirs, &v), "run.mp4");
        std::fs::write(v.join("run.mp4"), "").unwrap();
        assert_eq!(movie_name("run", "mp4", &dirs, &v), "run_2.mp4");
        std::fs::write(dirs[0].join("run_2.mp4"), "").unwrap();
        assert_eq!(movie_name("run", "mp4", &dirs, &v), "run_3.mp4");
        std::fs::write(dirs[1].join("run_3.mp4"), "").unwrap();
        assert_eq!(movie_name("run", "mp4", &dirs, &v), "run_4.mp4");
    }
}

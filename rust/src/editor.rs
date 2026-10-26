//! Browser editor: tasurf editor [timeline] (tasurf/editor.py).
//!
//! A small local web server (127.0.0.1 only) serving editor_static/index.html and
//! a JSON API. Timelines are read from and saved to timelines/. "Run" saves, bakes,
//! installs the baked file into the game and drops a request file that the plugin
//! picks up (data/tasurf/requests/request.txt). The result comes back as
//! requests/<id>.result plus the usual run CSV.
//!
//! All movement analysis here (best yaw, estimates) uses the reference model in
//! physics.rs; the engine's run CSVs stay the ground truth.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};

use crate::bake::{bake_inputs, bake_to_file, hash_of, read_baked, render_baked, TickInput};
use crate::camera::camera_yaws;
use crate::config::{self, Config};
use crate::editdoc::{self, apply_edits, apply_op, format_json, freeze_strafe, normalized, obj, opt, save_document,
                     splice_recording, strafe_at, to_int, to_str, Edits};
use crate::interp::sample;
use crate::physics::{best_yaw, check_run, estimate_free_flight, header_float, params_from_run, CheckReport};
use crate::py;
use crate::runcsv::{find_runs, read_run, RunCsv};
use crate::editdoc::MAX_TICKS;
use crate::timeline::{name_ok, parse_start, parse_timeline, Res, StartState, Timeline, TlError, FORMAT_NAME, FORMAT_VERSION};

const BUSY: &str = "the game hasn't picked up the previous run request yet (it needs a map loaded, the replay bot \
                    ready, and no run playing); try again in a moment";
const NO_GAME: &str = "cstrike_dir is not set in tasurf.local.json, so the editor can't reach the game";
const VIEW_PITCH_PLUGIN: [i64; 2] = [0, 9];
const STRAIGHT_PLUGIN: [i64; 2] = [0, 11]; // the first that plays straight auto-strafe (@strafe with a heading)
const AUTOBHOP_PLUGIN: [i64; 2] = [0, 12]; // the first that plays auto bhop (@autobhop, baked version 4)
const SWING_PLUGIN: [i64; 2] = [0, 13]; // the first that plays a straight range's swing (baked version 5)
const GAIN_PLUGIN: [i64; 2] = [0, 14]; // the first that plays a swing's gain (smooth turning, baked version 6)
const THUMB_POINTS: usize = 240; // at most, along a timeline's path on the start screen (Editor::thumb)
const VIEW_YAW_PLUGIN: [i64; 2] = [0, 15]; // the first that shows @view_yaw lines (older ones refuse the file, so they get none)
const RENDER_PLUGIN: [i64; 2] = [0, 10]; // the first plugin that renders (render.sp) // the first plugin that plays @view_pitch lines (older ones refuse the file)
const INDEX_FALLBACK: &str = include_str!("../../tasurf/editor_static/index.html");
const VIEW3D_FALLBACK: &str = include_str!("../../tasurf/editor_static/view3d.js"); // 3D view
const PREDICT_FALLBACK: &str = include_str!("../../tasurf/editor_static/predict.js"); // the instant prediction

/// An API failure answered with 400 and {"error": ...} (Python: an exception
/// the request handler caught).
pub type Api<T> = Result<T, String>;

fn tl_err(e: TlError) -> String {
    e.0
}

pub fn now() -> f64 {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9
}

/// os.stat(path).st_mtime
pub fn mtime(path: &Path) -> Option<f64> {
    let m = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = m.duration_since(UNIX_EPOCH).ok()?;
    Some(d.as_secs() as f64 + d.subsec_nanos() as f64 * 1e-9)
}

/// uuid.uuid4().hex[:12]: 12 random hex digits.
pub fn random_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos());
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())[..12].to_string()
}

fn read_lossy(path: &Path) -> std::io::Result<String> {
    Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned())
}

/// `key value` lines (heartbeat, results) as a dict.
fn fields_of(text: &str) -> Vec<(String, String)> {
    let mut fields: Vec<(String, String)> = Vec::new();
    for line in py::splitlines(text) {
        let (key, value) = py::partition(py::strip(line), " ");
        if !key.is_empty() {
            crate::bake::put(&mut fields, key, value.to_string());
        }
    }
    fields
}

fn field<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn opt_s(v: Option<&str>) -> Value {
    v.map_or(Value::Null, s)
}

fn error_value(e: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("error".into(), opt_s(e));
    Value::Object(m)
}

fn r4(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|v| py::num(py::round(*v, 4))).collect())
}

fn ints(xs: &[i64]) -> Value {
    Value::Array(xs.iter().map(|v| Value::from(*v)).collect())
}

fn floats(xs: impl Iterator<Item = f64>) -> Value {
    Value::Array(xs.map(py::num).collect())
}

fn is_alnum(s: &str) -> bool {
    !s.is_empty() && s.chars().all(char::is_alphanumeric)
}

type Loaded = (Arc<RunCsv>, Arc<CheckReport>);

pub struct Editor {
    // where timelines are, the settings and the game's data folder: the desktop app's Setup page can change
    // them while it runs (reload)
    timelines: RwLock<PathBuf>,
    pub out_dir: PathBuf,
    cfg: RwLock<Config>,
    data: RwLock<Option<PathBuf>>,
    lock: Mutex<()>,
    run_cache: Mutex<HashMap<PathBuf, (f64, Loaded)>>,
    sent: Mutex<HashMap<String, f64>>, // request id -> time sent
    replaced: Mutex<HashSet<String>>,  // ids replaced by a newer request before the game took them
    maps: Mutex<Option<(PathBuf, f64, Arc<Vec<u8>>, Option<Arc<Vec<u8>>>)>>, // 3D view: the last map's mesh, textured
    files: Mutex<Option<Arc<crate::gamefiles::GameFiles>>>, // 3D view: the game's materials and textures
}

impl Editor {
    pub fn new(timelines_dir: PathBuf, cfg: Config, out_dir: Option<PathBuf>) -> Editor {
        let data_dir = cfg.game_data_dir();
        Editor {
            timelines: RwLock::new(timelines_dir),
            out_dir: out_dir.unwrap_or_else(|| config::repo_root().join("out").join("baked")),
            cfg: RwLock::new(cfg),
            data: RwLock::new(data_dir),
            lock: Mutex::new(()),
            run_cache: Mutex::new(HashMap::new()),
            sent: Mutex::new(HashMap::new()),
            replaced: Mutex::new(HashSet::new()),
            maps: Mutex::new(None),
            files: Mutex::new(None),
        }
    }

    // ---- paths ------------------------------------------------------------

    pub fn timelines_dir(&self) -> PathBuf {
        self.timelines.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn cfg(&self) -> Config {
        self.cfg.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// addons/sourcemod/data/tasurf in the game (None: no game folder set).
    pub fn data_dir(&self) -> Option<PathBuf> {
        self.data.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Another timelines folder and settings (the desktop app's Setup page changed them).
    pub fn reload(&self, timelines_dir: PathBuf, cfg: Config) {
        *self.data.write().unwrap_or_else(|e| e.into_inner()) = cfg.game_data_dir();
        *self.cfg.write().unwrap_or_else(|e| e.into_inner()) = cfg;
        *self.timelines.write().unwrap_or_else(|e| e.into_inner()) = timelines_dir;
    }

    pub fn timeline_path(&self, name: &str) -> Res<PathBuf> {
        if !name_ok(name) {
            return Err(TlError(format!("bad timeline name {}", py::repr_str(name))));
        }
        Ok(self.timelines_dir().join(format!("{}.json", name)))
    }

    pub fn search_dirs(&self) -> Vec<PathBuf> {
        self.data_dir().iter().map(|d| d.join("starts")).collect()
    }

    fn parse(&self, doc: &Value, name: &str) -> Res<Timeline> {
        parse_timeline(doc, Some(&self.timeline_path(name)?), &self.search_dirs())
    }

    // ---- listing ----------------------------------------------------------

    /// Timelines and recordings, with what the Setup page shows about each (map,
    /// length, runs) in "info" and "recording_info", in the same order.
    pub fn list(&self) -> Value {
        let mut timelines = glob_stems(&self.timelines_dir(), "json");
        timelines.sort();
        let (mut recordings, mut recording_info) = (Vec::new(), Vec::new());
        if let Some(d) = self.data_dir() {
            let baked = d.join("baked");
            if baked.is_dir() {
                let mut files = glob_files(&baked, "tas");
                files.sort();
                for p in files {
                    let head: String = read_lossy(&p).unwrap_or_default().chars().take(600).collect();
                    if head.contains("@hash fnv1a32:") {
                        // written by tas_record
                        recordings.push(stem(&p));
                        recording_info.push(recording_info_of(&stem(&p), &head));
                    }
                }
            }
        }
        let info: Vec<Value> = timelines.iter().map(|n| self.timeline_info(n)).collect();
        let mut m = Map::new();
        m.insert("timelines".into(), Value::Array(timelines.into_iter().map(Value::String).collect()));
        m.insert("recordings".into(), Value::Array(recordings.into_iter().map(Value::String).collect()));
        m.insert("game".into(), Value::Bool(self.data_dir().is_some()));
        m.insert("info".into(), Value::Array(info));
        m.insert("recording_info".into(), Value::Array(recording_info));
        Value::Object(m)
    }

    fn timeline_info(&self, name: &str) -> Value {
        let path = self.timelines_dir().join(format!("{}.json", name));
        let mut info = Map::new();
        info.insert("name".into(), s(name));
        info.insert("map".into(), Value::Null);
        info.insert("length".into(), Value::Null);
        info.insert("runs".into(), Value::from(0));
        info.insert("mtime".into(), mtime(&path).map_or(Value::Null, py::num));
        let doc = match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| py::loads(&t)) {
            Ok(d) => d,
            Err(_) => {
                info.insert("error".into(), s("not valid JSON"));
                return Value::Object(info);
            }
        };
        if let Value::Object(d) = &doc {
            if let Some(Value::String(m)) = d.get("map") {
                info.insert("map".into(), s(m));
            }
            if let Some(l @ Value::Number(n)) = d.get("length") {
                if n.is_i64() || n.is_u64() {
                    info.insert("length".into(), l.clone());
                }
            }
            // runs are named after the timeline's own name, which may differ from the file's
            let tl_name = match d.get("name") {
                Some(Value::String(n)) if name_ok(n) => n.as_str(),
                _ => name,
            };
            info.insert("runs".into(), Value::from(self.runs(tl_name).len()));
        }
        Value::Object(info)
    }

    /// The start positions captured in the game (data/tasurf/starts, from tas_setstart and recordings).
    pub fn starts(&self) -> Value {
        let mut out = Vec::new();
        if let Some(d) = self.data_dir() {
            let dir = d.join("starts");
            if dir.is_dir() {
                let mut files = glob_files(&dir, "json");
                files.sort();
                for p in files {
                    let mut entry = Map::new();
                    entry.insert("name".into(), s(&stem(&p)));
                    for k in ["map", "yaw", "pitch", "origin"] {
                        entry.insert(k.into(), Value::Null);
                    }
                    entry.insert("mtime".into(), mtime(&p).map_or(Value::Null, py::num));
                    match read_start(&p) {
                        Err(e) => {
                            entry.insert("error".into(), s(&e));
                        }
                        Ok((_, st)) => {
                            let (pitch, yaw) = exact_angles(&st);
                            entry.insert("map".into(), opt_s(st.map.as_deref()));
                            entry.insert("yaw".into(), py::num(yaw));
                            entry.insert("pitch".into(), py::num(pitch));
                            entry.insert("origin".into(), Value::Array(st.origin.iter().map(|v| py::num(*v)).collect()));
                        }
                    }
                    out.push(Value::Object(entry));
                }
            }
        }
        let mut m = Map::new();
        m.insert("starts".into(), Value::Array(out));
        m.insert("game".into(), Value::Bool(self.data_dir().is_some()));
        Value::Object(m)
    }

    /// A new timeline: from a start position (copied in, facing its yaw, no keys
    /// held), or a copy of another timeline under a new name.
    pub fn create(&self, name: &Value, start: Option<&Value>, copy: Option<&Value>, length: Option<&Value>, map: Option<&Value>) -> Value {
        let name = match name.as_str() {
            Some(n) if name_ok(n) => n,
            _ => return error_value(Some("names may only use letters, digits, '_', '-' and '.'")),
        };
        let dest = match self.timeline_path(name) {
            Ok(p) => p,
            Err(e) => return error_value(Some(&e.0)),
        };
        if dest.exists() {
            return error_value(Some(&format!("timelines/{}.json already exists; pick another name", name)));
        }
        let doc = if let Some(copy) = copy {
            let src = match copy.as_str() {
                Some(c) if name_ok(c) => self.timelines_dir().join(format!("{}.json", c)),
                _ => return error_value(Some(&format!("no timeline {}", py::repr_value(copy)))),
            };
            let text = match std::fs::read_to_string(&src) {
                Ok(t) => t,
                Err(e) => return error_value(Some(&crate::io_error(&e, &src))),
            };
            let mut doc = match py::loads(&text) {
                Ok(Value::Object(d)) => d,
                Ok(_) => return error_value(Some(&format!("timelines/{}.json is not a timeline", copy.as_str().unwrap_or("")))),
                Err(e) => return error_value(Some(&e)),
            };
            doc.insert("name".into(), s(name));
            Value::Object(doc)
        } else {
            let d = match self.data_dir() {
                Some(d) => d,
                None => return error_value(Some("cstrike_dir is not set in tasurf.local.json, so there are no start positions")),
            };
            let start = match start.and_then(Value::as_str) {
                Some(st) if name_ok(st) => st,
                _ => return error_value(Some("pick a start position")),
            };
            let (obj, st) = match read_start(&d.join("starts").join(format!("{}.json", start))) {
                Ok(x) => x,
                Err(e) => return error_value(Some(&e)),
            };
            let map_v = match st.map.as_deref().filter(|m| !m.is_empty()) {
                Some(m) => s(m),
                None => map.cloned().unwrap_or(Value::Null),
            };
            let map_name = match map_v.as_str() {
                Some(m) if name_ok(m) => m.to_string(),
                _ => return error_value(Some(&format!("the start position {} doesn't say which map it's on; pick the map", start))),
            };
            let length = match length {
                None => 660,
                Some(v) => match to_int(v) {
                    Ok(l) => l,
                    Err(_) => return error_value(Some(&format!("length: expected a number of ticks, got {}", py::repr_value(v)))),
                },
            };
            if !(1..=MAX_TICKS).contains(&length) {
                return error_value(Some(&format!("length: 1 to {} ticks", MAX_TICKS)));
            }
            let (_, yaw) = exact_angles(&st);
            let start_obj: Map<String, Value> = match obj {
                Value::Object(o) => o.into_iter().filter(|(k, _)| k != "format" && k != "version").collect(),
                _ => Map::new(),
            };
            let mut yaw_track = Map::new();
            // smooth: no corners in the yaw
            yaw_track.insert("default_interp".into(), s("cubic"));
            yaw_track.insert("keys".into(), Value::Array(vec![Value::Array(vec![Value::from(0), py::num(yaw), s("cubic")])]));
            let mut doc = Map::new();
            doc.insert(
                "_comment".into(),
                s(&format!(
                    "Created in the editor from the start position {} (copied in, so capturing that start again doesn't change this timeline).",
                    start
                )),
            );
            doc.insert("format".into(), s(FORMAT_NAME));
            doc.insert("version".into(), Value::from(FORMAT_VERSION));
            doc.insert("name".into(), s(name));
            doc.insert("map".into(), s(&map_name));
            doc.insert("length".into(), Value::from(length));
            doc.insert("start".into(), Value::Object(start_obj));
            doc.insert("yaw".into(), Value::Object(yaw_track));
            doc.insert("keys".into(), Value::Array(Vec::new()));
            doc.insert("autobhop".into(), Value::Bool(true));
            Value::Object(doc)
        };
        let saved = {
            let _guard = self.lock.lock().unwrap();
            save_document(&dest, &doc, &self.search_dirs())
        };
        if let Err(e) = saved {
            return error_value(Some(&e.0));
        }
        let mut m = Map::new();
        m.insert("error".into(), Value::Null);
        m.insert("name".into(), s(name));
        Value::Object(m)
    }

    /// A timeline's path for its picture on the start screen: from its newest run or preview, x, y and the
    /// horizontal speed at about THUMB_POINTS ticks, with a null where it teleports (the page draws each stretch).
    /// points is null before it has run.
    pub fn thumb(&self, name: &str) -> Api<Value> {
        if !name_ok(name) {
            return Ok(json!({"error": format!("bad name {}", py::repr_str(name))}));
        }
        let mut tl_name = name.to_string(); // runs are named after the timeline's own name, which may differ from the file's
        if let Ok(text) = std::fs::read_to_string(self.timelines_dir().join(format!("{}.json", name))) {
            if let Ok(Value::Object(doc)) = py::loads(&text) {
                if let Some(Value::String(n)) = doc.get("name") {
                    if name_ok(n) {
                        tl_name = n.clone();
                    }
                }
            }
        }
        let mut files = self.runs(&tl_name);
        if let Some(d) = self.data_dir() {
            let pv = d.join("runs").join(format!("{}.preview.csv", tl_name));
            if pv.is_file() {
                files.push(pv);
            }
        }
        let mut newest: Option<(PathBuf, f64)> = None;
        for f in files {
            let m = mtime(&f).unwrap_or(0.0);
            if newest.as_ref().map_or(true, |(_, best)| m > *best) {
                newest = Some((f, m));
            }
        }
        let newest = match newest {
            Some((f, _)) => f,
            None => return Ok(json!({"error": null, "points": null})),
        };
        let read = || -> Result<(Vec<f64>, Vec<f64>, Vec<f64>, f64), String> {
            let run = read_run(&newest)?;
            let jump = py::parse_float(run.get("tick_interval").unwrap_or("0.015"))? * 3500.0 * 2.5;
            Ok((run.floats("x")?, run.floats("y")?, run.floats("hspeed")?, jump))
        };
        let (xs, ys, sp, jump) = match read() {
            Ok(v) => v,
            Err(e) => return Ok(json!({"error": e, "points": null})),
        };
        let n = xs.len();
        let step = 1.max((n + THUMB_POINTS - 1) / THUMB_POINTS);
        let mut keep: Vec<usize> = (0..n).step_by(step).collect();
        if keep.last().map_or(false, |&k| k != n - 1) {
            keep.push(n - 1);
        }
        let mut points: Vec<Value> = Vec::new();
        let mut prev: Option<usize> = None;
        for i in keep {
            if let Some(p) = prev {
                let teleport = (p + 1..=i).any(|t| {
                    let (dx, dy) = (xs[t] - xs[t - 1], ys[t] - ys[t - 1]);
                    dx * dx + dy * dy > jump * jump
                });
                if teleport {
                    points.push(Value::Null);
                }
            }
            points.push(json!([py::num(py::round(xs[i], 1)), py::num(py::round(ys[i], 1)), py::num(py::round(sp[i], 1))]));
            prev = Some(i);
        }
        Ok(json!({"error": null, "file": file_name(&newest), "points": points}))
    }

    // ---- runs -------------------------------------------------------------

    pub fn runs(&self, name: &str) -> Vec<PathBuf> {
        match self.data_dir() {
            Some(d) if d.join("runs").is_dir() => find_runs(&d.join("runs"), name),
            _ => Vec::new(),
        }
    }

    fn load_run(&self, path: &Path) -> Api<Loaded> {
        let m = mtime(path).ok_or_else(|| crate::io_error(&std::io::Error::from(std::io::ErrorKind::NotFound), path))?;
        if let Some((cm, loaded)) = self.run_cache.lock().unwrap().get(path) {
            if *cm == m {
                return Ok(loaded.clone());
            }
        }
        let run = read_run(path)?;
        let rep = check_run(&run)?;
        let loaded = (Arc::new(run), Arc::new(rep));
        self.run_cache.lock().unwrap().insert(path.to_path_buf(), (m, loaded.clone()));
        Ok(loaded)
    }

    /// Latest run and the one before (ghost). A timeline with no runs of its own
    /// can name a baseline (e.g. the recording it was imported from).
    #[allow(clippy::type_complexity)]
    fn latest_runs(&self, name: &str, baseline: Option<&Value>) -> Api<(Option<Loaded>, Option<Arc<RunCsv>>, Vec<String>)> {
        let mut paths = self.runs(name);
        if paths.is_empty() {
            if let Some(Value::String(b)) = baseline {
                if name_ok(b) {
                    paths = self.runs(b);
                }
            }
        }
        let latest = match paths.last() {
            Some(p) => Some(self.load_run(p)?),
            None => None,
        };
        let ghost = if paths.len() >= 2 { Some(self.load_run(&paths[paths.len() - 2])?.0) } else { None };
        Ok((latest, ghost, paths.iter().map(|p| file_name(p)).collect()))
    }

    fn run_payload(run: Option<&RunCsv>, rep: Option<&CheckReport>) -> Api<Value> {
        let run = match run {
            Some(r) => r,
            None => return Ok(Value::Null),
        };
        let mut out = Map::new();
        out.insert("file".into(), run.path.as_ref().map_or(Value::Null, |p| s(&file_name(p))));
        out.insert(
            "mtime".into(),
            match run.path.as_ref().and_then(|p| mtime(p)) {
                Some(m) => py::num(m),
                None => Value::from(0),
            },
        );
        out.insert("status".into(), opt_s(run.get("status")));
        out.insert("hash".into(), opt_s(run.get("baked_hash")));
        out.insert("tick_interval".into(), py::num(py::parse_float(run.get("tick_interval").unwrap_or("0.015"))?));
        for c in ["x", "y", "z", "vx", "vy", "vz", "hspeed", "yaw"] {
            // yaw: what was played (auto-strafe ticks: the game's own aim)
            out.insert(c.into(), r4(&run.floats(c)?));
        }
        out.insert("on_ground".into(), ints(&run.ints("on_ground")?));
        out.insert("ducked".into(), ints(&run.ints("ducked")?));
        // for the instant prediction: the inputs played, base velocity (pushes), the movement settings
        out.insert("fwd".into(), r4(&run.floats("forwardmove")?));
        out.insert("side".into(), r4(&run.floats("sidemove")?));
        out.insert("buttons".into(), ints(&run.ints("buttons")?));
        for c in ["base_vx", "base_vy", "base_vz"] {
            out.insert(c.into(), r4(&run.floats(c)?));
        }
        out.insert("move".into(), move_settings(run));
        if let Some(rep) = rep {
            out.insert("kinds".into(), Value::Array(rep.kinds.iter().map(|k| s(k)).collect()));
        }
        Ok(Value::Object(out))
    }

    // ---- analysis of the current (possibly unsaved) timeline ---------------

    /// Ok(Err(..)) is a timeline error (shown in the editor), Err(..) a failed request.
    fn analyse(&self, doc: &Value, name: &str) -> Api<Res<Map<String, Value>>> {
        let tl = match self.parse(doc, name) {
            Ok(tl) => tl,
            Err(e) => return Ok(Err(e)),
        };
        let inputs = bake_inputs(&tl);
        let yaw_u = sample(&tl.yaw, tl.length);
        let mut preview = Map::new();
        preview.insert("yaw_u".into(), r4(&yaw_u));
        preview.insert("pitch".into(), r4(&inputs.iter().map(|ti| ti.pitch).collect::<Vec<_>>()));
        preview.insert("fwd".into(), floats(inputs.iter().map(|ti| ti.forwardmove)));
        preview.insert("side".into(), floats(inputs.iter().map(|ti| ti.sidemove)));
        preview.insert("buttons".into(), ints(&inputs.iter().map(|ti| ti.buttons).collect::<Vec<_>>()));
        preview.insert("view_pitch".into(), tl.view_pitch.as_ref().map_or(Value::Null, |k| r4(&sample(k, tl.length))));
        let (latest, _, _) = self.latest_runs(&tl.name, doc.get("_baseline"))?;
        let mut analysis = Map::new();
        for k in ["first_diff", "estimate", "best", "params"] {
            analysis.insert(k.into(), Value::Null);
        }
        if let Some((run, rep)) = latest {
            let p = &rep.params;
            let mut params = Map::new();
            for (k, v) in [("dt", p.dt), ("gravity", p.gravity), ("airaccelerate", p.airaccelerate), ("maxspeed", p.maxspeed), ("maxvelocity", p.maxvelocity)] {
                params.insert(k.into(), py::num(v));
            }
            analysis.insert("params".into(), Value::Object(params));
            let first = first_difference(&inputs, &run)?;
            analysis.insert("first_diff".into(), first.map_or(Value::Null, Value::from));
            if let Some(k) = first {
                let strafe: HashMap<i64, (f64, Option<f64>, Option<f64>, Option<f64>)> = tl
                    .strafe
                    .iter()
                    .flat_map(|r| (r.start..r.end).map(move |t| (t, (r.offset, r.heading, r.swing, r.gain))))
                    .collect();
                let est = estimate_free_flight(&run, &rep, k as usize, &inputs, 20000, &strafe)?;
                if !est.is_empty() {
                    let mut e = Map::new();
                    e.insert("tick".into(), ints(&est.iter().map(|x| x.0).collect::<Vec<_>>()));
                    e.insert("x".into(), r4(&est.iter().map(|x| x.1 .0).collect::<Vec<_>>()));
                    e.insert("y".into(), r4(&est.iter().map(|x| x.1 .1).collect::<Vec<_>>()));
                    e.insert("z".into(), r4(&est.iter().map(|x| x.1 .2).collect::<Vec<_>>()));
                    e.insert("hspeed".into(), r4(&est.iter().map(|x| py::hypot(x.2 .0, x.2 .1)).collect::<Vec<_>>()));
                    analysis.insert("estimate".into(), Value::Object(e));
                }
            }
            analysis.insert("best".into(), best_yaws(&run, &rep, &inputs, &yaw_u)?);
        }
        let text = render_baked(&tl, &inputs);
        let mut out = Map::new();
        out.insert("norm".into(), normalized(&tl));
        out.insert("preview".into(), Value::Object(preview));
        out.insert("analysis".into(), Value::Object(analysis));
        out.insert("warnings".into(), Value::Array(tl.warnings.iter().map(|w| s(w)).collect()));
        out.insert("hash".into(), s(&hash_of(&text)));
        Ok(Ok(out))
    }

    pub fn open(&self, name: &str) -> Api<Value> {
        let path = self.timeline_path(name).map_err(tl_err)?;
        let text = std::fs::read_to_string(&path).map_err(|e| crate::io_error(&e, &path))?;
        let doc: Value = py::loads(&text)?;
        let mut state = self.state(name, &doc)?;
        state.insert("doc".into(), doc);
        Ok(Value::Object(state))
    }

    pub fn state(&self, name: &str, doc: &Value) -> Api<Map<String, Value>> {
        let mut out = match self.analyse(doc, name)? {
            Ok(mut out) => {
                out.insert("error".into(), Value::Null);
                out
            }
            Err(e) => {
                let mut m = Map::new();
                m.insert("error".into(), s(&e.0));
                m
            }
        };
        let tl_name = out
            .get("norm")
            .and_then(|n| n.get("name"))
            .and_then(Value::as_str)
            .unwrap_or(name)
            .to_string();
        let (latest, ghost, files) = self.latest_runs(&tl_name, doc.get("_baseline"))?;
        out.insert(
            "run".into(),
            Self::run_payload(latest.as_ref().map(|l| &*l.0), latest.as_ref().map(|l| &*l.1))?,
        );
        out.insert("ghost".into(), Self::run_payload(ghost.as_deref(), None)?);
        out.insert("runs".into(), Value::Array(files.into_iter().map(Value::String).collect()));
        out.insert("pv".into(), self.preview_payload(&tl_name)?);
        Ok(out)
    }

    /// The latest fast engine preview of this timeline (runs/<name>.preview.csv).
    fn preview_payload(&self, name: &str) -> Api<Value> {
        let d = match self.data_dir() {
            Some(d) if name_ok(name) => d,
            _ => return Ok(Value::Null),
        };
        let path = d.join("runs").join(format!("{}.preview.csv", name));
        if !path.is_file() {
            return Ok(Value::Null);
        }
        let (run, rep) = self.load_run(&path)?;
        let mut out = Self::run_payload(Some(&run), Some(&rep))?;
        if let (Value::Object(o), Some(m)) = (&mut out, mtime(&path)) {
            o.insert("mtime".into(), py::num(m));
        }
        Ok(out)
    }

    pub fn update(&self, name: &str, doc: &Value, edits: &Value) -> Api<Value> {
        let new_doc = match apply_edits(doc, edits) {
            Ok(d) => d,
            Err(e) => return Ok(error_value(Some(&e.0))),
        };
        let mut out = match self.analyse(&new_doc, name)? {
            Ok(o) => o,
            Err(e) => return Ok(error_value(Some(&e.0))),
        };
        out.insert("error".into(), Value::Null);
        out.insert("doc".into(), new_doc);
        Ok(Value::Object(out))
    }

    /// A tick operation (insert, delete, length, turn, freeze, splice) on the
    /// current edits; the answer is an update plus the new normalised timeline.
    pub fn op(&self, name: &str, doc: &Value, edits: &Value, op: &Value) -> Api<Value> {
        let result: Api<Res<(Edits, String)>> = (|| {
            let op = match obj(op, "op") {
                Ok(o) => o,
                Err(e) => return Ok(Err(e)),
            };
            let kind = op.get("op").and_then(Value::as_str);
            let parsed = match Edits::from_json(edits) {
                Ok(e) => e,
                Err(e) => return Ok(Err(e)),
            };
            match kind {
                Some("freeze") => {
                    let tick = match editdoc::need(op, "tick").and_then(to_int) {
                        Ok(t) => t,
                        Err(e) => return Ok(Err(e)),
                    };
                    self.freeze(name, doc, edits, &parsed, tick)
                }
                Some("splice") => {
                    let rec = match editdoc::need(op, "recording") {
                        Ok(v) => to_str(v),
                        Err(e) => return Ok(Err(e)),
                    };
                    let at = match editdoc::need(op, "at").and_then(to_int) {
                        Ok(t) => t,
                        Err(e) => return Ok(Err(e)),
                    };
                    self.splice(name, doc, edits, &parsed, &rec, at)
                }
                _ => Ok(apply_op(&parsed, op)),
            }
        })();
        let (new_edits, message) = match result? {
            Ok(x) => x,
            Err(e) => return Ok(error_value(Some(&e.0))),
        };
        let mut out = self.update(name, doc, &new_edits.to_json())?;
        if let Value::Object(o) = &mut out {
            if o.get("error").map_or(true, Value::is_null) {
                o.insert("message".into(), s(&message));
            }
        }
        Ok(out)
    }

    /// Auto-strafe range at tick -> keyframes with the yaws the game played there,
    /// from a preview or run of exactly these inputs.
    fn freeze(&self, name: &str, doc: &Value, raw: &Value, edits: &Edits, tick: i64) -> Api<Res<(Edits, String)>> {
        let r = match strafe_at(edits, tick) {
            Some(r) => r.clone(),
            None => return Ok(Err(TlError(format!("freeze: tick {} isn't auto-strafe", tick)))),
        };
        let applied = match apply_edits(doc, raw) {
            Ok(d) => d,
            Err(e) => return Ok(Err(e)),
        };
        let current = match self.analyse(&applied, name)? {
            Ok(c) => c,
            Err(e) => return Ok(Err(e)),
        };
        let tl_name = current["norm"]["name"].as_str().unwrap_or("").to_string();
        let hash = current["hash"].as_str().unwrap_or("").to_string();
        let (from, to) = match (to_int(&r["from"]), to_int(&r["to"])) {
            (Ok(f), Ok(t)) => (f, t),
            (Err(e), _) | (_, Err(e)) => return Ok(Err(e)),
        };
        let mut paths = Vec::new();
        if let Some(d) = self.data_dir() {
            paths.push(d.join("runs").join(format!("{}.preview.csv", tl_name)));
            paths.extend(self.runs(&tl_name).into_iter().rev());
        }
        for path in paths {
            if !path.is_file() {
                continue;
            }
            let (run, _) = match self.load_run(&path) {
                Ok(x) => x,
                Err(e) => return Ok(Err(TlError(e))),
            };
            if run.get("baked_hash") != Some(hash.as_str()) || (run.rows.len() as i64) < to {
                continue;
            }
            let exact = run.has_column("yaw_bits");
            let yaws = match run.floats("yaw") {
                Ok(y) => y,
                Err(e) => return Ok(Err(TlError(e))),
            };
            let lo = (from.max(0) as usize).min(yaws.len());
            let hi = (to.max(0) as usize).clamp(lo, yaws.len());
            // a straight range: the A and D the game picked, from the run's sidemove
            let sides = if opt(&r, "heading").is_some() {
                match run.floats("sidemove") {
                    Ok(s) => Some(s[lo.min(s.len())..hi.min(s.len())].to_vec()),
                    Err(e) => return Ok(Err(TlError(e))),
                }
            } else {
                None
            };
            // a swing with a gain: how hard it pressed W, too
            let fwds = if opt(&r, "gain").is_some() {
                match run.floats("forwardmove") {
                    Ok(f) => Some(f[lo.min(f.len())..hi.min(f.len())].to_vec()),
                    Err(e) => return Ok(Err(TlError(e))),
                }
            } else {
                None
            };
            let new = match freeze_strafe(edits, from, to, &yaws[lo..hi], sides.as_deref(), fwds.as_deref()) {
                Ok(n) => n,
                Err(e) => return Ok(Err(e)),
            };
            let note = if exact { "" } else { " (from 6-decimal yaws: replay to check, the game's plugin is older than 0.7)" };
            return Ok(Ok((new, format!("froze ticks {}..{} into keyframes from {}{}", from, to - 1, file_name(&path), note))));
        }
        Ok(Err(TlError("freeze: no preview or run of these exact inputs yet; preview first (P), then freeze".into())))
    }

    /// A recording made from tick `at` of this timeline, joined on from that tick.
    fn splice(&self, name: &str, doc: &Value, raw: &Value, edits: &Edits, recording: &str, at: i64) -> Api<Res<(Edits, String)>> {
        let d = match self.data_dir() {
            Some(d) if name_ok(recording) => d,
            _ => return Ok(Err(TlError("splice: no such recording".into()))),
        };
        let path = d.join("baked").join(format!("{}.tas", recording));
        let text = std::fs::read_to_string(&path).map_err(|e| crate::io_error(&e, &path))?;
        let inner = || -> Res<(Edits, String)> {
            let rec: Vec<TickInput> = read_baked(&text)?.inputs;
            let tl = self.parse(&apply_edits(doc, raw)?, name)?;
            let auto: BTreeSet<i64> = tl.strafe.iter().flat_map(|r| r.start..r.end).collect();
            let new = splice_recording(edits, at, &rec, tl.move_speed, &bake_inputs(&tl), &auto, tl.start.angles[0])?;
            // check: from `at` on, the new timeline bakes to exactly what was recorded
            let baked = bake_inputs(&self.parse(&apply_edits(doc, &new.to_json())?, name)?);
            let a = at as usize;
            let key = |t: &TickInput| (py::float_to_bits(t.yaw), py::float_to_bits(t.pitch), t.forwardmove, t.sidemove, t.buttons);
            let differ = rec[a.min(rec.len())..].iter().zip(baked[a.min(baked.len())..].iter()).filter(|(x, y)| key(x) != key(y)).count();
            let live = rec.len() as i64 - at;
            let note = if differ == 0 { "exact".to_string() } else { format!("{} tick(s) differ from the recording", differ) };
            Ok((new, format!("joined your recording on from tick {}: {} live tick(s), {} in all ({})", at, live, rec.len(), note)))
        };
        Ok(inner())
    }

    /// Ask the game to play this (unsaved) document on you up to tick `at`, then record you.
    pub fn record(&self, name: &str, doc: &Value, at: i64) -> Api<Value> {
        if self.data_dir().is_none() {
            return Ok(error_value(Some(NO_GAME)));
        }
        let tl = match self.parse(doc, name) {
            Ok(tl) => tl,
            Err(e) => return Ok(error_value(Some(&e.0))),
        };
        if !(1 <= at && at <= tl.length) {
            return Ok(error_value(Some(&format!(
                "record from a tick between 1 and {} (from tick 0, use tas_record in the game)",
                tl.length
            ))));
        }
        if let Some(why) = self.plugin_too_old(&tl) {
            return Ok(error_value(Some(&why)));
        }
        if self.busy() {
            return Ok(error_value(Some(BUSY)));
        }
        self.send_request(&tl, &format!("{}.record", tl.name), &format!("record {}", at))
    }

    pub fn save(&self, name: &str, doc: &Value) -> Value {
        let _guard = self.lock.lock().unwrap();
        let saved = self.timeline_path(name).and_then(|p| save_document(&p, doc, &self.search_dirs()));
        if let Err(e) = saved {
            return error_value(Some(&e.0));
        }
        let mut m = Map::new();
        m.insert("error".into(), Value::Null);
        m.insert("text".into(), s(&format_json(doc, 0)));
        Value::Object(m)
    }

    // ---- running in the game -----------------------------------------------

    pub fn run(&self, name: &str, doc: &Value) -> Api<Value> {
        if self.data_dir().is_none() {
            return Ok(error_value(Some(NO_GAME)));
        }
        if self.busy() {
            return Ok(error_value(Some(BUSY)));
        }
        let saved = self.save(name, doc);
        if !saved["error"].is_null() {
            return Ok(saved);
        }
        let tl = self.parse(doc, name).map_err(tl_err)?;
        if let Some(why) = self.plugin_too_old(&tl) {
            return Ok(error_value(Some(&why)));
        }
        self.send_request(&tl, &tl.name, "run")
    }

    /// Save, bake and ask the game to render the run to video (render.rs made the request's lines: the movie's
    /// file and how). The plugin needs 0.10 or later.
    pub fn render(&self, name: &str, doc: &Value, action: &str) -> Api<Value> {
        if self.data_dir().is_none() {
            return Ok(error_value(Some(NO_GAME)));
        }
        if self.busy() {
            return Ok(error_value(Some(BUSY)));
        }
        let version = self.game_status().get("version").and_then(Value::as_str).unwrap_or("").to_string();
        let have: Vec<i64> = version.split('.').take(2).filter_map(|p| py::parse_int(p).ok()).collect();
        if have.len() == 2 && have < RENDER_PLUGIN.to_vec() {
            return Ok(error_value(Some(&format!(
                "the game's TASurf plugin is {}, which can't render (0.10 can): close the game and Launch it again from the \
                 Setup page, which installs the new one",
                version
            ))));
        }
        let saved = self.save(name, doc);
        if !saved["error"].is_null() {
            return Ok(saved);
        }
        let tl = self.parse(doc, name).map_err(tl_err)?;
        self.send_request(&tl, &tl.name, action)
    }

    /// Bake the current (unsaved) document and ask the game for a fast preview.
    pub fn preview(&self, name: &str, doc: &Value) -> Api<Value> {
        if self.data_dir().is_none() {
            return Ok(error_value(Some(NO_GAME)));
        }
        if self.busy() {
            return Ok(error_value(Some(BUSY)));
        }
        let tl = match self.parse(doc, name) {
            Ok(tl) => tl,
            Err(e) => return Ok(error_value(Some(&e.0))),
        };
        if let Some(why) = self.plugin_too_old(&tl) {
            return Ok(error_value(Some(&why)));
        }
        self.send_request(&tl, &format!("{}.preview", tl.name), "preview")
    }

    /// Why the game's plugin can't play this timeline: one with a view pitch needs 0.9 or
    /// later. None when it can, or when the game hasn't said which version it runs.
    fn plugin_too_old(&self, tl: &Timeline) -> Option<String> {
        let straight = tl.strafe.iter().any(|r| r.heading.is_some());
        let swing = tl.strafe.iter().any(|r| r.swing.is_some());
        let gain = tl.strafe.iter().any(|r| r.gain.is_some());
        if tl.view_pitch.is_none() && !straight && !tl.autobhop {
            return None;
        }
        let version = self.game_status().get("version").and_then(Value::as_str).unwrap_or("").to_string();
        let mut have = Vec::new();
        for part in version.split('.').take(2) {
            have.push(py::parse_int(part).ok()?);
        }
        if gain && have < GAIN_PLUGIN.to_vec() {
            return Some(format!(
                "the game's TASurf plugin is {}, which can't play a swing's gain (0.14 can): build and install it again \
                 (python -m tasurf build-plugin, then install; the desktop app's Launch does both), then type  sm plugins \
                 reload tasurf  in the game",
                version
            ));
        }
        if swing && have < SWING_PLUGIN.to_vec() {
            return Some(format!(
                "the game's TASurf plugin is {}, which can't play a straight auto-strafe's swing (0.13 can): build and \
                 install it again (python -m tasurf build-plugin, then install; the desktop app's Launch does both), then type  \
                 sm plugins reload tasurf  in the game",
                version
            ));
        }
        if tl.autobhop && have < AUTOBHOP_PLUGIN.to_vec() {
            return Some(format!(
                "the game's TASurf plugin is {}, which can't play auto bhop (0.12 can): build and install it again \
                 (python -m tasurf build-plugin, then install; the desktop app's Launch does both), then type  sm plugins \
                 reload tasurf  in the game",
                version
            ));
        }
        if straight && have < STRAIGHT_PLUGIN.to_vec() {
            return Some(format!(
                "the game's TASurf plugin is {}, which can't play straight auto-strafe (0.11 can): build and install it \
                 again (python -m tasurf build-plugin, then install; the desktop app's Launch does both), then type  sm \
                 plugins reload tasurf  in the game",
                version
            ));
        }
        if tl.view_pitch.is_none() || have >= VIEW_PITCH_PLUGIN.to_vec() {
            return None;
        }
        Some(format!(
            "the game's TASurf plugin is {}, which can't play a view pitch (0.9 can): build and install it again \
             (python -m tasurf build-plugin, then install), then type  sm plugins reload tasurf  in the game",
            version
        ))
    }

    /// A run request the game hasn't picked up yet. The plugin reads one request
    /// file, so a second one would replace the first. A pending preview may be
    /// replaced (a newer edit makes it stale anyway), and so may anything older
    /// than a minute (game closed?).
    fn busy(&self) -> bool {
        let pending = match self.data_dir() {
            Some(d) => d.join("requests").join("request.txt"),
            None => return false,
        };
        match (mtime(&pending), read_lossy(&pending)) {
            (Some(m), Ok(text)) => now() - m < 60.0 && !text.contains("\npreview\n"),
            _ => false,
        }
    }

    /// The camera replays show (camera.rs), from a finished run of exactly these inputs (the same hash: the preview
    /// the editor asks for after an edit, or a run), when the game's plugin can show it: smoothed on the auto-strafe
    /// ranges, or on the whole run (the timeline's camera_smoothing). None when it's off (a robotic look), with nothing
    /// to smooth, or before the game has played these inputs (the replay looks along the yaw played, as it always did).
    fn camera(&self, tl: &Timeline, digest: &str) -> Option<BTreeMap<usize, f64>> {
        let ranges: Vec<(i64, i64)> = match tl.camera_smoothing.as_str() {
            "all" => vec![(0, tl.length)],
            "strafe" => tl.strafe.iter().map(|r| (r.start, r.end)).collect(),
            _ => Vec::new(),
        };
        if ranges.is_empty() {
            return None;
        }
        let d = self.data_dir()?;
        let version = self.game_status().get("version").and_then(Value::as_str).unwrap_or("").to_string();
        let mut have = Vec::new();
        for part in version.split('.').take(2) {
            have.push(py::parse_int(part).ok()?);
        }
        if have < VIEW_YAW_PLUGIN.to_vec() {
            return None;
        }
        let runs = d.join("runs");
        let mut paths = vec![runs.join(format!("{}.preview.csv", tl.name))];
        paths.extend(self.runs(&tl.name).into_iter().rev());
        for path in paths {
            if !path.is_file() {
                continue;
            }
            let run = match self.load_run(&path) {
                Ok((run, _)) => run,
                Err(_) => continue,
            };
            let yaws = match run.floats("yaw") {
                Ok(y) => y,
                Err(_) => continue,
            };
            if run.get("baked_hash") == Some(digest) && run.get("status") == Some("complete") && yaws.len() as i64 >= tl.length {
                return Some(camera_yaws(&yaws[..tl.length.max(0) as usize], &ranges));
            }
        }
        None
    }

    fn send_request(&self, tl: &Timeline, baked_name: &str, action: &str) -> Api<Value> {
        let d = self.data_dir().ok_or_else(|| NO_GAME.to_string())?;
        let _guard = self.lock.lock().unwrap();
        let out = self.out_dir.join(format!("{}.tas", baked_name));
        // replays show auto-strafe's camera smoothed, but not a recording's lead-in: you take over from the yaw played
        let camera_for = |digest: &str| self.camera(tl, digest);
        bake_to_file(tl, &out, if action.starts_with("record") { None } else { Some(&camera_for) })?;
        let baked = d.join("baked");
        std::fs::create_dir_all(&baked).map_err(|e| crate::io_error(&e, &baked))?;
        let dest = baked.join(file_name(&out));
        std::fs::copy(&out, &dest).map_err(|e| crate::io_error(&e, &dest))?;
        let req_id = random_id();
        let requests = d.join("requests");
        std::fs::create_dir_all(&requests).map_err(|e| crate::io_error(&e, &requests))?;
        let pending = requests.join("request.txt");
        // a request the game hadn't taken yet is replaced
        if let Ok(old) = read_lossy(&pending) {
            let first = old.split('\n').next().unwrap_or("");
            if let Some(id) = first.strip_prefix("id ") {
                self.replaced.lock().unwrap().insert(py::strip(id).to_string());
            }
        }
        let tmp = requests.join("request.tmp");
        std::fs::write(&tmp, format!("id {}\nload {}\n{}\n", req_id, baked_name, action)).map_err(|e| crate::io_error(&e, &tmp))?;
        std::fs::rename(&tmp, &pending).map_err(|e| crate::io_error(&e, &pending))?;
        self.sent.lock().unwrap().insert(req_id.clone(), now());
        let mut m = Map::new();
        m.insert("error".into(), Value::Null);
        m.insert("id".into(), s(&req_id));
        Ok(Value::Object(m))
    }

    /// Ask the game to change to this map (the editor opened a timeline on another one): requests/changemap.txt,
    /// which the plugin takes while no run plays. One older than 30 seconds is dropped, so a game started later
    /// doesn't act on it. The plugin answers in requests/<id>.result as for a run.
    pub fn change_map(&self, map: Option<&Value>) -> Api<Value> {
        let d = match self.data_dir() {
            Some(d) => d,
            None => return Ok(error_value(Some(NO_GAME))),
        };
        let name = match map {
            Some(Value::String(s)) if name_ok(s) => s.clone(),
            other => {
                let shown = other.map_or("None".to_string(), py::repr_value);
                return Ok(error_value(Some(&format!("bad map name {}", shown))));
            }
        };
        let req_id = random_id();
        let requests = d.join("requests");
        std::fs::create_dir_all(&requests).map_err(|e| crate::io_error(&e, &requests))?;
        let _guard = self.lock.lock().unwrap();
        let tmp = requests.join("changemap.tmp");
        std::fs::write(&tmp, format!("id {}\nmap {}\n", req_id, name)).map_err(|e| crate::io_error(&e, &tmp))?;
        let dest = requests.join("changemap.txt");
        std::fs::rename(&tmp, &dest).map_err(|e| crate::io_error(&e, &dest))?;
        self.sent.lock().unwrap().insert(req_id.clone(), now());
        let mut m = Map::new();
        m.insert("error".into(), Value::Null);
        m.insert("id".into(), s(&req_id));
        Ok(Value::Object(m))
    }

    /// Tell the game which run CSVs to draw (requests/draw.txt, kept in place so a
    /// plugin reload or map change draws them again). cursor: the editor's selected
    /// tick, drawn red up to it and white for a second after it. files: changes
    /// whenever the CSVs do, so the game reads them again.
    pub fn draw(&self, body: &Map<String, Value>) -> Api<Value> {
        let d = match self.data_dir() {
            Some(d) => d,
            None => return Ok(error_value(Some("cstrike_dir is not set in tasurf.local.json"))),
        };
        let map_name = editdoc::need(body, "map").map_err(tl_err)?;
        let names = [Some(map_name), opt(body, "path"), opt(body, "ghost"), opt(body, "timeline")];
        for v in names.iter().flatten() {
            match v.as_str() {
                Some(t) if name_ok(t) => {}
                _ => return Ok(error_value(Some(&format!("bad name {}", py::repr_value(v))))),
            }
        }
        let cursor = opt(body, "cursor");
        if let Some(c) = cursor {
            let ok = match c {
                Value::Number(n) => n.as_i64().map_or(false, |i| i >= 0),
                Value::Bool(_) => true,
                _ => false,
            };
            if !ok {
                return Ok(error_value(Some(&format!("bad cursor {}", py::repr_value(c)))));
            }
        }
        let files = opt(body, "files");
        if let Some(f) = files {
            if !is_alnum(&to_str(f)) {
                return Ok(error_value(Some(&format!("bad files stamp {}", py::repr_value(f)))));
            }
        }
        let name_or_dash = |v: Option<&Value>| v.and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or("-").to_string();
        let files_text = match files {
            Some(f) if truthy(f) => to_str(f),
            _ => "-".to_string(),
        };
        let cursor_text = match cursor {
            None => "-".to_string(),
            Some(Value::Bool(b)) => if *b { "True".into() } else { "False".into() },
            Some(c) => to_str(c),
        };
        let mut lines = vec![
            format!("id {}", random_id()),
            format!("map {}", map_name.as_str().unwrap_or("")),
            format!("path {}", name_or_dash(opt(body, "path"))),
            format!("ghost {}", name_or_dash(opt(body, "ghost"))),
            format!("timeline {}", name_or_dash(opt(body, "timeline"))),
            format!("cursor {}", cursor_text),
            format!("files {}", files_text),
        ];
        if let Some(b) = opt(body, "box") {
            lines.push(format!("box {}", if truthy(b) { 1 } else { 0 })); // the player's box at the cursor (Visuals toolbox)
        }
        let requests = d.join("requests");
        std::fs::create_dir_all(&requests).map_err(|e| crate::io_error(&e, &requests))?;
        let _guard = self.lock.lock().unwrap();
        let tmp = requests.join("draw.tmp");
        std::fs::write(&tmp, lines.join("\n") + "\n").map_err(|e| crate::io_error(&e, &tmp))?;
        let dest = requests.join("draw.txt");
        std::fs::rename(&tmp, &dest).map_err(|e| crate::io_error(&e, &dest))?;
        Ok(error_value(None))
    }

    /// What the plugin last reported in requests/heartbeat.txt (written about once a second).
    pub fn game_status(&self) -> Value {
        let mut out = Map::new();
        let d = match self.data_dir() {
            Some(d) => d,
            None => {
                out.insert("ok".into(), Value::Bool(false));
                out.insert("detail".into(), s("cstrike_dir is not set in tasurf.local.json"));
                return Value::Object(out);
            }
        };
        let beat = d.join("requests").join("heartbeat.txt");
        let (m, text) = match (beat.is_file(), mtime(&beat), read_lossy(&beat)) {
            (true, Some(m), Ok(t)) => (m, t),
            _ => {
                out.insert("ok".into(), Value::Bool(false));
                out.insert(
                    "detail".into(),
                    s("the game hasn't reported in: start CS:S and load a map. If it's running, the loaded plugin is too old: \
                       type  sm plugins reload tasurf  in its console"),
                );
                return Value::Object(out);
            }
        };
        let age = now() - m;
        let fields = fields_of(&text);
        let f = |k: &str| field(&fields, k);
        for k in ["version", "map", "player", "state", "mode", "bot"] {
            out.insert(k.into(), opt_s(f(k)));
        }
        out.insert("age".into(), py::num(py::round(age, 1)));
        let (ok, detail) = if age > 5.0 {
            (false, "the game isn't responding (closed, loading a map, or the plugin was unloaded); if it's running, \
                     type  sm plugins reload tasurf"
                .to_string())
        } else if let Some(ready) = f("ready") {
            // 0.6+: the plugin says whether it can play a request, and why not
            if ready != "yes" {
                (false, f("why").filter(|w| !w.is_empty()).unwrap_or("not ready").to_string())
            } else if f("state") != Some("idle") {
                (true, format!("busy ({}); a new preview replaces a running one", f("state").unwrap_or("None")))
            } else {
                (true, format!("ready: TASurf {} on {}", f("version").unwrap_or("None"), f("map").unwrap_or("None")))
            }
        } else if f("player") != Some("alive") {
            let d = match f("player") {
                Some("none") => "no player in the game yet",
                Some("spectator") => "you're spectating: join a team",
                Some("dead") => "you're dead: respawn",
                _ => "you need to be alive",
            };
            (false, d.to_string())
        } else if f("state") != Some("idle") {
            (true, format!("busy ({}); it will start when that's done", f("state").unwrap_or("None")))
        } else {
            (true, format!("ready: TASurf {} on {}", f("version").unwrap_or("None"), f("map").unwrap_or("None")))
        };
        out.insert("ok".into(), Value::Bool(ok));
        out.insert("detail".into(), s(&detail));
        Value::Object(out)
    }

    pub fn request_status(&self, req_id: &str) -> Value {
        let state = |st: &str, extra: &[(&str, &str)]| {
            let mut m = Map::new();
            m.insert("state".into(), s(st));
            for (k, v) in extra {
                m.insert((*k).into(), s(v));
            }
            Value::Object(m)
        };
        let d = match self.data_dir() {
            Some(d) if is_alnum(req_id) => d,
            _ => return state("error", &[("detail", "no such request")]),
        };
        let requests = d.join("requests");
        let result = requests.join(format!("{}.result", req_id));
        if result.is_file() {
            let mut m = Map::new();
            m.insert("state".into(), s("done"));
            for (k, v) in fields_of(&read_lossy(&result).unwrap_or_default()) {
                m.insert(k, Value::String(v));
            }
            return Value::Object(m);
        }
        let text = read_lossy(&requests.join("request.txt")).unwrap_or_default();
        if text.contains(&format!("id {}\n", req_id)) {
            let detail = self.game_status()["detail"].as_str().unwrap_or("").to_string();
            return state("waiting", &[("detail", &format!("waiting for the game: {}", detail))]);
        }
        let sent = self.sent.lock().unwrap().get(req_id).copied();
        if self.replaced.lock().unwrap().contains(req_id) {
            return state("done", &[("status", "superseded"), ("csv", "-"), ("detail", "a newer request replaced it")]);
        }
        let beat = requests.join("heartbeat.txt");
        let (beat_time, idle) = match (mtime(&beat), read_lossy(&beat)) {
            (Some(m), Ok(t)) => (m, t.contains("\nstate idle\n")),
            _ => (0.0, false),
        };
        if let Some(sent) = sent {
            if idle && beat_time > sent + 2.5 {
                // the game has been idle since well after this was sent, and never answered
                return state("done", &[("status", "lost"), ("csv", "-"), ("detail", "the game dropped the request; try again")]);
            }
        }
        state("running", &[])
    }

    // ---- the 3D view (see bspmap.rs); Err: 404 with the reason

    pub fn map_path(&self, name: &str) -> Result<PathBuf, String> {
        let cstrike = self.cfg().cstrike_dir();
        if !name_ok(name) {
            return Err(format!("bad map name {}", py::repr_str(name)));
        }
        let cstrike = cstrike.ok_or_else(|| "cstrike_dir is not set in tasurf.local.json, so the editor can't find the map".to_string())?;
        crate::bspmap::find_map(&cstrike, name).ok_or_else(|| format!("no {}.bsp in cstrike/maps or cstrike/download/maps", name))
    }

    fn map(&self, name: &str, textures: bool) -> Result<(Arc<Vec<u8>>, Option<Arc<Vec<u8>>>), String> {
        let path = self.map_path(name)?;
        let m = mtime(&path).unwrap_or(0.0);
        if let Some((p, t, mesh, textured)) = self.maps.lock().unwrap().as_ref() {
            if *p == path && *t == m && (textured.is_some() || !textures) {
                return Ok((mesh.clone(), textured.clone()));
            }
        }
        let (mesh, textured) = crate::bspmap::read_map(&path, textures)?;
        let (mesh, textured) = (Arc::new(mesh), textured.map(Arc::new));
        *self.maps.lock().unwrap() = Some((path, m, mesh.clone(), textured.clone())); // one map at a time: they can be tens of MB
        Ok((mesh, textured))
    }

    /// The map's geometry (TASM).
    pub fn map_mesh(&self, name: &str) -> Result<Arc<Vec<u8>>, String> {
        Ok(self.map(name, false)?.0)
    }

    /// What it takes to draw the map's geometry textured and lit (TAST).
    pub fn map_textures(&self, name: &str) -> Result<Arc<Vec<u8>>, String> {
        Ok(self.map(name, true)?.1.unwrap_or_default())
    }

    /// The map's triggers as polygons (TATR).
    pub fn map_triggers(&self, name: &str) -> Result<Vec<u8>, String> {
        crate::bspmap::read_triggers(&self.map_path(name)?)
    }

    /// What the player collides with (TACL), for the editor's instant prediction.
    pub fn map_clip(&self, name: &str) -> Result<Vec<u8>, String> {
        crate::bspmap::read_clip(&self.map_path(name)?)
    }

    /// The map's sky, 3D skybox camera and props (bspmap::read_props), for the 3D view.
    pub fn map_props(&self, name: &str) -> Result<Value, String> {
        crate::bspmap::read_props(&self.map_path(name)?)
    }

    /// A material or texture (materials/....vmt or .vtf), or a model's file (models/....mdl, .vvd or .vtx), as
    /// the game finds it on the map.
    pub fn map_file(&self, name: &str, rel: &str) -> Result<Vec<u8>, String> {
        let path = self.map_path(name)?;
        let files = {
            let mut f = self.files.lock().unwrap();
            f.get_or_insert_with(|| Arc::new(crate::gamefiles::GameFiles::new(&self.cfg().cstrike_dir().unwrap_or_default()))).clone()
        };
        files.read(&path, rel)
    }

    pub fn import_recording(&self, recording: &str, name: &str) -> Value {
        let d = match self.data_dir() {
            Some(d) => d,
            None => return error_value(Some("cstrike_dir is not set in tasurf.local.json")),
        };
        if !name_ok(recording) || !name_ok(name) {
            return error_value(Some("names may only use letters, digits, '_', '-' and '.'"));
        }
        let src = d.join("baked").join(format!("{}.tas", recording));
        let dest = match self.timeline_path(name) {
            Ok(p) => p,
            Err(e) => return error_value(Some(&e.0)),
        };
        if dest.exists() {
            return error_value(Some(&format!("timelines/{}.json already exists; pick another name", name)));
        }
        match editdoc::import_recording(&src, &dest, name, 0.0) {
            Err(e) => error_value(Some(&e.0)),
            Ok(report) => {
                let mut m = Map::new();
                m.insert("error".into(), Value::Null);
                m.insert("report".into(), Value::Object(report));
                m.insert("name".into(), s(name));
                Value::Object(m)
            }
        }
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(t) => !t.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

pub fn stem(p: &Path) -> String {
    p.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// dir.glob("*.ext") (case-insensitive on Windows, as pathlib is there).
pub fn glob_files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let suffix = format!(".{}", ext);
            let ok = if cfg!(windows) { name.to_lowercase().ends_with(&suffix) } else { name.ends_with(&suffix) };
            if ok {
                out.push(dir.join(name));
            }
        }
    }
    out
}

fn glob_stems(dir: &Path, ext: &str) -> Vec<String> {
    glob_files(dir, ext).iter().map(|p| stem(p)).collect()
}

/// A start file: its JSON and the parsed start (errors as Python words them).
fn read_start(path: &Path) -> Result<(Value, StartState), String> {
    let text = std::fs::read_to_string(path).map_err(|e| crate::io_error(&e, path))?;
    let obj = py::loads(&text)?;
    let ctx = format!("start file {}", file_name(path));
    let st = parse_start(&obj, &ctx, &path.display().to_string()).map_err(tl_err)?;
    Ok((obj, st))
}

/// The settings the player moves with in this run (from its header), or null when it lacks some.
fn move_settings(run: &RunCsv) -> Value {
    let inner = || -> Result<Value, String> {
        let (p, _) = params_from_run(run)?;
        let mut m = Map::new();
        for (k, v) in [("dt", p.dt), ("gravity", p.gravity), ("airaccelerate", p.airaccelerate), ("maxspeed", p.maxspeed), ("maxvelocity", p.maxvelocity)] {
            m.insert(k.into(), py::num(v));
        }
        for (k, c) in [("accelerate", "sv_accelerate"), ("friction", "sv_friction"), ("stopspeed", "sv_stopspeed"), ("stepsize", "sv_stepsize"), ("bounce", "sv_bounce")] {
            m.insert(k.into(), py::num(header_float(run, &format!("cvar.{}", c))?));
        }
        // 0: a jump cuts the speed to 1.1 x maxspeed; a game without it: the page assumes no cut
        if let Ok(v) = header_float(run, "cvar.sv_enablebunnyhopping") {
            m.insert("bunnyhop".into(), py::num(v));
        }
        Ok(Value::Object(m))
    };
    inner().unwrap_or(Value::Null)
}

/// (pitch, yaw) of a start state, as the exact float32 values when it has them.
fn exact_angles(st: &StartState) -> (f64, f64) {
    match &st.angles_bits {
        Some(b) => (py::bits_to_float(&b[0]).unwrap_or(st.angles[0]), py::bits_to_float(&b[1]).unwrap_or(st.angles[1])),
        None => (st.angles[0], st.angles[1]),
    }
}

/// Map and length from the start of a recording's baked file.
fn recording_info_of(name: &str, head: &str) -> Value {
    let mut info = Map::new();
    info.insert("name".into(), s(name));
    info.insert("map".into(), Value::Null);
    info.insert("ticks".into(), Value::Null);
    for line in py::splitlines(head) {
        if let Some(m) = line.strip_prefix("@map ") {
            info.insert("map".into(), s(py::strip(m)));
        } else if let Some(t) = line.strip_prefix("@ticks ") {
            if let Ok(n) = py::parse_int(t) {
                info.insert("ticks".into(), Value::from(n));
            }
        }
    }
    Value::Object(info)
}

/// First tick whose input differs from what the run processed (None if the same).
fn first_difference(inputs: &[TickInput], run: &RunCsv) -> Api<Option<i64>> {
    let (yaw, pitch) = (run.floats("yaw")?, run.floats("pitch")?);
    let (fwd, side, btn) = (run.floats("forwardmove")?, run.floats("sidemove")?, run.ints("buttons")?);
    let n = inputs.len().min(run.rows.len());
    for t in 0..n {
        let ti = &inputs[t];
        let dyaw = (py::fmod(ti.yaw - yaw[t] + 180.0, 360.0) - 180.0).abs();
        if dyaw > 1e-5
            || (ti.pitch - pitch[t]).abs() > 1e-5
            || (ti.forwardmove - fwd[t]).abs() > 1e-4
            || (ti.sidemove - side[t]).abs() > 1e-4
            || ti.buttons != btn[t]
        {
            return Ok(Some(t as i64));
        }
    }
    Ok(if inputs.len() == run.rows.len() { None } else { Some(n as i64) })
}

/// For each tick airborne in the run: the yaw giving the most speed for the held
/// keys, from the run's velocity before that tick.
fn best_yaws(run: &RunCsv, rep: &CheckReport, inputs: &[TickInput], yaw_u: &[f64]) -> Api<Value> {
    let (vx, vy, ground) = (run.floats("vx")?, run.floats("vy")?, run.ints("on_ground")?);
    let (mut ticks, mut best, mut gain) = (Vec::new(), Vec::new(), Vec::new());
    for t in 1..inputs.len().min(run.rows.len()) {
        let ti = &inputs[t];
        // ducked air ticks too: ducking crops wish speed on the ground only, and air
        // acceleration is capped at 30 either way
        if ground[t - 1] != 0 || ground[t] != 0 {
            continue;
        }
        if let Some((b, s_best, s_cur)) = best_yaw(vx[t - 1], vy[t - 1], ti.forwardmove, ti.sidemove, &rep.params, yaw_u[t]) {
            ticks.push(t as i64);
            best.push(py::num(py::round(b, 4)));
            gain.push(py::num(py::round(s_best - s_cur, 5)));
        }
    }
    let mut m = Map::new();
    m.insert("tick".into(), ints(&ticks));
    m.insert("yaw".into(), Value::Array(best));
    m.insert("gain".into(), Value::Array(gain));
    Ok(Value::Object(m))
}

// --------------------------------------------------------------------------
// HTTP

struct Request {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 {
            break;
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body)?;
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.clone(), String::new()),
    };
    let path = path.split('#').next().unwrap_or("").to_string();
    Ok(Some(Request { method, path, query, body }))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// {k: v[0] for k, v in parse_qs(query).items()} (blank values dropped, as parse_qs does).
fn parse_query(q: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for part in q.split(['&', ';']) {
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        let (k, v) = (percent_decode(k), percent_decode(v));
        if !v.is_empty() {
            out.entry(k).or_insert(v);
        }
    }
    out
}

fn respond(stream: &mut TcpStream, status: u16, body: &[u8], ctype: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        501 => "Not Implemented",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.0 {} {}\r\nServer: TASurf/{}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        status,
        reason,
        env!("CARGO_PKG_VERSION"),
        ctype,
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn json_response(stream: &mut TcpStream, v: &Value, status: u16) -> std::io::Result<()> {
    respond(stream, status, py::dumps(v, true).as_bytes(), "application/json")
}

fn body_str<'a>(body: &'a Map<String, Value>, key: &str) -> Api<&'a str> {
    let v = editdoc::need(body, key).map_err(tl_err)?;
    v.as_str().ok_or_else(|| format!("expected string or bytes-like object, got '{}'", py::type_name(v)))
}

fn body_val<'a>(body: &'a Map<String, Value>, key: &str) -> Api<&'a Value> {
    editdoc::need(body, key).map_err(tl_err)
}

fn index_html() -> Vec<u8> {
    let path = config::repo_root().join("tasurf").join("editor_static").join("index.html");
    std::fs::read(path).unwrap_or_else(|_| INDEX_FALLBACK.as_bytes().to_vec())
}

/// None: 404.
fn handle_get(ed: &Editor, path: &str, query: &str) -> Option<Api<Value>> {
    let q = parse_query(query);
    let need = |k: &str| q.get(k).cloned().ok_or_else(|| py::repr_str(k));
    Some(match path {
        "/api/list" => Ok(ed.list()),
        "/api/open" => need("name").and_then(|n| ed.open(&n)),
        "/api/thumb" => need("name").and_then(|n| ed.thumb(&n)),
        "/api/runs" => need("name").map(|n| {
            let mut m = Map::new();
            m.insert("runs".into(), Value::Array(ed.runs(&n).iter().map(|p| s(&file_name(p))).collect()));
            Value::Object(m)
        }),
        "/api/request" => Ok(ed.request_status(q.get("id").map(String::as_str).unwrap_or(""))),
        "/api/game" => Ok(ed.game_status()),
        "/api/starts" => Ok(ed.starts()),
        "/api/setup" => Ok(crate::launch::status()), // the desktop app: the Setup page's game & folders
        "/api/render" => Ok(crate::render::status()), // the desktop app: render settings, the render under way
        "/api/fonts" => Ok(json!({"fonts": crate::render::fonts()})), // for the speed overlay's font
        _ => return None,
    })
}

fn handle_post(ed: &Editor, path: &str, raw: &[u8]) -> Option<Api<Value>> {
    let parsed: Api<Value> = if raw.is_empty() {
        Ok(Value::Object(Map::new()))
    } else {
        py::loads(&String::from_utf8_lossy(raw))
    };
    let known = ["/api/update", "/api/op", "/api/state", "/api/save", "/api/run", "/api/preview", "/api/record", "/api/draw", "/api/changemap", "/api/import", "/api/create",
        "/api/setup", "/api/launch", "/api/browse", "/api/render", "/api/render/settings", "/api/render/open"];
    if !known.contains(&path) {
        return match parsed {
            Err(e) => Some(Err(e)),
            Ok(_) => None,
        };
    }
    let result = (|| -> Api<Value> {
        let body_v = parsed?;
        let body = body_v.as_object().ok_or_else(|| "the request body must be a JSON object".to_string())?;
        match path {
            "/api/update" => {
                let empty = Value::Object(Map::new());
                let edits = body.get("edits").unwrap_or(&empty);
                ed.update(body_str(body, "name")?, body_val(body, "doc")?, edits)
            }
            "/api/op" => ed.op(body_str(body, "name")?, body_val(body, "doc")?, body_val(body, "edits")?, body_val(body, "op")?),
            "/api/state" => {
                let doc = body_val(body, "doc")?;
                let mut out = ed.state(body_str(body, "name")?, doc)?;
                out.insert("doc".into(), doc.clone());
                Ok(Value::Object(out))
            }
            "/api/save" => Ok(ed.save(body_str(body, "name")?, body_val(body, "doc")?)),
            "/api/run" => ed.run(body_str(body, "name")?, body_val(body, "doc")?),
            "/api/preview" => ed.preview(body_str(body, "name")?, body_val(body, "doc")?),
            "/api/record" => {
                let at = to_int(body_val(body, "at")?).map_err(tl_err)?;
                ed.record(body_str(body, "name")?, body_val(body, "doc")?, at)
            }
            "/api/draw" => ed.draw(body),
            "/api/changemap" => ed.change_map(body.get("map")),
            "/api/import" => Ok(ed.import_recording(body_str(body, "recording")?, body_str(body, "name")?)),
            // the desktop app's Setup page: settings (the editor follows the project and game folders), launching
            "/api/setup" | "/api/launch" => {
                let r = if path == "/api/setup" { crate::launch::save(&body_v).map(|_| ()) } else { crate::launch::launch(&body_v).map(|_| ()) };
                ed.reload(crate::launch::timelines_dir(), config::load());
                r.map(|_| crate::launch::status())
            }
            // the desktop app: rendering runs to video with SVR (render.rs)
            "/api/render/settings" => crate::render::save(body).map(|_| crate::render::status()),
            "/api/render/open" => crate::render::open(
                body.get("what").and_then(Value::as_str).unwrap_or("file"),
                body.get("name").and_then(Value::as_str),
            )
            .map(|_| json!({"error": null})),
            "/api/render" => {
                let name = body_str(body, "name")?;
                let video = body.get("video").and_then(Value::as_str).unwrap_or("");
                let (file, action) = match crate::render::prepare(name, video) {
                    Ok(p) => p,
                    Err(e) => return Ok(error_value(Some(&e))),
                };
                let out = ed.render(name, body_val(body, "doc")?, &action)?;
                if let (Some(id), Some(d)) = (out.get("id").and_then(Value::as_str), ed.data_dir()) {
                    crate::render::watch(id.to_string(), file.clone(), d);
                }
                let mut out = out;
                if let Value::Object(m) = &mut out {
                    m.insert("file".into(), Value::String(file));
                }
                Ok(out)
            }
            "/api/browse" => {
                let title = body.get("title").and_then(Value::as_str).unwrap_or("Pick a folder");
                let start = body.get("start").and_then(Value::as_str).unwrap_or("");
                Ok(json!({"path": crate::launch::browse(title, start)}))
            }
            "/api/create" => Ok(ed.create(body_val(body, "name")?, opt(body, "start"), opt(body, "copy"), opt(body, "length"), opt(body, "map"))),
            _ => unreachable!(),
        }
    })();
    Some(result)
}

fn handle(ed: &Editor, mut stream: TcpStream) -> std::io::Result<()> {
    let req = match read_request(&mut stream)? {
        Some(r) => r,
        None => return Ok(()),
    };
    let result = match req.method.as_str() {
        "GET" if req.path == "/view3d.js" || req.path == "/predict.js" => {
            // 3D view, the instant prediction
            let path = config::repo_root().join("tasurf").join("editor_static").join(&req.path[1..]);
            let fallback = if req.path == "/view3d.js" { VIEW3D_FALLBACK } else { PREDICT_FALLBACK };
            let body = std::fs::read(path).unwrap_or_else(|_| fallback.as_bytes().to_vec());
            return respond(&mut stream, 200, &body, "text/javascript; charset=utf-8");
        }
        "GET" if ["/api/map", "/api/maptex", "/api/maptrig", "/api/mapclip", "/api/mapfile"].contains(&req.path.as_str()) => {
            // 3D view
            let q = parse_query(&req.query);
            let name = q.get("name").cloned().unwrap_or_default();
            let body = match req.path.as_str() {
                "/api/map" => ed.map_mesh(&name),
                "/api/maptex" => ed.map_textures(&name),
                "/api/maptrig" => ed.map_triggers(&name).map(Arc::new),
                "/api/mapclip" => ed.map_clip(&name).map(Arc::new),
                _ => ed.map_file(&name, &q.get("path").cloned().unwrap_or_default()).map(Arc::new),
            };
            return match body {
                Ok(body) => respond(&mut stream, 200, &body, "application/octet-stream"),
                Err(e) => json_response(&mut stream, &error_value(Some(&e)), 404),
            };
        }
        "GET" if req.path == "/api/mapprops" => {
            // 3D view: the sky and the props
            let q = parse_query(&req.query);
            return match ed.map_props(&q.get("name").cloned().unwrap_or_default()) {
                Ok(v) => json_response(&mut stream, &v, 200),
                Err(e) => json_response(&mut stream, &error_value(Some(&e)), 404),
            };
        }
        "GET" if req.path == "/" || req.path == "/index.html" => {
            return respond(&mut stream, 200, &index_html(), "text/html; charset=utf-8");
        }
        "GET" => handle_get(ed, &req.path, &req.query),
        "POST" => handle_post(ed, &req.path, &req.body),
        _ => return respond(&mut stream, 501, b"unsupported method", "text/plain"),
    };
    match result {
        None => respond(&mut stream, 404, b"not found", "text/plain"),
        Some(Ok(v)) => json_response(&mut stream, &v, 200),
        Some(Err(e)) => json_response(&mut stream, &error_value(Some(&e)), 400),
    }
}

pub fn make_server(port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port))
}

pub fn serve_on(listener: TcpListener, editor: Arc<Editor>) {
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(_) => continue,
        };
        let ed = editor.clone();
        std::thread::spawn(move || {
            let _ = handle(&ed, stream);
        });
    }
}

/// Open a URL in the system browser.
pub fn open_browser(url: &str) {
    let result = if cfg!(windows) {
        std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    let _ = result;
}

pub fn serve(timeline: Option<&str>, port: u16, browser: bool) -> Result<(), String> {
    let cfg = config::load();
    let editor = Arc::new(Editor::new(config::repo_root().join("timelines"), cfg, None));
    let listener = make_server(port).map_err(|e| e.to_string())?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?.port();
    let mut url = format!("http://127.0.0.1:{}/", bound);
    if let Some(t) = timeline {
        url.push('#');
        url.push_str(&stem(&config::pypath(t)));
    }
    println!("TASurf editor at {}  (Ctrl+C to stop)", url);
    if editor.data_dir().is_none() {
        println!("note: cstrike_dir is not set in tasurf.local.json; Run and recordings are disabled");
    }
    if browser {
        open_browser(&url);
    }
    serve_on(listener, editor);
    Ok(())
}

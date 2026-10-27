//! Load and validate timeline JSON files (tasurf/timeline.py). Same rules and the
//! same error messages: they name the JSON location (`keys[3].to`) because the file
//! is edited by hand. Any object key that starts with an underscore is a comment.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::buttons;
use crate::config::native;
use crate::interp::{Interp, Key, INTERP_MODES};
use crate::py;

pub const FORMAT_NAME: &str = "tasurf-timeline";
pub const FORMAT_VERSION: i64 = 1;
pub const START_FORMAT_NAME: &str = "tasurf-start";

#[derive(Debug, Clone)]
pub struct TlError(pub String);

impl fmt::Display for TlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TlError {
    fn from(s: String) -> Self {
        TlError(s)
    }
}

pub type Res<T> = Result<T, TlError>;

pub fn err<T>(msg: impl Into<String>) -> Res<T> {
    Err(TlError(msg.into()))
}

/// `^[A-Za-z0-9_.\-]+$`
pub fn name_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

fn setting_name_ok(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn hex_ok(s: &str) -> bool {
    (1..=8).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

#[derive(Clone, Debug)]
pub struct StartState {
    pub origin: [f64; 3],
    pub angles: [f64; 3],
    pub velocity: [f64; 3],
    // exact float32 bit patterns (8 hex digits) when the start was captured in game
    pub origin_bits: Option<[String; 3]>,
    pub angles_bits: Option<[String; 3]>,
    pub velocity_bits: Option<[String; 3]>,
    pub base_velocity: [f64; 3],
    pub base_velocity_flag: bool,
    pub on_ground: String, // "yes", "no" or "auto"
    pub ground_entity: i64,
    pub ducked: bool,
    pub view_offset_z: Option<f64>,
    pub gravity: f64,
    pub lagged_movement: f64,
    // CS:S slowdowns, always reset: 1.0 = no damage slowdown, 0.0 = rested (no jump/landing slowdown)
    pub velocity_modifier: f64,
    pub stamina: f64,
    pub health: Option<i64>,
    pub maxspeed: Option<f64>,
    pub map: Option<String>,
    pub source: String,
}

#[derive(Clone, Debug)]
pub struct KeyRange {
    pub key: String, // canonical key name
    pub start: i64,  // first tick held (inclusive)
    pub end: i64,    // first tick NOT held (exclusive)
    pub value: Option<f64>,
    pub index: usize,
}

#[derive(Clone, Debug)]
pub struct StrafeRange {
    pub start: i64,
    pub end: i64,
    pub offset: f64,
    pub index: usize,
    pub heading: Option<f64>, // straight: the direction to keep going (world yaw); the plugin picks A or D
    pub swing: Option<f64>,   // straight: keep a side until the direction is this many degrees past the heading
    pub gain: Option<f64>,    // with a swing: keep this much of the most speed gain (75 to 100 percent), smoothly
}

/// A turn the editor's Turn made as a curve. The yaw keyframes are what plays; this only names the stretch they
/// make, so the editor can show it as one block, edit it again or remove it. Not baked.
#[derive(Clone, Debug)]
pub struct TurnBlock {
    pub start: i64, // first tick (inclusive)
    pub end: i64,   // first tick after it (exclusive)
    pub rate: f64,  // degrees per tick, positive turns left
    pub shape: String, // "linear" (constant speed) or "smooth" (ease in and out)
}

#[derive(Clone, Debug)]
pub struct Timeline {
    pub name: String,
    pub map: String,
    pub length: i64,
    pub settings: Vec<(String, Value)>,
    pub start: StartState,
    pub move_speed: f64,
    pub yaw: Vec<Key>,
    pub pitch: Option<Vec<Key>>,
    pub keys: Vec<KeyRange>,
    #[allow(dead_code)]
    pub path: Option<PathBuf>,
    pub strafe: Vec<StrafeRange>,
    /// pitch shown on replays only (the replay bot's eyes); the run's movement uses `pitch`
    pub view_pitch: Option<Vec<Key>>,
    /// a held jump jumps again on landing (the plugin lets it go in the air)
    pub autobhop: bool,
    /// what replays show (@view_yaw, camera.rs): "strafe" auto-strafe's yaw smoothed, "all" the whole run's, "off" the
    /// yaw played, zigzag and all
    pub camera_smoothing: String,
    pub turns: Vec<TurnBlock>, // the editor's curve turns, by name only
    /// the editor's: ticks [from, to) whose A and D were set by hand (A and D following the yaw leave them)
    pub ad_by_hand: Vec<(i64, i64)>,
    pub warnings: Vec<String>,
}

// --------------------------------------------------------------------------
// small typed accessors

pub fn is_num(v: &Value) -> bool {
    v.is_number()
}

pub fn num(v: &Value, ctx: &str) -> Res<f64> {
    match v.as_f64() {
        Some(x) if v.is_number() && x.is_finite() => Ok(x),
        _ => err(format!("{}: expected a number, got {}", ctx, py::repr_value(v))),
    }
}

pub fn int(v: &Value, ctx: &str) -> Res<i64> {
    let bad = || err(format!("{}: expected an integer, got {}", ctx, py::repr_value(v)));
    match v {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if let Some(f) = n.as_f64() {
                if f.is_finite() && f == f.trunc() {
                    Ok(f as i64)
                } else {
                    bad()
                }
            } else {
                bad()
            }
        }
        _ => bad(),
    }
}

fn boolean(v: &Value, ctx: &str) -> Res<bool> {
    v.as_bool().map_or_else(|| err(format!("{}: expected true or false, got {}", ctx, py::repr_value(v))), Ok)
}

fn string(v: &Value, ctx: &str) -> Res<String> {
    match v.as_str() {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => err(format!("{}: expected a non-empty string, got {}", ctx, py::repr_value(v))),
    }
}

fn vec3(v: &Value, ctx: &str) -> Res<[f64; 3]> {
    match v.as_array() {
        Some(a) if a.len() == 3 => {
            let mut out = [0.0; 3];
            for (i, x) in a.iter().enumerate() {
                out[i] = num(x, &format!("{}[{}]", ctx, i))?;
            }
            Ok(out)
        }
        _ => err(format!("{}: expected a list of 3 numbers, got {}", ctx, py::repr_value(v))),
    }
}

fn bits3(v: &Value, ctx: &str) -> Res<[String; 3]> {
    let a = match v.as_array() {
        Some(a) if a.len() == 3 => a,
        _ => return err(format!("{}: expected a list of 3 hex strings, got {}", ctx, py::repr_value(v))),
    };
    let mut out: [String; 3] = Default::default();
    for (i, x) in a.iter().enumerate() {
        match x.as_str() {
            Some(s) if hex_ok(s) => out[i] = format!("{:0>8}", s.to_uppercase()),
            _ => return err(format!("{}[{}]: expected up to 8 hex digits, got {}", ctx, i, py::repr_value(x))),
        }
    }
    Ok(out)
}

/// A list entry like {"_comment": "..."} that only holds comments.
pub fn is_comment(v: &Value) -> bool {
    matches!(v, Value::Object(o) if !o.is_empty() && o.keys().all(|k| k.starts_with('_')))
}

pub fn fields(obj: &Value, ctx: &str, allowed: &[&str], required: &[&str]) -> Res<Map<String, Value>> {
    let o = match obj.as_object() {
        Some(o) => o,
        None => return err(format!("{}: expected an object, got {}", ctx, py::type_name(obj))),
    };
    let clean: Map<String, Value> = o.iter().filter(|(k, _)| !k.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect();
    let mut unknown: Vec<&String> = clean.keys().filter(|k| !allowed.contains(&k.as_str())).collect();
    unknown.sort();
    unknown.dedup();
    if !unknown.is_empty() {
        let mut allowed_sorted: Vec<&str> = allowed.to_vec();
        allowed_sorted.sort();
        return err(format!(
            "{}: unknown field(s) {} (allowed: {}; prefix a key with _ to comment it out)",
            ctx,
            unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            allowed_sorted.join(", ")
        ));
    }
    let mut missing: Vec<&str> = required.iter().copied().filter(|r| !clean.contains_key(*r)).collect();
    missing.sort();
    if !missing.is_empty() {
        return err(format!("{}: missing required field(s) {}", ctx, missing.join(", ")));
    }
    Ok(clean)
}

// --------------------------------------------------------------------------
// start state

const START_FIELDS: [&str; 21] = [
    "format", "version", "map", "origin", "origin_bits", "angles", "angles_bits", "velocity", "velocity_bits",
    "base_velocity", "base_velocity_flag", "on_ground", "ground_entity", "ducked", "view_offset_z", "gravity",
    "lagged_movement", "velocity_modifier", "stamina", "health", "maxspeed",
];

fn check_bits(values: &[f64; 3], bits: &[String; 3], ctx: &str) -> Res<()> {
    for i in 0..3 {
        let exact = py::bits_to_float(&bits[i]).map_err(TlError)?;
        let v = values[i];
        if !exact.is_finite() || (exact - v).abs() > 1e-4 + 1e-6 * v.abs() {
            return err(format!(
                "{}[{}]: decimal value {} does not match its _bits value {} (= {}). If you edited the decimal by hand, delete the matching *_bits entry.",
                ctx, i, py::repr(v), bits[i], py::repr(exact)
            ));
        }
    }
    Ok(())
}

pub fn parse_start(obj: &Value, ctx: &str, source: &str) -> Res<StartState> {
    let d = fields(obj, ctx, &START_FIELDS, &["origin", "angles"])?;
    if let Some(f) = d.get("format") {
        if f.as_str() != Some(START_FORMAT_NAME) {
            return err(format!("{}.format: expected {}, got {}", ctx, py::repr_str(START_FORMAT_NAME), py::repr_value(f)));
        }
    }
    if let Some(v) = d.get("version") {
        if !value_equals_int(v, 1) {
            return err(format!("{}.version: unsupported version {}", ctx, py::repr_value(v)));
        }
    }
    let mut st = StartState {
        origin: vec3(&d["origin"], &format!("{}.origin", ctx))?,
        angles: vec3(&d["angles"], &format!("{}.angles", ctx))?,
        velocity: [0.0; 3],
        origin_bits: None,
        angles_bits: None,
        velocity_bits: None,
        base_velocity: [0.0; 3],
        base_velocity_flag: false,
        on_ground: "auto".into(),
        ground_entity: 0,
        ducked: false,
        view_offset_z: None,
        gravity: 1.0,
        lagged_movement: 1.0,
        velocity_modifier: 1.0,
        stamina: 0.0,
        health: None,
        maxspeed: None,
        map: None,
        source: source.to_string(),
    };
    if let Some(v) = d.get("velocity") {
        st.velocity = vec3(v, &format!("{}.velocity", ctx))?;
    }
    for name in ["origin", "angles", "velocity"] {
        let key = format!("{}_bits", name);
        if let Some(v) = d.get(&key) {
            let bits = bits3(v, &format!("{}.{}", ctx, key))?;
            let values = match name {
                "origin" => st.origin,
                "angles" => st.angles,
                _ => st.velocity,
            };
            check_bits(&values, &bits, &format!("{}.{}", ctx, name))?;
            match name {
                "origin" => st.origin_bits = Some(bits),
                "angles" => st.angles_bits = Some(bits),
                _ => st.velocity_bits = Some(bits),
            }
        }
    }
    if let Some(v) = d.get("base_velocity") {
        st.base_velocity = vec3(v, &format!("{}.base_velocity", ctx))?;
    }
    if let Some(v) = d.get("base_velocity_flag") {
        st.base_velocity_flag = boolean(v, &format!("{}.base_velocity_flag", ctx))?;
    }
    if let Some(og) = d.get("on_ground") {
        st.on_ground = match og {
            Value::Bool(true) => "yes".into(),
            Value::Bool(false) => "no".into(),
            Value::String(s) if s == "auto" => "auto".into(),
            _ => return err(format!("{}.on_ground: expected true, false or \"auto\", got {}", ctx, py::repr_value(og))),
        };
    }
    if let Some(v) = d.get("ground_entity") {
        st.ground_entity = int(v, &format!("{}.ground_entity", ctx))?;
        if st.ground_entity < 0 {
            return err(format!("{}.ground_entity: must be >= 0 (0 is the world)", ctx));
        }
    }
    if let Some(v) = d.get("ducked") {
        st.ducked = boolean(v, &format!("{}.ducked", ctx))?;
        if st.ducked {
            return err(format!("{}.ducked: ducked start states are not supported yet (stand up and capture again)", ctx));
        }
    }
    for name in ["view_offset_z", "velocity_modifier", "stamina", "maxspeed"] {
        if let Some(v) = d.get(name) {
            let x = num(v, &format!("{}.{}", ctx, name))?;
            match name {
                "view_offset_z" => st.view_offset_z = Some(x),
                "velocity_modifier" => st.velocity_modifier = x,
                "stamina" => st.stamina = x,
                _ => st.maxspeed = Some(x),
            }
        }
    }
    for name in ["gravity", "lagged_movement"] {
        if let Some(v) = d.get(name) {
            let x = num(v, &format!("{}.{}", ctx, name))?;
            if name == "gravity" {
                st.gravity = x;
            } else {
                st.lagged_movement = x;
            }
        }
    }
    if let Some(v) = d.get("health") {
        st.health = Some(int(v, &format!("{}.health", ctx))?);
    }
    if let Some(v) = d.get("map") {
        st.map = Some(string(v, &format!("{}.map", ctx))?);
    }
    if st.angles[2].abs() > 0.0 {
        return err(format!("{}.angles[2]: roll must be 0", ctx));
    }
    Ok(st)
}

/// Python's `v == n` for a JSON value (1 == 1.0 == True).
fn value_equals_int(v: &Value, n: i64) -> bool {
    match v {
        Value::Number(x) => x.as_f64() == Some(n as f64),
        Value::Bool(b) => (*b as i64) == n,
        _ => false,
    }
}

fn load_start_file(reference: &str, base_dir: &Path, search_dirs: &[PathBuf]) -> Res<StartState> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let r = native(reference);
    for d in std::iter::once(base_dir.to_path_buf()).chain(search_dirs.iter().cloned()) {
        candidates.push(d.join(&r));
        if !reference.to_lowercase().ends_with(".json") {
            candidates.push(d.join(format!("{}.json", r.display())));
        }
    }
    for c in &candidates {
        if c.is_file() {
            let text = std::fs::read_to_string(c).map_err(|e| TlError(e.to_string()))?;
            let obj: Value =
                py::loads(&text).map_err(|e| TlError(format!("start file {}: invalid JSON: {}", c.display(), e)))?;
            let name = c.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            return parse_start(&obj, &format!("start file {}", name), &c.display().to_string());
        }
    }
    let tried: Vec<String> = candidates.iter().map(|c| c.display().to_string()).collect();
    err(format!("start: could not find start file {}; tried:\n  {}", py::repr_str(reference), tried.join("\n  ")))
}

// --------------------------------------------------------------------------
// tracks and key ranges

fn interp_of(v: &Value, ctx: &str) -> Res<Interp> {
    v.as_str().and_then(Interp::parse).map_or_else(
        || err(format!("{}: interp must be one of {}, got {}", ctx, INTERP_MODES.join(", "), py::repr_value(v))),
        Ok,
    )
}

fn parse_track(obj: &Value, ctx: &str, length: i64) -> Res<Vec<Key>> {
    let d = fields(obj, ctx, &["keys", "default_interp"], &["keys"])?;
    let default = d.get("default_interp").cloned().unwrap_or(Value::String("linear".into()));
    if default.as_str().and_then(Interp::parse).is_none() {
        return err(format!(
            "{}.default_interp: expected one of {}, got {}",
            ctx,
            INTERP_MODES.join(", "),
            py::repr_value(&default)
        ));
    }
    let raw = match d.get("keys").and_then(Value::as_array) {
        Some(a) if a.iter().any(|k| !is_comment(k)) => a.clone(),
        _ => return err(format!("{}.keys: expected a non-empty list", ctx)),
    };
    let mut keys: Vec<Key> = Vec::new();
    for (i, k) in raw.iter().enumerate() {
        let kctx = format!("{}.keys[{}]", ctx, i);
        if is_comment(k) {
            continue;
        }
        let (tick, value, slope, joint, interp_v) = if let Some(a) = k.as_array() {
            if !(2..=5).contains(&a.len()) {
                return err(format!(
                    "{}: expected [tick, value], [tick, value, \"interp\"], [tick, value, \"interp\", slope] or [tick, value, \"interp\", slope, joint], got {}",
                    kctx,
                    py::repr_value(k)
                ));
            }
            let tick = int(&a[0], &format!("{}[0]", kctx))?;
            let value = num(&a[1], &format!("{}[1]", kctx))?;
            let slope =
                if a.len() >= 4 && (a.len() == 4 || !a[3].is_null()) { Some(num(&a[3], &format!("{}[3]", kctx))?) } else { None };
            let joint = if a.len() == 5 { Some(num(&a[4], &format!("{}[4]", kctx))?) } else { None };
            (tick, value, slope, joint, if a.len() >= 3 { a[2].clone() } else { default.clone() })
        } else {
            let kd = fields(k, &kctx, &["tick", "value", "interp", "slope", "joint"], &["tick", "value"])?;
            let tick = int(&kd["tick"], &format!("{}.tick", kctx))?;
            let value = num(&kd["value"], &format!("{}.value", kctx))?;
            let slope = match kd.get("slope") {
                Some(s) => Some(num(s, &format!("{}.slope", kctx))?),
                None => None,
            };
            let joint = match kd.get("joint") {
                Some(s) => Some(num(s, &format!("{}.joint", kctx))?),
                None => None,
            };
            (tick, value, slope, joint, kd.get("interp").cloned().unwrap_or(default.clone()))
        };
        if let Some(j) = joint {
            if j < 0.0 {
                return err(format!("{}: a joint's width can't be negative, got {}", kctx, py::repr(j)));
            }
        }
        let interp = interp_of(&interp_v, &kctx)?;
        if !(0..length).contains(&tick) {
            return err(format!("{}: tick {} is outside the timeline (0 .. {})", kctx, tick, length - 1));
        }
        if let Some(last) = keys.last() {
            if tick <= last.tick {
                return err(format!("{}: tick {} must be greater than the previous key's tick {}", kctx, tick, last.tick));
            }
        }
        keys.push(Key { slope, joint, ..Key::new(tick, value, interp) });
    }
    Ok(keys)
}

fn parse_key_ranges(raw: &Value, length: i64) -> Res<Vec<KeyRange>> {
    let list = match raw.as_array() {
        Some(a) => a,
        None => return err("keys: expected a list"),
    };
    let mut out: Vec<KeyRange> = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let ctx = format!("keys[{}]", i);
        if is_comment(r) {
            continue;
        }
        let (name, start, end, value) = if let Some(a) = r.as_array() {
            if a.len() != 3 {
                return err(format!("{}: expected [\"key\", from, to], got {}", ctx, py::repr_value(r)));
            }
            (a[0].clone(), a[1].clone(), a[2].clone(), Value::Null)
        } else {
            let d = fields(r, &ctx, &["key", "from", "to", "value"], &["key", "from", "to"])?;
            (d["key"].clone(), d["from"].clone(), d["to"].clone(), d.get("value").cloned().unwrap_or(Value::Null))
        };
        let name = string(&name, &format!("{}.key", ctx))?;
        let key = match buttons::canonical_key(&name) {
            Some(k) => k,
            None => {
                return err(format!(
                    "{}.key: unknown key {} (known: {}, bit0..bit31)",
                    ctx,
                    py::repr_str(&name),
                    buttons::known_names().join(", ")
                ))
            }
        };
        let start = int(&start, &format!("{}.from", ctx))?;
        let end = int(&end, &format!("{}.to", ctx))?;
        if !(0..length).contains(&start) {
            return err(format!("{}.from: {} is outside the timeline (0 .. {})", ctx, start, length - 1));
        }
        if !(start < end && end <= length) {
            return err(format!("{}.to: {} must be > from ({}) and <= length ({}); 'to' is exclusive", ctx, end, start, length));
        }
        let value = if value.is_null() {
            None
        } else {
            if !buttons::is_movement_key(&key) {
                return err(format!("{}.value: only movement keys (W/A/S/D) take a value", ctx));
            }
            let v = num(&value, &format!("{}.value", ctx))?;
            if v <= 0.0 {
                return err(format!("{}.value: must be > 0 (the direction comes from the key)", ctx));
            }
            Some(v)
        };
        out.push(KeyRange { key, start, end, value, index: i });
    }
    // per key (in order of first appearance), sorted by start: no overlaps
    let mut order: Vec<String> = Vec::new();
    for kr in &out {
        if !order.contains(&kr.key) {
            order.push(kr.key.clone());
        }
    }
    for key in &order {
        let mut ranges: Vec<&KeyRange> = out.iter().filter(|k| &k.key == key).collect();
        ranges.sort_by_key(|k| k.start);
        for w in ranges.windows(2) {
            let (a, b) = (w[0], w[1]);
            if b.start < a.end {
                return err(format!(
                    "keys[{}]: {} [{}, {}) overlaps keys[{}] [{}, {}); a key can't be held twice",
                    b.index, key, b.start, b.end, a.index, a.start, a.end
                ));
            }
        }
    }
    Ok(out)
}

/// Auto-strafe ranges: on those ticks the plugin aims the held keys from the real velocity.
fn parse_strafe(raw: &Value, length: i64) -> Res<Vec<StrafeRange>> {
    let list = match raw.as_array() {
        Some(a) => a,
        None => return err("strafe: expected a list"),
    };
    let mut out: Vec<StrafeRange> = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let ctx = format!("strafe[{}]", i);
        if is_comment(r) {
            continue;
        }
        let (mut heading, mut swing, mut gain) = (None, None, None);
        let (start, end, offset) = if let Some(a) = r.as_array() {
            if a.len() != 2 && a.len() != 3 {
                return err(format!("{}: expected [from, to] or [from, to, offset], got {}", ctx, py::repr_value(r)));
            }
            (a[0].clone(), a[1].clone(), if a.len() == 3 { a[2].clone() } else { py::num(0.0) })
        } else {
            let d = fields(r, &ctx, &["from", "to", "offset", "heading", "swing", "gain", "smooth"], &["from", "to"])?;
            if let Some(h) = d.get("heading").filter(|h| !h.is_null()) {
                heading = Some(num(h, &format!("{}.heading", ctx))?);
            }
            if let Some(s) = d.get("swing").filter(|s| !s.is_null()) {
                let s = num(s, &format!("{}.swing", ctx))?;
                if heading.is_none() {
                    return err(format!("{}.swing: only for a straight range (one with a heading)", ctx));
                }
                if !(0.0 < s && s <= 90.0) {
                    return err(format!("{}.swing: must be more than 0 and at most 90 degrees", ctx));
                }
                swing = Some(s);
            }
            if let Some(g) = d.get("gain").filter(|g| !g.is_null()) {
                gain = Some(num(g, &format!("{}.gain", ctx))?);
            } else if let Some(m) = d.get("smooth").filter(|m| !m.is_null()) {
                // as a first version had it: how close to perfect, 0 to 100
                let short = 1.0 - num(m, &format!("{}.smooth", ctx))? / 100.0; // how far from perfect
                gain = Some(100.0 * (1.0 - 0.25 * short * short));
            }
            if let Some(g) = gain {
                if swing.is_none() {
                    return err(format!("{}.gain: only with a swing (how far it sweeps)", ctx));
                }
                if !(75.0..=100.0).contains(&g) {
                    return err(format!("{}.gain: how much of the most speed gain to keep, 75 to 100 (percent)", ctx));
                }
            }
            (d["from"].clone(), d["to"].clone(), d.get("offset").cloned().unwrap_or(py::num(0.0)))
        };
        let start = int(&start, &format!("{}.from", ctx))?;
        let end = int(&end, &format!("{}.to", ctx))?;
        let offset = num(&offset, &format!("{}.offset", ctx))?;
        if !(0..length).contains(&start) {
            return err(format!("{}.from: {} is outside the timeline (0 .. {})", ctx, start, length - 1));
        }
        if !(start < end && end <= length) {
            return err(format!("{}.to: {} must be > from ({}) and <= length ({}); 'to' is exclusive", ctx, end, start, length));
        }
        if !(-90.0..=90.0).contains(&offset) {
            return err(format!("{}.offset: must be between -90 and 90 degrees", ctx));
        }
        out.push(StrafeRange { start, end, offset, index: i, heading, swing, gain });
    }
    out.sort_by_key(|r| r.start);
    for w in out.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if b.start < a.end {
            return err(format!(
                "strafe[{}]: [{}, {}) overlaps strafe[{}] [{}, {})",
                b.index, b.start, b.end, a.index, a.start, a.end
            ));
        }
    }
    Ok(out)
}

/// The editor's turn blocks (see TurnBlock): sorted, each inside the timeline.
fn parse_turns(raw: &Value, length: i64) -> Res<Vec<TurnBlock>> {
    let list = match raw.as_array() {
        Some(a) => a,
        None => return err("turns: expected a list"),
    };
    let mut out: Vec<TurnBlock> = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let ctx = format!("turns[{}]", i);
        if is_comment(r) {
            continue;
        }
        let all = ["from", "to", "rate", "shape"];
        let d = fields(r, &ctx, &all, &all)?;
        let start = int(&d["from"], &format!("{}.from", ctx))?;
        let end = int(&d["to"], &format!("{}.to", ctx))?;
        let rate = num(&d["rate"], &format!("{}.rate", ctx))?;
        let shape = match d["shape"].as_str() {
            Some(s) if s == "linear" || s == "smooth" => s.to_string(),
            _ => return err(format!("{}.shape: expected 'linear' or 'smooth', got {}", ctx, py::repr_value(&d["shape"]))),
        };
        if !(0 <= start && start < end && end <= length) {
            return err(format!("{}: [{}, {}) must be inside the timeline (0 .. {}); 'to' is exclusive", ctx, start, end, length));
        }
        out.push(TurnBlock { start, end, rate, shape });
    }
    out.sort_by_key(|b| b.start);
    Ok(out)
}

/// The editor's ticks whose A and D were set by hand: [from, to) spans, sorted, each inside the timeline.
fn parse_by_hand(raw: &Value, length: i64) -> Res<Vec<(i64, i64)>> {
    let list = match raw.as_array() {
        Some(a) => a,
        None => return err("ad_by_hand: expected a list"),
    };
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (i, r) in list.iter().enumerate() {
        let ctx = format!("ad_by_hand[{}]", i);
        if is_comment(r) {
            continue;
        }
        let all = ["from", "to"];
        let d = fields(r, &ctx, &all, &all)?;
        let start = int(&d["from"], &format!("{}.from", ctx))?;
        let end = int(&d["to"], &format!("{}.to", ctx))?;
        if !(0 <= start && start < end && end <= length) {
            return err(format!("{}: [{}, {}) must be inside the timeline (0 .. {}); 'to' is exclusive", ctx, start, end, length));
        }
        out.push((start, end));
    }
    out.sort();
    Ok(out)
}

// --------------------------------------------------------------------------
// timeline

/// the camera replays show: smoothed on auto-strafe, everywhere, or not
pub const CAMERA_SMOOTHING: [&str; 3] = ["strafe", "all", "off"];

const TOP_FIELDS: [&str; 17] = [
    "format", "version", "name", "map", "length", "settings", "start", "input", "yaw", "pitch", "keys", "strafe", "view_pitch",
    "autobhop", "camera_smoothing", "turns", "ad_by_hand",
];

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn parse_timeline(obj: &Value, path: Option<&Path>, start_search_dirs: &[PathBuf]) -> Res<Timeline> {
    let d = fields(obj, "timeline", &TOP_FIELDS, &["format", "version", "map", "length", "start", "yaw"])?;
    if d["format"].as_str() != Some(FORMAT_NAME) {
        return err(format!("format: expected {}, got {}", py::repr_str(FORMAT_NAME), py::repr_value(&d["format"])));
    }
    if !value_equals_int(&d["version"], FORMAT_VERSION) {
        return err(format!("version: this baker reads version {}, got {}", FORMAT_VERSION, py::repr_value(&d["version"])));
    }
    let name_v = match d.get("name") {
        Some(v) if truthy(v) => v.clone(),
        _ => match path.and_then(|p| p.file_stem()) {
            Some(stem) => Value::String(stem.to_string_lossy().to_string()),
            None => Value::Null,
        },
    };
    let name = match name_v.as_str() {
        Some(s) if name_ok(s) => s.to_string(),
        _ => return err(format!("name: expected letters, digits, '_', '-' or '.', got {}", py::repr_value(&name_v))),
    };
    let map = string(&d["map"], "map")?;
    if !name_ok(&map) {
        return err(format!("map: unexpected characters in {}", py::repr_str(&map)));
    }
    let length = int(&d["length"], "length")?;
    if length <= 0 {
        return err("length: must be at least 1 tick");
    }

    let mut settings: Vec<(String, Value)> = Vec::new();
    if let Some(sd) = d.get("settings") {
        let so = match sd.as_object() {
            Some(o) => o,
            None => return err("settings: expected an object"),
        };
        for (k, v) in so {
            if k.starts_with('_') {
                continue;
            }
            if !setting_name_ok(k) {
                return err(format!("settings.{}: setting names are convar names (letters, digits, _)", k));
            }
            let ok = is_num(v) || matches!(v.as_str(), Some(s) if !s.is_empty() && !s.chars().any(char::is_whitespace));
            if !ok {
                return err(format!("settings.{}: expected a number or a string without spaces, got {}", k, py::repr_value(v)));
            }
            settings.push((k.clone(), v.clone()));
        }
    }

    let base_dir = match path.and_then(Path::parent) {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let start = match &d["start"] {
        Value::String(s) => load_start_file(s, &base_dir, start_search_dirs)?,
        other => parse_start(other, "start", "inline")?,
    };

    let autobhop = match d.get("autobhop") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(other) => return err(format!("autobhop: expected true or false, got {}", py::repr_value(other))),
    };
    let camera_smoothing = match d.get("camera_smoothing") {
        None => "strafe".to_string(),
        Some(Value::String(s)) if CAMERA_SMOOTHING.contains(&s.as_str()) => s.clone(),
        Some(other) => {
            return err(format!(
                "camera_smoothing: expected \"strafe\", \"all\" or \"off\", got {}",
                py::repr_value(other)
            ))
        }
    };

    let mut move_speed = 400.0;
    if let Some(iv) = d.get("input") {
        let idict = fields(iv, "input", &["move_speed"], &[])?;
        if let Some(ms) = idict.get("move_speed") {
            move_speed = num(ms, "input.move_speed")?;
            if move_speed <= 0.0 {
                return err("input.move_speed: must be > 0");
            }
        }
    }

    let yaw = parse_track(&d["yaw"], "yaw", length)?;
    let pitch = match d.get("pitch") {
        Some(p) => Some(parse_track(p, "pitch", length)?),
        None => None,
    };
    let keys = parse_key_ranges(d.get("keys").unwrap_or(&Value::Array(vec![])), length)?;
    let strafe = parse_strafe(d.get("strafe").unwrap_or(&Value::Array(vec![])), length)?;
    let view_pitch = match d.get("view_pitch") {
        Some(p) => Some(parse_track(p, "view_pitch", length)?),
        None => None,
    };
    let turns = parse_turns(d.get("turns").unwrap_or(&Value::Array(vec![])), length)?;
    let ad_by_hand = parse_by_hand(d.get("ad_by_hand").unwrap_or(&Value::Array(vec![])), length)?;
    let mut tl = Timeline {
        name,
        map: map.clone(),
        length,
        settings,
        start,
        move_speed,
        yaw,
        pitch,
        keys,
        path: path.map(Path::to_path_buf),
        strafe,
        view_pitch,
        autobhop,
        camera_smoothing,
        turns,
        ad_by_hand,
        warnings: Vec::new(),
    };

    if let Some(sm) = &tl.start.map {
        if sm != &map {
            return err(format!("start: captured on map {} but the timeline is for {}", py::repr_str(sm), py::repr_str(&map)));
        }
    }
    for w in tl.yaw.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (b.value - a.value).abs() > 180.0 {
            tl.warnings.push(format!(
                "yaw turns {} degrees between ticks {} and {}. Yaw is unwrapped, so that is the way it turns; if you meant the short way, write {}.",
                py::fixed_signed(b.value - a.value, 1),
                a.tick,
                b.tick,
                py::fixed(a.value + (py::fmod(b.value - a.value + 180.0, 360.0) - 180.0), 1)
            ));
        }
    }
    let pitch_warnings: Vec<String> = tl
        .pitch
        .iter()
        .flatten()
        .filter(|k| k.value.abs() > 89.0)
        .map(|k| format!("pitch key at tick {} is {}; a real client clamps pitch to +-89", k.tick, py::repr(k.value)))
        .collect();
    tl.warnings.extend(pitch_warnings);
    let view_warnings: Vec<String> = tl
        .view_pitch
        .iter()
        .flatten()
        .filter(|k| k.value.abs() > 89.0)
        .map(|k| format!("view_pitch key at tick {} is {}; the game shows at most +-89", k.tick, py::repr(k.value)))
        .collect();
    tl.warnings.extend(view_warnings);
    Ok(tl)
}

pub fn load_timeline(path: &Path, start_search_dirs: &[PathBuf]) -> Res<Timeline> {
    let text = std::fs::read_to_string(path).map_err(|e| TlError(crate::io_error(&e, path)))?;
    let obj: Value =
        py::loads(&text).map_err(|e| TlError(format!("{}: invalid JSON: {}", path.display(), e)))?;
    parse_timeline(&obj, Some(path), start_search_dirs)
}

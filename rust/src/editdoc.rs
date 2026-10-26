//! Timelines as editable structure (for the browser editor), and importing
//! recordings as timelines (tasurf/editdoc.py).
//!
//! The editor works on a normalised view of a timeline: yaw/pitch/view_pitch keys as
//! [tick, value, interp] and key ranges as {"key", "from", "to"[, "value"]} with
//! canonical key names. Edits come back in the same shape and are merged into the
//! original JSON document, so everything else in the file (start, settings,
//! comments) is kept as written. Parsing rules stay in timeline.rs.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::bake::{bake_inputs, read_baked, BakedFile, TickInput};
use crate::buttons;
use crate::interp::{key_slope, segment_slope, value_at, wrap_degrees, Interp, Key, INTERP_MODES};
use crate::py;
use crate::timeline::{err, is_comment, parse_timeline, Res, Timeline, TlError};

pub const MAX_TICKS: i64 = 1_000_000;

// --------------------------------------------------------------------------
// Python-like access to loosely typed JSON from the browser

/// `d[key]`: a missing key is a KeyError, whose str() is the quoted key.
pub fn need<'a>(d: &'a Map<String, Value>, key: &str) -> Res<&'a Value> {
    d.get(key).ok_or_else(|| TlError(py::repr_str(key)))
}

/// `d.get(key)`, with JSON null as None.
pub fn opt<'a>(d: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    d.get(key).filter(|v| !v.is_null())
}

pub fn obj<'a>(v: &'a Value, what: &str) -> Res<&'a Map<String, Value>> {
    v.as_object().ok_or_else(|| TlError(format!("{}: expected an object, got {}", what, py::type_name(v))))
}

/// `int(v)`.
pub fn to_int(v: &Value) -> Res<i64> {
    match v {
        Value::Number(n) => match n.as_i64() {
            Some(i) => Ok(i),
            None => {
                let x = n.as_f64().unwrap_or(f64::NAN);
                if x.is_nan() {
                    err("cannot convert float NaN to integer")
                } else if x.is_infinite() {
                    err("cannot convert float infinity to integer")
                } else {
                    Ok(x.trunc() as i64)
                }
            }
        },
        Value::Bool(b) => Ok(*b as i64),
        Value::String(s) => py::parse_int(s).map_err(TlError),
        other => err(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        )),
    }
}

/// `float(v)`.
pub fn to_float(v: &Value) -> Res<f64> {
    match v {
        Value::Number(n) => Ok(n.as_f64().unwrap_or(f64::NAN)),
        Value::Bool(b) => Ok(*b as i64 as f64),
        Value::String(s) => py::parse_float(s).map_err(TlError),
        other => err(format!("float() argument must be a string or a real number, not '{}'", py::type_name(other))),
    }
}

/// `str(v)`.
pub fn to_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py::repr_value(other),
    }
}

/// A raw number for comparisons (`r["from"] <= cur["to"]`).
fn cmpv(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(b)) => *b as i64 as f64,
        _ => f64::NAN,
    }
}

/// Python `==` for JSON values (1 == 1.0, None == missing).
fn py_eq(a: Option<&Value>, b: Option<&Value>) -> bool {
    let a = a.filter(|v| !v.is_null());
    let b = b.filter(|v| !v.is_null());
    match (a, b) {
        (None, None) => true,
        (Some(Value::Number(x)), Some(Value::Number(y))) => x.as_f64() == y.as_f64(),
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// max(a, b) of two raw numbers: a unless b is larger.
fn vmax(a: &Value, b: &Value) -> Value {
    if cmpv(Some(b)) > cmpv(Some(a)) {
        b.clone()
    } else {
        a.clone()
    }
}

fn ival(i: i64) -> Value {
    Value::from(i)
}

/// dict(r, **changes)
fn with_fields(r: &Map<String, Value>, changes: &[(&str, Value)]) -> Map<String, Value> {
    let mut out = r.clone();
    for (k, v) in changes {
        out.insert((*k).to_string(), v.clone());
    }
    out
}

// --------------------------------------------------------------------------
// track keys: [tick, value, interp] or [tick, value, interp, slope]

#[derive(Clone, Debug)]
pub struct TK {
    pub tick: i64,
    pub value: f64,
    pub int: bool, // the value came in as a JSON integer and is passed on as one
    pub interp: String,
    pub slope: Option<f64>,
}

impl TK {
    fn new(tick: i64, value: f64, interp: &str) -> TK {
        TK { tick, value, int: false, interp: interp.to_string(), slope: None }
    }

    fn with_value(&self, tick: i64, value: Option<f64>) -> TK {
        match value {
            Some(v) => TK { tick, value: v, int: false, interp: self.interp.clone(), slope: self.slope },
            None => TK { tick, ..self.clone() },
        }
    }

    fn to_json(&self) -> Value {
        let v = if self.int && self.value.fract() == 0.0 && self.value.abs() < 9e15 {
            ival(self.value as i64)
        } else {
            py::num(self.value)
        };
        let mut a = vec![ival(self.tick), v, Value::String(self.interp.clone())];
        if let Some(s) = self.slope {
            a.push(py::num(s));
        }
        Value::Array(a)
    }
}

pub fn track_from(v: &Value) -> Res<Vec<TK>> {
    let list = v.as_array().ok_or_else(|| TlError(format!("'{}' object is not iterable", py::type_name(v))))?;
    list.iter()
        .map(|k| {
            let a = k.as_array().filter(|a| a.len() == 3 || a.len() == 4).ok_or_else(|| {
                TlError("a key is [tick, value, interp] or [tick, value, interp, slope]".into())
            })?;
            Ok(TK {
                tick: to_int(&a[0])?,
                value: to_float(&a[1])?,
                int: a[1].is_i64() || a[1].is_u64(),
                interp: to_str(&a[2]),
                slope: match a.get(3) {
                    Some(s) if !s.is_null() => Some(to_float(s)?),
                    _ => None,
                },
            })
        })
        .collect()
}

pub fn track_json(t: &[TK]) -> Value {
    Value::Array(t.iter().map(TK::to_json).collect())
}

fn sorted_track(mut t: Vec<TK>) -> Vec<TK> {
    t.sort_by_key(|k| k.tick);
    t
}

/// value_at for a track from the browser; an unknown interp fails only where it is used.
fn interp_keys(track: &[TK]) -> Vec<Key> {
    track
        .iter()
        .map(|k| Key { slope: k.slope, ..Key::new(k.tick, k.value, Interp::parse(&k.interp).unwrap_or(Interp::Linear)) })
        .collect()
}

fn tvalue(track: &[TK], tick: i64) -> Res<f64> {
    if track.is_empty() {
        return err("track has no keys");
    }
    let keys: Vec<Key> = interp_keys(track);
    let n = keys.len();
    if tick > keys[0].tick && tick < keys[n - 1].tick {
        let (mut lo, mut hi) = (0usize, n - 1);
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if keys[mid].tick <= tick {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        if Interp::parse(&track[lo].interp).is_none() {
            return err(format!("unknown interpolation mode {}", py::repr_str(&track[lo].interp)));
        }
    }
    Ok(value_at(&keys, tick))
}

/// The slope (degrees per tick) the curve arrives at tick with (into) or leaves it with (not into). Flat before
/// the first key and after the last.
fn curve_slope(track: &[TK], tick: i64, into: bool) -> f64 {
    let keys = interp_keys(track);
    for i in 0..keys.len().saturating_sub(1) {
        let (k0, k1) = (&track[i], &track[i + 1]);
        let inside = if into { k0.tick < tick && tick <= k1.tick } else { k0.tick <= tick && tick < k1.tick };
        if inside {
            let span = (k1.tick - k0.tick) as f64;
            return match k0.interp.as_str() {
                "hold" => 0.0,
                "linear" => (k1.value - k0.value) / span,
                "smooth" => {
                    let s = (tick - k0.tick) as f64 / span;
                    (k1.value - k0.value) * 6.0 * s * (1.0 - s) / span
                }
                _ => segment_slope(&keys, i, tick),
            };
        }
    }
    0.0
}

/// Interp of the segment going forward from tick ("linear" before the first key).
fn segment_interp(track: &[TK], tick: i64) -> String {
    let mut mode = "linear".to_string();
    for k in track {
        if k.tick > tick {
            break;
        }
        mode = k.interp.clone();
    }
    mode
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Before,
    After,
    Both,
}

/// keys with a key at tick on the curve of source, such that the values on one
/// side of it (Before: ticks <= tick, After: ticks >= tick, or Both) stay what
/// source gives. A hold or linear segment just gets the key. A cubic segment is
/// cut by a key with the slope the curve has there, its end keys saying theirs: a
/// part of a cubic is a cubic, so both parts are the curve they were. A smooth
/// segment can't be cut into two of its kind, so the kept part is written out tick
/// by tick. A cubic segment on the kept side that meets the key where things
/// change gets that key's slope written in, so it no longer depends on the keys
/// beyond. Before the first key and after the last the curve is flat; the new key
/// there holds that value.
fn pin(keys: &[TK], tick: Option<i64>, side: Side, source: &[TK]) -> Res<Vec<TK>> {
    let mut keys: Vec<TK> = keys.to_vec();
    let tick = match tick {
        Some(t) => t,
        None => return Ok(keys),
    };
    if keys.is_empty() {
        return err("list index out of range");
    }
    let exact = |t: i64, interp: &str| -> Res<TK> { Ok(TK::new(t, tvalue(source, t)?, interp)) };
    // segment keys[j] .. keys[j + 1], tick by tick
    fn spell_out(keys: &mut [TK], j: usize, source: &[TK], added: &mut Vec<TK>) -> Res<()> {
        keys[j].interp = "linear".into();
        for t in keys[j].tick + 1..keys[j + 1].tick {
            added.push(TK::new(t, tvalue(source, t)?, "linear"));
        }
        Ok(())
    }
    let src = interp_keys(source);
    // key j says the slope cubic segments meet it with, as source has it
    fn freeze(keys: &mut [TK], j: usize, src: &[Key]) {
        if keys[j].slope.is_none() {
            let s = match src.iter().position(|k| k.tick == keys[j].tick) {
                Some(at) => key_slope(src, at),
                None => key_slope(&interp_keys(keys), j),
            };
            keys[j].slope = Some(s);
        }
    }
    // a cubic segment's slopes come from the neighbouring keys (unless its keys give them)
    fn cubic_back(keys: &mut [TK], j: isize, src: &[Key]) {
        if j >= 0 && keys[j as usize].interp == "cubic" {
            freeze(keys, j as usize + 1, src);
        }
    }
    fn cubic_forward(keys: &mut [TK], j: usize, src: &[Key]) {
        if j + 1 < keys.len() && keys[j].interp == "cubic" {
            freeze(keys, j, src);
        }
    }
    let before = side == Side::Before || side == Side::Both;
    let after = side == Side::After || side == Side::Both;
    let mut added: Vec<TK> = Vec::new();
    let n = keys.len();
    if let Some(at) = keys.iter().position(|k| k.tick == tick) {
        // the key is there already, but the caller puts new keys next to it
        if before {
            cubic_back(&mut keys, at as isize - 1, &src);
        }
        if after {
            cubic_forward(&mut keys, at, &src);
        }
    } else if tick < keys[0].tick {
        added.push(exact(tick, "hold")?);
        cubic_forward(&mut keys, 0, &src);
    } else if tick > keys[n - 1].tick {
        keys[n - 1].interp = "hold".into(); // it had no segment after it, so its interp didn't matter until now
        added.push(exact(tick, "linear")?);
        cubic_back(&mut keys, n as isize - 2, &src);
    } else {
        // tick is inside keys[i] .. keys[i + 1]
        let i = keys.iter().enumerate().filter(|(_, k)| k.tick < tick).map(|(j, _)| j).max().unwrap_or(0);
        let (lo, hi, mode) = (keys[i].tick, keys[i + 1].tick, keys[i].interp.clone());
        if mode == "hold" || mode == "linear" {
            added.push(exact(tick, &mode)?);
        } else if mode == "cubic" {
            freeze(&mut keys, i, &src);
            freeze(&mut keys, i + 1, &src);
            let mut cut = exact(tick, "cubic")?;
            cut.slope = Some(segment_slope(&interp_keys(&keys), i, tick));
            added.push(cut);
        } else if side == Side::Before {
            for t in lo + 1..tick + 1 {
                added.push(exact(t, "linear")?);
            }
            if let Some(last) = added.last_mut() {
                last.interp = mode.clone(); // the rest of the segment, which isn't kept, stays smooth/cubic
            }
            keys[i].interp = "linear".into();
        } else if side == Side::After {
            for t in tick..hi {
                added.push(exact(t, "linear")?);
            }
        } else {
            spell_out(&mut keys, i, source, &mut added)?;
        }
        if before {
            cubic_back(&mut keys, i as isize - 1, &src);
        }
        if after {
            cubic_forward(&mut keys, i + 1, &src);
        }
    }
    keys.extend(added);
    Ok(sorted_track(keys))
}

// --------------------------------------------------------------------------
// the normalised view and merging edits into the document

fn range_out(key: &str, start: i64, end: i64, value: Option<f64>) -> Value {
    let mut r = Map::new();
    r.insert("key".into(), Value::String(key.to_string()));
    r.insert("from".into(), ival(start));
    r.insert("to".into(), ival(end));
    if let Some(v) = value {
        r.insert("value".into(), py::num(v));
    }
    Value::Object(r)
}

fn keys_json(keys: &[Key]) -> Value {
    Value::Array(
        keys.iter()
            .map(|k| {
                let mut a = vec![ival(k.tick), py::num(k.value), Value::String(k.interp.name().into())];
                if let Some(s) = k.slope {
                    a.push(py::num(s));
                }
                Value::Array(a)
            })
            .collect(),
    )
}

pub fn normalized(tl: &Timeline) -> Value {
    let mut ranges: Vec<_> = tl.keys.iter().collect();
    ranges.sort_by(|a, b| (a.start, &a.key).cmp(&(b.start, &b.key)));
    let mut m = Map::new();
    m.insert("name".into(), Value::String(tl.name.clone()));
    m.insert("map".into(), Value::String(tl.map.clone()));
    m.insert("length".into(), ival(tl.length));
    m.insert("move_speed".into(), py::num(tl.move_speed));
    m.insert("start_pitch".into(), py::num(tl.start.angles[0]));
    m.insert("yaw".into(), keys_json(&tl.yaw));
    m.insert("pitch".into(), tl.pitch.as_ref().map_or(Value::Null, |p| keys_json(p)));
    m.insert("keys".into(), Value::Array(ranges.iter().map(|kr| range_out(&kr.key, kr.start, kr.end, kr.value)).collect()));
    m.insert(
        "strafe".into(),
        Value::Array(
            tl.strafe
                .iter()
                .map(|r| {
                    let mut o = Map::new();
                    o.insert("from".into(), ival(r.start));
                    o.insert("to".into(), ival(r.end));
                    o.insert("offset".into(), py::num(r.offset));
                    if let Some(h) = r.heading {
                        o.insert("heading".into(), py::num(h));
                    }
                    if let Some(s) = r.swing {
                        o.insert("swing".into(), py::num(s));
                    }
                    if let Some(s) = r.gain {
                        o.insert("gain".into(), py::num(s));
                    }
                    Value::Object(o)
                })
                .collect(),
        ),
    );
    m.insert("view_pitch".into(), tl.view_pitch.as_ref().map_or(Value::Null, |p| keys_json(p)));
    if tl.autobhop {
        m.insert("autobhop".into(), Value::Bool(true));
    }
    if tl.camera_smoothing != "strafe" {
        m.insert("camera_smoothing".into(), Value::String(tl.camera_smoothing.clone()));
    }
    if !tl.turns.is_empty() {
        let blocks = tl.turns.iter().map(|b| {
            let mut o = Map::new();
            o.insert("from".into(), ival(b.start));
            o.insert("to".into(), ival(b.end));
            o.insert("rate".into(), py::num(b.rate));
            o.insert("shape".into(), Value::String(b.shape.clone()));
            Value::Object(o)
        });
        m.insert("turns".into(), Value::Array(blocks.collect()));
    }
    Value::Object(m)
}

/// Python's sort of raw values: numbers by value, strings by code point.
fn cmp_raw(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.cmp(y),
        _ => cmpv(Some(a)).partial_cmp(&cmpv(Some(b))).unwrap_or(Ordering::Equal),
    }
}

fn track_edit(keys: &Value, what: &str) -> Res<Value> {
    let list = match keys.as_array() {
        Some(l) if !l.is_empty() => l,
        _ => return err(format!("{}: needs at least one key", what)),
    };
    let first = |k: &Value| -> Value { k.as_array().and_then(|a| a.first().cloned()).unwrap_or(Value::Null) };
    let mut sorted: Vec<&Value> = list.iter().collect();
    sorted.sort_by(|a, b| cmp_raw(&first(a), &first(b)));
    let mut out = Vec::new();
    for k in sorted {
        let a = k.as_array().filter(|a| a.len() >= 3).ok_or_else(|| TlError("list index out of range".into()))?;
        let (tick, value, interp) = (to_int(&a[0])?, to_float(&a[1])?, to_str(&a[2]));
        let slope = match a.get(3) {
            Some(s) if !s.is_null() => Some(to_float(s)?),
            _ => None,
        };
        if !INTERP_MODES.contains(&interp.as_str()) {
            return err(format!("{}: bad interp {}", what, py::repr_str(&interp)));
        }
        // no rounding: imported values are exact float32s
        let mut key = vec![ival(tick), py::num(value), Value::String(interp)];
        if let Some(s) = slope {
            key.push(py::num(s));
        }
        out.push(Value::Array(key));
    }
    Ok(Value::Array(out))
}

fn list_of_objects<'a>(v: Option<&'a Value>) -> Res<Vec<&'a Map<String, Value>>> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(|r| obj(r, "range")).collect(),
        Some(other) => err(format!("'{}' object is not iterable", py::type_name(other))),
    }
}

/// A copy of doc with length, yaw keys, pitch keys, key ranges, auto-strafe and view pitch replaced.
pub fn apply_edits(doc: &Value, edits: &Value) -> Res<Value> {
    let mut new = obj(doc, "doc")?.clone();
    let edits = obj(edits, "edits")?;
    if let Some(l) = edits.get("length") {
        new.insert("length".into(), ival(to_int(l)?));
    }
    match edits.get("autobhop") {
        // written only when on
        None => {}
        Some(Value::Bool(true)) => {
            new.insert("autobhop".into(), Value::Bool(true));
        }
        Some(Value::Bool(false)) => {
            new.shift_remove("autobhop");
        }
        Some(other) => return err(format!("autobhop: expected true or false, got {}", py::repr_value(other))),
    }
    match edits.get("camera_smoothing") {
        // written only when not the default
        None => {}
        Some(Value::String(v)) if v == "strafe" => {
            new.shift_remove("camera_smoothing");
        }
        Some(Value::String(v)) if v == "all" || v == "off" => {
            new.insert("camera_smoothing".into(), Value::String(v.clone()));
        }
        Some(other) => {
            return err(format!(
                "camera_smoothing: expected \"strafe\", \"all\" or \"off\", got {}",
                py::repr_value(other)
            ))
        }
    }
    for name in ["yaw", "pitch", "view_pitch"] {
        let e = match edits.get(name) {
            Some(e) => e,
            None => continue,
        };
        if name != "yaw" && e.is_null() {
            new.shift_remove(name); // optional tracks: None removes them
            continue;
        }
        let mut track = match new.get(name) {
            Some(Value::Object(o)) => o.clone(),
            _ => Map::new(),
        };
        track.insert("keys".into(), track_edit(e, name)?);
        new.insert(name.into(), Value::Object(track));
    }
    if let Some(k) = edits.get("keys") {
        let mut out: Vec<Value> = match doc.get("keys") {
            Some(Value::Array(a)) => a.iter().filter(|x| is_comment(x)).cloned().collect(),
            _ => Vec::new(),
        };
        let mut rs = list_of_objects(Some(k))?;
        let mut keyed = Vec::new();
        for r in rs.drain(..) {
            keyed.push((to_int(need(r, "from")?)?, need(r, "key")?.clone(), r));
        }
        keyed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| cmp_raw(&a.1, &b.1)));
        for (_, keyv, r) in keyed {
            let key = buttons::canonical_key(&to_str(&keyv))
                .ok_or_else(|| TlError(format!("keys: unknown key {}", py::repr_value(&keyv))))?;
            let mut o = Map::new();
            o.insert("key".into(), Value::String(buttons::display_name(&key)));
            o.insert("from".into(), ival(to_int(need(r, "from")?)?));
            o.insert("to".into(), ival(to_int(need(r, "to")?)?));
            if let Some(v) = opt(r, "value") {
                o.insert("value".into(), py::num(to_float(v)?));
            }
            out.push(Value::Object(o));
        }
        new.insert("keys".into(), Value::Array(out));
    }
    if let Some(s) = edits.get("strafe") {
        let mut rs = Vec::new();
        for r in list_of_objects(Some(s))? {
            rs.push((to_int(need(r, "from")?)?, r));
        }
        rs.sort_by_key(|x| x.0);
        let mut out = Vec::new();
        for (from, r) in rs {
            let mut entry = Map::new();
            entry.insert("from".into(), ival(from));
            entry.insert("to".into(), ival(to_int(need(r, "to")?)?));
            let offset = match opt(r, "offset") {
                Some(v) if py_truthy(v) => to_float(v)?,
                _ => 0.0,
            };
            if offset != 0.0 {
                entry.insert("offset".into(), py::num(to_float(need(r, "offset")?)?));
            }
            if let Some(h) = opt(r, "heading") {
                entry.insert("heading".into(), py::num(to_float(h)?));
            }
            if let Some(s) = opt(r, "swing") {
                entry.insert("swing".into(), py::num(to_float(s)?));
            }
            if let Some(s) = opt(r, "gain") {
                entry.insert("gain".into(), py::num(to_float(s)?));
            }
            out.push(Value::Object(entry));
        }
        if out.is_empty() {
            new.shift_remove("strafe");
        } else {
            new.insert("strafe".into(), Value::Array(out));
        }
    }
    if let Some(t) = edits.get("turns") {
        // the editor's curve turns, by name
        let mut rs = Vec::new();
        if py_truthy(t) {
            for r in list_of_objects(Some(t))? {
                rs.push((to_int(need(r, "from")?)?, r));
            }
        }
        rs.sort_by_key(|x| x.0);
        let mut out = Vec::new();
        for (from, r) in rs {
            let mut entry = Map::new();
            entry.insert("from".into(), ival(from));
            entry.insert("to".into(), ival(to_int(need(r, "to")?)?));
            entry.insert("rate".into(), py::num(to_float(need(r, "rate")?)?));
            entry.insert("shape".into(), Value::String(to_str(need(r, "shape")?)));
            out.push(Value::Object(entry));
        }
        if out.is_empty() {
            new.shift_remove("turns");
        } else {
            new.insert("turns".into(), Value::Array(out));
        }
    }
    Ok(Value::Object(new))
}

fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

// --------------------------------------------------------------------------
// tick operations on the editor's edits ({"length", "yaw", "pitch", "keys", "strafe", "view_pitch"}):
// inserting and deleting ticks, changing the length, and generated turns.
// Keyframes after the edited ticks move by whole ticks with their values
// unchanged, so from the next keyframe on the run bakes to the same inputs.

pub type Range = Map<String, Value>;

#[derive(Clone, Debug)]
pub struct Edits {
    pub length: i64,
    pub yaw: Vec<TK>,
    pub pitch: Option<Vec<TK>>,
    pub keys: Vec<Range>,
    pub strafe: Vec<Range>,
    pub view_pitch: Option<Vec<TK>>,
    pub turns: Vec<Range>, // the editor's curve turns, by name
}

impl Edits {
    /// What `_with(edits)` reads: length and yaw are required.
    pub fn from_json(v: &Value) -> Res<Edits> {
        let d = obj(v, "edits")?;
        let length = to_int(need(d, "length")?)?;
        let yaw = track_from(need(d, "yaw")?)?;
        let pitch = match opt(d, "pitch") {
            Some(p) => Some(track_from(p)?),
            None => None,
        };
        let keys = match d.get("keys") {
            None => Vec::new(),
            Some(k) => list_of_objects(Some(k))?.into_iter().cloned().collect(),
        };
        let strafe = match d.get("strafe") {
            Some(s) if py_truthy(s) => list_of_objects(Some(s))?.into_iter().cloned().collect(),
            _ => Vec::new(),
        };
        let view_pitch = match opt(d, "view_pitch") {
            Some(p) => Some(track_from(p)?),
            None => None,
        };
        let turns = match d.get("turns") {
            Some(s) if py_truthy(s) => list_of_objects(Some(s))?.into_iter().cloned().collect(),
            _ => Vec::new(),
        };
        Ok(Edits { length, yaw, pitch, keys, strafe, view_pitch, turns })
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("length".into(), ival(self.length));
        m.insert("yaw".into(), track_json(&self.yaw));
        m.insert("pitch".into(), self.pitch.as_ref().map_or(Value::Null, |p| track_json(p)));
        m.insert("keys".into(), Value::Array(self.keys.iter().cloned().map(Value::Object).collect()));
        m.insert("strafe".into(), Value::Array(self.strafe.iter().cloned().map(Value::Object).collect()));
        m.insert("view_pitch".into(), self.view_pitch.as_ref().map_or(Value::Null, |p| track_json(p)));
        m.insert("turns".into(), Value::Array(self.turns.iter().cloned().map(Value::Object).collect()));
        Value::Object(m)
    }
}

fn canon(key: &Value) -> Res<String> {
    buttons::canonical_key(&to_str(key)).ok_or_else(|| TlError(format!("unknown key {}", py::repr_value(key))))
}

fn rget<'a>(r: &'a Range, k: &str) -> Res<&'a Value> {
    need(r, k)
}

/// Drop empty ranges and join touching or overlapping ranges of the same key and value.
pub fn merge_ranges(ranges: &[Range]) -> Res<Vec<Range>> {
    let mut by_key: Vec<(String, Vec<Range>)> = Vec::new();
    for r in ranges {
        if to_int(rget(r, "to")?)? > to_int(rget(r, "from")?)? {
            let ck = canon(rget(r, "key")?)?;
            let entry = with_fields(r, &[("key", Value::String(ck.clone()))]);
            match by_key.iter_mut().find(|(k, _)| *k == ck) {
                Some((_, list)) => list.push(entry),
                None => by_key.push((ck, vec![entry])),
            }
        }
    }
    let mut out: Vec<Range> = Vec::new();
    for (_, mut rs) in by_key {
        rs.sort_by(|a, b| cmp_raw(&a["from"], &b["from"]));
        let mut cur: Option<Range> = None;
        for r in rs {
            match cur.as_mut() {
                Some(c) if cmpv(r.get("from")) <= cmpv(c.get("to")) && py_eq(c.get("value"), r.get("value")) => {
                    let to = vmax(&c["to"], &r["to"]);
                    c.insert("to".into(), to);
                }
                _ => {
                    if let Some(c) = cur.take() {
                        out.push(c);
                    }
                    cur = Some(r);
                }
            }
        }
        if let Some(c) = cur {
            out.push(c);
        }
    }
    out.sort_by(|a, b| cmp_raw(&a["from"], &b["from"]).then_with(|| cmp_raw(&a["key"], &b["key"])));
    Ok(out)
}

/// Release key over ticks [a, b).
fn cut(ranges: &[Range], key: &str, a: i64, b: i64) -> Res<Vec<Range>> {
    let (af, bf) = (a as f64, b as f64);
    let mut out = Vec::new();
    for r in ranges {
        let (f, t) = (cmpv(r.get("from")), cmpv(r.get("to")));
        if canon(rget(r, "key")?)? != key || t <= af || f >= bf {
            out.push(r.clone());
            continue;
        }
        if f < af {
            out.push(with_fields(r, &[("to", ival(a))]));
        }
        if t > bf {
            out.push(with_fields(r, &[("from", ival(b))]));
        }
    }
    Ok(out)
}

/// Sorted strafe ranges, touching ones with the same offset, heading, swing and gain joined.
fn merge_strafe(ranges: &[Range]) -> Res<Vec<Range>> {
    let mut kept = Vec::new();
    for r in ranges {
        if to_int(rget(r, "to")?)? > to_int(rget(r, "from")?)? {
            kept.push(r.clone());
        }
    }
    kept.sort_by(|a, b| cmp_raw(&a["from"], &b["from"]));
    let offset = |r: &Range| -> Res<f64> {
        match opt(r, "offset") {
            Some(v) if py_truthy(v) => to_float(v),
            _ => Ok(0.0),
        }
    };
    let heading = |r: &Range| -> Res<Option<f64>> {
        match opt(r, "heading") {
            Some(v) => to_float(v).map(Some),
            None => Ok(None),
        }
    };
    let swing = |r: &Range| -> Res<Option<f64>> {
        match opt(r, "swing") {
            Some(v) => to_float(v).map(Some),
            None => Ok(None),
        }
    };
    let gain = |r: &Range| -> Res<Option<f64>> {
        match opt(r, "gain") {
            Some(v) => to_float(v).map(Some),
            None => Ok(None),
        }
    };
    let mut out: Vec<Range> = Vec::new();
    for r in kept {
        let join = match out.last() {
            Some(last) => {
                cmpv(r.get("from")) <= cmpv(last.get("to"))
                    && offset(&r)? == offset(last)?
                    && heading(&r)? == heading(last)?
                    && swing(&r)? == swing(last)?
                    && gain(&r)? == gain(last)?
            }
            None => false,
        };
        if join {
            let last = out.last_mut().unwrap();
            let to = vmax(&last["to"], &r["to"]);
            last.insert("to".into(), to);
        } else {
            out.push(r);
        }
    }
    Ok(out)
}

/// Auto-strafe off over ticks [a, b).
fn cut_strafe(ranges: &[Range], a: i64, b: i64) -> Vec<Range> {
    let (af, bf) = (a as f64, b as f64);
    let mut out = Vec::new();
    for r in ranges {
        let (f, t) = (cmpv(r.get("from")), cmpv(r.get("to")));
        if t <= af || f >= bf {
            out.push(r.clone());
            continue;
        }
        if f < af {
            out.push(with_fields(r, &[("to", ival(a))]));
        }
        if t > bf {
            out.push(with_fields(r, &[("from", ival(b))]));
        }
    }
    out
}

pub fn strafe_at(edits: &Edits, tick: i64) -> Option<&Range> {
    let t = tick as f64;
    edits.strafe.iter().find(|r| cmpv(r.get("from")) <= t && t < cmpv(r.get("to")))
}

/// count new ticks at tick `at`, each a copy of tick `at` (or of the last tick when
/// appending at the end): same yaw, pitch and held keys.
pub fn insert_ticks(edits: &Edits, at: i64, count: i64) -> Res<Edits> {
    let length = edits.length;
    if !(0 <= at && at <= length) {
        return err(format!("insert: tick {} is outside 0 .. {}", at, length));
    }
    if !(1 <= count && count <= MAX_TICKS - length) {
        return err(format!("insert: count must be 1 .. {}", MAX_TICKS - length));
    }
    let track = |tr: &[TK]| -> Res<Vec<TK>> {
        if at == length {
            return Ok(tr.to_vec()); // past the last key the value holds already
        }
        let pinned = pin(tr, Some(at), Side::Both, tr)?;
        let here = pinned.iter().find(|k| k.tick == at).and_then(|k| k.slope);
        let mut out: Vec<TK> =
            pinned.into_iter().map(|k| if k.tick >= at { k.with_value(k.tick + count, None) } else { k }).collect();
        // hold tick at's value over the new ticks (a cubic arriving there meets it with the slope it had)
        let mut hold = TK::new(at, tvalue(tr, at)?, "hold");
        hold.slope = here;
        out.push(hold);
        Ok(sorted_track(out))
    };
    let src = at.min(length - 1);
    let ranges = |rs: &[Range]| -> Res<Vec<Range>> {
        let mut out = Vec::new();
        for r in rs {
            let (f, t) = (to_int(rget(r, "from")?)?, to_int(rget(r, "to")?)?);
            if f <= src && src < t {
                out.push(with_fields(r, &[("to", ival(t + count))]));
            } else if f > src {
                out.push(with_fields(r, &[("from", ival(f + count)), ("to", ival(t + count))]));
            } else {
                out.push(r.clone());
            }
        }
        Ok(out)
    };
    Ok(Edits {
        length: length + count,
        yaw: track(&edits.yaw)?,
        pitch: match &edits.pitch {
            Some(p) => Some(track(p)?),
            None => None,
        },
        keys: merge_ranges(&ranges(&edits.keys)?)?,
        strafe: merge_strafe(&ranges(&edits.strafe)?)?,
        view_pitch: match &edits.view_pitch {
            Some(p) => Some(track(p)?),
            None => None,
        },
        turns: ranges(&edits.turns)?,
    })
}

/// Remove ticks [start, end). What came after moves back by end - start ticks.
pub fn delete_ticks(edits: &Edits, start: i64, end: i64) -> Res<Edits> {
    let length = edits.length;
    if !(0 <= start && start < end && end <= length) {
        return err(format!("delete: needs 0 <= from < to <= {}", length));
    }
    let n = end - start;
    if n >= length {
        return err("delete: a timeline needs at least one tick");
    }
    let track = |tr: &[TK]| -> Res<Vec<TK>> {
        // keep the curve as it was up to the cut and from the cut on
        let first = pin(tr, if start > 0 { Some(start - 1) } else { None }, Side::Before, tr)?;
        let pinned = pin(&first, if end < length { Some(end) } else { None }, Side::After, tr)?;
        let mut out: Vec<TK> = pinned
            .into_iter()
            .filter(|k| !(start <= k.tick && k.tick < end))
            .map(|k| if k.tick >= end { k.with_value(k.tick - n, None) } else { k })
            .collect();
        if out.is_empty() {
            // every key was in the deleted ticks: keep the value the curve had at the cut
            out = vec![TK::new(0, tvalue(tr, if end < length { end } else { start - 1 })?, "linear")];
        }
        Ok(out)
    };
    let mv = |x: i64| if x <= start { x } else if x <= end { start } else { x - n };
    let ranges = |rs: &[Range]| -> Res<Vec<Range>> {
        let mut out = Vec::new();
        for r in rs {
            let (f, t) = (to_int(rget(r, "from")?)?, to_int(rget(r, "to")?)?);
            let f2 = if f < start { f } else if f < end { start } else { f - n };
            let t2 = mv(t);
            if t2 > f2 {
                out.push(with_fields(r, &[("from", ival(f2)), ("to", ival(t2))]));
            }
        }
        Ok(out)
    };
    Ok(Edits {
        length: length - n,
        yaw: track(&edits.yaw)?,
        pitch: match &edits.pitch {
            Some(p) => Some(track(p)?),
            None => None,
        },
        keys: merge_ranges(&ranges(&edits.keys)?)?,
        strafe: merge_strafe(&ranges(&edits.strafe)?)?,
        view_pitch: match &edits.view_pitch {
            Some(p) => Some(track(p)?),
            None => None,
        },
        turns: ranges(&edits.turns)?,
    })
}

/// Turn auto-strafe ticks [first, end) into keyframes holding exactly the yaws the
/// game played there (yaws[i] for tick first + i, as float32 values in (-180, 180]),
/// so the timeline bakes to the same inputs and plays the same. sides (a straight
/// range: the game picked A or D each tick): the sidemove it played on each tick,
/// which becomes A and D key ranges.
pub fn freeze_strafe(
    edits: &Edits,
    first: i64,
    end: i64,
    yaws: &[f64],
    sides: Option<&[f64]>,
    fwds: Option<&[f64]>,
) -> Res<Edits> {
    let length = edits.length;
    if !(0 <= first && first < end && end <= length) || (yaws.len() as i64) < end - first {
        return err("freeze: the run doesn't cover those ticks");
    }
    let yaw = &edits.yaw;
    let mut reference = if first > 0 { tvalue(yaw, first - 1)? } else { yaws[0] };
    let mut values = Vec::new();
    for &y in &yaws[..(end - first) as usize] {
        // unwrapped, continuing from the curve before the range
        let v = f32_offset(reference + wrap_degrees(y - wrap_degrees(reference)), y);
        values.push(v);
        reference = v;
    }
    let p1 = pin(yaw, if first > 0 { Some(first - 1) } else { None }, Side::Before, yaw)?;
    let pinned = pin(&p1, if end < length { Some(end) } else { None }, Side::After, yaw)?;
    let mut out: Vec<TK> = pinned.into_iter().filter(|k| !(first <= k.tick && k.tick < end)).collect();
    for i in simplify(&values, 0.0) {
        out.push(TK::new(first + i as i64, values[i], "linear"));
    }
    let mut keys = edits.keys.clone();
    if let Some(sides) = sides {
        if (sides.len() as i64) < end - first {
            return err("freeze: the run doesn't cover those ticks");
        }
        let mut held: Option<Range> = None;
        for r in &edits.keys {
            let k = canon(rget(r, "key")?)?;
            if (k == "moveleft" || k == "moveright") && cmpv(r.get("from")) <= first as f64 && (first as f64) < cmpv(r.get("to")) {
                held = Some(r.clone());
                break;
            }
        }
        let mut ranges = edits.keys.clone();
        let cuts: &[&str] = if fwds.is_some() { &["moveleft", "moveright", "forward", "back"] } else { &["moveleft", "moveright"] };
        for k in cuts {
            ranges = cut(&ranges, k, first, end)?;
        }
        if let Some(fwds) = fwds {
            // a swing with a gain: every tick as hard as it was pressed
            if (fwds.len() as i64) < end - first {
                return err("freeze: the run doesn't cover those ticks");
            }
            for t in first..end {
                let (side, fwd) = (sides[(t - first) as usize], fwds[(t - first) as usize]);
                if side != 0.0 {
                    let mut r = Map::new();
                    r.insert("key".into(), Value::String((if side < 0.0 { "moveleft" } else { "moveright" }).into()));
                    r.insert("from".into(), ival(t));
                    r.insert("to".into(), ival(t + 1));
                    r.insert("value".into(), py::num(side.abs()));
                    ranges.push(r);
                }
                if fwd > 0.0 {
                    let mut r = Map::new();
                    r.insert("key".into(), Value::String("forward".into()));
                    r.insert("from".into(), ival(t));
                    r.insert("to".into(), ival(t + 1));
                    r.insert("value".into(), py::num(fwd));
                    ranges.push(r);
                }
            }
        }
        let at = |t: i64| sides[(t - first) as usize];
        let mut t = if fwds.is_some() { end } else { first };
        while t < end {
            let side = at(t);
            let mut u = t + 1;
            while u < end && (at(u) > 0.0) == (side > 0.0) && (at(u) < 0.0) == (side < 0.0) {
                u += 1;
            }
            if side != 0.0 {
                let mut r = Map::new();
                r.insert("key".into(), Value::String((if side < 0.0 { "moveleft" } else { "moveright" }).into()));
                r.insert("from".into(), ival(t));
                r.insert("to".into(), ival(u));
                if let Some(v) = held.as_ref().and_then(|h| opt(h, "value")) {
                    r.insert("value".into(), v.clone());
                }
                ranges.push(r);
            }
            t = u;
        }
        keys = merge_ranges(&ranges)?;
    }
    Ok(Edits { yaw: sorted_track(out), keys, strafe: cut_strafe(&edits.strafe, first, end), ..edits.clone() })
}

/// The timeline up to tick `at`, then the recording from `at` on.
///
/// rec: the inputs of a recording made from tick `at` of this timeline (its first
/// `at` ticks are the timeline as the game played it), with exact float32 values.
/// current: this timeline's baked inputs, to check the recording really starts
/// from it (yaw is skipped on auto-strafe ticks, where the game chose it). A view
/// pitch goes on with the pitch you played, so replays show where you really looked.
pub fn splice_recording(
    edits: &Edits,
    at: i64,
    rec: &[TickInput],
    move_speed: f64,
    current: &[TickInput],
    auto_ticks: &BTreeSet<i64>,
    start_pitch: f64,
) -> Res<Edits> {
    let length = edits.length;
    if !(1 <= at && at <= length) || rec.len() as i64 <= at {
        return err(format!("splice: the recording has to go past tick {} (it has {} ticks)", at, rec.len()));
    }
    for t in 0..at as usize {
        let (a, b) = (&rec[t], current.get(t).ok_or_else(|| TlError("list index out of range".into()))?);
        let same = a.forwardmove == b.forwardmove
            && a.sidemove == b.sidemove
            && a.buttons == b.buttons
            && py::float_to_bits(a.pitch) == py::float_to_bits(b.pitch)
            && (auto_ticks.contains(&(t as i64)) || py::float_to_bits(wrap_degrees(a.yaw)) == py::float_to_bits(b.yaw));
        if !same {
            return err(format!(
                "splice: the timeline changed since the recording started (tick {} differs); record again from this version",
                t
            ));
        }
    }
    let suffix = |track: &[TK], played: &[f64], mut reference: f64, unwrap: bool| -> Res<Vec<TK>> {
        let mut values = Vec::new();
        for &y in played {
            // yaw continues from the curve before the cut
            let v = if unwrap { f32_offset(reference + wrap_degrees(y - wrap_degrees(reference)), y) } else { y };
            values.push(v);
            reference = v;
        }
        let pinned = pin(track, Some(at - 1), Side::Before, track)?;
        let mut out: Vec<TK> = pinned.into_iter().filter(|k| k.tick < at).collect();
        for i in simplify(&values, 0.0) {
            out.push(TK::new(at + i as i64, values[i], "linear"));
        }
        Ok(out)
    };
    let a = at as usize;
    let yaw = &edits.yaw;
    let mut reference = tvalue(yaw, at - 1)?;
    reference = f32_offset(reference + wrap_degrees(rec[a - 1].yaw - wrap_degrees(reference)), rec[a - 1].yaw);
    let new_yaw = suffix(yaw, &rec[a..].iter().map(|ti| ti.yaw).collect::<Vec<_>>(), reference, true)?;
    let pitch = match &edits.pitch {
        Some(p) if !p.is_empty() => p.clone(),
        _ => vec![TK::new(0, start_pitch, "linear")],
    };
    let new_pitch = suffix(&pitch, &rec[a..].iter().map(|ti| ti.pitch).collect::<Vec<_>>(), 0.0, false)?;
    let new_view = match &edits.view_pitch {
        Some(v) if !v.is_empty() => Some(suffix(v, &rec[a..].iter().map(|ti| ti.pitch).collect::<Vec<_>>(), 0.0, false)?),
        _ => None,
    };

    let mut keys = Vec::new();
    for r in &edits.keys {
        if to_int(rget(r, "from")?)? >= at {
            continue;
        }
        keys.push(with_fields(r, &[("to", ival(to_int(rget(r, "to")?)?.min(at)))]));
    }
    for r in key_ranges(&rec[a..], move_speed) {
        let (f, t) = (to_int(&r["from"])?, to_int(&r["to"])?);
        keys.push(with_fields(&r, &[("from", ival(f + at)), ("to", ival(t + at))]));
    }
    let strafe = cut_strafe(&edits.strafe, at, length.max(rec.len() as i64));
    Ok(Edits {
        length: rec.len() as i64,
        yaw: new_yaw,
        pitch: Some(new_pitch),
        keys: merge_ranges(&keys)?,
        strafe,
        view_pitch: new_view,
        turns: cut_strafe(&edits.turns, at, length.max(rec.len() as i64)),
    })
}

/// Longer: the last tick repeats (keys held at the end stay held). Shorter: cut the end.
pub fn set_length(edits: &Edits, new_length: i64) -> Res<Edits> {
    let length = edits.length;
    if new_length > length {
        return insert_ticks(edits, length, new_length - length);
    }
    if new_length < length {
        return delete_ticks(edits, new_length, length);
    }
    Ok(edits.clone())
}

/// A turn from tick first to tick last (both included) at rate degrees per tick
/// (positive turns left), starting from the current yaw at first unless start_yaw
/// is given. shape "linear" turns at a constant rate, "smooth" eases in and out
/// over the same total, "auto" makes those ticks auto-strafe (offset degrees past
/// the max-gain angle; with a heading, straight that way: the game also picks A or D
/// each tick) and leaves the yaw keyframes as the fallback. keys maps key
/// names to "hold", "release" or "keep" for ticks first..last. after: "keep" leaves
/// later keyframes where they are, "shift" moves them by the change the turn made.
#[allow(clippy::too_many_arguments)]
const TURN_EASE: i64 = 8; // ticks at most a constant-speed turn eases in and out over, to meet the curve without a corner

pub fn make_turn(
    edits: &Edits,
    first: i64,
    last: i64,
    rate: f64,
    shape: &Value,
    keys: Option<&Map<String, Value>>,
    after: &Value,
    start_yaw: Option<f64>,
    offset: f64,
    heading: Option<f64>,
    swing: Option<f64>,
    gain: Option<f64>,
) -> Res<Edits> {
    let length = edits.length;
    if !(0 <= first && first < last && last <= length - 1) {
        return err(format!("turn: needs 0 <= from < to <= {}", length - 1));
    }
    let shape_s = shape.as_str().unwrap_or("");
    if !["linear", "smooth", "auto"].contains(&shape_s) {
        return err(format!("turn: shape must be linear, smooth or auto, got {}", py::repr_value(shape)));
    }
    if !(-90.0..=90.0).contains(&offset) {
        return err("turn: offset must be between -90 and 90 degrees");
    }
    let after_s = after.as_str().unwrap_or("");
    if after_s != "keep" && after_s != "shift" {
        return err(format!("turn: after must be keep or shift, got {}", py::repr_value(after)));
    }
    if !rate.is_finite() {
        return err("turn: rate must be a number");
    }
    let yaw = &edits.yaw;
    if let Some(s) = swing {
        if heading.is_none() || !(0.0 < s && s <= 90.0) {
            return err("turn: a swing is for straight auto-strafe, more than 0 and at most 90 degrees");
        }
    }
    if let Some(g) = gain {
        if swing.is_none() || !(75.0..=100.0).contains(&g) {
            return err("turn: a gain goes with a swing: how much of the most speed gain to keep, 75 to 100");
        }
    }
    let mut strafe = cut_strafe(&edits.strafe, first, last + 1);
    let mut turns = cut_strafe(&edits.turns, first, last + 1);
    let out: Vec<TK>;
    if shape_s == "auto" {
        out = yaw.clone();
        let mut r = Map::new();
        r.insert("from".into(), ival(first));
        r.insert("to".into(), ival(last + 1));
        r.insert("offset".into(), py::num(offset));
        if let Some(h) = heading {
            r.insert("heading".into(), py::num(h));
        }
        if let Some(s) = swing {
            r.insert("swing".into(), py::num(s));
        }
        if let Some(g) = gain {
            r.insert("gain".into(), py::num(g));
        }
        strafe.push(r);
        strafe = merge_strafe(&strafe)?;
    } else {
        let y0 = match start_yaw {
            Some(y) => y,
            None => tvalue(yaw, first)?,
        };
        // the curve's slopes where the turn meets it
        let (s0, s1) = (curve_slope(yaw, first, true), curve_slope(yaw, last, false));
        let span = last - first;
        let sloped = |tick: i64, value: f64, slope: f64| TK { slope: Some(slope), ..TK::new(tick, value, "cubic") };
        let mut turn = vec![sloped(first, y0, s0)];
        let r = if shape_s == "linear" { TURN_EASE.min(span / 4) } else { 0 };
        let y1 = if r > 0 {
            // ease into the rate, turn at it, ease out (a cubic whose slopes average the secant is a parabola)
            let ya = y0 + r as f64 * (s0 + rate) / 2.0;
            let yb = ya + rate * (span - 2 * r) as f64;
            turn.push(sloped(first + r, ya, rate));
            turn.push(sloped(last - r, yb, rate));
            yb + r as f64 * (rate + s1) / 2.0
        } else {
            y0 + rate * span as f64
        };
        let delta = y1 - tvalue(yaw, last)?;
        let mut pinned = pin(yaw, Some(first), Side::Before, yaw)?; // the curve before the turn stays as it was
        let shift = after_s == "shift";
        if shift {
            pinned = pin(&pinned, Some(last), Side::After, yaw)?; // and after it, moved by delta
        }
        let end_interp = segment_interp(&pinned, last);
        let mut o: Vec<TK> = pinned
            .into_iter()
            .filter(|k| !(first <= k.tick && k.tick <= last))
            .map(|k| if shift && k.tick > last { k.with_value(k.tick, Some(k.value + delta)) } else { k })
            .collect();
        if !shift {
            // the curve on to the next key leaves with the slope it had; that key keeps its own
            if let Some(nxt) = o.iter_mut().find(|k| k.tick > last) {
                if nxt.slope.is_none() {
                    nxt.slope = Some(curve_slope(yaw, nxt.tick, false));
                }
            }
            o.extend(turn);
            o.push(sloped(last, y1, s1));
        } else {
            // the curve after is the one it was, moved: the turn arrives with its slope
            o.extend(turn);
            let mut end = TK::new(last, y1, &end_interp);
            if end_interp == "cubic" {
                end.slope = Some(s1);
            }
            o.push(end);
        }
        out = o;
        let mut block = Map::new();
        block.insert("from".into(), ival(first));
        block.insert("to".into(), ival(last + 1));
        block.insert("rate".into(), py::num(rate));
        block.insert("shape".into(), Value::String(shape_s.to_string()));
        turns.push(block);
        turns.sort_by(|a, b| cmp_raw(&a["from"], &b["from"]));
    }
    let mut ranges: Vec<Range> = edits.keys.clone();
    if let Some(keys) = keys {
        for (key, action) in keys {
            let a = action.as_str().unwrap_or("");
            if a == "keep" {
                continue;
            }
            if a != "hold" && a != "release" {
                return err(format!("turn: key action must be hold, release or keep, got {}", py::repr_value(action)));
            }
            let ck = canon(&Value::String(key.clone()))?;
            ranges = cut(&ranges, &ck, first, last + 1)?;
            if a == "hold" {
                let mut r = Map::new();
                r.insert("key".into(), Value::String(ck));
                r.insert("from".into(), ival(first));
                r.insert("to".into(), ival(last + 1));
                ranges.push(r);
            }
        }
    }
    Ok(Edits { yaw: sorted_track(out), keys: merge_ranges(&ranges)?, strafe, turns, ..edits.clone() })
}

/// Run one editor operation; returns the new edits and a short description.
pub fn apply_op(edits: &Edits, op: &Map<String, Value>) -> Res<(Edits, String)> {
    let kind = op.get("op").cloned().unwrap_or(Value::Null);
    match kind.as_str() {
        Some("insert") => {
            let (at, count) = (to_int(need(op, "at")?)?, to_int(need(op, "count")?)?);
            Ok((insert_ticks(edits, at, count)?, format!("inserted {} tick(s) at tick {}", count, at)))
        }
        Some("delete") => {
            let (a, b) = (to_int(need(op, "from")?)?, to_int(need(op, "to")?)?);
            Ok((delete_ticks(edits, a, b)?, format!("deleted ticks {}..{} ({})", a, b - 1, b - a)))
        }
        Some("length") => {
            let n = to_int(need(op, "length")?)?;
            Ok((set_length(edits, n)?, format!("length {} ticks", n)))
        }
        Some("turn") => {
            let (a, b) = (to_int(need(op, "from")?)?, to_int(need(op, "to")?)?);
            let rate = to_float(need(op, "rate")?)?;
            let start = opt(op, "start_yaw");
            let shape = op.get("shape").cloned().unwrap_or_else(|| Value::String("linear".into()));
            let offset = match op.get("offset") {
                Some(v) if py_truthy(v) => to_float(v)?,
                _ => 0.0,
            };
            let keys = match opt(op, "keys") {
                Some(k) => Some(obj(k, "keys")?),
                None => None,
            };
            let after = op.get("after").cloned().unwrap_or_else(|| Value::String("keep".into()));
            let start = match start {
                Some(s) => Some(to_float(s)?),
                None => None,
            };
            let heading = match opt(op, "heading") {
                Some(h) => {
                    let h = to_float(h)?;
                    if !h.is_finite() {
                        return err("turn: heading must be a number");
                    }
                    Some(h)
                }
                None => None,
            };
            let swing = match opt(op, "swing") {
                Some(s) => Some(to_float(s)?),
                None => None,
            };
            let gain = match opt(op, "gain") {
                Some(s) => Some(to_float(s)?),
                None => None,
            };
            let new = make_turn(edits, a, b, rate, &shape, keys, &after, start, offset, heading, swing, gain)?;
            if shape.as_str() == Some("auto") {
                let straight = heading.map(|h| format!(" straight at {}°", py::g(h))).unwrap_or_default()
                    + &swing.map(|s| format!(", swinging ±{}°", py::g(s))).unwrap_or_default()
                    + &gain.map(|g| format!(" smoothly, keeping {}% of the speed gain", py::g(g))).unwrap_or_default();
                let past = if offset != 0.0 { format!(", {}° past the max-gain angle", py::g_signed(offset)) } else { String::new() };
                let how = if heading.is_some() {
                    "the game picks A or D and aims from the real velocity"
                } else {
                    "the game aims from the real velocity"
                };
                return Ok((new, format!("auto-strafe ticks {}..{}{}{}: {}", a, b, straight, past, how)));
            }
            let side = if rate > 0.0 {
                "left"
            } else if rate < 0.0 {
                "right"
            } else {
                "no turn"
            };
            let total = tvalue(&new.yaw, b)? - tvalue(&new.yaw, a)?;
            Ok((
                new,
                format!(
                    "turn {} ticks {}..{}: {}°/tick, {}° in total, joining the curve smoothly",
                    side,
                    a,
                    b,
                    py::g(rate.abs()),
                    py::g_signed(total)
                ),
            ))
        }
        _ => err(format!("unknown operation {}", py::repr_value(&kind))),
    }
}

// --------------------------------------------------------------------------
// JSON layout that stays pleasant to hand-edit

fn is_container(v: &Value) -> bool {
    v.is_object() || v.is_array()
}

fn flat(v: &Value) -> bool {
    match v {
        Value::Object(_) => false,
        Value::Array(a) => a.iter().all(|x| !is_container(x)),
        _ => true,
    }
}

pub fn format_json(v: &Value, level: usize) -> String {
    let pad = "  ".repeat(level);
    match v {
        Value::Object(o) => {
            if o.is_empty() {
                return "{}".into();
            }
            let one_line = py::dumps(v, false);
            if level > 0 && o.values().all(flat) && one_line.chars().count() + pad.len() <= 110 {
                return format!("{{ {} }}", &one_line[1..one_line.len() - 1]);
            }
            let items: Vec<String> = o
                .iter()
                .map(|(k, x)| format!("{}  {}: {}", pad, py::dumps(&Value::String(k.clone()), false), format_json(x, level + 1)))
                .collect();
            format!("{{\n{}\n{}}}", items.join(",\n"), pad)
        }
        Value::Array(a) => {
            if a.is_empty() {
                return "[]".into();
            }
            if a.iter().all(|x| !is_container(x)) {
                return py::dumps(v, false);
            }
            let items: Vec<String> = a.iter().map(|x| format!("{}  {}", pad, format_json(x, level + 1))).collect();
            format!("[\n{}\n{}]", items.join(",\n"), pad)
        }
        _ => py::dumps(v, false),
    }
}

/// Validate and write; returns the parsed timeline. Writes nothing if invalid.
pub fn save_document(path: &Path, doc: &Value, start_search_dirs: &[PathBuf]) -> Res<Timeline> {
    let tl = parse_timeline(doc, Some(path), start_search_dirs)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| TlError(crate::io_error(&e, parent)))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, format_json(doc, 0) + "\n").map_err(|e| TlError(crate::io_error(&e, &tmp)))?;
    std::fs::rename(&tmp, path).map_err(|e| TlError(crate::io_error(&e, path)))?;
    Ok(tl)
}

// --------------------------------------------------------------------------
// importing a recording (a baked file from tas_record) as an editable timeline

/// Indices of keyframes such that linear interpolation between them gives every
/// value back: exactly as float32 when tol == 0, else within tol.
pub fn simplify(values: &[f64], tol: f64) -> Vec<usize> {
    let n = values.len();
    if n <= 2 {
        return (0..n).collect();
    }
    // what the plugin ends up with for a value: float32 of the wrapped angle
    let target: Vec<f64> = values.iter().map(|v| py::f32_round(wrap_degrees(*v))).collect();
    let err_of = |i: usize, j: usize| -> (f64, usize) {
        let (mut worst, mut at) = (0.0f64, usize::MAX);
        let (a, b) = (values[i], values[j]);
        for t in i + 1..j {
            let v = a + (b - a) * (t - i) as f64 / (j - i) as f64; // same formula as interp's linear segment
            let e = if tol > 0.0 {
                (v - values[t]).abs()
            } else if py::f32_round(wrap_degrees(v)) == target[t] {
                0.0
            } else {
                (v - values[t]).abs() + 1e-12
            };
            if e > worst {
                worst = e;
                at = t;
            }
        }
        (worst, at)
    };
    let mut keep: BTreeSet<usize> = [0, n - 1].into_iter().collect();
    let mut stack = vec![(0usize, n - 1)];
    while let Some((i, j)) = stack.pop() {
        if j - i < 2 {
            continue;
        }
        let (worst, at) = err_of(i, j);
        if worst > tol {
            keep.insert(at);
            stack.push((i, at));
            stack.push((at, j));
        }
    }
    keep.into_iter().collect()
}

/// A setting value from a baked file: an int, a float or the text.
fn setting_num(s: &str) -> Value {
    match py::parse_float(s) {
        Err(_) => Value::String(s.to_string()),
        Ok(f) => {
            if f.is_finite() && f == f.trunc() && !s.contains('.') && f.abs() < 9.2e18 {
                ival(f as i64)
            } else {
                py::num(f)
            }
        }
    }
}

fn start_from_baked(baked: &BakedFile) -> Res<Value> {
    let get = |k: &str| baked.start_get(k);
    let missing = |k: &str| TlError(py::repr_str(k));
    let mut out = Map::new();
    for name in ["origin", "angles", "velocity"] {
        let key = format!("{}_bits", name);
        let bits = get(&key).ok_or_else(|| missing(&key))?;
        let mut vals = Vec::new();
        for b in bits {
            vals.push(py::num(py::bits_to_float(b).map_err(TlError)?));
        }
        out.insert(name.into(), Value::Array(vals));
        out.insert(key.clone(), Value::Array(bits.iter().map(|b| Value::String(b.clone())).collect()));
    }
    let zeros = vec!["0".to_string(); 3];
    let bv = get("base_velocity").unwrap_or(&zeros);
    let mut vals = Vec::new();
    for v in bv {
        vals.push(py::num(py::parse_float(v).map_err(TlError)?));
    }
    out.insert("base_velocity".into(), Value::Array(vals));
    let first = |k: &str, default: &str| -> Res<String> {
        match get(k) {
            None => Ok(default.to_string()),
            Some(v) => v.first().cloned().ok_or_else(|| TlError("list index out of range".into())),
        }
    };
    out.insert("base_velocity_flag".into(), Value::Bool(first("base_velocity_flag", "0")? != "0"));
    let og = first("on_ground", "auto")?;
    out.insert(
        "on_ground".into(),
        match og.as_str() {
            "yes" => Value::Bool(true),
            "no" => Value::Bool(false),
            _ => Value::String("auto".into()),
        },
    );
    out.insert("ground_entity".into(), ival(py::parse_int(&first("ground_entity", "0")?).map_err(TlError)?));
    out.insert("ducked".into(), Value::Bool(false));
    for name in ["gravity", "lagged_movement", "velocity_modifier", "stamina", "view_offset_z", "maxspeed"] {
        if get(name).is_some() {
            out.insert(name.into(), py::num(py::parse_float(&first(name, "")?).map_err(TlError)?));
        }
    }
    if get("health").is_some() {
        out.insert("health".into(), ival(py::parse_int(&first("health", "")?).map_err(TlError)?));
    }
    Ok(Value::Object(out))
}

/// The move magnitude a held movement key contributed on this tick.
fn magnitude(ti: &TickInput, key: &str, move_speed: f64) -> f64 {
    use buttons::*;
    match key {
        "forward" | "back" => {
            let both = ti.buttons & IN_FORWARD != 0 && ti.buttons & IN_BACK != 0;
            if both || ti.forwardmove == 0.0 {
                move_speed
            } else {
                ti.forwardmove.abs()
            }
        }
        "moveleft" | "moveright" => {
            let both = ti.buttons & IN_MOVELEFT != 0 && ti.buttons & IN_MOVERIGHT != 0;
            if both || ti.sidemove == 0.0 {
                move_speed
            } else {
                ti.sidemove.abs()
            }
        }
        _ => 0.0,
    }
}

/// Key ranges reproducing the recorded buttons and move values (ticks that keys
/// can't express show up in the import check).
pub fn key_ranges(inputs: &[TickInput], move_speed: f64) -> Vec<Range> {
    let mut ranges: Vec<Range> = Vec::new();
    let n = inputs.len();
    for bit_index in 0..32u32 {
        let key = buttons::key_for_bit(bit_index);
        let (bit, fsign, ssign) = buttons::key_info(&key);
        let mut t = 0;
        while t < n {
            if inputs[t].buttons & bit == 0 {
                t += 1;
                continue;
            }
            let (start, mag) = (t, magnitude(&inputs[t], &key, move_speed));
            t += 1;
            while t < n && inputs[t].buttons & bit != 0 && magnitude(&inputs[t], &key, move_speed) == mag {
                t += 1;
            }
            let mut r = Map::new();
            r.insert("key".into(), Value::String(buttons::display_name(&key)));
            r.insert("from".into(), ival(start as i64));
            r.insert("to".into(), ival(t as i64));
            if (fsign != 0 || ssign != 0) && mag != move_speed {
                r.insert("value".into(), py::num(mag));
            }
            ranges.push(r);
        }
    }
    ranges.sort_by(|a, b| cmp_raw(&a["from"], &b["from"]).then_with(|| cmp_raw(&a["key"], &b["key"])));
    ranges
}

/// Keep the unwrapped value an exact 360-degree multiple away from the recorded
/// float32, so wrapping it again restores the recorded value.
pub fn f32_offset(unwrapped: f64, recorded: f64) -> f64 {
    let turns = py::round_int((unwrapped - recorded) / 360.0);
    recorded + 360.0 * turns
}

/// A timeline document from a recording, plus a report. With tolerance 0 the
/// timeline bakes back to exactly the recorded float32 inputs.
pub fn recording_to_timeline(baked: &BakedFile, name: &str, tolerance: f64) -> Res<(Value, Map<String, Value>)> {
    let inputs = &baked.inputs;
    let mut yaw_u: Vec<f64> = Vec::new();
    for ti in inputs {
        // unwrap: no human turns more than 180 degrees in one tick
        let v = match yaw_u.last() {
            None => ti.yaw,
            Some(&prev) => prev + wrap_degrees(ti.yaw - wrap_degrees(prev)),
        };
        yaw_u.push(v);
    }
    let yaw_u: Vec<f64> = yaw_u.iter().zip(inputs).map(|(y, ti)| f32_offset(*y, ti.yaw)).collect();
    let pitch: Vec<f64> = inputs.iter().map(|ti| ti.pitch).collect();

    // Counter(...).most_common(1): the most frequent, the first seen among ties
    let mut mags: Vec<(f64, usize)> = Vec::new();
    for ti in inputs {
        for v in [ti.forwardmove, ti.sidemove] {
            if v != 0.0 {
                let m = v.abs();
                match mags.iter_mut().find(|(x, _)| *x == m) {
                    Some(slot) => slot.1 += 1,
                    None => mags.push((m, 1)),
                }
            }
        }
    }
    let move_speed = mags.iter().fold(None::<(f64, usize)>, |best, &(m, c)| match best {
        Some((_, bc)) if bc >= c => best,
        _ => Some((m, c)),
    });
    let move_speed = move_speed.map(|(m, _)| m).unwrap_or(400.0);

    let pair = |t: usize, v: f64| Value::Array(vec![ival(t as i64), py::num(v)]);
    let yaw_keys: Vec<Value> = simplify(&yaw_u, tolerance).into_iter().map(|t| pair(t, yaw_u[t])).collect();
    let pitch_keys: Vec<Value> = simplify(&pitch, tolerance).into_iter().map(|t| pair(t, pitch[t])).collect();
    let ranges = key_ranges(inputs, move_speed);
    let mut settings = Map::new();
    for (k, v) in &baked.settings {
        settings.insert(k.clone(), setting_num(v));
    }
    let track = |keys: Vec<Value>| {
        let mut m = Map::new();
        m.insert("default_interp".into(), Value::String("linear".into()));
        m.insert("keys".into(), Value::Array(keys));
        Value::Object(m)
    };
    let (n_yaw, n_pitch, n_ranges) = (yaw_keys.len(), pitch_keys.len(), ranges.len());
    let mut doc = Map::new();
    doc.insert(
        "_comment".into(),
        Value::String(format!(
            "Imported from the recording {} ({} ticks). Edit freely; the original recording is unchanged.",
            baked.name,
            inputs.len()
        )),
    );
    doc.insert("_baseline".into(), Value::String(baked.name.clone()));
    doc.insert("format".into(), Value::String("tasurf-timeline".into()));
    doc.insert("version".into(), ival(1));
    doc.insert("name".into(), Value::String(name.to_string()));
    doc.insert("map".into(), Value::String(baked.map.clone()));
    doc.insert("length".into(), ival(inputs.len() as i64));
    doc.insert("settings".into(), Value::Object(settings));
    doc.insert("start".into(), start_from_baked(baked)?);
    let mut input = Map::new();
    input.insert("move_speed".into(), py::num(move_speed));
    doc.insert("input".into(), Value::Object(input));
    doc.insert("yaw".into(), track(yaw_keys));
    doc.insert("pitch".into(), track(pitch_keys));
    doc.insert("keys".into(), Value::Array(ranges.into_iter().map(Value::Object).collect()));
    if baked.autobhop {
        // the plugin's recordings never are (they hold the jumps as played), but a baked timeline can be
        doc.insert("autobhop".into(), Value::Bool(true));
    }
    let doc = Value::Object(doc);

    // Check: bake it and compare with the recording, float32 bit for bit.
    let tl = parse_timeline(&doc, None, &[])?;
    let rebaked = bake_inputs(&tl);
    let mut diffs: Vec<(i64, Vec<&str>)> = Vec::new();
    for (a, b) in inputs.iter().zip(rebaked.iter()) {
        let mut fields = Vec::new();
        for (f, x, y) in [
            ("yaw", a.yaw, b.yaw),
            ("pitch", a.pitch, b.pitch),
            ("forwardmove", a.forwardmove, b.forwardmove),
            ("sidemove", a.sidemove, b.sidemove),
        ] {
            if py::float_to_bits(x) != py::float_to_bits(y) {
                fields.push(f);
            }
        }
        if a.buttons != b.buttons {
            fields.push("buttons");
        }
        if !fields.is_empty() {
            diffs.push((a.tick, fields));
        }
    }
    let mut report = Map::new();
    report.insert("ticks".into(), ival(inputs.len() as i64));
    report.insert("yaw_keys".into(), ival(n_yaw as i64));
    report.insert("pitch_keys".into(), ival(n_pitch as i64));
    report.insert("key_ranges".into(), ival(n_ranges as i64));
    report.insert("move_speed".into(), py::num(move_speed));
    report.insert("exact".into(), Value::Bool(diffs.is_empty()));
    report.insert("differing_ticks".into(), ival(diffs.len() as i64));
    report.insert(
        "first_differences".into(),
        Value::Array(diffs.iter().take(10).map(|(t, f)| Value::String(format!("tick {}: {}", t, f.join(", ")))).collect()),
    );
    Ok((doc, report))
}

pub fn import_recording(baked_path: &Path, out_path: &Path, name: &str, tolerance: f64) -> Res<Map<String, Value>> {
    let text = std::fs::read_to_string(baked_path).map_err(|e| TlError(crate::io_error(&e, baked_path)))?;
    let baked = read_baked(&text)?;
    let (doc, mut report) = recording_to_timeline(&baked, name, tolerance)?;
    save_document(out_path, &doc, &[])?;
    report.insert("path".into(), Value::String(out_path.display().to_string()));
    Ok(report)
}

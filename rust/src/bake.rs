//! Bake a timeline into the flat per-tick file the SourceMod plugin plays back
//! (tasurf/bake.py). The output is a pure function of the timeline, so baking the
//! same timeline twice gives byte-identical files, and the same bytes as Python.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::buttons;
use crate::camera::view_yaw_lines;
use crate::interp::{sample, wrap_degrees};
use crate::py;
use crate::timeline::{err, Res, StartState, Timeline, TlError};

pub const BAKED_FORMAT: &str = "tasurf-baked";
pub const BAKED_VERSION: i64 = 1; // files without auto-strafe stay version 1, byte for byte
pub const BAKED_VERSION_STRAFE: i64 = 2; // @strafe lines: the plugin aims those ticks itself
pub const BAKED_VERSION_STRAIGHT: i64 = 3; // @strafe lines with a heading: the plugin also picks A or D, to keep going that way
pub const BAKED_VERSION_AUTOBHOP: i64 = 4; // @autobhop: the plugin lets a held jump go in the air, so it jumps again on landing
pub const BAKED_VERSION_SWING: i64 = 5; // @strafe lines with a heading and a swing: a side is kept until the direction is that far past
pub const BAKED_VERSION_GAIN: i64 = 6; // and a gain: the swing turns smoothly, keeping that much of the most speed gain (percent)
pub const COLUMNS: &str = "tick,yaw,pitch,forwardmove,sidemove,buttons";
pub const VIEW_PITCH_PER_LINE: usize = 16; // values on one @view_pitch line

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TickInput {
    pub tick: i64,
    pub yaw: f64, // wrapped to (-180, 180]
    pub pitch: f64,
    pub forwardmove: f64,
    pub sidemove: f64,
    pub buttons: i64,
}

pub fn f6(x: f64) -> String {
    let s = py::fixed(x, 6);
    if s == "-0.000000" {
        "0.000000".into()
    } else {
        s
    }
}

pub fn f2(x: f64) -> String {
    let s = py::fixed(x, 2);
    if s == "-0.00" {
        "0.00".into()
    } else {
        s
    }
}

/// Decimal (6 places, more only when needed) that parses back to exactly
/// float32(x), which is what the plugin will use.
pub fn f32r(x: f64) -> String {
    let x32 = py::f32_round(x);
    let want = py::float_to_bits(x32);
    for places in 6..16 {
        let s = py::fixed(x32, places);
        if let Ok(back) = py::parse_float(&s) {
            if py::float_to_bits(back) == want {
                return s;
            }
        }
    }
    py::repr(x32)
}

pub fn bake_inputs(tl: &Timeline) -> Vec<TickInput> {
    let n = tl.length.max(0) as usize;
    let yaw = sample(&tl.yaw, tl.length);
    let pitch = match &tl.pitch {
        Some(p) => sample(p, tl.length),
        None => vec![tl.start.angles[0]; n],
    };
    let mut fwd = vec![0.0f64; n];
    let mut side = vec![0.0f64; n];
    let mut bits = vec![0i64; n];
    for kr in &tl.keys {
        let (bit, fsign, ssign) = buttons::key_info(&kr.key);
        let mag = kr.value.unwrap_or(tl.move_speed);
        for t in kr.start..kr.end {
            let t = t as usize;
            bits[t] |= bit;
            fwd[t] += fsign as f64 * mag; // W and S together cancel, as on a real client
            side[t] += ssign as f64 * mag;
        }
    }
    (0..n)
        .map(|t| TickInput {
            tick: t as i64,
            yaw: wrap_degrees(yaw[t]),
            pitch: pitch[t],
            forwardmove: fwd[t],
            sidemove: side[t],
            buttons: bits[t],
        })
        .collect()
}

fn vec_line(name: &str, v: &[f64]) -> String {
    format!("@start {} {}", name, v.iter().map(|x| f6(*x)).collect::<Vec<_>>().join(" "))
}

fn start_lines(st: &StartState) -> Vec<String> {
    let mut lines = Vec::new();
    let groups: [(&str, &[f64; 3], &Option<[String; 3]>); 3] = [
        ("origin", &st.origin, &st.origin_bits),
        ("angles", &st.angles, &st.angles_bits),
        ("velocity", &st.velocity, &st.velocity_bits),
    ];
    for (name, values, given) in groups {
        let bits: Vec<String> = match given {
            Some(b) => b.to_vec(),
            None => values.iter().map(|x| py::float_to_bits(*x)).collect(),
        };
        // the decimal of the exact float32, so the two lines always agree
        let exact: Vec<f64> = bits.iter().map(|b| py::bits_to_float(b).unwrap_or(f64::NAN)).collect();
        lines.push(vec_line(name, &exact));
        lines.push(format!("@start {}_bits {}", name, bits.join(" ")));
    }
    lines.push(vec_line("base_velocity", &st.base_velocity));
    lines.push(format!("@start base_velocity_flag {}", st.base_velocity_flag as i64));
    lines.push(format!("@start on_ground {}", st.on_ground));
    lines.push(format!("@start ground_entity {}", st.ground_entity));
    lines.push(format!("@start ducked {}", st.ducked as i64));
    lines.push(format!("@start gravity {}", f6(st.gravity)));
    lines.push(format!("@start lagged_movement {}", f6(st.lagged_movement)));
    let optional = [
        ("view_offset_z", st.view_offset_z),
        ("velocity_modifier", Some(st.velocity_modifier)),
        ("stamina", Some(st.stamina)),
        ("maxspeed", st.maxspeed),
    ];
    for (name, v) in optional {
        if let Some(v) = v {
            lines.push(format!("@start {} {}", name, f6(v)));
        }
    }
    if let Some(h) = st.health {
        lines.push(format!("@start health {}", h));
    }
    lines
}

/// A setting's value as the baked file writes it: strings as they are, whole
/// numbers without a decimal point, others with at most 6 places.
pub fn setting_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                let x = n.as_f64().unwrap_or(f64::NAN);
                if x.is_finite() && x == x.trunc() {
                    format!("{:.0}", x + 0.0)
                } else {
                    let s = py::fixed(x, 6);
                    s.trim_end_matches('0').trim_end_matches('.').to_string()
                }
            }
        }
        other => py::repr_value(other),
    }
}

/// camera: the yaw replays show on auto-strafe, tick -> yaw (camera::camera_yaws, from a run of these inputs), baked as
/// @view_yaw lines; like @view_pitch they aren't hashed, so the file's hash is the same with them or without.
pub fn render_baked(tl: &Timeline, inputs: &[TickInput]) -> String {
    render_baked_with(tl, inputs, None)
}

pub fn render_baked_with(tl: &Timeline, inputs: &[TickInput], camera: Option<&BTreeMap<usize, f64>>) -> String {
    let version = if tl.strafe.iter().any(|r| r.gain.is_some()) {
        BAKED_VERSION_GAIN
    } else if tl.strafe.iter().any(|r| r.swing.is_some()) {
        BAKED_VERSION_SWING
    } else if tl.autobhop {
        BAKED_VERSION_AUTOBHOP
    } else if tl.strafe.iter().any(|r| r.heading.is_some()) {
        BAKED_VERSION_STRAIGHT
    } else if tl.strafe.is_empty() {
        BAKED_VERSION
    } else {
        BAKED_VERSION_STRAFE
    };
    let mut body = vec![
        format!("@format {} {}", BAKED_FORMAT, version),
        format!("@name {}", tl.name),
        "@hash HASH".to_string(),
        format!("@map {}", tl.map),
        format!("@ticks {}", inputs.len()),
    ];
    body.extend(tl.settings.iter().map(|(k, v)| format!("@setting {} {}", k, setting_str(v))));
    body.extend(start_lines(&tl.start));
    if tl.autobhop {
        body.push("@autobhop".to_string()); // jump held in the air is let go, so landing with it held jumps
    }
    // ticks [from, to): the plugin sets the yaw from the real velocity (the tick lines' yaw is the fallback)
    // with a heading: the plugin picks A or D each tick to keep going that way (straight)
    body.extend(tl.strafe.iter().map(|r| {
        // with a swing as well: the side is kept until the direction has swung that far past the heading
        format!("@strafe {} {} {}", r.start, r.end, f6(r.offset))
            + &r.heading.map(|h| format!(" {}", f6(h))).unwrap_or_default()
            + &r.swing.map(|s| format!(" {}", f6(s))).unwrap_or_default()
            + &r.gain.map(|s| format!(" {}", f6(s))).unwrap_or_default()
    }));
    body.push(COLUMNS.to_string());
    body.extend(inputs.iter().map(|ti| {
        format!(
            "{},{},{},{},{},{}",
            ti.tick,
            f32r(ti.yaw),
            f32r(ti.pitch),
            f32r(ti.forwardmove),
            f32r(ti.sidemove),
            ti.buttons
        )
    }));
    body.extend(view_pitch_lines(tl, inputs.len()));
    if let Some(c) = camera {
        body.extend(view_yaw_lines(c));
    }
    let digest = content_hash(&body);
    body[2] = format!("@hash {}", digest);
    let mut out = String::from(
        "# TASurf baked timeline, generated by `python -m tasurf bake`.\n\
         # Edit the timeline JSON and re-bake; do not edit this file by hand.\n",
    );
    for line in &body {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The `@hash` value of a baked file's text.
pub fn hash_of(text: &str) -> String {
    py::splitlines(text)
        .into_iter()
        .find(|l| l.starts_with("@hash "))
        .and_then(|l| l.split_whitespace().nth(1).map(str::to_string))
        .unwrap_or_default()
}

/// `@view_pitch <first tick> <pitch>...`: the pitch replays show on each tick, which the
/// plugin sets on the replay bot's eyes after the tick's movement. Not part of the
/// hash, so changing it leaves the run, and which runs compare with it, as they were.
fn view_pitch_lines(tl: &Timeline, ticks: usize) -> Vec<String> {
    let keys = match &tl.view_pitch {
        Some(k) => k,
        None => return Vec::new(),
    };
    let values = sample(keys, ticks as i64);
    values
        .chunks(VIEW_PITCH_PER_LINE)
        .enumerate()
        .map(|(i, chunk)| {
            format!("@view_pitch {} {}", i * VIEW_PITCH_PER_LINE, chunk.iter().map(|v| f2(*v)).collect::<Vec<_>>().join(" "))
        })
        .collect()
}

fn hashed(line: &str) -> bool {
    !(line.starts_with('#') || line.starts_with("@hash ") || line.starts_with("@view_pitch ") || line.starts_with("@view_yaw "))
}

/// sha256 over every non-comment line except the @hash, @view_pitch and @view_yaw lines, first 16 hex digits.
pub fn content_hash<S: AsRef<str>>(lines: &[S]) -> String {
    let mut h = Sha256::new();
    for line in lines {
        let line = line.as_ref();
        if !hashed(line) {
            continue;
        }
        h.update(line.as_bytes());
        h.update(b"\n");
    }
    let hex: String = h.finalize().iter().map(|b| format!("{:02x}", b)).collect();
    hex[..16].to_string()
}

pub const FNV_OFFSET: u32 = 0x811C_9DC5;
pub const FNV_PRIME: u32 = 0x0100_0193;

pub fn fnv1a32(data: &[u8], mut h: u32) -> u32 {
    for b in data {
        h = (h ^ *b as u32).wrapping_mul(FNV_PRIME);
    }
    h
}

/// The hash the plugin writes into recordings (SourcePawn has no SHA-256):
/// FNV-1a 32 over the same lines as content_hash, as "fnv1a32:xxxxxxxx".
pub fn fnv1a32_hash<S: AsRef<str>>(lines: &[S]) -> String {
    let mut h = FNV_OFFSET;
    for line in lines {
        let line = line.as_ref();
        if !hashed(line) {
            continue;
        }
        h = fnv1a32(line.as_bytes(), h);
        h = fnv1a32(b"\n", h);
    }
    format!("fnv1a32:{:08x}", h)
}

pub fn summarize(tl: &Timeline, _inputs: &[TickInput]) -> Vec<String> {
    let mut out = vec![format!("{}: map {}, {} ticks, start from {}", tl.name, tl.map, tl.length, tl.start.source)];
    if !tl.settings.is_empty() {
        out.push(format!(
            "  expects {}",
            tl.settings.iter().map(|(k, v)| format!("{}={}", k, setting_str(v))).collect::<Vec<_>>().join(", ")
        ));
    }
    let mut keys: Vec<_> = tl.keys.iter().collect();
    keys.sort_by(|a, b| (a.start, &a.key).cmp(&(b.start, &b.key)));
    for kr in keys {
        let extra = kr.value.map(|v| format!(" value {}", py::g(v))).unwrap_or_default();
        let n = kr.end - kr.start;
        out.push(format!(
            "  {:<9} ticks {}..{} ({} tick{}){}",
            kr.key,
            kr.start,
            kr.end - 1,
            n,
            if n != 1 { "s" } else { "" },
            extra
        ));
    }
    out.push(format!(
        "  yaw keys: {}",
        tl.yaw.iter().map(|k| format!("{}:{}({})", k.tick, py::g(k.value), k.interp.name())).collect::<Vec<_>>().join(", ")
    ));
    out
}

/// camera_for(hash): the camera replays show for the inputs with that hash, or None (see render_baked).
pub type CameraFor<'a> = &'a dyn Fn(&str) -> Option<BTreeMap<usize, f64>>;

pub fn bake_to_file(tl: &Timeline, out_path: &Path, camera_for: Option<CameraFor>) -> Result<(String, Vec<TickInput>), String> {
    let inputs = bake_inputs(tl);
    let mut text = render_baked(tl, &inputs);
    if let Some(f) = camera_for {
        if let Some(camera) = f(&hash_of(&text)).filter(|c| !c.is_empty()) {
            text = render_baked_with(tl, &inputs, Some(&camera));
        }
    }
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| crate::io_error(&e, parent))?;
    }
    std::fs::write(out_path, &text).map_err(|e| crate::io_error(&e, out_path))?;
    Ok((text, inputs))
}

// --------------------------------------------------------------------------
// reading baked files (same rules as plugin/scripting/tasurf/baked.sp)

#[derive(Clone, Debug)]
#[allow(dead_code)] // hash, strafe and view_pitch: read by the plugin, kept as bake.py has them
pub struct BakedFile {
    pub name: String,
    pub hash: String,
    pub map: String,
    pub settings: Vec<(String, String)>,
    pub start: Vec<(String, Vec<String>)>,
    pub inputs: Vec<TickInput>,
    pub strafe: Vec<(i64, i64, f64, Option<f64>, Option<f64>, Option<f64>)>, // (from, to, offset, heading of a straight range, swing, gain)
    pub view_pitch: Option<Vec<f64>>,  // one per tick, or None (replays show the run's pitch)
    pub autobhop: bool,
    pub view_yaw: Option<BTreeMap<usize, f64>>, // the camera replays show on these ticks (the rest: the yaw played)
}

impl BakedFile {
    pub fn start_get(&self, name: &str) -> Option<&Vec<String>> {
        self.start.iter().find(|(k, _)| k == name).map(|(_, v)| v)
    }
}

/// dict[key] = value, keeping the first position of a repeated key.
pub fn put<V>(list: &mut Vec<(String, V)>, key: &str, value: V) {
    match list.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value,
        None => list.push((key.to_string(), value)),
    }
}

fn ctx_err<T, E: std::fmt::Display>(r: Result<T, E>) -> Res<T> {
    r.map_err(|e| TlError(e.to_string()))
}

pub fn read_baked(text: &str) -> Res<BakedFile> {
    let mut header: Vec<(String, String)> = Vec::new();
    let mut settings: Vec<(String, String)> = Vec::new();
    let mut start: Vec<(String, Vec<String>)> = Vec::new();
    let mut inputs: Vec<TickInput> = Vec::new();
    let mut strafe: Vec<(i64, i64, f64, Option<f64>, Option<f64>, Option<f64>)> = Vec::new();
    let mut view_pitch: Vec<f64> = Vec::new();
    let mut view_yaw: BTreeMap<usize, f64> = BTreeMap::new();
    let mut autobhop = false;
    let lines = py::splitlines(text);
    for (idx, raw) in lines.iter().enumerate() {
        let n = idx + 1;
        let line = py::strip(raw);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('@') {
            let parts: Vec<&str> = line.split(' ').collect();
            let tag = &parts[0][1..];
            let part = |i: usize| -> Res<&str> {
                parts.get(i).copied().ok_or_else(|| TlError("list index out of range".into()))
            };
            if tag == "setting" {
                put(&mut settings, part(1)?, part(2)?.to_string());
            } else if tag == "start" {
                put(&mut start, part(1)?, parts[2..].iter().map(|s| s.to_string()).collect());
            } else if tag == "autobhop" && parts.len() == 1 {
                autobhop = true;
            } else if tag == "strafe" && (4..=7).contains(&parts.len()) {
                strafe.push((
                    ctx_err(py::parse_int(parts[1]))?,
                    ctx_err(py::parse_int(parts[2]))?,
                    ctx_err(py::parse_float(parts[3]))?,
                    if parts.len() >= 5 { Some(ctx_err(py::parse_float(parts[4]))?) } else { None },
                    if parts.len() >= 6 { Some(ctx_err(py::parse_float(parts[5]))?) } else { None },
                    if parts.len() == 7 { Some(ctx_err(py::parse_float(parts[6]))?) } else { None },
                ));
            } else if tag == "view_pitch" && parts.len() >= 3 {
                if ctx_err(py::parse_int(parts[1]))? != view_pitch.len() as i64 {
                    return err(format!("line {}: expected @view_pitch {}, got {}", n, view_pitch.len(), parts[1]));
                }
                for p in &parts[2..] {
                    view_pitch.push(ctx_err(py::parse_float(p))?);
                }
            } else if tag == "view_yaw" && parts.len() >= 3 {
                let first = ctx_err(py::parse_int(parts[1]))?;
                for (i, p) in parts[2..].iter().enumerate() {
                    view_yaw.insert((first + i as i64) as usize, ctx_err(py::parse_float(p))?);
                }
            } else if ["format", "name", "hash", "map", "ticks"].contains(&tag) {
                put(&mut header, tag, parts[1..].join(" "));
            } else {
                return err(format!("line {}: unknown header {}", n, parts[0]));
            }
            continue;
        }
        if line.starts_with("tick,") {
            continue;
        }
        let cols: Vec<&str> = line.split(',').collect();
        if cols.len() != 6 && cols.len() != 10 {
            return err(format!("line {}: expected 6 or 10 columns, got {}", n, cols.len()));
        }
        let tick = ctx_err(py::parse_int(cols[0]))?;
        if tick != inputs.len() as i64 {
            return err(format!("line {}: expected tick {}, got {}", n, inputs.len(), tick));
        }
        let mut v = [0.0f64; 4];
        for i in 0..4 {
            v[i] = if cols.len() == 10 {
                // recordings: exact float32 bits of yaw, pitch, forwardmove, sidemove
                ctx_err(py::bits_to_float(cols[6 + i]))?
            } else {
                ctx_err(py::parse_float(cols[1 + i]))?
            };
        }
        let buttons = ctx_err(py::parse_int(cols[5]))?;
        inputs.push(TickInput { tick, yaw: v[0], pitch: v[1], forwardmove: v[2], sidemove: v[3], buttons });
    }
    let get = |k: &str| header.iter().find(|(h, _)| h == k).map(|(_, v)| v.clone());
    let format = get("format");
    let versions = [
        BAKED_VERSION,
        BAKED_VERSION_STRAFE,
        BAKED_VERSION_STRAIGHT,
        BAKED_VERSION_AUTOBHOP,
        BAKED_VERSION_SWING,
        BAKED_VERSION_GAIN,
    ];
    let version = match versions.iter().find(|v| format.as_deref() == Some(format!("{} {}", BAKED_FORMAT, v).as_str())) {
        Some(v) => *v,
        None => {
            return err(format!(
                "not a {} {}, {}, {}, {}, {} or {} file",
                BAKED_FORMAT,
                BAKED_VERSION,
                BAKED_VERSION_STRAFE,
                BAKED_VERSION_STRAIGHT,
                BAKED_VERSION_AUTOBHOP,
                BAKED_VERSION_SWING,
                BAKED_VERSION_GAIN
            ))
        }
    };
    if !strafe.is_empty() && version < BAKED_VERSION_STRAFE {
        return err(format!("@strafe needs {} {}", BAKED_FORMAT, BAKED_VERSION_STRAFE));
    }
    if strafe.iter().any(|r| r.3.is_some()) && version < BAKED_VERSION_STRAIGHT {
        return err(format!("@strafe with a heading needs {} {}", BAKED_FORMAT, BAKED_VERSION_STRAIGHT));
    }
    if strafe.iter().any(|r| r.4.is_some()) && version < BAKED_VERSION_SWING {
        return err(format!("@strafe with a swing needs {} {}", BAKED_FORMAT, BAKED_VERSION_SWING));
    }
    if strafe.iter().any(|r| r.5.is_some()) && version < BAKED_VERSION_GAIN {
        return err(format!("@strafe with a swing's gain needs {} {}", BAKED_FORMAT, BAKED_VERSION_GAIN));
    }
    if autobhop && version < BAKED_VERSION_AUTOBHOP {
        return err(format!("@autobhop needs {} {}", BAKED_FORMAT, BAKED_VERSION_AUTOBHOP));
    }
    let missing = |k: &str| TlError(format!("'{}'", k));
    let ticks = get("ticks").ok_or_else(|| missing("ticks"))?;
    if ctx_err(py::parse_int(&ticks))? != inputs.len() as i64 {
        return err(format!("@ticks says {} but the file has {} tick lines", ticks, inputs.len()));
    }
    if !view_pitch.is_empty() && view_pitch.len() != inputs.len() {
        return err(format!("@view_pitch has {} values for {} ticks", view_pitch.len(), inputs.len()));
    }
    if view_yaw.keys().any(|&t| t >= inputs.len()) {
        return err(format!("@view_yaw names a tick past the last ({})", inputs.len() as i64 - 1));
    }
    let hash = get("hash").ok_or_else(|| missing("hash"))?;
    let expected = if hash.starts_with("fnv1a32:") { fnv1a32_hash(&lines) } else { content_hash(&lines) };
    if expected != hash {
        return err("@hash does not match the content (was the file edited by hand?)");
    }
    Ok(BakedFile {
        name: get("name").ok_or_else(|| missing("name"))?,
        hash,
        map: get("map").ok_or_else(|| missing("map"))?,
        settings,
        start,
        inputs,
        strafe,
        view_pitch: if view_pitch.is_empty() { None } else { Some(view_pitch) },
        autobhop,
        view_yaw: if view_yaw.is_empty() { None } else { Some(view_yaw) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32r_is_exact() {
        assert_eq!(f32r(0.0), "0.000000");
        assert_eq!(f32r(-0.0), "0.000000");
        assert_eq!(f32r(400.0), "400.000000");
        assert_eq!(f32r(92.75032806396484), "92.750328");
        let s = f32r(0.1);
        assert_eq!(py::float_to_bits(s.parse().unwrap()), py::float_to_bits(0.1));
    }

    #[test]
    fn settings_text() {
        let v: Value = serde_json::from_str("[800, 150.0, 0.015, \"abc\", 1e20, 0.30000001]").unwrap();
        let s: Vec<String> = v.as_array().unwrap().iter().map(setting_str).collect();
        assert_eq!(s, ["800", "150", "0.015", "abc", "100000000000000000000", "0.3"]);
    }

    #[test]
    fn fnv() {
        assert_eq!(fnv1a32(b"", FNV_OFFSET), 0x811C9DC5);
        assert_eq!(fnv1a32(b"a", FNV_OFFSET), 0xE40C292C);
    }
}

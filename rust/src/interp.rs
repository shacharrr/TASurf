//! Keyframe interpolation for timeline tracks (tasurf/interp.py).
//!
//! Values are plain floats in degrees. Yaw is unwrapped: 170 -> 190 turns 20 degrees
//! through 180. Nothing here wraps angles except `wrap_degrees`, which the baker uses
//! once. Each key's interp shapes the segment from that key to the next one. A key can
//! say its slope (degrees per tick), which cubic segments at it use instead of the estimate,
//! and for a joint segment its line's slope and its joint's width (see interp.py).

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interp {
    Hold,
    Linear,
    Smooth,
    Cubic,
    Joint,
}

pub const INTERP_MODES: [&str; 5] = ["hold", "linear", "smooth", "cubic", "joint"];
pub const JOINT_PASSES: usize = 6; // how many times each joint segment's line is fitted to its neighbours'
const REACH: f64 = 3.0 / 16.0; // a half joint moves where its line reaches by this much of half its width times the rate change

impl Interp {
    pub fn parse(s: &str) -> Option<Interp> {
        match s {
            "hold" => Some(Interp::Hold),
            "linear" => Some(Interp::Linear),
            "smooth" => Some(Interp::Smooth),
            "cubic" => Some(Interp::Cubic),
            "joint" => Some(Interp::Joint),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Interp::Hold => "hold",
            Interp::Linear => "linear",
            Interp::Smooth => "smooth",
            Interp::Cubic => "cubic",
            Interp::Joint => "joint",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub tick: i64,
    pub value: f64,
    pub interp: Interp,
    pub slope: Option<f64>, // degrees per tick where a cubic segment meets this key (None: from the neighbours); a joint segment's line
    pub joint: Option<f64>, // ticks the joint at this key takes (None: a quarter of the shorter segment beside it, each side)
}

impl Key {
    pub fn new(tick: i64, value: f64, interp: Interp) -> Key {
        Key { tick, value, interp, slope: None, joint: None }
    }
}

/// Wrap to (-180, 180], the range the CS:S client sends. A value already in that
/// range comes back unchanged bit for bit, which keeps imported recordings exact.
pub fn wrap_degrees(angle: f64) -> f64 {
    angle - 360.0 * ((angle - 180.0) / 360.0).ceil()
}

/// The slope a key says. A joint key's line counts only together with its joint's width, the two an edit writes
/// in (interp.py's _said_slope): a slope alone there is one a smooth curve left, and the line is fitted.
fn said_slope(k: &Key) -> Option<f64> {
    if k.interp != Interp::Joint || k.joint.is_some() {
        k.slope
    } else {
        None
    }
}

/// Tangent (degrees per tick) at key i for cubic segments: the key's own; where a joint segment's line arrives
/// or leaves, that line's; else from the neighbouring keys.
fn slope(keys: &[Key], i: usize) -> f64 {
    if joint_half(keys, i) > 0.0 {
        // a joint kept at the key a cubic leaves (an edit): the rate the joint has there
        let ml = joint_slopes(keys, i - 1, i - 1)[&(i - 1)];
        return ml + (keys[i].slope.unwrap_or(0.0) - ml) * 0.5;
    }
    if let Some(s) = said_slope(&keys[i]) {
        return s;
    }
    if i > 0 && joint_segment(keys, i - 1) {
        return joint_slopes(keys, i - 1, i - 1)[&(i - 1)];
    }
    if joint_segment(keys, i) {
        return joint_slopes(keys, i, i)[&i];
    }
    if keys.len() < 2 {
        return 0.0;
    }
    let (a, b) = if i == 0 {
        (keys[0], keys[1])
    } else if i == keys.len() - 1 {
        (keys[keys.len() - 2], keys[keys.len() - 1])
    } else {
        (keys[i - 1], keys[i + 1])
    };
    (b.value - a.value) / (b.tick - a.tick) as f64
}

/// The slope (degrees per tick) cubic segments meet key i with: its own, or the neighbours' estimate.
pub fn key_slope(keys: &[Key], i: usize) -> f64 {
    slope(keys, i)
}

/// The slope (degrees per tick) of the cubic segment keys[i] .. keys[i + 1] at a tick inside it.
pub fn segment_slope(keys: &[Key], i: usize, tick: i64) -> f64 {
    let (k0, k1) = (keys[i], keys[i + 1]);
    let span = (k1.tick - k0.tick) as f64;
    let s = (tick - k0.tick) as f64 / span;
    let m0 = slope(keys, i) * span;
    let m1 = slope(keys, i + 1) * span;
    let s2 = s * s;
    ((6.0 * s2 - 6.0 * s) * k0.value
        + (3.0 * s2 - 4.0 * s + 1.0) * m0
        + (-6.0 * s2 + 6.0 * s) * k1.value
        + (3.0 * s2 - 2.0 * s) * m1)
        / span
}

fn joint_segment(keys: &[Key], j: usize) -> bool {
    j + 1 < keys.len() && keys[j].interp == Interp::Joint
}

/// Half the width (ticks) of the joint at key i, 0 where there's none (interp.py's joint_half).
pub fn joint_half(keys: &[Key], i: usize) -> f64 {
    if i == 0 || i + 1 >= keys.len() || !joint_segment(keys, i - 1) {
        return 0.0;
    }
    let k = keys[i];
    if !joint_segment(keys, i) {
        return match (k.slope, k.joint) {
            (Some(_), Some(w)) => w.max(0.0) / 2.0,
            _ => 0.0,
        };
    }
    if let Some(w) = k.joint {
        return w.max(0.0) / 2.0;
    }
    (k.tick - keys[i - 1].tick).min(keys[i + 1].tick - k.tick) as f64 / 4.0
}

/// The line leaving key j: its joint segment's, or the slope the key says (a joint arriving there).
fn line_out(keys: &[Key], j: usize, m: &HashMap<usize, f64>) -> f64 {
    if joint_segment(keys, j) {
        m[&j]
    } else {
        keys[j].slope.unwrap_or(0.0)
    }
}

/// The line's slope of each joint segment lo .. hi (interp.py's joint_slopes): the key's own, or fitted to the
/// neighbours' over JOINT_PASSES passes from the key-to-key slopes.
pub fn joint_slopes(keys: &[Key], lo: usize, hi: usize) -> HashMap<usize, f64> {
    let last_seg = keys.len() as i64 - 2;
    let (lo, hi) = (lo as i64, hi as i64);
    let p_max = JOINT_PASSES as i64;
    let mut m: HashMap<usize, f64> = HashMap::new();
    for j in (lo - p_max).max(0)..=(hi + p_max).min(last_seg) {
        let j = j as usize;
        if joint_segment(keys, j) {
            m.insert(j, (keys[j + 1].value - keys[j].value) / (keys[j + 1].tick - keys[j].tick) as f64);
        }
    }
    for p in 1..=p_max {
        let mut nxt: HashMap<usize, f64> = HashMap::new();
        for j in (lo - p_max + p).max(0)..=(hi + p_max - p).min(last_seg) {
            let j = j as usize;
            if !joint_segment(keys, j) {
                continue;
            }
            let span = (keys[j + 1].tick - keys[j].tick) as f64;
            let (hl, hr) = (joint_half(keys, j), joint_half(keys, j + 1));
            let left = if hl > 0.0 { m[&(j - 1)] } else { 0.0 };
            let right = if hr > 0.0 { line_out(keys, j + 1, &m) } else { 0.0 };
            let den = span - REACH * hl - REACH * hr;
            if den <= 0.25 * span {
                // joints far wider than the segment: keep the slope it has
                nxt.insert(j, m[&j]);
            } else {
                nxt.insert(j, (keys[j + 1].value - keys[j].value - REACH * hl * left - REACH * hr * right) / den);
            }
        }
        m = nxt;
    }
    let mut out = HashMap::new();
    for j in lo.max(0)..=hi.min(last_seg) {
        let j = j as usize;
        if joint_segment(keys, j) {
            out.insert(j, said_slope(&keys[j]).unwrap_or_else(|| m[&j]));
        }
    }
    out
}

/// How far the yaw goes from a joint's key to d ticks after it (d < 0: before), inside the joint, where the rate
/// goes from ml to mr along a smoothstep over the 2h ticks round the key.
fn joint_part(ml: f64, mr: f64, h: f64, d: f64) -> f64 {
    let w = 2.0 * h;
    let u = (d + h) / w;
    w * (ml * u + (mr - ml) * (u * u * u * (1.0 - 0.5 * u))) - w * (ml * 0.5 + (mr - ml) * 0.09375)
}

/// The joint segment keys[i] .. keys[i + 1] at a tick inside it; m: the joint segments' slopes, i - 1 .. i + 1.
fn joint_value(keys: &[Key], i: usize, tick: f64, m: &HashMap<usize, f64>) -> f64 {
    let (k0, k1) = (keys[i], keys[i + 1]);
    if tick == k0.tick as f64 {
        return k0.value; // the key itself (even where the next key's joint reaches past it, on a short segment)
    }
    let (hl, hr) = (joint_half(keys, i), joint_half(keys, i + 1));
    let (a, b) = (k0.tick as f64 + hl, k1.tick as f64 - hr);
    if tick < a {
        return k0.value + joint_part(m[&(i - 1)], m[&i], hl, tick - k0.tick as f64);
    }
    if tick > b {
        return k1.value + joint_part(m[&i], line_out(keys, i + 1, m), hr, tick - k1.tick as f64);
    }
    let ya = if hl > 0.0 { k0.value + joint_part(m[&(i - 1)], m[&i], hl, hl) } else { k0.value };
    let yb = if hr > 0.0 { k1.value + joint_part(m[&i], line_out(keys, i + 1, m), hr, -hr) } else { k1.value };
    if b > a {
        ya + (yb - ya) * ((tick - a) / (b - a))
    } else {
        ya
    }
}

/// The slope (degrees per tick) of the joint segment keys[i] .. keys[i + 1] at a tick in it (its ends included).
pub fn joint_rate(keys: &[Key], i: usize, tick: i64) -> f64 {
    let m = joint_slopes(keys, i.saturating_sub(1), i + 1);
    let (k0, k1) = (keys[i], keys[i + 1]);
    let (hl, hr) = (joint_half(keys, i), joint_half(keys, i + 1));
    let (a, b) = (k0.tick as f64 + hl, k1.tick as f64 - hr);
    let t = tick as f64;
    if t < a || t > b {
        let (ml, mr, h, d) =
            if t < a { (m[&(i - 1)], m[&i], hl, t - k0.tick as f64) } else { (m[&i], line_out(keys, i + 1, &m), hr, t - k1.tick as f64) };
        let u = (d + h) / (2.0 * h);
        return ml + (mr - ml) * (u * u * (3.0 - 2.0 * u));
    }
    if b > a {
        return (joint_value(keys, i, b, &m) - joint_value(keys, i, a, &m)) / (b - a);
    }
    m[&i]
}

fn segment_value(keys: &[Key], i: usize, tick: i64) -> f64 {
    let (k0, k1) = (keys[i], keys[i + 1]);
    let span = (k1.tick - k0.tick) as f64;
    let s = (tick - k0.tick) as f64 / span;
    match k0.interp {
        Interp::Hold => k0.value,
        Interp::Linear => k0.value + (k1.value - k0.value) * s,
        Interp::Smooth => k0.value + (k1.value - k0.value) * (s * s * (3.0 - 2.0 * s)),
        Interp::Cubic => {
            let m0 = slope(keys, i) * span;
            let m1 = slope(keys, i + 1) * span;
            let s2 = s * s;
            let s3 = s2 * s;
            (2.0 * s3 - 3.0 * s2 + 1.0) * k0.value
                + (s3 - 2.0 * s2 + s) * m0
                + (-2.0 * s3 + 3.0 * s2) * k1.value
                + (s3 - s2) * m1
        }
        Interp::Joint => joint_value(keys, i, tick as f64, &joint_slopes(keys, i.saturating_sub(1), i + 1)),
    }
}

/// Value of a track at an integer tick. `keys` must be sorted by tick and not empty.
pub fn value_at(keys: &[Key], tick: i64) -> f64 {
    if tick <= keys[0].tick {
        return keys[0].value;
    }
    if tick >= keys[keys.len() - 1].tick {
        return keys[keys.len() - 1].value;
    }
    let (mut lo, mut hi) = (0usize, keys.len() - 1);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if keys[mid].tick <= tick {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    segment_value(keys, lo, tick)
}

/// Values for ticks 0 .. length-1.
pub fn sample(keys: &[Key], length: i64) -> Vec<f64> {
    if !(0..keys.len().saturating_sub(1)).any(|j| joint_segment(keys, j)) {
        return (0..length).map(|t| value_at(keys, t)).collect();
    }
    let m = joint_slopes(keys, 0, keys.len() - 2); // once for the whole track: the same numbers value_at works out
    let n = keys.len();
    (0..length)
        .map(|t| {
            if keys[0].tick < t && t < keys[n - 1].tick {
                let (mut lo, mut hi) = (0usize, n - 1);
                while hi - lo > 1 {
                    let mid = (lo + hi) / 2;
                    if keys[mid].tick <= t {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                if keys[lo].interp == Interp::Joint {
                    return joint_value(keys, lo, t as f64, &m);
                }
            }
            value_at(keys, t)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_keeps_180() {
        assert_eq!(wrap_degrees(180.0), 180.0);
        assert_eq!(wrap_degrees(-180.0), 180.0);
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(540.0), 180.0);
    }

    #[test]
    fn modes() {
        let k = |t, v, i| Key::new(t, v, i);
        let lin = [k(0, 0.0, Interp::Linear), k(10, 10.0, Interp::Linear)];
        assert_eq!(value_at(&lin, 5), 5.0);
        assert_eq!(value_at(&lin, -3), 0.0);
        assert_eq!(value_at(&lin, 30), 10.0);
        let hold = [k(0, 1.0, Interp::Hold), k(10, 2.0, Interp::Linear)];
        assert_eq!(value_at(&hold, 9), 1.0);
        let smooth = [k(0, 0.0, Interp::Smooth), k(10, 10.0, Interp::Linear)];
        assert_eq!(value_at(&smooth, 5), 5.0);
        assert!(value_at(&smooth, 1) < 1.0);
    }

    #[test]
    fn joints_go_through_every_key_with_straight_lines_between() {
        let k = |t, v| Key::new(t, v, Interp::Joint);
        let keys = [k(0, 0.0), k(40, -80.0), k(80, 0.0), k(100, 30.0)];
        let v = sample(&keys, 101);
        for key in &keys {
            assert_eq!(v[key.tick as usize], key.value);
        }
        assert_eq!(joint_half(&keys, 1), 10.0);
        let r: Vec<f64> = (0..100).map(|t| v[t + 1] - v[t]).collect();
        assert!((r[0] - r[29]).abs() < 1e-9 && r[0] < -2.0); // a steady rate, tilted past key to key
        assert!((0..99).all(|t| (r[t + 1] - r[t]).abs() < 0.35)); // no corner
        assert_eq!((0..101).map(|t| value_at(&keys, t)).collect::<Vec<_>>(), v); // the same numbers either way
        let sharp = [k(0, 0.0), Key { joint: Some(0.0), ..k(40, -80.0) }, k(80, 0.0)];
        assert_eq!((value_at(&sharp, 40) - value_at(&sharp, 39), value_at(&sharp, 41) - value_at(&sharp, 40)), (-2.0, 2.0));
    }

    #[test]
    fn a_slope_a_smooth_curve_left_on_a_joint_key_is_ignored() {
        let k = |t, v, s: Option<f64>| Key { slope: s, ..Key::new(t, v, Interp::Joint) };
        let first = Key { slope: Some(11.3), ..Key::new(0, 2.6, Interp::Cubic) };
        let last = Key::new(30, 37.0, Interp::Linear);
        let clean = [first, k(5, 62.5, None), k(15, 36.8, None), k(25, 37.5, None), last];
        let left = [first, k(5, 62.5, Some(11.98)), k(15, 36.8, Some(-2.57)), k(25, 37.5, Some(11.98)), last];
        assert_eq!(sample(&left, 31), sample(&clean, 31));
        let said = [first, Key { joint: Some(2.5), ..k(5, 62.5, Some(-1.0)) }, clean[2], clean[3], last];
        assert_eq!(joint_slopes(&said, 1, 1)[&1], -1.0); // with its width, the slope is the line's
    }
}

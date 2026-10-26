//! Keyframe interpolation for timeline tracks (tasurf/interp.py).
//!
//! Values are plain floats in degrees. Yaw is unwrapped: 170 -> 190 turns 20 degrees
//! through 180. Nothing here wraps angles except `wrap_degrees`, which the baker uses
//! once. Each key's interp shapes the segment from that key to the next one. A key can
//! say its slope (degrees per tick), which cubic segments at it use instead of the estimate.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interp {
    Hold,
    Linear,
    Smooth,
    Cubic,
}

pub const INTERP_MODES: [&str; 4] = ["hold", "linear", "smooth", "cubic"];

impl Interp {
    pub fn parse(s: &str) -> Option<Interp> {
        match s {
            "hold" => Some(Interp::Hold),
            "linear" => Some(Interp::Linear),
            "smooth" => Some(Interp::Smooth),
            "cubic" => Some(Interp::Cubic),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Interp::Hold => "hold",
            Interp::Linear => "linear",
            Interp::Smooth => "smooth",
            Interp::Cubic => "cubic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub tick: i64,
    pub value: f64,
    pub interp: Interp,
    pub slope: Option<f64>, // degrees per tick where a cubic segment meets this key (None: from the neighbours)
}

impl Key {
    pub fn new(tick: i64, value: f64, interp: Interp) -> Key {
        Key { tick, value, interp, slope: None }
    }
}

/// Wrap to (-180, 180], the range the CS:S client sends. A value already in that
/// range comes back unchanged bit for bit, which keeps imported recordings exact.
pub fn wrap_degrees(angle: f64) -> f64 {
    angle - 360.0 * ((angle - 180.0) / 360.0).ceil()
}

/// Tangent (degrees per tick) at key i for cubic segments.
fn slope(keys: &[Key], i: usize) -> f64 {
    if let Some(s) = keys[i].slope {
        return s;
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
    (0..length).map(|t| value_at(keys, t)).collect()
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
}

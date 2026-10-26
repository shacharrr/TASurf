//! The camera replays show on auto-strafe (tasurf/camera.py; the editor page's cameraYaws is the same rule).
//!
//! Auto-strafe plays the most speed: it looks along the velocity, and on a straight range it switches A and D every
//! tick or two, so the view zigzags around the way it goes by atan(30/speed) a tick. The run plays that as it is.
//! Once the game has played these exact inputs (a preview or a run with the same hash), the camera is that path's
//! yaw smoothed with the ticks on both sides of each one: a binomial (near Gaussian) average over CAM_HALF ticks
//! either way, so no lag and no corners, used on the auto-strafe ranges and blending from the yaw played into it
//! over the CAM_MARGIN ticks either side. A tick-to-tick change of more than CAM_BREAK degrees (a teleport) isn't
//! averaged across. Baked as `@view_yaw` lines, not hashed.

use std::collections::BTreeMap;

use crate::py;

pub const CAM_HALF: usize = 24; // ticks either side averaged: weights C(48, 24 + k), sigma about 3.5 ticks
pub const CAM_MARGIN: i64 = 16; // ticks either side of a range over which it blends into the smoothed yaw
pub const CAM_BREAK: f64 = 30.0; // degrees in one tick: a jump (a teleport), not averaged across
pub const VIEW_YAW_PER_LINE: usize = 16; // values on one @view_yaw line

fn wrap(a: f64) -> f64 {
    py::fmod(a + 180.0, 360.0) - 180.0
}

fn weights() -> Vec<f64> {
    let n = 2 * CAM_HALF as u64;
    let mut out = Vec::with_capacity(n as usize + 1);
    let mut c: u64 = 1; // C(n, j), exact in a double
    for j in 0..=n {
        out.push(c as f64);
        if j < n {
            c = c * (n - j) / (j + 1);
        }
    }
    out
}

/// The camera on the ticks it differs from the yaw played: tick -> yaw. yaws: the yaw played on each tick (a run of
/// these inputs); ranges: the auto-strafe ranges, [start, end).
pub fn camera_yaws(yaws: &[f64], ranges: &[(i64, i64)]) -> BTreeMap<usize, f64> {
    let mut out = BTreeMap::new();
    let n = yaws.len();
    if n == 0 || ranges.is_empty() {
        return out;
    }
    let (mut u, mut seg) = (vec![yaws[0]], vec![0usize]); // unwrapped, and which stretch between jumps
    for i in 1..n {
        let d = wrap(yaws[i] - yaws[i - 1]);
        u.push(u[i - 1] + d);
        seg.push(seg[i - 1] + if d.abs() > CAM_BREAK { 1 } else { 0 });
    }
    let mut near = vec![CAM_MARGIN + 1; n]; // ticks to the nearest auto-strafe tick
    for &(start, end) in ranges {
        for t in (start - CAM_MARGIN).max(0)..(end + CAM_MARGIN).min(n as i64) {
            let d = if t < start {
                start - t
            } else if t >= end {
                t - (end - 1)
            } else {
                0
            };
            let i = t as usize;
            near[i] = near[i].min(d);
        }
    }
    let w = weights();
    for t in 0..n {
        if near[t] >= CAM_MARGIN {
            continue;
        }
        let x = 1.0 - near[t] as f64 / CAM_MARGIN as f64;
        let e = x * x * (3.0 - 2.0 * x); // 1 on the range, easing to 0 over the margin
        let (mut acc, mut wsum) = (0.0, 0.0);
        for k in 0..=2 * CAM_HALF {
            let j = t as i64 + k as i64 - CAM_HALF as i64;
            if j >= 0 && (j as usize) < n && seg[j as usize] == seg[t] {
                acc += w[k] * u[j as usize];
                wsum += w[k];
            }
        }
        out.insert(t, wrap(u[t] + e * (acc / wsum - u[t])));
    }
    out
}

fn f4(x: f64) -> String {
    let s = py::fixed(x, 4);
    if s == "-0.0000" {
        "0.0000".into()
    } else {
        s
    }
}

/// `@view_yaw <first tick> <yaw>...`: the yaw replays show on those ticks (runs of consecutive ticks, at most
/// VIEW_YAW_PER_LINE a line). Not hashed.
pub fn view_yaw_lines(camera: &BTreeMap<usize, f64>) -> Vec<String> {
    let mut lines = Vec::new();
    let ticks: Vec<usize> = camera.keys().copied().collect();
    let mut i = 0;
    while i < ticks.len() {
        let first = ticks[i];
        let mut vals: Vec<String> = Vec::new();
        while i < ticks.len() && ticks[i] == first + vals.len() && vals.len() < VIEW_YAW_PER_LINE {
            vals.push(f4(camera[&ticks[i]]));
            i += 1;
        }
        lines.push(format!("@view_yaw {} {}", first, vals.join(" ")));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same lines as tasurf/camera.py for the same yaws (a zigzag turning across -180, a teleport at 55).
    #[test]
    fn same_as_python() {
        let ys: Vec<f64> = (0..90)
            .map(|t| {
                let mut y = 170.0 + 0.37 * t as f64;
                if (10..40).contains(&t) {
                    y += if t % 2 == 1 { 1.25 } else { -0.75 };
                }
                if t >= 55 {
                    y += 150.0;
                }
                py::fmod(y + 180.0, 360.0) - 180.0
            })
            .collect();
        let want = [
            "@view_yaw 0 170.2885 170.6546 170.9979 171.3249 171.6447 171.9667 172.2990 172.6468 173.0122 173.3936 173.7866 174.1858 174.5871 174.9864 175.3811 175.7699",
            "@view_yaw 16 176.1528 176.5310 176.9057 177.2781 177.6492 178.0197 178.3899 178.7600 179.1300 179.5000 179.8700 -179.7600 -179.3899 -179.0198 -178.6495 -178.2790",
            "@view_yaw 32 -177.9083 -177.5377 -177.1677 -176.7994 -176.4344 -176.0743 -175.7205 -175.3734 -175.0340 -174.7001 -174.3666 -174.0292 -173.6824 -173.3324 -172.9863 -172.6486",
            "@view_yaw 48 -172.3247 -172.0206 -171.7427 -171.4947 -171.2778 -171.0913 -170.9328 -18.7372 -18.5787 -18.3921 -18.1750 -17.9265 -17.6474 -17.3409 -17.0119 -16.6661",
            "@view_yaw 64 -16.3092 -15.9456 -15.5784 -15.2095 -14.8398 -14.4700 -14.1000 -13.7300 -13.3600 -12.9900 -12.6200 -12.2500 -11.8801 -11.5103 -11.1407 -10.7714",
            "@view_yaw 80 -10.4025 -10.0337 -9.6644 -9.2938 -8.9217",
        ];
        assert_eq!(view_yaw_lines(&camera_yaws(&ys, &[(10, 40), (48, 70)])), want);
    }
}

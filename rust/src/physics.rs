//! Reference air-movement model (Source SDK 2013 gamemovement) for sanity checks
//! (tasurf/physics.py). This never drives the simulation; the game engine is the
//! ground truth. It re-predicts each recorded airborne tick from the previous one:
//!
//!   1. half gravity:      vz -= 0.5 * g * dt            (StartGravity)
//!   2. clamp each axis to sv_maxvelocity                (CheckVelocity)
//!   3. air acceleration:  wishspeed capped at 30 for the add-speed test (AirAccelerate)
//!   4. move:              pos += v * dt   (TryPlayerMove; collisions clip v)
//!   5. clamp, then the other half of gravity            (FinishGravity)
//!
//! Ticks where vz does not follow gravity exactly are "contact" ticks (a surface
//! clipped the velocity, a trigger fired, ...) and are not checked.

use std::collections::HashMap;

use crate::bake::TickInput;
use crate::buttons::IN_DUCK;
use crate::py;
use crate::runcsv::RunCsv;

pub const AIR_SPEED_CAP: f64 = 30.0;
pub const DUCK_SPEED_MULTIPLIER: f64 = 0.33333334; // ducked on the ground: wish speed cropped to a third

/// Python's min(a, b) and max(a, b): the first argument unless the second is strictly smaller/larger.
pub fn pmin(a: f64, b: f64) -> f64 {
    if b < a {
        b
    } else {
        a
    }
}

pub fn pmax(a: f64, b: f64) -> f64 {
    if b > a {
        b
    } else {
        a
    }
}

fn pmax_all(xs: &[f64]) -> f64 {
    xs[1..].iter().fold(xs[0], |m, &x| pmax(m, x))
}

#[derive(Clone, Copy, Debug)]
pub struct AirParams {
    pub dt: f64,
    pub gravity: f64, // sv_gravity * entity gravity scale
    pub airaccelerate: f64,
    pub maxspeed: f64, // min(sv_maxspeed, player m_flMaxspeed)
    pub maxvelocity: f64,
}

/// Wish direction (unit, horizontal) and wish speed, as CheckParameters + AirMove compute them.
pub fn wish(yaw_deg: f64, mut fmove: f64, mut smove: f64, maxspeed: f64) -> ((f64, f64), f64) {
    let spd2 = fmove * fmove + smove * smove;
    if spd2 > maxspeed * maxspeed && spd2 > 0.0 {
        let r = maxspeed / spd2.sqrt();
        fmove *= r;
        smove *= r;
    }
    let y = py::radians(yaw_deg);
    let (fx, fy) = (y.cos(), y.sin()); // forward with z zeroed and renormalised
    let (rx, ry) = (y.sin(), -y.cos()); // right vector for roll 0
    let (wx, wy) = (fx * fmove + rx * smove, fy * fmove + ry * smove);
    let wishspeed = py::hypot(wx, wy);
    if wishspeed == 0.0 {
        return ((0.0, 0.0), 0.0);
    }
    ((wx / wishspeed, wy / wishspeed), pmin(wishspeed, maxspeed))
}

pub fn air_accelerate(
    vx: f64,
    vy: f64,
    wishdir: (f64, f64),
    wishspeed: f64,
    airaccelerate: f64,
    dt: f64,
    surface_friction: f64,
) -> (f64, f64) {
    let wishspd = pmin(wishspeed, AIR_SPEED_CAP);
    let current = vx * wishdir.0 + vy * wishdir.1;
    let add = wishspd - current;
    if add <= 0.0 {
        return (vx, vy);
    }
    let accel = pmin(airaccelerate * wishspeed * dt * surface_friction, add);
    (vx + accel * wishdir.0, vy + accel * wishdir.1)
}

fn clamp(v: f64, limit: f64) -> f64 {
    pmax(-limit, pmin(limit, v))
}

pub type Vec3 = (f64, f64, f64);

/// Predict (pos, vel) after one collision-free air tick.
pub fn predict_air_tick(pos: Vec3, vel: Vec3, yaw: f64, fmove: f64, smove: f64, p: &AirParams, surface_friction: f64) -> (Vec3, Vec3) {
    let (mut vx, mut vy, mut vz) = vel;
    vz -= 0.5 * p.gravity * p.dt;
    vx = clamp(vx, p.maxvelocity);
    vy = clamp(vy, p.maxvelocity);
    vz = clamp(vz, p.maxvelocity);
    let (wishdir, wishspeed) = wish(yaw, fmove, smove, p.maxspeed);
    let (ax, ay) = air_accelerate(vx, vy, wishdir, wishspeed, p.airaccelerate, p.dt, surface_friction);
    vx = ax;
    vy = ay;
    let new_pos = (pos.0 + vx * p.dt, pos.1 + vy * p.dt, pos.2 + vz * p.dt);
    vx = clamp(vx, p.maxvelocity);
    vy = clamp(vy, p.maxvelocity);
    vz = clamp(vz, p.maxvelocity);
    vz -= 0.5 * p.gravity * p.dt;
    (new_pos, (vx, vy, vz))
}

// --------------------------------------------------------------------------
// checking a recorded run

#[derive(Clone, Debug)]
pub struct CheckReport {
    pub params: AirParams,
    pub air_ticks: i64,
    pub free_flight: i64,
    pub matched: i64,
    pub matched_friction_1: i64,
    pub matched_friction_quarter_only: i64,
    pub mismatches: Vec<String>, // genuine model disagreements
    pub teleports: Vec<String>,
    pub events: Vec<String>, // velocity changes no input could cause
    pub perpendicular: i64,
    pub perpendicular_max_err: f64,
    pub notes: Vec<String>,
    // per row: start, ground, skip (duck/base velocity), contact, free, teleport, event, mismatch
    pub kinds: Vec<&'static str>,
}

fn listed(items: &[String], limit: usize) -> Vec<String> {
    let mut out: Vec<String> = items.iter().take(limit).map(|m| format!("      {}", m)).collect();
    if items.len() > limit {
        out.push(format!("      ... {} more", items.len() - limit));
    }
    out
}

impl CheckReport {
    pub fn lines(&self) -> Vec<String> {
        let p = &self.params;
        let mut out = vec![format!(
            "model: dt={} gravity={} sv_airaccelerate={} maxspeed={} sv_maxvelocity={}",
            py::fixed(p.dt, 6),
            py::g(p.gravity),
            py::g(p.airaccelerate),
            py::g(p.maxspeed),
            py::g(p.maxvelocity)
        )];
        out.extend(self.notes.iter().map(|n| format!("note: {}", n)));
        out.push(format!("airborne ticks checked: {}", self.air_ticks));
        out.push(format!("  free flight (vz followed gravity exactly): {}", self.free_flight));
        out.push(format!(
            "    match reference model: {}{}",
            self.matched,
            if self.matched_friction_quarter_only != 0 {
                format!(" ({} only with surface friction 0.25)", self.matched_friction_quarter_only)
            } else {
                String::new()
            }
        ));
        out.push(format!("    do not match: {}", self.mismatches.len()));
        out.extend(listed(&self.mismatches, 10));
        out.push(format!("    teleported (velocity kept, position jumped): {}", self.teleports.len()));
        out.extend(listed(&self.teleports, 10));
        out.push(format!(
            "    map events (velocity changed by more than air acceleration allows; wall hit or trigger): {}",
            self.events.len()
        ));
        out.extend(listed(&self.events, 10));
        out.push(format!("  contact/other (surface clip, trigger, teleport): {}", self.air_ticks - self.free_flight));
        if self.perpendicular != 0 {
            out.push(format!(
                "perpendicular-strafe ticks: {}, max |new_speed - sqrt(speed^2 + 900)| = {}",
                self.perpendicular,
                py::fixed(self.perpendicular_max_err, 6)
            ));
        } else {
            out.push(
                "perpendicular-strafe ticks: none in this run (wish direction never perpendicular to velocity within 0.1%)"
                    .into(),
            );
        }
        out
    }
}

/// A missing header is a KeyError in Python; this is its str().
pub fn header_float(run: &RunCsv, key: &str) -> Result<f64, String> {
    let v = run.get(key).ok_or_else(|| {
        py::repr_str(&format!("run CSV header has no {}; was it written by the TASurf plugin?", py::repr_str(key)))
    })?;
    let first = v.split_whitespace().next().ok_or_else(|| "list index out of range".to_string())?;
    py::parse_float(first)
}

pub fn params_from_run(run: &RunCsv) -> Result<(AirParams, Vec<String>), String> {
    let mut notes = Vec::new();
    let interval = match run.get("tick_interval_bits") {
        Some(b) => py::bits_to_float(b)?,
        None => header_float(run, "tick_interval")?,
    };
    let lagged = if run.has("start.lagged_movement") { header_float(run, "start.lagged_movement")? } else { 1.0 };
    if lagged != 1.0 {
        notes.push(format!("lagged movement is {}; the model assumes it scales the frame time", py::g(lagged)));
    }
    let mut ent_g = if run.has("start.gravity") { header_float(run, "start.gravity")? } else { 1.0 };
    if ent_g == 0.0 {
        ent_g = 1.0;
    }
    let sv_maxspeed = header_float(run, "cvar.sv_maxspeed")?;
    let player_maxspeed = if run.has("player.maxspeed") { header_float(run, "player.maxspeed")? } else { sv_maxspeed };
    let maxspeed = if player_maxspeed != 0.0 { pmin(sv_maxspeed, player_maxspeed) } else { sv_maxspeed };
    Ok((
        AirParams {
            dt: interval * lagged,
            gravity: header_float(run, "cvar.sv_gravity")? * ent_g,
            airaccelerate: header_float(run, "cvar.sv_airaccelerate")?,
            maxspeed,
            maxvelocity: header_float(run, "cvar.sv_maxvelocity")?,
        },
        notes,
    ))
}

pub fn check_run(run: &RunCsv) -> Result<CheckReport, String> {
    let (params, notes) = params_from_run(run)?;
    let n = run.rows.len();
    let mut rep = CheckReport {
        params,
        air_ticks: 0,
        free_flight: 0,
        matched: 0,
        matched_friction_1: 0,
        matched_friction_quarter_only: 0,
        mismatches: Vec::new(),
        teleports: Vec::new(),
        events: Vec::new(),
        perpendicular: 0,
        perpendicular_max_err: 0.0,
        notes,
        kinds: if n > 0 { std::iter::once("start").chain(std::iter::repeat("ground").take(n - 1)).collect() } else { Vec::new() },
    };
    let (x, y, z) = (run.floats("x")?, run.floats("y")?, run.floats("z")?);
    let (vx, vy, vz) = (run.floats("vx")?, run.floats("vy")?, run.floats("vz")?);
    let (bvx, bvy, bvz) = (run.floats("base_vx")?, run.floats("base_vy")?, run.floats("base_vz")?);
    let (yaw, fwd, side) = (run.floats("yaw")?, run.floats("forwardmove")?, run.floats("sidemove")?);
    let (btn, ground, ducked) = (run.ints("buttons")?, run.ints("on_ground")?, run.ints("ducked")?);
    let ticks = run.ints("tick")?;
    let p = params;

    for i in 1..n {
        let j = i - 1;
        if ground[j] != 0 || ground[i] != 0 {
            continue;
        }
        let based = |k: usize| !(bvx[k] == 0.0 && bvy[k] == 0.0 && bvz[k] == 0.0);
        if ducked[j] != 0 || ducked[i] != 0 || btn[i] & IN_DUCK != 0 || based(i) || based(j) {
            rep.kinds[i] = "skip";
            continue;
        }
        rep.air_ticks += 1;
        rep.kinds[i] = "contact";
        let expected_vz = clamp(clamp(vz[j] - 0.5 * p.gravity * p.dt, p.maxvelocity) - 0.5 * p.gravity * p.dt, p.maxvelocity);
        if (vz[i] - expected_vz).abs() > 1e-3 + 1e-6 * vz[i].abs() {
            continue;
        }
        rep.free_flight += 1;

        let (pos, vel) = ((x[j], y[j], z[j]), (vx[j], vy[j], vz[j]));
        let mut results: [(bool, f64, f64); 2] = [(false, 0.0, 0.0); 2];
        for (slot, friction) in [1.0, 0.25].into_iter().enumerate() {
            let (ppos, pvel) = predict_air_tick(pos, vel, yaw[i], fwd[i], side[i], &p, friction);
            let verr = pmax_all(&[(pvel.0 - vx[i]).abs(), (pvel.1 - vy[i]).abs(), (pvel.2 - vz[i]).abs()]);
            let perr = pmax_all(&[(ppos.0 - x[i]).abs(), (ppos.1 - y[i]).abs(), (ppos.2 - z[i]).abs()]);
            let vtol = 1e-2 + 1e-5 * py::hypot(vx[i], vy[i]);
            let ptol = 1e-2 + 1e-5 * pmax_all(&[x[i].abs(), y[i].abs(), z[i].abs()]);
            results[slot] = (verr <= vtol && perr <= ptol, verr, perr);
        }
        rep.kinds[i] = "free";
        if results[0].0 {
            rep.matched += 1;
            rep.matched_friction_1 += 1;
        } else if results[1].0 {
            rep.matched += 1;
            rep.matched_friction_quarter_only += 1;
        } else {
            let (_, verr, perr) = results[0];
            let note = format!(
                "tick {}: velocity off by {}, position off by {} (speed {} -> {})",
                ticks[i],
                py::fixed(verr, 4),
                py::fixed(perr, 4),
                py::fixed(py::hypot(vx[j], vy[j]), 1),
                py::fixed(py::hypot(vx[i], vy[i]), 1)
            );
            if verr <= 1e-2 + 1e-5 * py::hypot(vx[i], vy[i]) && perr > p.maxvelocity * p.dt {
                rep.teleports.push(note); // moved further than any velocity allows, velocity untouched
                rep.kinds[i] = "teleport";
            } else if verr > AIR_SPEED_CAP {
                rep.events.push(note); // more than a full tick of air acceleration: not from the inputs
                rep.kinds[i] = "event";
            } else {
                rep.mismatches.push(note);
                rep.kinds[i] = "mismatch";
            }
            continue;
        }

        let (wishdir, wishspeed) = wish(yaw[i], fwd[i], side[i], p.maxspeed);
        let speed = py::hypot(vx[j], vy[j]);
        let full_add = p.airaccelerate * wishspeed * p.dt >= AIR_SPEED_CAP;
        if !(wishdir.0 == 0.0 && wishdir.1 == 0.0) && speed > 0.0 && full_add {
            let c = vx[j] * wishdir.0 + vy[j] * wishdir.1;
            if c.abs() <= 1e-3 * speed {
                rep.perpendicular += 1;
                let e = (py::hypot(vx[i], vy[i]) - (speed * speed + AIR_SPEED_CAP * AIR_SPEED_CAP).sqrt()).abs();
                rep.perpendicular_max_err = pmax(rep.perpendicular_max_err, e);
            }
        }
    }
    Ok(rep)
}

// --------------------------------------------------------------------------
// helpers for the editor: best yaw and free-flight estimates

pub fn speed_after_air_accel(vx: f64, vy: f64, yaw: f64, fmove: f64, smove: f64, p: &AirParams) -> f64 {
    let (wishdir, wishspeed) = wish(yaw, fmove, smove, p.maxspeed);
    let (nx, ny) = air_accelerate(vx, vy, wishdir, wishspeed, p.airaccelerate, p.dt, 1.0);
    py::hypot(nx, ny)
}

/// Python's tuple comparison a < b for (float, float, float).
fn tuple_lt(a: (f64, f64, f64), b: (f64, f64, f64)) -> bool {
    if a.0 != b.0 {
        return a.0 < b.0;
    }
    if a.1 != b.1 {
        return a.1 < b.1;
    }
    a.2 < b.2
}

/// Yaw that maximises horizontal speed after one tick of air acceleration, for the
/// given (pre-tick) velocity and held movement keys: c* = max(30 - A, 0) with
/// A = airaccelerate * wishspeed * dt. Of the two such yaws, the one nearest
/// current_yaw, unwrapped near it. Returns (yaw, speed with it, speed with
/// current_yaw), or None when no movement key is held.
pub fn best_yaw(vx: f64, vy: f64, fmove: f64, smove: f64, p: &AirParams, current_yaw: f64) -> Option<(f64, f64, f64)> {
    if fmove == 0.0 && smove == 0.0 {
        return None;
    }
    let speed = py::hypot(vx, vy);
    let (_, wishspeed) = wish(0.0, fmove, smove, p.maxspeed);
    let accel = p.airaccelerate * wishspeed * p.dt;
    let c_star = pmax(AIR_SPEED_CAP - accel, 0.0);
    let phi = if speed > 0.0 { py::degrees(pmin(1.0, c_star / speed).acos()) } else { 0.0 };
    let theta = py::degrees(vy.atan2(vx));
    let delta = py::degrees((-smove).atan2(fmove)); // wish direction relative to the view yaw
    let near = |a: f64| a + 360.0 * py::round_int((current_yaw - a) / 360.0);
    let candidates = [near(theta + phi - delta), near(theta - phi - delta)];
    let mut best: Option<(f64, f64, f64)> = None;
    for a in candidates {
        let item = (py::round(speed_after_air_accel(vx, vy, a, fmove, smove, p), 9), -(a - current_yaw).abs(), a);
        best = match best {
            Some(b) if !tuple_lt(b, item) => Some(b),
            _ => Some(item),
        };
    }
    let b = best?.2;
    Some((b, speed_after_air_accel(vx, vy, b, fmove, smove, p), speed_after_air_accel(vx, vy, current_yaw, fmove, smove, p)))
}

/// What ground movement adds to AirParams (all from the game's convars).
#[derive(Clone, Copy, Debug)]
pub struct GroundParams {
    pub accelerate: f64, // sv_accelerate
    pub friction: f64,   // sv_friction (a surface's own friction is taken as 1)
    pub stopspeed: f64,  // sv_stopspeed
    pub ducked: bool,
}

/// A straight auto-strafe tick's sidemove (the plugin's AutoStrafe_Side: same rule): A (negative) when the
/// heading is to the left of the (pre-tick) velocity, D (positive) otherwise, as big as the held key's. At the
/// max-gain angle each tick turns the velocity by atan(30/speed) towards the heading, so it zigzags around it:
/// straight, gaining the most speed. Without a side key, or without speed, the held keys stay.
pub fn straight_sidemove(vx: f64, vy: f64, smove: f64, heading: f64, swing: Option<f64>, prev: f64) -> f64 {
    if smove == 0.0 || py::hypot(vx, vy) < 1.0 {
        return smove;
    }
    let err = py::fmod(heading - py::degrees(vy.atan2(vx)) + 180.0, 360.0) - 180.0;
    if let (Some(swing), true) = (swing, prev != 0.0) {
        // keep the side played the tick before until the direction is swing past the heading on that side
        if prev < 0.0 {
            return if err < -swing { smove.abs() } else { -smove.abs() };
        }
        return if err > swing { -smove.abs() } else { smove.abs() };
    }
    if err > 0.0 {
        -smove.abs()
    } else {
        smove.abs()
    }
}

/// How close to perfect a smooth swing turns (0 to 1, smooth_swing_input's close) to keep gain percent (75 to 100) of
/// the most speed gain: about 1 - (1 - close)^2 / 4 of it is kept (physics.py's swing_close).
pub fn swing_close(gain: f64) -> f64 {
    1.0 - 2.0 * (1.0 - gain / 100.0).max(0.0).sqrt()
}

/// A smooth-swinging straight tick in the air (the plugin's AutoStrafe_Smooth: physics.py's smooth_swing_input). The
/// direction sweeps +-swing degrees around the heading, its turn rate changing by at most a step a tick: it speeds up
/// to the most a tick turns, brakes in time to stop at the edge, and goes on through zero to turn the other way. The
/// step is a sine's at close 0 (the smoothest) and two whole turns at close 1 (always the most turn, the most speed).
/// The view stays along the velocity; the keys are the wish direction: some W, and A or D. (forwardmove, sidemove,
/// yaw), or None without a side key or speed.
#[allow(clippy::too_many_arguments)]
pub fn smooth_swing_input(
    vx: f64,
    vy: f64,
    prev_dir: Option<f64>,
    prev_side: f64,
    smove: f64,
    heading: f64,
    swing: f64,
    close: f64,
) -> Option<(f64, f64, f64)> {
    let (size, v) = (smove.abs(), py::hypot(vx, vy));
    if size == 0.0 || v < 1.0 {
        return None;
    }
    let vdir = py::degrees(vy.atan2(vx));
    let d = py::fmod(vdir - heading + 180.0, 360.0) - 180.0; // left of the heading: positive
    let w_max = py::degrees((30.0 / v).atan());
    let smooth = (w_max * w_max / swing).min(2.0 * w_max); // the step a sine of that size needs
    let step = smooth + close * (2.0 * w_max - smooth); // up to two whole turns: straight from one side's most to the other's
    let way = if prev_side > 0.0 {
        -1.0
    } else if prev_side < 0.0 {
        1.0
    } else if d < 0.0 {
        1.0
    } else {
        -1.0
    }; // 1: turning left (A)
    let u = match prev_dir {
        None => 0.0,
        Some(p) => way * (py::fmod(vdir - p + 180.0, 360.0) - 180.0),
    }; // how fast, that way
    let ahead = swing - way * d; // to the edge it's turning towards
    let stop = |w: f64| -> f64 {
        // how far it still turns braking from w, a step a tick
        let n = if w > 0.0 { (w / step).floor() } else { 0.0 };
        n * w - step * n * (n + 1.0) / 2.0
    };
    let faster = w_max.min(u + step);
    // speed up if it can still stop in time; hold a real turn if it can; else brake (a creep at the edge turns back)
    let w = if ahead >= faster + stop(faster) {
        faster
    } else if u >= step && ahead >= u + stop(u) {
        u
    } else {
        u - step
    };
    let turn = way * w.min(w_max).max(-w_max); // this tick's turn, positive left
    let cos_t = ((30.0 - v * py::radians(turn.abs()).tan()) / v).min(1.0).max(0.0);
    let side = if turn > 0.0 { -size } else { size };
    Some((size * cos_t, side * (1.0 - cos_t * cos_t).sqrt(), vdir))
}

/// The yaw an auto-strafe tick plays (the plugin's AutoStrafe_Yaw: same rule).
/// Returns the yaw wrapped to (-180, 180], or None when the timeline's yaw should
/// be kept (no movement key, no speed, or W/S alone at an angle without a turn).
pub fn strafe_yaw(
    vx: f64,
    vy: f64,
    fmove: f64,
    smove: f64,
    p: &AirParams,
    offset: f64,
    turn: f64,
    ground: Option<&GroundParams>,
) -> Option<f64> {
    if fmove == 0.0 && smove == 0.0 {
        return None;
    }
    let mut speed = py::hypot(vx, vy);
    if speed < 1.0 {
        return None;
    }
    let (_, mut wishspeed) = wish(0.0, fmove, smove, p.maxspeed);
    let c_star = match ground {
        Some(g) => {
            if g.ducked {
                wishspeed *= DUCK_SPEED_MULTIPLIER;
            }
            speed -= pmax(speed, g.stopspeed) * g.friction * p.dt;
            if speed < 1.0 {
                return None;
            }
            pmax(wishspeed - g.accelerate * wishspeed * p.dt, 0.0)
        }
        None => pmax(AIR_SPEED_CAP - p.airaccelerate * wishspeed * p.dt, 0.0),
    };
    let theta = py::degrees(pmin(1.0, c_star / speed).acos()) + offset;
    let alpha = py::degrees((-smove).atan2(fmove)); // wish direction relative to the view, left positive
    let side = if 0.5 < alpha && alpha < 179.5 {
        1.0
    } else if -179.5 < alpha && alpha < -0.5 {
        -1.0
    } else if turn != 0.0 {
        if turn > 0.0 {
            1.0
        } else {
            -1.0
        }
    } else if theta == 0.0 {
        1.0 // straight along the velocity: either side is the same
    } else {
        return None;
    };
    let yaw = py::degrees(vy.atan2(vx)) + side * theta - alpha;
    Some(yaw - 360.0 * ((yaw - 180.0) / 360.0).ceil())
}

/// Horizontal velocity after one ground tick: Friction, then WalkMove's Accelerate.
#[allow(dead_code)] // the ground half of the model, as physics.py has it (its tests use it)
pub fn ground_accelerate(mut vx: f64, mut vy: f64, yaw: f64, fmove: f64, smove: f64, p: &AirParams, g: &GroundParams) -> (f64, f64) {
    let speed = py::hypot(vx, vy);
    if speed >= 0.1 {
        let new = pmax(speed - pmax(speed, g.stopspeed) * g.friction * p.dt, 0.0);
        vx = vx * new / speed;
        vy = vy * new / speed;
    }
    let (wishdir, mut wishspeed) = wish(yaw, fmove, smove, p.maxspeed);
    if g.ducked {
        wishspeed *= DUCK_SPEED_MULTIPLIER;
    }
    let add = wishspeed - (vx * wishdir.0 + vy * wishdir.1);
    if add <= 0.0 {
        return (vx, vy);
    }
    let accel = pmin(g.accelerate * wishspeed * p.dt, add);
    (vx + accel * wishdir.0, vy + accel * wishdir.1)
}

/// Predict ticks first_tick.. with new inputs, starting from the run's exact state
/// at first_tick - 1 (the inputs before first_tick must be the run's own). Stops
/// at the first tick the original run was not in free flight, and on new inputs
/// that duck. strafe: tick -> (offset, heading of a straight range) for auto-strafe
/// ticks, aimed from the predicted velocity as the plugin would.
pub fn estimate_free_flight(
    run: &RunCsv,
    rep: &CheckReport,
    first_tick: usize,
    inputs: &[TickInput],
    limit: usize,
    strafe: &HashMap<i64, (f64, Option<f64>, Option<f64>, Option<f64>)>,
) -> Result<Vec<(i64, Vec3, Vec3)>, String> {
    if first_tick < 1 || first_tick >= run.rows.len() {
        return Ok(Vec::new());
    }
    let (x, y, z) = (run.floats("x")?, run.floats("y")?, run.floats("z")?);
    let (vx, vy, vz) = (run.floats("vx")?, run.floats("vy")?, run.floats("vz")?);
    let j = first_tick - 1;
    let (mut pos, mut vel) = ((x[j], y[j], z[j]), (vx[j], vy[j], vz[j]));
    let mut prev = run.floats("sidemove")?[j]; // the side played the tick before (a swing keeps it)
    // the direction before the tick before (a smooth swing goes on from how it turned)
    let mut prev_dir = if j >= 1 { Some(py::degrees(vy[j - 1].atan2(vx[j - 1]))) } else { None };
    let mut out = Vec::new();
    let end = inputs.len().min(run.rows.len()).min(first_tick + limit);
    for t in first_tick..end {
        let ti = &inputs[t];
        if rep.kinds[t] != "free" || ti.buttons & IN_DUCK != 0 {
            break;
        }
        let (mut yaw, mut fmove, mut smove) = (ti.yaw, ti.forwardmove, ti.sidemove);
        if let Some(&(offset, heading, swing, gain)) = strafe.get(&(t as i64)) {
            let sw = match (gain, heading, swing) {
                (Some(g), Some(h), Some(s)) => smooth_swing_input(vel.0, vel.1, prev_dir, prev, smove, h, s, swing_close(g)),
                _ => None,
            };
            if let Some((f, s, y)) = sw {
                fmove = f;
                smove = s;
                yaw = y;
            } else {
                if let Some(h) = heading {
                    smove = straight_sidemove(vel.0, vel.1, smove, h, swing, prev);
                }
                let turn = py::fmod(ti.yaw - inputs[t - 1].yaw + 180.0, 360.0) - 180.0;
                yaw = strafe_yaw(vel.0, vel.1, ti.forwardmove, smove, &rep.params, offset, turn, None).unwrap_or(ti.yaw);
            }
        }
        prev_dir = Some(py::degrees(vel.1.atan2(vel.0)));
        let (np, nv) = predict_air_tick(pos, vel, yaw, fmove, smove, &rep.params, 1.0);
        pos = np;
        vel = nv;
        prev = smove;
        out.push((t as i64, pos, vel));
    }
    Ok(out)
}

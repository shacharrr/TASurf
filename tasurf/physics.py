"""Reference air-movement model (Source SDK 2013 gamemovement) for sanity checks.

This never drives the simulation; the game engine is the ground truth. It
re-predicts each recorded airborne tick from the previous recorded tick and
reports how well the engine's result matches the textbook model:

  1. half gravity:      vz -= 0.5 * g * dt            (StartGravity)
  2. clamp each axis to sv_maxvelocity                (CheckVelocity)
  3. air acceleration:  wishspeed capped at 30 for the add-speed test
                        (AirAccelerate)
  4. move:              pos += v * dt   (TryPlayerMove; collisions clip v)
  5. clamp, then the other half of gravity            (FinishGravity)

Ticks where vz does not follow gravity exactly are treated as "contact" ticks
(a surface clipped the velocity, a trigger fired, ...) and are not checked.
"""

from __future__ import annotations

import math
from dataclasses import dataclass, field

from .buttons import IN_DUCK
from .runcsv import RunCsv
from .timeline import bits_to_float

AIR_SPEED_CAP = 30.0


@dataclass(frozen=True)
class AirParams:
    dt: float
    gravity: float  # sv_gravity * entity gravity scale
    airaccelerate: float
    maxspeed: float  # min(sv_maxspeed, player m_flMaxspeed)
    maxvelocity: float


def wish(yaw_deg: float, fmove: float, smove: float, maxspeed: float) -> tuple[tuple[float, float], float]:
    """Wish direction (unit, horizontal) and wish speed, as CheckParameters + AirMove compute them."""
    spd2 = fmove * fmove + smove * smove
    if spd2 > maxspeed * maxspeed and spd2 > 0.0:
        r = maxspeed / math.sqrt(spd2)
        fmove *= r
        smove *= r
    y = math.radians(yaw_deg)
    fx, fy = math.cos(y), math.sin(y)  # forward with z zeroed and renormalised
    rx, ry = math.sin(y), -math.cos(y)  # right vector for roll 0
    wx, wy = fx * fmove + rx * smove, fy * fmove + ry * smove
    wishspeed = math.hypot(wx, wy)
    if wishspeed == 0.0:
        return (0.0, 0.0), 0.0
    wishdir = (wx / wishspeed, wy / wishspeed)
    return wishdir, min(wishspeed, maxspeed)


def air_accelerate(vx: float, vy: float, wishdir: tuple[float, float], wishspeed: float,
                   airaccelerate: float, dt: float, surface_friction: float = 1.0) -> tuple[float, float]:
    wishspd = min(wishspeed, AIR_SPEED_CAP)
    current = vx * wishdir[0] + vy * wishdir[1]
    add = wishspd - current
    if add <= 0.0:
        return vx, vy
    accel = min(airaccelerate * wishspeed * dt * surface_friction, add)
    return vx + accel * wishdir[0], vy + accel * wishdir[1]


def _clamp(v: float, limit: float) -> float:
    return max(-limit, min(limit, v))


def predict_air_tick(pos, vel, yaw, fmove, smove, p: AirParams, surface_friction: float = 1.0):
    """Predict (pos, vel) after one collision-free air tick."""
    vx, vy, vz = vel
    vz -= 0.5 * p.gravity * p.dt
    vx, vy, vz = (_clamp(c, p.maxvelocity) for c in (vx, vy, vz))
    wishdir, wishspeed = wish(yaw, fmove, smove, p.maxspeed)
    vx, vy = air_accelerate(vx, vy, wishdir, wishspeed, p.airaccelerate, p.dt, surface_friction)
    new_pos = (pos[0] + vx * p.dt, pos[1] + vy * p.dt, pos[2] + vz * p.dt)
    vx, vy, vz = (_clamp(c, p.maxvelocity) for c in (vx, vy, vz))
    vz -= 0.5 * p.gravity * p.dt
    return new_pos, (vx, vy, vz)


# --------------------------------------------------------------------------
# checking a recorded run


@dataclass
class CheckReport:
    params: AirParams
    air_ticks: int = 0
    free_flight: int = 0
    matched: int = 0
    matched_friction_1: int = 0
    matched_friction_quarter_only: int = 0
    mismatches: list[str] = field(default_factory=list)  # genuine model disagreements
    teleports: list[str] = field(default_factory=list)
    events: list[str] = field(default_factory=list)  # velocity changes no input could cause
    perpendicular: int = 0
    perpendicular_max_err: float = 0.0
    notes: list[str] = field(default_factory=list)
    # per row: start, ground, skip (duck/base velocity), contact, free, teleport, event, mismatch
    kinds: list[str] = field(default_factory=list)

    @staticmethod
    def _listed(items: list[str], limit: int = 10) -> list[str]:
        out = [f"      {m}" for m in items[:limit]]
        if len(items) > limit:
            out.append(f"      ... {len(items) - limit} more")
        return out

    def lines(self) -> list[str]:
        p = self.params
        out = [f"model: dt={p.dt:.6f} gravity={p.gravity:g} sv_airaccelerate={p.airaccelerate:g} "
               f"maxspeed={p.maxspeed:g} sv_maxvelocity={p.maxvelocity:g}"]
        out += [f"note: {n}" for n in self.notes]
        out.append(f"airborne ticks checked: {self.air_ticks}")
        out.append(f"  free flight (vz followed gravity exactly): {self.free_flight}")
        out.append(f"    match reference model: {self.matched}"
                   + (f" ({self.matched_friction_quarter_only} only with surface friction 0.25)"
                      if self.matched_friction_quarter_only else ""))
        out.append(f"    do not match: {len(self.mismatches)}")
        out += self._listed(self.mismatches)
        out.append(f"    teleported (velocity kept, position jumped): {len(self.teleports)}")
        out += self._listed(self.teleports)
        out.append(f"    map events (velocity changed by more than air acceleration allows; "
                   f"wall hit or trigger): {len(self.events)}")
        out += self._listed(self.events)
        out.append(f"  contact/other (surface clip, trigger, teleport): {self.air_ticks - self.free_flight}")
        if self.perpendicular:
            out.append(f"perpendicular-strafe ticks: {self.perpendicular}, max |new_speed - sqrt(speed^2 + 900)| "
                       f"= {self.perpendicular_max_err:.6f}")
        else:
            out.append("perpendicular-strafe ticks: none in this run (wish direction never perpendicular to "
                       "velocity within 0.1%)")
        return out


def _header_float(run: RunCsv, key: str) -> float:
    if key not in run.header:
        raise KeyError(f"run CSV header has no {key!r}; was it written by the TASurf plugin?")
    return float(run.header[key].split()[0])


def params_from_run(run: RunCsv) -> tuple[AirParams, list[str]]:
    notes = []
    if "tick_interval_bits" in run.header:
        interval = bits_to_float(run.header["tick_interval_bits"])
    else:
        interval = _header_float(run, "tick_interval")
    lagged = _header_float(run, "start.lagged_movement") if "start.lagged_movement" in run.header else 1.0
    if lagged != 1.0:
        notes.append(f"lagged movement is {lagged:g}; the model assumes it scales the frame time")
    ent_g = _header_float(run, "start.gravity") if "start.gravity" in run.header else 1.0
    if ent_g == 0.0:
        ent_g = 1.0
    sv_maxspeed = _header_float(run, "cvar.sv_maxspeed")
    player_maxspeed = _header_float(run, "player.maxspeed") if "player.maxspeed" in run.header else sv_maxspeed
    maxspeed = min(sv_maxspeed, player_maxspeed) if player_maxspeed else sv_maxspeed
    return AirParams(
        dt=interval * lagged,
        gravity=_header_float(run, "cvar.sv_gravity") * ent_g,
        airaccelerate=_header_float(run, "cvar.sv_airaccelerate"),
        maxspeed=maxspeed,
        maxvelocity=_header_float(run, "cvar.sv_maxvelocity"),
    ), notes


def check_run(run: RunCsv) -> CheckReport:
    params, notes = params_from_run(run)
    rep = CheckReport(params=params, notes=notes)
    rep.kinds = ["start"] + ["ground"] * (len(run.rows) - 1) if run.rows else []

    x, y, z = run.floats("x"), run.floats("y"), run.floats("z")
    vx, vy, vz = run.floats("vx"), run.floats("vy"), run.floats("vz")
    bvx, bvy, bvz = run.floats("base_vx"), run.floats("base_vy"), run.floats("base_vz")
    yaw, fwd, side = run.floats("yaw"), run.floats("forwardmove"), run.floats("sidemove")
    btn, ground, ducked = run.ints("buttons"), run.ints("on_ground"), run.ints("ducked")
    ticks = run.ints("tick")

    for i in range(1, len(run.rows)):
        j = i - 1
        if ground[j] or ground[i]:
            continue
        if ducked[j] or ducked[i] or btn[i] & IN_DUCK or any(
                (bvx[k], bvy[k], bvz[k]) != (0.0, 0.0, 0.0) for k in (i, j)):
            rep.kinds[i] = "skip"
            continue
        rep.air_ticks += 1
        rep.kinds[i] = "contact"
        expected_vz = _clamp(_clamp(vz[j] - 0.5 * params.gravity * params.dt, params.maxvelocity)
                             - 0.5 * params.gravity * params.dt, params.maxvelocity)
        if abs(vz[i] - expected_vz) > 1e-3 + 1e-6 * abs(vz[i]):
            continue
        rep.free_flight += 1

        pos, vel = (x[j], y[j], z[j]), (vx[j], vy[j], vz[j])
        results = {}
        for friction in (1.0, 0.25):
            ppos, pvel = predict_air_tick(pos, vel, yaw[i], fwd[i], side[i], params, friction)
            verr = max(abs(a - b) for a, b in zip(pvel, (vx[i], vy[i], vz[i])))
            perr = max(abs(a - b) for a, b in zip(ppos, (x[i], y[i], z[i])))
            vtol = 1e-2 + 1e-5 * math.hypot(vx[i], vy[i])
            ptol = 1e-2 + 1e-5 * max(abs(c) for c in (x[i], y[i], z[i]))
            results[friction] = (verr <= vtol and perr <= ptol, verr, perr)
        rep.kinds[i] = "free"
        if results[1.0][0]:
            rep.matched += 1
            rep.matched_friction_1 += 1
        elif results[0.25][0]:
            rep.matched += 1
            rep.matched_friction_quarter_only += 1
        else:
            _, verr, perr = results[1.0]
            note = (f"tick {ticks[i]}: velocity off by {verr:.4f}, position off by {perr:.4f} "
                    f"(speed {math.hypot(vx[j], vy[j]):.1f} -> {math.hypot(vx[i], vy[i]):.1f})")
            if verr <= 1e-2 + 1e-5 * math.hypot(vx[i], vy[i]) and perr > params.maxvelocity * params.dt:
                rep.teleports.append(note)  # moved further than any velocity allows, velocity untouched
                rep.kinds[i] = "teleport"
            elif verr > AIR_SPEED_CAP:
                rep.events.append(note)  # more than a full tick of air acceleration: not from the inputs
                rep.kinds[i] = "event"
            else:
                rep.mismatches.append(note)
                rep.kinds[i] = "mismatch"
            continue

        wishdir, wishspeed = wish(yaw[i], fwd[i], side[i], params.maxspeed)
        speed = math.hypot(vx[j], vy[j])
        full_add = params.airaccelerate * wishspeed * params.dt >= AIR_SPEED_CAP
        if wishdir != (0.0, 0.0) and speed > 0.0 and full_add:
            c = vx[j] * wishdir[0] + vy[j] * wishdir[1]
            if abs(c) <= 1e-3 * speed:
                rep.perpendicular += 1
                err = abs(math.hypot(vx[i], vy[i]) - math.sqrt(speed * speed + AIR_SPEED_CAP ** 2))
                rep.perpendicular_max_err = max(rep.perpendicular_max_err, err)
    return rep


# --------------------------------------------------------------------------
# helpers for the editor: best yaw and free-flight estimates


def speed_after_air_accel(vx: float, vy: float, yaw: float, fmove: float, smove: float, p: AirParams) -> float:
    wishdir, wishspeed = wish(yaw, fmove, smove, p.maxspeed)
    nx, ny = air_accelerate(vx, vy, wishdir, wishspeed, p.airaccelerate, p.dt)
    return math.hypot(nx, ny)


def best_yaw(vx: float, vy: float, fmove: float, smove: float, p: AirParams,
             current_yaw: float) -> tuple[float, float, float] | None:
    """Yaw that maximises horizontal speed after one tick of air acceleration,
    for the given (pre-tick) velocity and held movement keys.

    With add-speed a = min(A, 30 - c), where A = airaccelerate * wishspeed * dt
    and c = v . wishdir, the new speed^2 is s^2 + 2ac + a^2. That is largest at
    c* = max(30 - A, 0): perpendicular (sqrt(s^2 + 900)) whenever A >= 30.
    There are two such yaws (strafing either side of the velocity); this returns
    the one nearest current_yaw, expressed near current_yaw (unwrapped).

    Returns (yaw, speed with that yaw, speed with current_yaw), or None when no
    movement key is held.
    """
    if fmove == 0.0 and smove == 0.0:
        return None
    speed = math.hypot(vx, vy)
    _, wishspeed = wish(0.0, fmove, smove, p.maxspeed)
    accel = p.airaccelerate * wishspeed * p.dt
    c_star = max(AIR_SPEED_CAP - accel, 0.0)
    phi = math.degrees(math.acos(min(1.0, c_star / speed))) if speed > 0.0 else 0.0
    theta = math.degrees(math.atan2(vy, vx))
    delta = math.degrees(math.atan2(-smove, fmove))  # wish direction relative to the view yaw

    def near(a: float) -> float:
        return a + 360.0 * round((current_yaw - a) / 360.0)

    candidates = [near(theta + phi - delta), near(theta - phi - delta)]
    scored = [(round(speed_after_air_accel(vx, vy, a, fmove, smove, p), 9), -abs(a - current_yaw), a)
              for a in candidates]
    _, _, best = max(scored)
    return (best, speed_after_air_accel(vx, vy, best, fmove, smove, p),
            speed_after_air_accel(vx, vy, current_yaw, fmove, smove, p))


@dataclass(frozen=True)
class GroundParams:
    """What ground movement adds to AirParams (all from the game's convars)."""
    accelerate: float  # sv_accelerate
    friction: float    # sv_friction (a surface's own friction is taken as 1)
    stopspeed: float   # sv_stopspeed
    ducked: bool = False


DUCK_SPEED_MULTIPLIER = 0.33333334  # ducked on the ground: wish speed cropped to a third


def straight_sidemove(vx: float, vy: float, smove: float, heading: float, swing: float | None = None,
                      prev: float = 0.0) -> float:
    """A straight auto-strafe tick's sidemove (the plugin's AutoStrafe_Side: same rule): A (negative) when the
    heading is to the left of the (pre-tick) velocity, D (positive) otherwise, as big as the held key's. At the
    max-gain angle each tick turns the velocity by atan(30/speed) towards the heading, so it zigzags around it:
    straight, gaining the most speed. With a swing (degrees), the side played the tick before (prev: its
    sidemove) is kept until the direction has swung that far past the heading: fewer, longer sweeps that look
    smoother, a path that weaves about +-swing. Without a side key, or without speed, the held keys stay."""
    if smove == 0.0 or math.hypot(vx, vy) < 1.0:
        return smove
    err = (heading - math.degrees(math.atan2(vy, vx)) + 180.0) % 360.0 - 180.0
    if swing is not None and prev != 0.0:
        if prev < 0.0:  # A turns it left: until it's swing past the heading on that side
            return abs(smove) if err < -swing else -abs(smove)
        return -abs(smove) if err > swing else abs(smove)
    return -abs(smove) if err > 0.0 else abs(smove)


def swing_close(gain: float) -> float:
    """How close to perfect a smooth swing turns (0 to 1, smooth_swing_input's close) to keep gain percent (75 to 100)
    of the most speed gain: about 1 - (1 - close)^2 / 4 of it is kept."""
    return 1.0 - 2.0 * math.sqrt(max(0.0, 1.0 - gain / 100.0))


def smooth_swing_input(vx: float, vy: float, prev_dir: float | None, prev_side: float, smove: float, heading: float,
                       swing: float, close: float) -> tuple[float, float, float] | None:
    """A smooth-swinging straight tick in the air (the plugin's AutoStrafe_Smooth: same rule). The direction sweeps
    +-swing degrees around the heading, its turn rate changing by at most a step a tick, so the yaw has no corners:
    it speeds up to the most a tick turns (atan(30/speed)), brakes in time to stop turning at the edge of the sweep,
    and goes on through zero to turn the other way. The step is the one a sine of that size would need at close 0,
    and two whole turns at close 1, straight from turning the most one way to the other (how close to perfect: 1
    always turns the most, the most speed, a swing whose yaw zigzags; 0 is the smoothest, with about three quarters
    of the speed gain, since turning less than the most takes the wish direction short of square to the velocity,
    which adds less). A sweep of under about two ticks' turn can't be smooth. prev_dir: the direction before the
    tick before (how it turned then; None: not known), prev_side: the sidemove played the tick before (which way it
    was turning). The view stays along the velocity; the keys are the wish direction relative to it: some W, and A
    or D. Returns (forwardmove, sidemove, yaw), or None without a side key or speed."""
    size, v = abs(smove), math.hypot(vx, vy)
    if size == 0.0 or v < 1.0:
        return None
    vdir = math.degrees(math.atan2(vy, vx))
    d = (vdir - heading + 180.0) % 360.0 - 180.0  # left of the heading: positive
    w_max = math.degrees(math.atan(30.0 / v))
    smooth = min(w_max * w_max / swing, 2.0 * w_max)  # the step a sine of that size needs
    step = smooth + close * (2.0 * w_max - smooth)  # up to two whole turns: straight from one side's most to the other's
    way = -1.0 if prev_side > 0.0 else 1.0 if prev_side < 0.0 else (1.0 if d < 0.0 else -1.0)  # 1: turning left (A)
    u = 0.0 if prev_dir is None else way * ((vdir - prev_dir + 180.0) % 360.0 - 180.0)  # how fast, that way
    ahead = swing - way * d  # to the edge it's turning towards

    def stop(w):  # how far it still turns braking from w, a step a tick
        n = math.floor(w / step) if w > 0.0 else 0
        return n * w - step * n * (n + 1) / 2.0

    faster = min(w_max, u + step)
    # speed up if it can still stop in time; hold a real turn if it can; else brake (a creep at the edge turns back)
    w = faster if ahead >= faster + stop(faster) else u if u >= step and ahead >= u + stop(u) else u - step
    turn = way * max(-w_max, min(w_max, w))  # this tick's turn, positive left
    cos_t = max(0.0, min(1.0, (30.0 - v * math.tan(math.radians(abs(turn)))) / v))
    return size * cos_t, (-size if turn > 0.0 else size) * math.sqrt(1.0 - cos_t * cos_t), vdir


def strafe_yaw(vx: float, vy: float, fmove: float, smove: float, p: AirParams,
               offset: float = 0.0, turn: float = 0.0, ground: GroundParams | None = None) -> float | None:
    """The yaw an auto-strafe tick plays (the plugin's AutoStrafe_Yaw: same rule).

    The held keys' wish direction goes at the max-gain angle to the (pre-tick)
    velocity, acos(c*/speed), plus offset degrees (positive turns harder), on the
    side the keys strafe to: A to the left, D to the right. W or S alone strafe to
    the side the timeline's own yaw turns (turn: its change this tick).

    In the air c* = max(30 - A, 0) with A = airaccelerate * wishspeed * dt: 90
    degrees whenever A >= 30, as on surf. On the ground (ground given; a tick that
    jumps counts as air) friction comes first, as in the engine, then
    c* = max(wishspeed - A, 0) with A = accelerate * wishspeed * dt: the prestrafe
    angle, and straight along the velocity below c*.

    Returns the yaw wrapped to (-180, 180], or None when the timeline's yaw should
    be kept (no movement key, no speed, or W/S alone at an angle without a turn).
    """
    if fmove == 0.0 and smove == 0.0:
        return None
    speed = math.hypot(vx, vy)
    if speed < 1.0:
        return None
    _, wishspeed = wish(0.0, fmove, smove, p.maxspeed)
    if ground is not None:
        if ground.ducked:
            wishspeed *= DUCK_SPEED_MULTIPLIER
        speed -= max(speed, ground.stopspeed) * ground.friction * p.dt
        if speed < 1.0:
            return None
        c_star = max(wishspeed - ground.accelerate * wishspeed * p.dt, 0.0)
    else:
        c_star = max(AIR_SPEED_CAP - p.airaccelerate * wishspeed * p.dt, 0.0)
    theta = math.degrees(math.acos(min(1.0, c_star / speed))) + offset
    alpha = math.degrees(math.atan2(-smove, fmove))  # wish direction relative to the view, left positive
    if 0.5 < alpha < 179.5:
        side = 1.0
    elif -179.5 < alpha < -0.5:
        side = -1.0
    elif turn != 0.0:
        side = 1.0 if turn > 0.0 else -1.0
    elif theta == 0.0:
        side = 1.0  # straight along the velocity: either side is the same
    else:
        return None
    yaw = math.degrees(math.atan2(vy, vx)) + side * theta - alpha
    return yaw - 360.0 * math.ceil((yaw - 180.0) / 360.0)


def ground_accelerate(vx: float, vy: float, yaw: float, fmove: float, smove: float, p: AirParams,
                      g: GroundParams) -> tuple[float, float]:
    """Horizontal velocity after one ground tick: Friction, then WalkMove's Accelerate."""
    speed = math.hypot(vx, vy)
    if speed >= 0.1:
        new = max(speed - max(speed, g.stopspeed) * g.friction * p.dt, 0.0)
        vx, vy = vx * new / speed, vy * new / speed
    wishdir, wishspeed = wish(yaw, fmove, smove, p.maxspeed)
    if g.ducked:
        wishspeed *= DUCK_SPEED_MULTIPLIER
    add = wishspeed - (vx * wishdir[0] + vy * wishdir[1])
    if add <= 0.0:
        return vx, vy
    accel = min(g.accelerate * wishspeed * p.dt, add)
    return vx + accel * wishdir[0], vy + accel * wishdir[1]


def estimate_free_flight(run: RunCsv, rep: CheckReport, first_tick: int, inputs, limit: int = 20000,
                         strafe: dict | None = None):
    """Predict ticks first_tick.. with new inputs, starting from the run's exact
    state at first_tick - 1. The inputs before first_tick must be the run's own,
    so that state is what the engine would produce again. Stops at the first tick
    the original run was not in free flight (ground, ramp contact, trigger,
    teleport), because past that the model doesn't know the map, and on new
    inputs that duck. strafe: tick -> offset (or (offset, heading, swing, smooth),
    heading None unless straight, swing None unless swinging, gain None unless a
    swing turns smoothly: how much of the most speed gain it keeps, in percent)
    for auto-strafe ticks, which aim from the predicted velocity as the plugin
    would. Returns
    [(tick, (x, y, z), (vx, vy, vz))].
    """
    if first_tick < 1 or first_tick >= len(run.rows):
        return []
    x, y, z = run.floats("x"), run.floats("y"), run.floats("z")
    vx, vy, vz = run.floats("vx"), run.floats("vy"), run.floats("vz")
    j = first_tick - 1
    pos, vel = (x[j], y[j], z[j]), (vx[j], vy[j], vz[j])
    prev = run.floats("sidemove")[j]  # the side played the tick before (a swing keeps it)
    # the direction before the tick before (a smooth swing goes on from how it turned)
    prev_dir = math.degrees(math.atan2(vy[j - 1], vx[j - 1])) if j >= 1 else None
    out = []
    for t in range(first_tick, min(len(inputs), len(run.rows), first_tick + limit)):
        ti = inputs[t]
        if rep.kinds[t] != "free" or ti.buttons & IN_DUCK:
            break
        yaw, fmove, smove = ti.yaw, ti.forwardmove, ti.sidemove
        if strafe and t in strafe:
            offset, heading, swing, gain = strafe[t] if isinstance(strafe[t], tuple) else (strafe[t], None, None, None)
            sw = (smooth_swing_input(vel[0], vel[1], prev_dir, prev, smove, heading, swing, swing_close(gain))
                  if gain is not None else None)
            if sw is not None:
                fmove, smove, yaw = sw
            else:
                if heading is not None:
                    smove = straight_sidemove(vel[0], vel[1], smove, heading, swing, prev)
                turn = (ti.yaw - inputs[t - 1].yaw + 180.0) % 360.0 - 180.0
                auto = strafe_yaw(vel[0], vel[1], ti.forwardmove, smove, rep.params, offset, turn)
                yaw = ti.yaw if auto is None else auto
        prev_dir = math.degrees(math.atan2(vel[1], vel[0]))
        pos, vel = predict_air_tick(pos, vel, yaw, fmove, smove, rep.params)
        prev = smove
        out.append((t, pos, vel))
    return out

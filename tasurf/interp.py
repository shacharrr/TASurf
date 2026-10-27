"""Keyframe interpolation for timeline tracks (yaw, pitch).

Values are plain floats in degrees. Yaw is unwrapped: 170 -> 190 turns 20
degrees through 180, and 170 -> -170 turns 340 degrees the other way. Nothing
here wraps angles; that happens once, when the baker writes the per-tick file.

Each key's ``interp`` describes the segment from that key to the next one:

  hold    keep this key's value until the next key's tick
  linear  straight line
  smooth  ease in/out (smoothstep), zero slope at both ends
  cubic   cubic Hermite through the keys; slopes come from the neighbouring
          keys (non-uniform Catmull-Rom), so the curve is smooth across keys
  joint   a straight line (a steady turn rate), and where two joint segments
          meet, a rounded joint: over the ticks round the key the turn rate goes
          from one line's to the next along a smoothstep, so the yaw has no
          corner (and its rate none either). The curve goes exactly through every
          key: the lines tilt a little so the joints meet the keys (see
          _joint_slopes).

A key can also say its slope (degrees per tick): cubic segments that start or
end at it then use that instead of the estimate (where a joint segment's line
meets a cubic, the cubic takes the line's slope). The editor sets slopes so a
curve it bends joins the rest without a corner. For a joint segment the slope is
its line's, and a key can say its joint's width (ticks, half before it and half
after; 0: a corner): edits that keep part of a curve exactly write both in, so
that part no longer depends on the keys beyond. A joint key's slope counts only
with its width: a slope alone there is one a smooth curve left, and the line is
fitted (see _said_slope).

Before the first key the first value holds; after the last key the last value
holds.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

INTERP_MODES = ("hold", "linear", "smooth", "cubic", "joint")
JOINT_PASSES = 6  # how many times each joint segment's line is fitted to its neighbours' (see _joint_slopes)
_REACH = 3.0 / 16.0  # a half joint moves where its line reaches by this much of half its width times the rate change


@dataclass(frozen=True)
class Key:
    tick: int
    value: float
    interp: str = "linear"
    slope: float | None = None  # degrees per tick where a cubic segment meets this key (None: from the neighbours);
    #                             a joint segment's line from this key, with a joint width (else: fitted)
    joint: float | None = None  # ticks the joint at this key takes (None: a quarter of the shorter segment beside it, each side)


def wrap_degrees(angle: float) -> float:
    """Wrap to (-180, 180], the range the CS:S client sends (recordings contain
    180.0 but never -180.0). A value already in that range comes back unchanged
    bit for bit, which keeps imported recordings exact."""
    return angle - 360.0 * math.ceil((angle - 180.0) / 360.0)


def _said_slope(k: Key) -> float | None:
    """The slope a key says. A joint key's line counts only together with its joint's width, the two an edit
    writes in (_pin in editdoc.py): a slope alone there is one a smooth curve left when the key became a joint,
    and the line is fitted."""
    return k.slope if k.interp != "joint" or k.joint is not None else None


def _slope(keys: list[Key], i: int) -> float:
    """Tangent (degrees per tick) at key i for cubic segments: the key's own; where a joint segment's line arrives
    or leaves, that line's (so the curves join without a corner); else from the neighbouring keys."""
    if joint_half(keys, i) > 0.0:  # a joint kept at the key a cubic leaves (an edit): the rate the joint has there
        ml = joint_slopes(keys, i - 1, i - 1)[i - 1]
        return ml + (keys[i].slope - ml) * 0.5
    said = _said_slope(keys[i])
    if said is not None:
        return said
    if _joint_segment(keys, i - 1):
        return joint_slopes(keys, i - 1, i - 1)[i - 1]
    if _joint_segment(keys, i):
        return joint_slopes(keys, i, i)[i]
    if len(keys) < 2:
        return 0.0
    if i == 0:
        a, b = keys[0], keys[1]
    elif i == len(keys) - 1:
        a, b = keys[-2], keys[-1]
    else:
        a, b = keys[i - 1], keys[i + 1]
    return (b.value - a.value) / (b.tick - a.tick)


def key_slope(keys: list[Key], i: int) -> float:
    """The slope (degrees per tick) cubic segments meet key i with: its own, or the neighbours' estimate."""
    return _slope(keys, i)


def segment_slope(keys: list[Key], i: int, tick: int) -> float:
    """The slope (degrees per tick) of the cubic segment keys[i] .. keys[i + 1] at a tick inside it."""
    k0, k1 = keys[i], keys[i + 1]
    span = k1.tick - k0.tick
    s = (tick - k0.tick) / span
    m0 = _slope(keys, i) * span
    m1 = _slope(keys, i + 1) * span
    s2 = s * s
    return ((6.0 * s2 - 6.0 * s) * k0.value
            + (3.0 * s2 - 4.0 * s + 1.0) * m0
            + (-6.0 * s2 + 6.0 * s) * k1.value
            + (3.0 * s2 - 2.0 * s) * m1) / span


def _joint_segment(keys: list[Key], j: int) -> bool:
    return 0 <= j < len(keys) - 1 and keys[j].interp == "joint"


def joint_half(keys: list[Key], i: int) -> float:
    """Half the width (ticks) of the joint at key i, 0 where there's none. Joints are where two joint segments
    meet, and where a joint segment arrives at a key that says its line's slope and its joint's width (an edit
    kept the curve there exactly: _pin in editdoc.py)."""
    if not 0 < i < len(keys) - 1 or not _joint_segment(keys, i - 1):
        return 0.0
    k = keys[i]
    if not _joint_segment(keys, i):
        return max(0.0, k.joint) / 2.0 if k.slope is not None and k.joint is not None else 0.0
    if k.joint is not None:
        return max(0.0, k.joint) / 2.0
    return min(k.tick - keys[i - 1].tick, keys[i + 1].tick - k.tick) / 4.0


def _line_out(keys: list[Key], j: int, m: dict[int, float]) -> float:
    """The line leaving key j: its joint segment's, or the slope the key says (a joint arriving there)."""
    return m[j] if _joint_segment(keys, j) else keys[j].slope


def joint_slopes(keys: list[Key], lo: int, hi: int) -> dict[int, float]:
    """The line's slope (degrees per tick) of each joint segment lo .. hi: the key's own, or else fitted. A half
    joint shifts where its segment's line arrives (by _REACH x half width x the change of rate), so for a
    segment to arrive exactly at its next key its line depends on its neighbours' lines. The fit starts from the
    key-to-key slopes and goes over the neighbours JOINT_PASSES times: close enough that the straight parts and
    the joints meet without a visible kink, and every slope depends only on the keys a few segments away, so
    moving a key reshapes the curve near it only. The keys' own slopes don't enter the fit (except where a
    joint arrives at a key that isn't a joint segment's start), so writing in a fitted slope changes nothing
    else."""
    last_seg = len(keys) - 2
    first, last = max(0, lo - JOINT_PASSES), min(last_seg, hi + JOINT_PASSES)
    m: dict[int, float] = {}
    for j in range(first, last + 1):
        if _joint_segment(keys, j):
            m[j] = (keys[j + 1].value - keys[j].value) / (keys[j + 1].tick - keys[j].tick)
    for p in range(1, JOINT_PASSES + 1):
        nxt: dict[int, float] = {}
        for j in range(max(0, lo - JOINT_PASSES + p), min(last_seg, hi + JOINT_PASSES - p) + 1):
            if not _joint_segment(keys, j):
                continue
            span = keys[j + 1].tick - keys[j].tick
            hl, hr = joint_half(keys, j), joint_half(keys, j + 1)
            left = m[j - 1] if hl > 0.0 else 0.0
            right = _line_out(keys, j + 1, m) if hr > 0.0 else 0.0
            den = span - _REACH * hl - _REACH * hr
            if den <= 0.25 * span:  # joints far wider than the segment: keep the slope it has
                nxt[j] = m[j]
            else:
                nxt[j] = (keys[j + 1].value - keys[j].value - _REACH * hl * left - _REACH * hr * right) / den
        m = nxt
    return {j: (m[j] if _said_slope(keys[j]) is None else keys[j].slope) for j in range(lo, hi + 1) if _joint_segment(keys, j)}


def _joint_part(ml: float, mr: float, h: float, d: float) -> float:
    """How far the yaw goes from a joint's key to d ticks after it (d < 0: before), inside the joint, where the
    rate goes from ml to mr along a smoothstep over the 2h ticks round the key."""
    w = 2.0 * h
    u = (d + h) / w
    return w * (ml * u + (mr - ml) * (u * u * u * (1.0 - 0.5 * u))) - w * (ml * 0.5 + (mr - ml) * 0.09375)


def _joint_value(keys: list[Key], i: int, tick: int, m: dict[int, float]) -> float:
    """The joint segment keys[i] .. keys[i + 1] at a tick inside it; m: the joint segments' slopes, i - 1 .. i + 1.
    Each half joint goes from its own key, and the straight part joins their ends."""
    k0, k1 = keys[i], keys[i + 1]
    if tick == k0.tick:  # the key itself (even where the next key's joint reaches past it, on a short segment)
        return k0.value
    hl, hr = joint_half(keys, i), joint_half(keys, i + 1)
    a, b = k0.tick + hl, k1.tick - hr
    if tick < a:
        return k0.value + _joint_part(m[i - 1], m[i], hl, tick - k0.tick)
    if tick > b:
        return k1.value + _joint_part(m[i], _line_out(keys, i + 1, m), hr, tick - k1.tick)
    ya = k0.value + _joint_part(m[i - 1], m[i], hl, hl) if hl > 0.0 else k0.value
    yb = k1.value + _joint_part(m[i], _line_out(keys, i + 1, m), hr, -hr) if hr > 0.0 else k1.value
    return ya + (yb - ya) * ((tick - a) / (b - a)) if b > a else ya


def joint_rate(keys: list[Key], i: int, tick: float) -> float:
    """The slope (degrees per tick) of the joint segment keys[i] .. keys[i + 1] at a tick in it (its ends included)."""
    m = joint_slopes(keys, i - 1, i + 1)
    k0, k1 = keys[i], keys[i + 1]
    hl, hr = joint_half(keys, i), joint_half(keys, i + 1)
    a, b = k0.tick + hl, k1.tick - hr
    if tick < a or tick > b:
        ml, mr, h, d = (m[i - 1], m[i], hl, tick - k0.tick) if tick < a else (m[i], _line_out(keys, i + 1, m), hr, tick - k1.tick)
        u = (d + h) / (2.0 * h)
        return ml + (mr - ml) * (u * u * (3.0 - 2.0 * u))
    if b > a:
        return (_joint_value(keys, i, b, m) - _joint_value(keys, i, a, m)) / (b - a)
    return m[i]


def _segment_value(keys: list[Key], i: int, tick: int) -> float:
    k0, k1 = keys[i], keys[i + 1]
    span = k1.tick - k0.tick
    s = (tick - k0.tick) / span
    mode = k0.interp
    if mode == "hold":
        return k0.value
    if mode == "linear":
        return k0.value + (k1.value - k0.value) * s
    if mode == "smooth":
        return k0.value + (k1.value - k0.value) * (s * s * (3.0 - 2.0 * s))
    if mode == "cubic":
        m0 = _slope(keys, i) * span
        m1 = _slope(keys, i + 1) * span
        s2 = s * s
        s3 = s2 * s
        return ((2 * s3 - 3 * s2 + 1) * k0.value
                + (s3 - 2 * s2 + s) * m0
                + (-2 * s3 + 3 * s2) * k1.value
                + (s3 - s2) * m1)
    if mode == "joint":
        return _joint_value(keys, i, tick, joint_slopes(keys, i - 1, i + 1))
    raise ValueError(f"unknown interpolation mode {mode!r}")


def value_at(keys: list[Key], tick: int) -> float:
    """Value of a track at an integer tick. ``keys`` must be sorted by tick."""
    if not keys:
        raise ValueError("track has no keys")
    if tick <= keys[0].tick:
        return keys[0].value
    if tick >= keys[-1].tick:
        return keys[-1].value
    lo, hi = 0, len(keys) - 1
    while hi - lo > 1:  # find the segment with keys[lo].tick <= tick < keys[hi].tick
        mid = (lo + hi) // 2
        if keys[mid].tick <= tick:
            lo = mid
        else:
            hi = mid
    return _segment_value(keys, lo, tick)


def sample(keys: list[Key], length: int) -> list[float]:
    """Values for ticks 0 .. length-1."""
    if not any(_joint_segment(keys, j) for j in range(len(keys) - 1)):
        return [value_at(keys, t) for t in range(length)]
    m = joint_slopes(keys, 0, len(keys) - 2)  # once for the whole track: the same numbers value_at works out
    out = []
    for t in range(length):
        if keys[0].tick < t < keys[-1].tick:
            lo, hi = 0, len(keys) - 1
            while hi - lo > 1:
                mid = (lo + hi) // 2
                if keys[mid].tick <= t:
                    lo = mid
                else:
                    hi = mid
            if keys[lo].interp == "joint":
                out.append(_joint_value(keys, lo, t, m))
                continue
        out.append(value_at(keys, t))
    return out

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

A key can also say its slope (degrees per tick): cubic segments that start or
end at it then use that instead of the estimate. The editor sets slopes so a
curve it bends joins the rest without a corner.

Before the first key the first value holds; after the last key the last value
holds.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

INTERP_MODES = ("hold", "linear", "smooth", "cubic")


@dataclass(frozen=True)
class Key:
    tick: int
    value: float
    interp: str = "linear"
    slope: float | None = None  # degrees per tick where a cubic segment meets this key (None: from the neighbours)


def wrap_degrees(angle: float) -> float:
    """Wrap to (-180, 180], the range the CS:S client sends (recordings contain
    180.0 but never -180.0). A value already in that range comes back unchanged
    bit for bit, which keeps imported recordings exact."""
    return angle - 360.0 * math.ceil((angle - 180.0) / 360.0)


def _slope(keys: list[Key], i: int) -> float:
    """Tangent (degrees per tick) at key i for cubic segments."""
    if keys[i].slope is not None:
        return keys[i].slope
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
    return [value_at(keys, t) for t in range(length)]

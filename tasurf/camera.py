"""The camera replays show on auto-strafe (rust/src/camera.rs and the editor page's cameraYaws: the same rule).

Auto-strafe plays the most speed: it looks along the velocity, and on a straight range it switches A and D every
tick or two, so the view zigzags around the way it goes by atan(30/speed) a tick (about a degree at 1800 u/s, six
at 300). The run plays that as it is. Once the game has played these exact inputs (a preview or a run with the same
hash: runs are bit-identical, so it knows every yaw the run will play), the camera is that path's yaw smoothed with
the ticks on both sides of each one: a binomial (near Gaussian) average over CAM_HALF ticks either way, about 3.5
ticks wide, so no lag and no corners. It's used on the auto-strafe ranges and blends from the yaw played into it
over the CAM_MARGIN ticks either side. A tick-to-tick change of more than CAM_BREAK degrees (a teleport) isn't
averaged across. The editor bakes it as `@view_yaw` lines (not hashed, like `@view_pitch`): the plugin puts it on
the replay's eyes and the movement still uses the yaw played.
"""
from __future__ import annotations

import math

CAM_HALF = 24        # ticks either side averaged: weights C(48, 24 + k), sigma = sqrt(48) / 2, about 3.5 ticks
CAM_MARGIN = 16      # ticks either side of a range over which it blends from the yaw played into the smoothed one
CAM_BREAK = 30.0     # degrees in one tick: a jump (a teleport), not averaged across
VIEW_YAW_PER_LINE = 16  # values on one @view_yaw line

_WEIGHTS = [float(math.comb(2 * CAM_HALF, j)) for j in range(2 * CAM_HALF + 1)]  # exact in a double


def _wrap(a: float) -> float:
    return (a + 180.0) % 360.0 - 180.0


def camera_yaws(yaws: list[float], ranges: list[tuple[int, int]]) -> dict[int, float]:
    """The camera on the ticks it differs from the yaw played: {tick: yaw}. yaws: the yaw played on each tick (a run
    of these inputs); ranges: the auto-strafe ranges, [start, end)."""
    n = len(yaws)
    if not n or not ranges:
        return {}
    u, seg = [yaws[0]], [0]  # unwrapped, and which stretch between jumps
    for i in range(1, n):
        d = _wrap(yaws[i] - yaws[i - 1])
        u.append(u[-1] + d)
        seg.append(seg[-1] + (1 if abs(d) > CAM_BREAK else 0))
    near = [CAM_MARGIN + 1] * n  # ticks to the nearest auto-strafe tick
    for start, end in ranges:
        for t in range(max(0, start - CAM_MARGIN), min(n, end + CAM_MARGIN)):
            d = start - t if t < start else t - (end - 1) if t >= end else 0
            near[t] = min(near[t], d)
    out = {}
    for t in range(n):
        if near[t] >= CAM_MARGIN:
            continue
        x = 1.0 - near[t] / CAM_MARGIN
        e = x * x * (3.0 - 2.0 * x)  # 1 on the range, easing to 0 over the margin
        acc = wsum = 0.0
        for k in range(-CAM_HALF, CAM_HALF + 1):
            j = t + k
            if 0 <= j < n and seg[j] == seg[t]:
                w = _WEIGHTS[k + CAM_HALF]
                acc += w * u[j]
                wsum += w
        out[t] = _wrap(u[t] + e * (acc / wsum - u[t]))
    return out


def _f4(x: float) -> str:
    s = f"{x:.4f}"
    return "0.0000" if s == "-0.0000" else s


def view_yaw_lines(camera: dict[int, float]) -> list[str]:
    """`@view_yaw <first tick> <yaw>...`: the yaw replays show on those ticks (runs of consecutive ticks, at most
    VIEW_YAW_PER_LINE a line). Not hashed."""
    lines, ticks, i = [], sorted(camera), 0
    while i < len(ticks):
        first, vals = ticks[i], []
        while i < len(ticks) and ticks[i] == first + len(vals) and len(vals) < VIEW_YAW_PER_LINE:
            vals.append(_f4(camera[ticks[i]]))
            i += 1
        lines.append(f"@view_yaw {first} " + " ".join(vals))
    return lines

"""Timelines as editable structure (for the browser editor), and importing
recordings as timelines.

The editor works on a normalised view of a timeline: yaw/pitch/view_pitch keys as
[tick, value, interp], or [tick, value, interp, slope] when the key says its slope,
and key ranges as {"key", "from", "to"[, "value"]} with
canonical key names. Edits come back in the same shape and are merged into
the original JSON document, so everything else in the file (start, settings,
comments) is kept as written. Parsing rules stay in timeline.py.
"""

from __future__ import annotations

import copy
import json
import math
from collections import Counter
from pathlib import Path
from typing import Any

from . import buttons
from .bake import BakedFile, bake_inputs, read_baked
from .interp import Key, key_slope, segment_slope, value_at, wrap_degrees
from .timeline import INTERP_MODES, Timeline, TimelineError, bits_to_float, float_to_bits, parse_timeline

DISPLAY_NAMES = {"forward": "W", "back": "S", "moveleft": "A", "moveright": "D"}


def _is_comment(obj: Any) -> bool:
    return isinstance(obj, dict) and bool(obj) and all(k.startswith("_") for k in obj)


def _keys_out(keys) -> list[list]:
    return [[k.tick, k.value, k.interp] + ([k.slope] if k.slope is not None else []) for k in keys]


def normalized(tl: Timeline) -> dict:
    return {
        "name": tl.name,
        "map": tl.map,
        "length": tl.length,
        "move_speed": tl.move_speed,
        "start_pitch": tl.start.angles[0],
        "yaw": _keys_out(tl.yaw),
        "pitch": _keys_out(tl.pitch) if tl.pitch is not None else None,
        "keys": [_range_out(kr.key, kr.start, kr.end, kr.value)
                 for kr in sorted(tl.keys, key=lambda k: (k.start, k.key))],
        "strafe": [{"from": r.start, "to": r.end, "offset": r.offset, **({} if r.heading is None else {"heading": r.heading}),
                    **({} if r.swing is None else {"swing": r.swing}), **({} if r.gain is None else {"gain": r.gain})}
                   for r in tl.strafe],
        "view_pitch": _keys_out(tl.view_pitch) if tl.view_pitch is not None else None,
        **({"autobhop": True} if tl.autobhop else {}),
        **({} if tl.camera_smoothing == "strafe" else {"camera_smoothing": tl.camera_smoothing}),
        **({"turns": [{"from": b.start, "to": b.end, "rate": b.rate, "shape": b.shape} for b in tl.turns]} if tl.turns else {}),
    }


def _range_out(key: str, start: int, end: int, value: float | None) -> dict:
    r: dict = {"key": key, "from": start, "to": end}
    if value is not None:
        r["value"] = value
    return r


def _track(keys: Any, what: str) -> list[list]:
    if not isinstance(keys, list) or not keys:
        raise TimelineError(f"{what}: needs at least one key")
    out = []
    for k in sorted(keys, key=lambda k: k[0]):
        tick, value, interp = int(k[0]), float(k[1]), str(k[2])
        slope = float(k[3]) if len(k) > 3 and k[3] is not None else None
        if interp not in INTERP_MODES:
            raise TimelineError(f"{what}: bad interp {interp!r}")
        out.append([tick, value, interp] + ([slope] if slope is not None else []))  # no rounding: imported values are exact float32s
    return out


def apply_edits(doc: dict, edits: dict) -> dict:
    """A copy of doc with length, yaw keys, pitch keys, key ranges, auto-strafe, view pitch, auto bhop and the camera
    smoothing replaced."""
    new = copy.deepcopy(doc)
    if "length" in edits:
        new["length"] = int(edits["length"])
    if "autobhop" in edits:  # written only when on
        if edits["autobhop"] is True:
            new["autobhop"] = True
        elif edits["autobhop"] is False:
            new.pop("autobhop", None)
        else:
            raise TimelineError(f"autobhop: expected true or false, got {edits['autobhop']!r}")
    if "camera_smoothing" in edits:  # written only when not the default
        v = edits["camera_smoothing"]
        if v == "strafe":
            new.pop("camera_smoothing", None)
        elif v in ("all", "off"):
            new["camera_smoothing"] = v
        else:
            raise TimelineError(f"camera_smoothing: expected \"strafe\", \"all\" or \"off\", got {v!r}")
    if "yaw" in edits:
        yaw = dict(new.get("yaw") or {})
        yaw["keys"] = _track(edits["yaw"], "yaw")
        new["yaw"] = yaw
    for name in ("pitch", "view_pitch"):  # optional tracks: None removes them
        if name not in edits:
            continue
        if edits[name] is None:
            new.pop(name, None)
        else:
            track = dict(new.get(name) or {})
            track["keys"] = _track(edits[name], name)
            new[name] = track
    if "keys" in edits:
        comments = [k for k in doc.get("keys", []) if _is_comment(k)]
        ranges = []
        for r in sorted(edits["keys"], key=lambda r: (int(r["from"]), r["key"])):
            key = buttons.canonical_key(str(r["key"]))
            if key is None:
                raise TimelineError(f"keys: unknown key {r['key']!r}")
            out = {"key": DISPLAY_NAMES.get(key, key), "from": int(r["from"]), "to": int(r["to"])}
            if r.get("value") is not None:
                out["value"] = float(r["value"])
            ranges.append(out)
        new["keys"] = comments + ranges
    if "strafe" in edits:
        out = []
        for r in sorted(edits["strafe"] or [], key=lambda r: int(r["from"])):
            entry = {"from": int(r["from"]), "to": int(r["to"])}
            if float(r.get("offset") or 0.0) != 0.0:
                entry["offset"] = float(r["offset"])
            if r.get("heading") is not None:
                entry["heading"] = float(r["heading"])
            if r.get("swing") is not None:
                entry["swing"] = float(r["swing"])
            if r.get("gain") is not None:
                entry["gain"] = float(r["gain"])
            out.append(entry)
        if out:
            new["strafe"] = out
        else:
            new.pop("strafe", None)
    if "turns" in edits:  # the editor's curve turns, by name
        out = [{"from": int(r["from"]), "to": int(r["to"]), "rate": float(r["rate"]), "shape": str(r["shape"])}
               for r in sorted(edits["turns"] or [], key=lambda r: int(r["from"]))]
        if out:
            new["turns"] = out
        else:
            new.pop("turns", None)
    return new


# --------------------------------------------------------------------------
# tick operations on the editor's edits ({"length", "yaw", "pitch", "keys", "strafe", "view_pitch"}):
# inserting and deleting ticks, changing the length, and generated turns.
# Keyframes after the edited ticks move by whole ticks with their values
# unchanged, so from the next keyframe on the run bakes to the same inputs.

MAX_TICKS = 1_000_000


def _key(k) -> Key:
    """A track key from the editor: [tick, value, interp], or [tick, value, interp, slope]."""
    if len(k) not in (3, 4):
        raise TimelineError("a key is [tick, value, interp] or [tick, value, interp, slope]")
    slope = k[3] if len(k) == 4 else None
    return Key(int(k[0]), float(k[1]), str(k[2]), None if slope is None else float(slope))


def _keys(track: list[list]) -> list[Key]:
    return [_key(k) for k in track]


def _value(track: list[list], tick: int) -> float:
    return value_at(_keys(track), tick)


def _segment_interp(track: list[list], tick: int) -> str:
    """Interp of the segment going forward from tick ("linear" before the first key)."""
    mode = "linear"
    for k in track:
        if k[0] > tick:
            break
        mode = k[2]
    return mode


def _own_slope(k: list) -> bool:
    return len(k) > 3 and k[3] is not None


def _curve_slope(track: list[list], tick: int, side: str) -> float:
    """The slope (degrees per tick) the curve arrives at tick with (side "in") or leaves it with ("out"). Flat
    before the first key and after the last."""
    keys = _keys(track)
    for i in range(len(keys) - 1):
        k0, k1 = keys[i], keys[i + 1]
        if (k0.tick < tick <= k1.tick) if side == "in" else (k0.tick <= tick < k1.tick):
            span = k1.tick - k0.tick
            if k0.interp == "hold":
                return 0.0
            if k0.interp == "linear":
                return (k1.value - k0.value) / span
            if k0.interp == "smooth":
                s = (tick - k0.tick) / span
                return (k1.value - k0.value) * 6.0 * s * (1.0 - s) / span
            return segment_slope(keys, i, tick)
    return 0.0


def _pin(keys: list[list], tick: int | None, side: str, source: list[list]) -> list[list]:
    """keys with a key at tick on the curve of source, such that the values on
    one side of it (side "before": ticks <= tick, "after": ticks >= tick, or
    "both") stay what source gives. A hold or linear segment just gets the key.
    A cubic segment is cut by a key with the slope the curve has there, its end
    keys saying theirs: a part of a cubic is a cubic, so both parts are the
    curve they were. A smooth segment can't be cut into two of its kind, so the
    kept part is written out tick by tick. A cubic segment on the kept side
    that meets the key where things change gets that key's slope written in, so
    it no longer depends on the keys beyond. Before the first key and after the
    last the curve is flat; the new key there holds that value."""
    keys = [list(k) for k in keys]
    if tick is None:
        return keys
    src = _keys(source)

    def exact(t, interp="linear"):
        return [t, _value(source, t), interp]

    def spell_out(j):  # segment keys[j] .. keys[j + 1], tick by tick
        keys[j][2] = "linear"
        return [exact(t) for t in range(keys[j][0] + 1, keys[j + 1][0])]

    added: list[list] = []

    def freeze(j):  # key j says the slope cubic segments meet it with, as source has it
        if not _own_slope(keys[j]):
            at = next((x for x, k in enumerate(src) if k.tick == keys[j][0]), None)
            keys[j] = keys[j][:3] + [key_slope(src, at) if at is not None else key_slope(_keys(keys), j)]

    def cubic_back(j):  # a cubic segment's slopes come from the neighbouring keys (unless its keys give them)
        if j >= 0 and keys[j][2] == "cubic":
            freeze(j + 1)

    def cubic_forward(j):
        if 0 <= j < len(keys) - 1 and keys[j][2] == "cubic":
            freeze(j)

    at = next((j for j, k in enumerate(keys) if k[0] == tick), None)
    if at is not None:
        # the key is there already, but the caller puts new keys next to it
        if side in ("before", "both"):
            cubic_back(at - 1)
        if side in ("after", "both"):
            cubic_forward(at)
    elif tick < keys[0][0]:
        added.append(exact(tick, "hold"))
        cubic_forward(0)
    elif tick > keys[-1][0]:
        keys[-1][2] = "hold"  # it had no segment after it, so its interp didn't matter until now
        added.append(exact(tick))
        cubic_back(len(keys) - 2)
    else:
        i = max(j for j, k in enumerate(keys) if k[0] < tick)  # tick is inside keys[i] .. keys[i + 1]
        lo, hi, mode = keys[i][0], keys[i + 1][0], keys[i][2]
        if mode in ("hold", "linear"):
            added.append(exact(tick, mode))
        elif mode == "cubic":
            freeze(i)
            freeze(i + 1)
            added.append(exact(tick, "cubic") + [segment_slope(_keys(keys), i, tick)])
        elif side == "before":
            added.extend(exact(t) for t in range(lo + 1, tick + 1))
            added[-1][2] = mode  # the rest of the segment, which isn't kept, stays smooth/cubic
            keys[i][2] = "linear"
        elif side == "after":
            added.extend(exact(t) for t in range(tick, hi))
        else:
            added.extend(spell_out(i))
        if side in ("before", "both"):
            cubic_back(i - 1)
        if side in ("after", "both"):
            cubic_forward(i + 1)
    return sorted(keys + added, key=lambda k: k[0])


def _canon(key: str) -> str:
    ck = buttons.canonical_key(str(key))
    if ck is None:
        raise TimelineError(f"unknown key {key!r}")
    return ck


def merge_ranges(ranges: list[dict]) -> list[dict]:
    """Drop empty ranges and join touching or overlapping ranges of the same key and value."""
    by_key: dict[str, list[dict]] = {}
    for r in ranges:
        if int(r["to"]) > int(r["from"]):
            by_key.setdefault(_canon(r["key"]), []).append(dict(r, key=_canon(r["key"])))
    out = []
    for rs in by_key.values():
        cur = None
        for r in sorted(rs, key=lambda r: r["from"]):
            if cur and r["from"] <= cur["to"] and cur.get("value") == r.get("value"):
                cur["to"] = max(cur["to"], r["to"])
            else:
                if cur:
                    out.append(cur)
                cur = r
        if cur:
            out.append(cur)
    return sorted(out, key=lambda r: (r["from"], r["key"]))


def _cut(ranges: list[dict], key: str, a: int, b: int) -> list[dict]:
    """Release key over ticks [a, b)."""
    out = []
    for r in ranges:
        if _canon(r["key"]) != key or r["to"] <= a or r["from"] >= b:
            out.append(r)
            continue
        if r["from"] < a:
            out.append(dict(r, to=a))
        if r["to"] > b:
            out.append(dict(r, **{"from": b}))
    return out


def _with(edits: dict, **changes) -> dict:
    out = {"length": int(edits["length"]), "yaw": edits["yaw"], "pitch": edits.get("pitch"),
           "keys": [dict(r) for r in edits.get("keys", [])],
           "strafe": [dict(r) for r in edits.get("strafe") or []], "view_pitch": edits.get("view_pitch"),
           "turns": [dict(r) for r in edits.get("turns") or []]}
    out.update(changes)
    return out


def insert_ticks(edits: dict, at: int, count: int) -> dict:
    """count new ticks at tick `at`, each a copy of tick `at` (or of the last
    tick when appending at the end): same yaw, pitch and held keys."""
    length = int(edits["length"])
    if not 0 <= at <= length:
        raise TimelineError(f"insert: tick {at} is outside 0 .. {length}")
    if not 1 <= count <= MAX_TICKS - length:
        raise TimelineError(f"insert: count must be 1 .. {MAX_TICKS - length}")

    def track(tr):
        if tr is None:
            return None
        if at == length:
            return [list(k) for k in tr]  # past the last key the value holds already
        pinned = _pin(tr, at, "both", tr)
        here = next(k for k in pinned if k[0] == at)
        out = [[k[0] + count if k[0] >= at else k[0], *k[1:]] for k in pinned]
        # hold tick at's value over the new ticks (a cubic arriving there meets it with the slope it had)
        out.append([at, _value(tr, at), "hold"] + ([here[3]] if _own_slope(here) else []))
        return sorted(out, key=lambda k: k[0])

    src = min(at, length - 1)

    def ranges(rs):
        out = []
        for r in rs:
            f, t = int(r["from"]), int(r["to"])
            if f <= src < t:
                out.append(dict(r, to=t + count))
            elif f > src:
                out.append(dict(r, **{"from": f + count, "to": t + count}))
            else:
                out.append(dict(r))
        return out

    return _with(edits, length=length + count, yaw=track(edits["yaw"]), pitch=track(edits.get("pitch")),
                 keys=merge_ranges(ranges(edits.get("keys", []))), strafe=_merge_strafe(ranges(edits.get("strafe") or [])),
                 view_pitch=track(edits.get("view_pitch")), turns=ranges(edits.get("turns") or []))


def delete_ticks(edits: dict, start: int, end: int) -> dict:
    """Remove ticks [start, end). What came after moves back by end - start ticks."""
    length = int(edits["length"])
    if not 0 <= start < end <= length:
        raise TimelineError(f"delete: needs 0 <= from < to <= {length}")
    n = end - start
    if n >= length:
        raise TimelineError("delete: a timeline needs at least one tick")

    def track(tr):
        if tr is None:
            return None
        # keep the curve as it was up to the cut and from the cut on
        pinned = _pin(_pin(tr, start - 1 if start > 0 else None, "before", tr), end if end < length else None, "after", tr)
        out = [[k[0] - n if k[0] >= end else k[0], *k[1:]] for k in pinned if not start <= k[0] < end]
        if not out:  # every key was in the deleted ticks: keep the value the curve had at the cut
            out = [[0, _value(tr, end if end < length else start - 1), "linear"]]
        return out

    def move(x):
        return x if x <= start else (start if x <= end else x - n)

    def ranges(rs):
        out = []
        for r in rs:
            f, t = int(r["from"]), int(r["to"])
            f2 = f if f < start else (start if f < end else f - n)
            t2 = move(t)
            if t2 > f2:
                out.append(dict(r, **{"from": f2, "to": t2}))
        return out

    return _with(edits, length=length - n, yaw=track(edits["yaw"]), pitch=track(edits.get("pitch")),
                 keys=merge_ranges(ranges(edits.get("keys", []))), strafe=_merge_strafe(ranges(edits.get("strafe") or [])),
                 view_pitch=track(edits.get("view_pitch")), turns=ranges(edits.get("turns") or []))


def _merge_strafe(ranges: list[dict]) -> list[dict]:
    """Sorted strafe ranges, touching ones with the same offset, heading, swing and gain joined."""
    out: list[dict] = []
    for r in sorted((dict(r) for r in ranges if int(r["to"]) > int(r["from"])), key=lambda r: r["from"]):
        if (out and r["from"] <= out[-1]["to"] and float(r.get("offset") or 0.0) == float(out[-1].get("offset") or 0.0)
                and r.get("heading") == out[-1].get("heading") and r.get("swing") == out[-1].get("swing")
                and r.get("gain") == out[-1].get("gain")):
            out[-1]["to"] = max(out[-1]["to"], r["to"])
        else:
            out.append(r)
    return out


def _cut_strafe(ranges: list[dict], a: int, b: int) -> list[dict]:
    """Auto-strafe off over ticks [a, b)."""
    out = []
    for r in ranges:
        if r["to"] <= a or r["from"] >= b:
            out.append(dict(r))
            continue
        if r["from"] < a:
            out.append(dict(r, to=a))
        if r["to"] > b:
            out.append(dict(r, **{"from": b}))
    return out


def strafe_at(edits: dict, tick: int) -> dict | None:
    return next((r for r in edits.get("strafe") or [] if r["from"] <= tick < r["to"]), None)


def freeze_strafe(edits: dict, first: int, end: int, yaws: list[float], sides: list[float] | None = None,
                  fwds: list[float] | None = None) -> dict:
    """Turn auto-strafe ticks [first, end) into keyframes holding exactly the
    yaws the game played there (yaws[i] for tick first + i, as float32 values in
    (-180, 180]), so the timeline bakes to the same inputs and plays the same.
    sides (a straight range: the game picked A or D each tick): the sidemove it
    played on each tick, which becomes A and D key ranges. fwds (a swing with a
    gain: it also picked how hard): the forwardmove too, and then every tick's A
    or D, and W, says how hard it was pressed."""
    length = int(edits["length"])
    if not 0 <= first < end <= length or len(yaws) < end - first:
        raise TimelineError("freeze: the run doesn't cover those ticks")
    yaw = edits["yaw"]
    ref = _value(yaw, first - 1) if first > 0 else yaws[0]
    values = []
    for y in yaws[:end - first]:  # unwrapped, continuing from the curve before the range
        v = _f32_offset(ref + wrap_degrees(y - wrap_degrees(ref)), y)
        values.append(v)
        ref = v
    pinned = _pin(_pin(yaw, first - 1 if first > 0 else None, "before", yaw), end if end < length else None, "after", yaw)
    out = [k for k in pinned if not first <= k[0] < end]
    out += [[first + i, values[i], "linear"] for i in _simplify(values, 0.0)]
    keys = edits.get("keys", [])
    if sides is not None:
        if len(sides) < end - first:
            raise TimelineError("freeze: the run doesn't cover those ticks")
        held = next((r for r in keys if _canon(r["key"]) in ("moveleft", "moveright") and r["from"] <= first < r["to"]), None)
        ranges = [dict(r) for r in keys]
        for k in ("moveleft", "moveright") + (("forward", "back") if fwds is not None else ()):
            ranges = _cut(ranges, k, first, end)
        if fwds is not None:  # a swing with a gain: every tick as hard as it was pressed
            if len(fwds) < end - first:
                raise TimelineError("freeze: the run doesn't cover those ticks")
            for t in range(first, end):
                side, fwd = sides[t - first], fwds[t - first]
                if side != 0.0:
                    ranges.append({"key": "moveleft" if side < 0 else "moveright", "from": t, "to": t + 1, "value": abs(side)})
                if fwd > 0.0:
                    ranges.append({"key": "forward", "from": t, "to": t + 1, "value": fwd})
        t = end if fwds is not None else first
        while t < end:
            side = sides[t - first]
            u = t + 1
            while u < end and (sides[u - first] > 0) == (side > 0) and (sides[u - first] < 0) == (side < 0):
                u += 1
            if side != 0.0:
                r = {"key": "moveleft" if side < 0 else "moveright", "from": t, "to": u}
                if held is not None and held.get("value") is not None:
                    r["value"] = held["value"]
                ranges.append(r)
            t = u
        keys = merge_ranges(ranges)
    return _with(edits, yaw=sorted(out, key=lambda k: k[0]), keys=keys, strafe=_cut_strafe(edits.get("strafe") or [], first, end))


def splice_recording(edits: dict, at: int, rec: list, move_speed: float, current: list,
                     auto_ticks: set[int] | frozenset = frozenset(), start_pitch: float = 0.0) -> dict:
    """The timeline up to tick `at`, then the recording from `at` on.

    rec: the inputs of a recording made from tick `at` of this timeline (its first
    `at` ticks are the timeline as the game played it), with exact float32 values.
    current: this timeline's baked inputs, to check the recording really starts from
    it (yaw is skipped on auto-strafe ticks, where the game chose it). Everything
    before `at` stays as it was: keyframes, key ranges and auto-strafe; from `at` on
    the recording's inputs become keyframes and key ranges, exactly as an import. A view
    pitch goes on with the pitch you played, so replays show where you really looked."""
    length = int(edits["length"])
    if not 1 <= at <= length or len(rec) <= at:
        raise TimelineError(f"splice: the recording has to go past tick {at} (it has {len(rec)} ticks)")
    for t in range(at):
        a, b = rec[t], current[t]
        same = (a.forwardmove == b.forwardmove and a.sidemove == b.sidemove and a.buttons == b.buttons
                and float_to_bits(a.pitch) == float_to_bits(b.pitch)
                and (t in auto_ticks or float_to_bits(wrap_degrees(a.yaw)) == float_to_bits(b.yaw)))
        if not same:
            raise TimelineError(f"splice: the timeline changed since the recording started (tick {t} differs); "
                                f"record again from this version")

    def suffix(track: list[list], played: list[float], ref: float, unwrap: bool) -> list[list]:
        values = []
        for y in played:  # yaw continues from the curve before the cut
            v = _f32_offset(ref + wrap_degrees(y - wrap_degrees(ref)), y) if unwrap else y
            values.append(v)
            ref = v
        pinned = _pin(track, at - 1, "before", track)
        return [k for k in pinned if k[0] < at] + [[at + i, values[i], "linear"] for i in _simplify(values, 0.0)]

    yaw = edits["yaw"]
    ref = _value(yaw, at - 1)
    ref = _f32_offset(ref + wrap_degrees(rec[at - 1].yaw - wrap_degrees(ref)), rec[at - 1].yaw)
    new_yaw = suffix(yaw, [ti.yaw for ti in rec[at:]], ref, True)
    pitch = edits.get("pitch") or [[0, start_pitch, "linear"]]
    new_pitch = suffix(pitch, [ti.pitch for ti in rec[at:]], 0.0, False)
    view = edits.get("view_pitch")
    new_view = suffix(view, [ti.pitch for ti in rec[at:]], 0.0, False) if view else None

    keys = []
    for r in edits.get("keys", []):
        if int(r["from"]) >= at:
            continue
        keys.append(dict(r, to=min(int(r["to"]), at)))
    for r in _key_ranges(rec[at:], move_speed):
        keys.append(dict(r, **{"from": r["from"] + at, "to": r["to"] + at}))
    strafe = _cut_strafe(edits.get("strafe") or [], at, max(length, len(rec)))
    return _with(edits, length=len(rec), yaw=new_yaw, pitch=new_pitch, keys=merge_ranges(keys), strafe=strafe,
                 view_pitch=new_view, turns=_cut_strafe(edits.get("turns") or [], at, max(length, len(rec))))


def set_length(edits: dict, new_length: int) -> dict:
    """Longer: the last tick repeats (keys held at the end stay held). Shorter: cut the end."""
    length = int(edits["length"])
    if new_length > length:
        return insert_ticks(edits, length, new_length - length)
    if new_length < length:
        return delete_ticks(edits, new_length, length)
    return _with(edits)


TURN_EASE = 8  # ticks at most a constant-speed turn eases in and out over, to meet the curve without a corner


def make_turn(edits: dict, first: int, last: int, rate: float, shape: str = "linear",
              keys: dict | None = None, after: str = "keep", start_yaw: float | None = None,
              offset: float = 0.0, heading: float | None = None, swing: float | None = None,
              gain: float | None = None) -> dict:
    """A turn from tick first to tick last (both included) at rate degrees per
    tick (positive turns left), starting from the current yaw at first unless
    start_yaw is given. Either shape meets the curve before and after it with the
    slope the curve has there, so the yaw gets no corners: "linear" eases into
    the rate over at most TURN_EASE ticks (a quarter of the turn), turns at it,
    and eases out (so the total is a little more or less than rate times ticks);
    "smooth" is one curve over the whole turn, with that total. keys maps key names to "hold", "release" or
    "keep" for ticks first..last. after: "keep" leaves later keyframes where
    they are, "shift" moves them by the change the turn made at tick last.
    shape "auto" doesn't write a curve: those ticks become auto-strafe (the game
    aims the held keys from the real velocity, offset degrees past the max-gain
    angle), and the yaw keyframes stay as the fallback for ground ticks. With a
    heading, they go straight that way: the game also picks A or D each tick (with a
    swing, keeping a side until the direction is that far past the heading; with
    a gain as well, turning smoothly and keeping that much of the most speed gain). A
    curve turn is also named in the edits' turns (a block the editor can show,
    edit again or remove); a turn over part of another one cuts it."""
    length = int(edits["length"])
    if not 0 <= first < last <= length - 1:
        raise TimelineError(f"turn: needs 0 <= from < to <= {length - 1}")
    if shape not in ("linear", "smooth", "auto"):
        raise TimelineError(f"turn: shape must be linear, smooth or auto, got {shape!r}")
    if not -90.0 <= float(offset) <= 90.0:
        raise TimelineError("turn: offset must be between -90 and 90 degrees")
    if after not in ("keep", "shift"):
        raise TimelineError(f"turn: after must be keep or shift, got {after!r}")
    rate = float(rate)
    if not math.isfinite(rate):
        raise TimelineError("turn: rate must be a number")
    yaw = edits["yaw"]
    if swing is not None and (heading is None or not 0.0 < float(swing) <= 90.0):
        raise TimelineError("turn: a swing is for straight auto-strafe, more than 0 and at most 90 degrees")
    if gain is not None and (swing is None or not 75.0 <= float(gain) <= 100.0):
        raise TimelineError("turn: a gain goes with a swing: how much of the most speed gain to keep, 75 to 100")
    strafe = _cut_strafe(edits.get("strafe") or [], first, last + 1)
    turns = _cut_strafe(edits.get("turns") or [], first, last + 1)
    if shape == "auto":
        out = [list(k) for k in yaw]
        entry = {"from": first, "to": last + 1, "offset": float(offset)}
        if heading is not None:
            entry["heading"] = float(heading)
        if swing is not None:
            entry["swing"] = float(swing)
        if gain is not None:
            entry["gain"] = float(gain)
        strafe = _merge_strafe(strafe + [entry])
    else:
        y0 = _value(yaw, first) if start_yaw is None else float(start_yaw)
        s0, s1 = _curve_slope(yaw, first, "in"), _curve_slope(yaw, last, "out")  # the curve's, where the turn meets it
        span = last - first
        turn = [[first, y0, "cubic", s0]]
        r = min(TURN_EASE, span // 4) if shape == "linear" else 0
        if r > 0:  # ease into the rate, turn at it, ease out (a cubic whose slopes average the secant is a parabola)
            ya = y0 + r * (s0 + rate) / 2.0
            yb = ya + rate * (span - 2 * r)
            turn += [[first + r, ya, "cubic", rate], [last - r, yb, "cubic", rate]]
            y1 = yb + r * (rate + s1) / 2.0
        else:
            y1 = y0 + rate * span
        delta = y1 - _value(yaw, last)
        pinned = _pin(yaw, first, "before", yaw)  # the curve before the turn stays as it was
        if after == "shift":
            pinned = _pin(pinned, last, "after", yaw)  # and after it, moved by delta
        end_interp = _segment_interp(pinned, last)
        out = [[k[0], k[1] + delta if after == "shift" and k[0] > last else k[1], *k[2:]] for k in pinned if not first <= k[0] <= last]
        if after == "keep":  # the curve on to the next key leaves with the slope it had; that key keeps its own
            out += turn + [[last, y1, "cubic", s1]]
            nxt = next((k for k in out if k[0] > last), None)
            if nxt is not None and not _own_slope(nxt):
                nxt[3:] = [_curve_slope(yaw, nxt[0], "out")]
        else:  # the curve after is the one it was, moved: the turn arrives with its slope
            out += turn + [[last, y1, end_interp] + ([s1] if end_interp == "cubic" else [])]
        turns = sorted(turns + [{"from": first, "to": last + 1, "rate": rate, "shape": shape}], key=lambda r: r["from"])
    ranges = [dict(r) for r in edits.get("keys", [])]
    for key, action in (keys or {}).items():
        if action == "keep":
            continue
        if action not in ("hold", "release"):
            raise TimelineError(f"turn: key action must be hold, release or keep, got {action!r}")
        ck = _canon(key)
        ranges = _cut(ranges, ck, first, last + 1)
        if action == "hold":
            ranges.append({"key": ck, "from": first, "to": last + 1})
    return _with(edits, yaw=sorted(out, key=lambda k: k[0]), keys=merge_ranges(ranges), strafe=strafe, turns=turns)


def apply_op(edits: dict, op: dict) -> tuple[dict, str]:
    """Run one editor operation; returns the new edits and a short description."""
    kind = op.get("op")
    if kind == "insert":
        at, count = int(op["at"]), int(op["count"])
        return insert_ticks(edits, at, count), f"inserted {count} tick(s) at tick {at}"
    if kind == "delete":
        a, b = int(op["from"]), int(op["to"])
        return delete_ticks(edits, a, b), f"deleted ticks {a}..{b - 1} ({b - a})"
    if kind == "length":
        n = int(op["length"])
        return set_length(edits, n), f"length {n} ticks"
    if kind == "turn":
        a, b = int(op["from"]), int(op["to"])
        rate = float(op["rate"])
        start = op.get("start_yaw")
        shape = op.get("shape", "linear")
        offset = float(op.get("offset") or 0.0)
        heading, swing, gain = op.get("heading"), op.get("swing"), op.get("gain")
        if heading is not None:
            heading = float(heading)
            if not math.isfinite(heading):
                raise TimelineError("turn: heading must be a number")
        if swing is not None:
            swing = float(swing)
        if gain is not None:
            gain = float(gain)
        new = make_turn(edits, a, b, rate, shape, op.get("keys"), op.get("after", "keep"),
                        None if start is None else float(start), offset, heading, swing, gain)
        if shape == "auto":
            return new, (f"auto-strafe ticks {a}..{b}" + (f" straight at {heading:g}°" if heading is not None else "")
                         + (f", swinging ±{swing:g}°" if swing is not None else "")
                         + (f" smoothly, keeping {gain:g}% of the speed gain" if gain is not None else "")
                         + (f", {offset:+g}° past the max-gain angle" if offset else "")
                         + (": the game picks A or D and aims from the real velocity" if heading is not None
                            else ": the game aims from the real velocity"))
        side = "left" if rate > 0 else "right" if rate < 0 else "no turn"
        total = _value(new["yaw"], b) - _value(new["yaw"], a)
        return new, f"turn {side} ticks {a}..{b}: {abs(rate):g}°/tick, {total:+g}° in total, joining the curve smoothly"
    raise TimelineError(f"unknown operation {kind!r}")


# --------------------------------------------------------------------------
# JSON layout that stays pleasant to hand-edit


def _flat(v: Any) -> bool:
    return not isinstance(v, (dict, list)) or (isinstance(v, list) and all(not isinstance(x, (dict, list)) for x in v))


def format_json(obj: Any, level: int = 0) -> str:
    pad = "  " * level
    if isinstance(obj, dict):
        if not obj:
            return "{}"
        one_line = json.dumps(obj, ensure_ascii=False)
        if level > 0 and all(_flat(v) for v in obj.values()) and len(one_line) + len(pad) <= 110:
            return "{ " + one_line[1:-1] + " }"
        items = [f"{pad}  {json.dumps(k, ensure_ascii=False)}: {format_json(v, level + 1)}" for k, v in obj.items()]
        return "{\n" + ",\n".join(items) + "\n" + pad + "}"
    if isinstance(obj, list):
        if not obj:
            return "[]"
        if all(not isinstance(v, (dict, list)) for v in obj):
            return json.dumps(obj, ensure_ascii=False)
        items = [pad + "  " + format_json(v, level + 1) for v in obj]
        return "[\n" + ",\n".join(items) + "\n" + pad + "]"
    return json.dumps(obj, ensure_ascii=False)


def save_document(path: Path, doc: dict, start_search_dirs: list[Path] | None = None) -> Timeline:
    """Validate and write; returns the parsed timeline. Raises TimelineError and writes nothing if invalid."""
    tl = parse_timeline(doc, path=path, start_search_dirs=start_search_dirs)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(format_json(doc) + "\n", encoding="utf-8", newline="\n")
    tmp.replace(path)
    return tl


# --------------------------------------------------------------------------
# importing a recording (a baked file from tas_record) as an editable timeline


def _f32(x: float) -> float:
    return bits_to_float(float_to_bits(x))


def _simplify(values: list[float], tol: float) -> list[int]:
    """Indices of keyframes such that linear interpolation between them gives
    every value back: exactly as float32 when tol == 0, else within tol."""
    n = len(values)
    if n <= 2:
        return list(range(n))

    # what the plugin ends up with for a value: float32 of the wrapped angle
    target = [_f32(wrap_degrees(v)) for v in values]

    def err(i: int, j: int) -> tuple[float, int]:
        worst, at = 0.0, -1
        a, b = values[i], values[j]
        for t in range(i + 1, j):
            v = a + (b - a) * (t - i) / (j - i)  # same formula as interp.py's linear segment
            if tol > 0:
                e = abs(v - values[t])
            else:
                e = 0.0 if _f32(wrap_degrees(v)) == target[t] else abs(v - values[t]) + 1e-12
            if e > worst:
                worst, at = e, t
        return worst, at

    keep = {0, n - 1}
    stack = [(0, n - 1)]
    while stack:
        i, j = stack.pop()
        if j - i < 2:
            continue
        worst, at = err(i, j)
        if worst > tol:
            keep.add(at)
            stack.append((i, at))
            stack.append((at, j))
    return sorted(keep)


def _num(s: str) -> int | float | str:
    try:
        f = float(s)
    except ValueError:
        return s
    return int(f) if f.is_integer() and "." not in s else f


def _start_from_baked(start: dict[str, list[str]]) -> dict:
    out: dict = {}
    for name in ("origin", "angles", "velocity"):
        bits = start[f"{name}_bits"]
        out[name] = [bits_to_float(b) for b in bits]
        out[f"{name}_bits"] = bits
    out["base_velocity"] = [float(v) for v in start.get("base_velocity", ["0", "0", "0"])]
    out["base_velocity_flag"] = start.get("base_velocity_flag", ["0"])[0] != "0"
    og = start.get("on_ground", ["auto"])[0]
    out["on_ground"] = True if og == "yes" else False if og == "no" else "auto"
    out["ground_entity"] = int(start.get("ground_entity", ["0"])[0])
    out["ducked"] = False
    for name in ("gravity", "lagged_movement", "velocity_modifier", "stamina", "view_offset_z", "maxspeed"):
        if name in start:
            out[name] = float(start[name][0])
    if "health" in start:
        out["health"] = int(start["health"][0])
    return out


def _key_ranges(inputs, move_speed: float) -> list[dict]:
    """Key ranges reproducing the recorded buttons and move values (ticks that
    keys can't express show up in the import check)."""
    ranges: list[dict] = []
    n = len(inputs)
    for bit_index in range(32):
        key = buttons.key_for_bit(bit_index)
        bit, fsign, ssign = buttons.key_info(key)
        t = 0
        while t < n:
            if not inputs[t].buttons & bit:
                t += 1
                continue
            start, mag = t, _magnitude(inputs[t], key, move_speed)
            t += 1
            while t < n and inputs[t].buttons & bit and _magnitude(inputs[t], key, move_speed) == mag:
                t += 1
            r = {"key": DISPLAY_NAMES.get(key, key), "from": start, "to": t}
            if (fsign or ssign) and mag != move_speed:
                r["value"] = mag
            ranges.append(r)
    ranges.sort(key=lambda r: (r["from"], r["key"]))
    return ranges


def _magnitude(ti, key: str, move_speed: float) -> float:
    """The move magnitude a held movement key contributed on this tick."""
    if key in ("forward", "back"):
        both = ti.buttons & buttons.IN_FORWARD and ti.buttons & buttons.IN_BACK
        return move_speed if both or ti.forwardmove == 0.0 else abs(ti.forwardmove)
    if key in ("moveleft", "moveright"):
        both = ti.buttons & buttons.IN_MOVELEFT and ti.buttons & buttons.IN_MOVERIGHT
        return move_speed if both or ti.sidemove == 0.0 else abs(ti.sidemove)
    return 0.0


def recording_to_timeline(baked: BakedFile, name: str, tolerance: float = 0.0) -> tuple[dict, dict]:
    """A timeline document from a recording, plus a report. With tolerance 0 the
    timeline bakes back to exactly the recorded float32 inputs."""
    inputs = baked.inputs
    yaw_u: list[float] = []
    for ti in inputs:  # unwrap: no human turns more than 180 degrees in one tick
        yaw_u.append(ti.yaw if not yaw_u else yaw_u[-1] + wrap_degrees(ti.yaw - wrap_degrees(yaw_u[-1])))
    yaw_u = [_f32_offset(y, ti.yaw) for y, ti in zip(yaw_u, inputs)]
    pitch = [ti.pitch for ti in inputs]

    mags = Counter(abs(v) for ti in inputs for v in (ti.forwardmove, ti.sidemove) if v != 0.0)
    move_speed = mags.most_common(1)[0][0] if mags else 400.0

    yaw_keys = [[t, yaw_u[t]] for t in _simplify(yaw_u, tolerance)]
    pitch_keys = [[t, pitch[t]] for t in _simplify(pitch, tolerance)]
    ranges = _key_ranges(inputs, move_speed)
    settings = {k: _num(v) for k, v in baked.settings.items()}

    doc = {
        "_comment": f"Imported from the recording {baked.name} ({len(inputs)} ticks). "
                    f"Edit freely; the original recording is unchanged.",
        "_baseline": baked.name,
        "format": "tasurf-timeline",
        "version": 1,
        "name": name,
        "map": baked.map,
        "length": len(inputs),
        "settings": settings,
        "start": _start_from_baked(baked.start),
        "input": {"move_speed": move_speed},
        "yaw": {"default_interp": "linear", "keys": yaw_keys},
        "pitch": {"default_interp": "linear", "keys": pitch_keys},
        "keys": ranges,
    }
    if baked.autobhop:  # the plugin's recordings never are (they hold the jumps as played), but a baked timeline can be
        doc["autobhop"] = True

    # Check: bake it and compare with the recording, float32 bit for bit.
    tl = parse_timeline(doc)
    rebaked = bake_inputs(tl)
    diffs = []
    for a, b in zip(inputs, rebaked):
        fields = []
        for f in ("yaw", "pitch", "forwardmove", "sidemove"):
            if float_to_bits(getattr(a, f)) != float_to_bits(getattr(b, f)):
                fields.append(f)
        if a.buttons != b.buttons:
            fields.append("buttons")
        if fields:
            diffs.append((a.tick, fields))
    report = {
        "ticks": len(inputs),
        "yaw_keys": len(yaw_keys),
        "pitch_keys": len(pitch_keys),
        "key_ranges": len(ranges),
        "move_speed": move_speed,
        "exact": not diffs,
        "differing_ticks": len(diffs),
        "first_differences": [f"tick {t}: {', '.join(f)}" for t, f in diffs[:10]],
    }
    return doc, report


def _f32_offset(unwrapped: float, recorded: float) -> float:
    """Keep the unwrapped value an exact 360-degree multiple away from the
    recorded float32, so wrapping it again restores the recorded value."""
    turns = round((unwrapped - recorded) / 360.0)
    return recorded + 360.0 * turns


def import_recording(baked_path: Path, out_path: Path, name: str, tolerance: float = 0.0) -> dict:
    baked = read_baked(baked_path.read_text(encoding="utf-8"))
    doc, report = recording_to_timeline(baked, name, tolerance)
    save_document(out_path, doc)
    report["path"] = str(out_path)
    return report

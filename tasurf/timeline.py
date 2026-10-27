"""Load and validate timeline JSON files.

See docs/timeline-format.md for the format. Errors name the JSON location
(e.g. ``keys[3].to``) because the file is edited by hand. Any object key that
starts with an underscore is a comment and is ignored.
"""

from __future__ import annotations

import json
import math
import re
import struct
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from . import buttons
from .interp import INTERP_MODES, Key

FORMAT_NAME = "tasurf-timeline"
FORMAT_VERSION = 1
START_FORMAT_NAME = "tasurf-start"

NAME_RE = re.compile(r"^[A-Za-z0-9_.\-]+$")
SETTING_NAME_RE = re.compile(r"^[A-Za-z0-9_]+$")
SETTING_VALUE_RE = re.compile(r"^\S+$")
HEX_RE = re.compile(r"^[0-9A-Fa-f]{1,8}$")


class TimelineError(ValueError):
    pass


@dataclass
class StartState:
    origin: tuple[float, float, float]
    angles: tuple[float, float, float]
    velocity: tuple[float, float, float] = (0.0, 0.0, 0.0)
    # Exact float32 bit patterns (8 hex digits) when the start was captured in game.
    origin_bits: tuple[str, str, str] | None = None
    angles_bits: tuple[str, str, str] | None = None
    velocity_bits: tuple[str, str, str] | None = None
    base_velocity: tuple[float, float, float] = (0.0, 0.0, 0.0)
    base_velocity_flag: bool = False
    on_ground: str = "auto"  # "yes", "no" or "auto"
    ground_entity: int = 0
    ducked: bool = False
    view_offset_z: float | None = None
    gravity: float = 1.0
    lagged_movement: float = 1.0
    # CS:S slowdowns, always reset: 1.0 = no damage slowdown, 0.0 = rested (no jump/landing slowdown)
    velocity_modifier: float = 1.0
    stamina: float = 0.0
    health: int | None = None
    maxspeed: float | None = None
    map: str | None = None
    source: str = "inline"


@dataclass
class KeyRange:
    key: str  # canonical key name
    start: int  # first tick held (inclusive)
    end: int  # first tick NOT held (exclusive)
    value: float | None  # forwardmove/sidemove magnitude override
    index: int  # position in the "keys" list, for messages


@dataclass
class StrafeRange:
    start: int  # first tick (inclusive)
    end: int  # first tick after the range (exclusive)
    offset: float  # degrees past the max-gain angle (positive turns harder)
    index: int  # position in the "strafe" list, for messages
    heading: float | None = None  # straight: the direction to keep going (world yaw); the plugin picks A or D
    swing: float | None = None  # straight: keep a side until the direction is this many degrees past the heading
    gain: float | None = None  # with a swing: keep this much of the most speed gain (75 to 100 percent), smoothly


@dataclass
class TurnBlock:
    """A turn the editor's Turn made as a curve. The yaw keyframes are what plays; this only names the stretch
    they make, so the editor can show it as one block, edit it again or remove it. Not baked."""
    start: int  # first tick (inclusive)
    end: int  # first tick after it (exclusive)
    rate: float  # degrees per tick, positive turns left
    shape: str  # "linear" (constant speed) or "smooth" (ease in and out)


@dataclass
class Timeline:
    name: str
    map: str
    length: int
    settings: dict[str, Any]
    start: StartState
    move_speed: float
    yaw: list[Key]
    pitch: list[Key] | None
    keys: list[KeyRange]
    path: Path | None = None
    strafe: list[StrafeRange] = field(default_factory=list)
    # pitch shown on replays only (the replay bot's eyes); the run's movement uses `pitch`
    view_pitch: list[Key] | None = None
    # a held jump jumps again on landing (the plugin lets it go in the air)
    autobhop: bool = False
    # what replays show (@view_yaw, camera.py): "strafe" auto-strafe's yaw smoothed, "all" the whole run's, "off" the
    # yaw played, zigzag and all
    camera_smoothing: str = "strafe"
    turns: list[TurnBlock] = field(default_factory=list)  # the editor's curve turns, by name only
    # the editor's: ticks [from, to) whose A and D were set by hand (A and D following the yaw leave them)
    ad_by_hand: list[tuple[int, int]] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)


# --------------------------------------------------------------------------
# small typed accessors


def _is_num(v: Any) -> bool:
    return isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v)


def _num(v: Any, ctx: str) -> float:
    if not _is_num(v):
        raise TimelineError(f"{ctx}: expected a number, got {v!r}")
    return float(v)


def _int(v: Any, ctx: str) -> int:
    if isinstance(v, bool) or not isinstance(v, (int, float)) or float(v) != int(v):
        raise TimelineError(f"{ctx}: expected an integer, got {v!r}")
    return int(v)


def _bool(v: Any, ctx: str) -> bool:
    if not isinstance(v, bool):
        raise TimelineError(f"{ctx}: expected true or false, got {v!r}")
    return v


def _str(v: Any, ctx: str) -> str:
    if not isinstance(v, str) or not v:
        raise TimelineError(f"{ctx}: expected a non-empty string, got {v!r}")
    return v


def _vec(v: Any, ctx: str, n: int = 3) -> tuple[float, ...]:
    if not isinstance(v, list) or len(v) != n:
        raise TimelineError(f"{ctx}: expected a list of {n} numbers, got {v!r}")
    return tuple(_num(x, f"{ctx}[{i}]") for i, x in enumerate(v))


def _bits(v: Any, ctx: str) -> tuple[str, str, str]:
    if not isinstance(v, list) or len(v) != 3:
        raise TimelineError(f"{ctx}: expected a list of 3 hex strings, got {v!r}")
    out = []
    for i, x in enumerate(v):
        if not isinstance(x, str) or not HEX_RE.match(x):
            raise TimelineError(f"{ctx}[{i}]: expected up to 8 hex digits, got {x!r}")
        out.append(x.upper().rjust(8, "0"))
    return tuple(out)  # type: ignore[return-value]


def bits_to_float(hex_bits: str) -> float:
    return struct.unpack("<f", struct.pack("<I", int(hex_bits, 16)))[0]


def float_to_bits(x: float) -> str:
    x = float(x) + 0.0  # turns -0.0 into 0.0
    return "%08X" % struct.unpack("<I", struct.pack("<f", x))[0]


def _is_comment(obj: Any) -> bool:
    """A list entry like {"_comment": "..."} that only holds comments."""
    return isinstance(obj, dict) and bool(obj) and all(k.startswith("_") for k in obj)


def _fields(obj: Any, ctx: str, allowed: set[str], required: set[str] = frozenset()) -> dict:
    if not isinstance(obj, dict):
        raise TimelineError(f"{ctx}: expected an object, got {type(obj).__name__}")
    clean = {k: v for k, v in obj.items() if not k.startswith("_")}
    unknown = sorted(set(clean) - allowed)
    if unknown:
        raise TimelineError(f"{ctx}: unknown field(s) {', '.join(unknown)} "
                            f"(allowed: {', '.join(sorted(allowed))}; prefix a key with _ to comment it out)")
    missing = sorted(required - set(clean))
    if missing:
        raise TimelineError(f"{ctx}: missing required field(s) {', '.join(missing)}")
    return clean


# --------------------------------------------------------------------------
# start state

_START_FIELDS = {
    "format", "version", "map", "origin", "origin_bits", "angles", "angles_bits",
    "velocity", "velocity_bits", "base_velocity", "base_velocity_flag", "on_ground",
    "ground_entity", "ducked", "view_offset_z", "gravity", "lagged_movement",
    "velocity_modifier", "stamina", "health", "maxspeed",
}


def _check_bits(values: tuple[float, ...], bits: tuple[str, str, str], ctx: str) -> None:
    for i, (v, b) in enumerate(zip(values, bits)):
        exact = bits_to_float(b)
        if not math.isfinite(exact) or abs(exact - v) > 1e-4 + 1e-6 * abs(v):
            raise TimelineError(
                f"{ctx}[{i}]: decimal value {v} does not match its _bits value {b} (= {exact!r}). "
                f"If you edited the decimal by hand, delete the matching *_bits entry.")


def parse_start(obj: Any, ctx: str = "start", source: str = "inline") -> StartState:
    d = _fields(obj, ctx, _START_FIELDS, {"origin", "angles"})
    if "format" in d and d["format"] != START_FORMAT_NAME:
        raise TimelineError(f"{ctx}.format: expected {START_FORMAT_NAME!r}, got {d['format']!r}")
    if "version" in d and d["version"] != 1:
        raise TimelineError(f"{ctx}.version: unsupported version {d['version']!r}")

    st = StartState(origin=_vec(d["origin"], f"{ctx}.origin"),  # type: ignore[arg-type]
                    angles=_vec(d["angles"], f"{ctx}.angles"),  # type: ignore[arg-type]
                    source=source)
    if "velocity" in d:
        st.velocity = _vec(d["velocity"], f"{ctx}.velocity")  # type: ignore[assignment]
    for name in ("origin", "angles", "velocity"):
        key = f"{name}_bits"
        if key in d:
            bits = _bits(d[key], f"{ctx}.{key}")
            _check_bits(getattr(st, name), bits, f"{ctx}.{name}")
            setattr(st, key, bits)
    if "base_velocity" in d:
        st.base_velocity = _vec(d["base_velocity"], f"{ctx}.base_velocity")  # type: ignore[assignment]
    if "base_velocity_flag" in d:
        st.base_velocity_flag = _bool(d["base_velocity_flag"], f"{ctx}.base_velocity_flag")
    if "on_ground" in d:
        og = d["on_ground"]
        if og is True:
            st.on_ground = "yes"
        elif og is False:
            st.on_ground = "no"
        elif og == "auto":
            st.on_ground = "auto"
        else:
            raise TimelineError(f"{ctx}.on_ground: expected true, false or \"auto\", got {og!r}")
    if "ground_entity" in d:
        st.ground_entity = _int(d["ground_entity"], f"{ctx}.ground_entity")
        if st.ground_entity < 0:
            raise TimelineError(f"{ctx}.ground_entity: must be >= 0 (0 is the world)")
    if "ducked" in d:
        st.ducked = _bool(d["ducked"], f"{ctx}.ducked")
        if st.ducked:
            raise TimelineError(f"{ctx}.ducked: ducked start states are not supported yet "
                                f"(stand up and capture again)")
    for name in ("view_offset_z", "velocity_modifier", "stamina", "maxspeed"):
        if name in d:
            setattr(st, name, _num(d[name], f"{ctx}.{name}"))
    for name in ("gravity", "lagged_movement"):
        if name in d:
            setattr(st, name, _num(d[name], f"{ctx}.{name}"))
    if "health" in d:
        st.health = _int(d["health"], f"{ctx}.health")
    if "map" in d:
        st.map = _str(d["map"], f"{ctx}.map")
    if abs(st.angles[2]) > 0.0:
        raise TimelineError(f"{ctx}.angles[2]: roll must be 0")
    return st


def _load_start_file(ref: str, base_dir: Path, search_dirs: list[Path]) -> StartState:
    candidates: list[Path] = []
    for d in [base_dir, *search_dirs]:
        candidates.append(d / ref)
        if not ref.lower().endswith(".json"):
            candidates.append(d / f"{ref}.json")
    for c in candidates:
        if c.is_file():
            try:
                obj = json.loads(c.read_text(encoding="utf-8"))
            except json.JSONDecodeError as e:
                raise TimelineError(f"start file {c}: invalid JSON: {e}") from e
            return parse_start(obj, ctx=f"start file {c.name}", source=str(c))
    tried = "\n  ".join(str(c) for c in candidates)
    raise TimelineError(f"start: could not find start file {ref!r}; tried:\n  {tried}")


# --------------------------------------------------------------------------
# tracks and key ranges


def _parse_track(obj: Any, ctx: str, length: int) -> list[Key]:
    d = _fields(obj, ctx, {"keys", "default_interp"}, {"keys"})
    default = d.get("default_interp", "linear")
    if default not in INTERP_MODES:
        raise TimelineError(f"{ctx}.default_interp: expected one of {', '.join(INTERP_MODES)}, got {default!r}")
    raw = d["keys"]
    if not isinstance(raw, list) or not [k for k in raw if not _is_comment(k)]:
        raise TimelineError(f"{ctx}.keys: expected a non-empty list")
    keys: list[Key] = []
    for i, k in enumerate(raw):
        kctx = f"{ctx}.keys[{i}]"
        if _is_comment(k):
            continue
        if isinstance(k, list):  # shorthand [tick, value], [tick, value, "interp"], [..., slope] or [..., slope or null, joint]
            if len(k) not in (2, 3, 4, 5):
                raise TimelineError(f"{kctx}: expected [tick, value], [tick, value, \"interp\"], "
                                    f"[tick, value, \"interp\", slope] or [tick, value, \"interp\", slope, joint], got {k!r}")
            tick, value = _int(k[0], f"{kctx}[0]"), _num(k[1], f"{kctx}[1]")
            slope = _num(k[3], f"{kctx}[3]") if len(k) >= 4 and (len(k) == 4 or k[3] is not None) else None
            joint = _num(k[4], f"{kctx}[4]") if len(k) == 5 else None
            interp = k[2] if len(k) >= 3 else default
        else:
            kd = _fields(k, kctx, {"tick", "value", "interp", "slope", "joint"}, {"tick", "value"})
            tick, value = _int(kd["tick"], f"{kctx}.tick"), _num(kd["value"], f"{kctx}.value")
            slope = _num(kd["slope"], f"{kctx}.slope") if "slope" in kd else None
            joint = _num(kd["joint"], f"{kctx}.joint") if "joint" in kd else None
            interp = kd.get("interp", default)
        if joint is not None and joint < 0:
            raise TimelineError(f"{kctx}: a joint's width can't be negative, got {joint!r}")
        if interp not in INTERP_MODES:
            raise TimelineError(f"{kctx}: interp must be one of {', '.join(INTERP_MODES)}, got {interp!r}")
        if not 0 <= tick < length:
            raise TimelineError(f"{kctx}: tick {tick} is outside the timeline (0 .. {length - 1})")
        if keys and tick <= keys[-1].tick:
            raise TimelineError(f"{kctx}: tick {tick} must be greater than the previous key's tick {keys[-1].tick}")
        keys.append(Key(tick, value, interp, slope, joint))
    return keys


def _parse_key_ranges(raw: Any, length: int) -> list[KeyRange]:
    if not isinstance(raw, list):
        raise TimelineError("keys: expected a list")
    out: list[KeyRange] = []
    for i, r in enumerate(raw):
        ctx = f"keys[{i}]"
        if _is_comment(r):
            continue
        if isinstance(r, list):  # shorthand ["D", from, to]
            if len(r) != 3:
                raise TimelineError(f"{ctx}: expected [\"key\", from, to], got {r!r}")
            name, start, end, value = r[0], r[1], r[2], None
        else:
            d = _fields(r, ctx, {"key", "from", "to", "value"}, {"key", "from", "to"})
            name, start, end, value = d["key"], d["from"], d["to"], d.get("value")
        name = _str(name, f"{ctx}.key")
        key = buttons.canonical_key(name)
        if key is None:
            known = ", ".join(sorted(set(buttons.KEYS) | set(buttons.ALIASES))) + ", bit0..bit31"
            raise TimelineError(f"{ctx}.key: unknown key {name!r} (known: {known})")
        start = _int(start, f"{ctx}.from")
        end = _int(end, f"{ctx}.to")
        if not 0 <= start < length:
            raise TimelineError(f"{ctx}.from: {start} is outside the timeline (0 .. {length - 1})")
        if not start < end <= length:
            raise TimelineError(f"{ctx}.to: {end} must be > from ({start}) and <= length ({length}); "
                                f"'to' is exclusive")
        if value is not None:
            if key not in buttons.MOVEMENT_KEYS:
                raise TimelineError(f"{ctx}.value: only movement keys (W/A/S/D) take a value")
            value = _num(value, f"{ctx}.value")
            if value <= 0:
                raise TimelineError(f"{ctx}.value: must be > 0 (the direction comes from the key)")
        out.append(KeyRange(key, start, end, value, i))

    by_key: dict[str, list[KeyRange]] = {}
    for kr in out:
        by_key.setdefault(kr.key, []).append(kr)
    for key, ranges in by_key.items():
        ranges.sort(key=lambda kr: kr.start)
        for a, b in zip(ranges, ranges[1:]):
            if b.start < a.end:
                raise TimelineError(f"keys[{b.index}]: {key} [{b.start}, {b.end}) overlaps keys[{a.index}] "
                                    f"[{a.start}, {a.end}); a key can't be held twice")
    return out


def _parse_strafe(raw: Any, length: int) -> list[StrafeRange]:
    """Auto-strafe ranges: on those ticks the plugin aims the held keys from the
    real velocity (see docs/timeline-format.md)."""
    if not isinstance(raw, list):
        raise TimelineError("strafe: expected a list")
    out: list[StrafeRange] = []
    for i, r in enumerate(raw):
        ctx = f"strafe[{i}]"
        if _is_comment(r):
            continue
        heading = swing = gain = None
        if isinstance(r, list):  # shorthand [from, to] or [from, to, offset]
            if len(r) not in (2, 3):
                raise TimelineError(f"{ctx}: expected [from, to] or [from, to, offset], got {r!r}")
            start, end, offset = r[0], r[1], r[2] if len(r) == 3 else 0.0
        else:
            d = _fields(r, ctx, {"from", "to", "offset", "heading", "swing", "gain", "smooth"}, {"from", "to"})
            start, end, offset = d["from"], d["to"], d.get("offset", 0.0)
            if d.get("heading") is not None:
                heading = float(_num(d["heading"], f"{ctx}.heading"))
            if d.get("swing") is not None:
                swing = float(_num(d["swing"], f"{ctx}.swing"))
                if heading is None:
                    raise TimelineError(f"{ctx}.swing: only for a straight range (one with a heading)")
                if not 0.0 < swing <= 90.0:
                    raise TimelineError(f"{ctx}.swing: must be more than 0 and at most 90 degrees")
            if d.get("gain") is not None:
                gain = float(_num(d["gain"], f"{ctx}.gain"))
            elif d.get("smooth") is not None:  # as a first version had it: how close to perfect, 0 to 100
                short = 1.0 - float(_num(d["smooth"], f"{ctx}.smooth")) / 100.0  # how far from perfect
                gain = 100.0 * (1.0 - 0.25 * short * short)
            if gain is not None:
                if swing is None:
                    raise TimelineError(f"{ctx}.gain: only with a swing (how far it sweeps)")
                if not 75.0 <= gain <= 100.0:
                    raise TimelineError(f"{ctx}.gain: how much of the most speed gain to keep, 75 to 100 (percent)")
        start = _int(start, f"{ctx}.from")
        end = _int(end, f"{ctx}.to")
        offset = _num(offset, f"{ctx}.offset")
        if not 0 <= start < length:
            raise TimelineError(f"{ctx}.from: {start} is outside the timeline (0 .. {length - 1})")
        if not start < end <= length:
            raise TimelineError(f"{ctx}.to: {end} must be > from ({start}) and <= length ({length}); "
                                f"'to' is exclusive")
        if not -90.0 <= offset <= 90.0:
            raise TimelineError(f"{ctx}.offset: must be between -90 and 90 degrees")
        out.append(StrafeRange(start, end, float(offset), i, heading, swing, gain))
    out.sort(key=lambda r: r.start)
    for a, b in zip(out, out[1:]):
        if b.start < a.end:
            raise TimelineError(f"strafe[{b.index}]: [{b.start}, {b.end}) overlaps strafe[{a.index}] [{a.start}, {a.end})")
    return out


def _parse_turns(raw: Any, length: int) -> list[TurnBlock]:
    """The editor's turn blocks (see TurnBlock): sorted, each inside the timeline."""
    if not isinstance(raw, list):
        raise TimelineError("turns: expected a list")
    out: list[TurnBlock] = []
    for i, r in enumerate(raw):
        ctx = f"turns[{i}]"
        if _is_comment(r):
            continue
        d = _fields(r, ctx, {"from", "to", "rate", "shape"}, {"from", "to", "rate", "shape"})
        start, end = _int(d["from"], f"{ctx}.from"), _int(d["to"], f"{ctx}.to")
        rate = float(_num(d["rate"], f"{ctx}.rate"))
        if d["shape"] not in ("linear", "smooth"):
            raise TimelineError(f"{ctx}.shape: expected 'linear' or 'smooth', got {d['shape']!r}")
        if not 0 <= start < end <= length:
            raise TimelineError(f"{ctx}: [{start}, {end}) must be inside the timeline (0 .. {length}); 'to' is exclusive")
        out.append(TurnBlock(start, end, rate, d["shape"]))
    return sorted(out, key=lambda b: b.start)


def _parse_by_hand(raw: Any, length: int) -> list[tuple[int, int]]:
    """The editor's ticks whose A and D were set by hand: [from, to) spans, sorted, each inside the timeline."""
    if not isinstance(raw, list):
        raise TimelineError("ad_by_hand: expected a list")
    out: list[tuple[int, int]] = []
    for i, r in enumerate(raw):
        ctx = f"ad_by_hand[{i}]"
        if _is_comment(r):
            continue
        d = _fields(r, ctx, {"from", "to"}, {"from", "to"})
        start, end = _int(d["from"], f"{ctx}.from"), _int(d["to"], f"{ctx}.to")
        if not 0 <= start < end <= length:
            raise TimelineError(f"{ctx}: [{start}, {end}) must be inside the timeline (0 .. {length}); 'to' is exclusive")
        out.append((start, end))
    return sorted(out)


# --------------------------------------------------------------------------
# timeline


CAMERA_SMOOTHING = ("strafe", "all", "off")  # the camera replays show: smoothed on auto-strafe, everywhere, or not
_TOP_FIELDS = {"format", "version", "name", "map", "length", "settings", "start", "input",
               "yaw", "pitch", "keys", "strafe", "view_pitch", "autobhop", "camera_smoothing", "turns", "ad_by_hand"}


def parse_timeline(obj: Any, path: Path | None = None, start_search_dirs: list[Path] | None = None) -> Timeline:
    d = _fields(obj, "timeline", _TOP_FIELDS, {"format", "version", "map", "length", "start", "yaw"})
    if d["format"] != FORMAT_NAME:
        raise TimelineError(f"format: expected {FORMAT_NAME!r}, got {d['format']!r}")
    if d["version"] != FORMAT_VERSION:
        raise TimelineError(f"version: this baker reads version {FORMAT_VERSION}, got {d['version']!r}")

    name = d.get("name") or (path.stem if path else None)
    if not name or not isinstance(name, str) or not NAME_RE.match(name):
        raise TimelineError(f"name: expected letters, digits, '_', '-' or '.', got {name!r}")
    map_name = _str(d["map"], "map")
    if not NAME_RE.match(map_name):
        raise TimelineError(f"map: unexpected characters in {map_name!r}")
    length = _int(d["length"], "length")
    if length <= 0:
        raise TimelineError("length: must be at least 1 tick")

    settings: dict[str, Any] = {}
    if "settings" in d:
        sd = d["settings"]
        if not isinstance(sd, dict):
            raise TimelineError("settings: expected an object")
        for k, v in sd.items():
            if k.startswith("_"):
                continue
            if not SETTING_NAME_RE.match(k):
                raise TimelineError(f"settings.{k}: setting names are convar names (letters, digits, _)")
            if _is_num(v):
                settings[k] = v
            elif isinstance(v, str) and SETTING_VALUE_RE.match(v):
                settings[k] = v
            else:
                raise TimelineError(f"settings.{k}: expected a number or a string without spaces, got {v!r}")

    base_dir = path.parent if path else Path.cwd()
    if isinstance(d["start"], str):
        start = _load_start_file(d["start"], base_dir, start_search_dirs or [])
    else:
        start = parse_start(d["start"])

    autobhop = d.get("autobhop", False)
    if not isinstance(autobhop, bool):
        raise TimelineError(f"autobhop: expected true or false, got {autobhop!r}")
    camera_smoothing = d.get("camera_smoothing", "strafe")
    if camera_smoothing not in CAMERA_SMOOTHING:
        raise TimelineError(f"camera_smoothing: expected \"strafe\", \"all\" or \"off\", got {camera_smoothing!r}")

    move_speed = 400.0
    if "input" in d:
        idict = _fields(d["input"], "input", {"move_speed"})
        if "move_speed" in idict:
            move_speed = _num(idict["move_speed"], "input.move_speed")
            if move_speed <= 0:
                raise TimelineError("input.move_speed: must be > 0")

    tl = Timeline(
        name=name,
        map=map_name,
        length=length,
        settings=settings,
        start=start,
        move_speed=move_speed,
        yaw=_parse_track(d["yaw"], "yaw", length),
        pitch=_parse_track(d["pitch"], "pitch", length) if "pitch" in d else None,
        keys=_parse_key_ranges(d.get("keys", []), length),
        path=path,
        strafe=_parse_strafe(d.get("strafe", []), length),
        view_pitch=_parse_track(d["view_pitch"], "view_pitch", length) if "view_pitch" in d else None,
        autobhop=autobhop,
        camera_smoothing=camera_smoothing,
        turns=_parse_turns(d.get("turns", []), length),
        ad_by_hand=_parse_by_hand(d.get("ad_by_hand", []), length),
    )

    if start.map is not None and start.map != map_name:
        raise TimelineError(f"start: captured on map {start.map!r} but the timeline is for {map_name!r}")
    for a, b in zip(tl.yaw, tl.yaw[1:]):
        if abs(b.value - a.value) > 180.0:
            tl.warnings.append(
                f"yaw turns {b.value - a.value:+.1f} degrees between ticks {a.tick} and {b.tick}. "
                f"Yaw is unwrapped, so that is the way it turns; if you meant the short way, "
                f"write {a.value + ((b.value - a.value + 180.0) % 360.0 - 180.0):.1f}.")
    for k in tl.pitch or []:
        if abs(k.value) > 89.0:
            tl.warnings.append(f"pitch key at tick {k.tick} is {k.value}; a real client clamps pitch to +-89")
    for k in tl.view_pitch or []:
        if abs(k.value) > 89.0:
            tl.warnings.append(f"view_pitch key at tick {k.tick} is {k.value}; the game shows at most +-89")
    return tl


def load_timeline(path: Path, start_search_dirs: list[Path] | None = None) -> Timeline:
    try:
        obj = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as e:
        raise TimelineError(f"{path}: invalid JSON: {e}") from e
    return parse_timeline(obj, path=path, start_search_dirs=start_search_dirs)

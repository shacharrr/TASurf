"""Bake a timeline into the flat per-tick file the SourceMod plugin plays back.

The output is a pure function of the timeline (no timestamps, no absolute
paths), so baking the same timeline twice gives byte-identical files. The
format is documented in docs/file-formats.md; ``read_baked`` below follows the
same rules as the plugin's parser so tests can check the two stay in sync.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from pathlib import Path

from . import buttons
from .camera import view_yaw_lines
from .interp import sample, wrap_degrees
from .timeline import StartState, Timeline, TimelineError, bits_to_float, float_to_bits

BAKED_FORMAT = "tasurf-baked"
BAKED_VERSION = 1           # files without auto-strafe stay version 1, byte for byte
BAKED_VERSION_STRAFE = 2    # @strafe lines: the plugin aims those ticks itself
BAKED_VERSION_STRAIGHT = 3  # @strafe lines with a heading: the plugin also picks A or D, to keep going that way
BAKED_VERSION_AUTOBHOP = 4  # @autobhop: the plugin lets a held jump go in the air, so it jumps again on landing
BAKED_VERSION_SWING = 5     # @strafe lines with a heading and a swing: a side is kept until the direction is that far past
BAKED_VERSION_GAIN = 6      # and a gain: the swing turns smoothly, keeping that much of the most speed gain (percent)
COLUMNS = "tick,yaw,pitch,forwardmove,sidemove,buttons"
VIEW_PITCH_PER_LINE = 16    # values on one @view_pitch line


@dataclass(frozen=True)
class TickInput:
    tick: int
    yaw: float  # wrapped to [-180, 180)
    pitch: float
    forwardmove: float
    sidemove: float
    buttons: int


def f6(x: float) -> str:
    s = f"{x:.6f}"
    return "0.000000" if s == "-0.000000" else s


def f2(x: float) -> str:
    s = f"{x:.2f}"
    return "0.00" if s == "-0.00" else s


def f32r(x: float) -> str:
    """Decimal (6 places, more only when needed) that parses back to exactly
    float32(x), which is what the plugin will use."""
    x32 = bits_to_float(float_to_bits(x))  # float_to_bits also turns -0.0 into 0.0
    for places in range(6, 16):
        s = f"{x32:.{places}f}"
        if float_to_bits(float(s)) == float_to_bits(x32):
            return s
    return repr(x32)


def bake_inputs(tl: Timeline) -> list[TickInput]:
    yaw = sample(tl.yaw, tl.length)
    if tl.pitch is not None:
        pitch = sample(tl.pitch, tl.length)
    else:
        pitch = [tl.start.angles[0]] * tl.length

    fwd = [0.0] * tl.length
    side = [0.0] * tl.length
    bits = [0] * tl.length
    for kr in tl.keys:
        bit, fsign, ssign = buttons.key_info(kr.key)
        mag = kr.value if kr.value is not None else tl.move_speed
        for t in range(kr.start, kr.end):
            bits[t] |= bit
            fwd[t] += fsign * mag  # W and S together cancel, as on a real client
            side[t] += ssign * mag

    return [TickInput(t, wrap_degrees(yaw[t]), pitch[t], fwd[t], side[t], bits[t]) for t in range(tl.length)]


def _vec_line(name: str, v) -> str:
    return f"@start {name} " + " ".join(f6(x) for x in v)


def _start_lines(st: StartState) -> list[str]:
    lines = []
    for name in ("origin", "angles", "velocity"):
        values = getattr(st, name)
        given = getattr(st, f"{name}_bits")
        bits = given if given is not None else tuple(float_to_bits(x) for x in values)
        # Write the decimal of the exact float32 so the two lines always agree.
        lines.append(_vec_line(name, [bits_to_float(b) for b in bits]))
        lines.append(f"@start {name}_bits " + " ".join(bits))
    lines.append(_vec_line("base_velocity", st.base_velocity))
    lines.append(f"@start base_velocity_flag {int(st.base_velocity_flag)}")
    lines.append(f"@start on_ground {st.on_ground}")
    lines.append(f"@start ground_entity {st.ground_entity}")
    lines.append(f"@start ducked {int(st.ducked)}")
    lines.append(f"@start gravity {f6(st.gravity)}")
    lines.append(f"@start lagged_movement {f6(st.lagged_movement)}")
    for name in ("view_offset_z", "velocity_modifier", "stamina", "maxspeed"):
        v = getattr(st, name)
        if v is not None:
            lines.append(f"@start {name} {f6(v)}")
    if st.health is not None:
        lines.append(f"@start health {st.health}")
    return lines


def _setting_str(v) -> str:
    if isinstance(v, str):
        return v
    if float(v).is_integer():
        return str(int(v))
    return f"{v:.6f}".rstrip("0").rstrip(".")


def render_baked(tl: Timeline, inputs: list[TickInput], camera: dict[int, float] | None = None) -> str:
    """camera: the yaw replays show on auto-strafe, {tick: yaw} (camera.camera_yaws, from a run of these inputs), baked
    as @view_yaw lines; like @view_pitch they aren't hashed, so the file's hash is the same with them or without."""
    version = (BAKED_VERSION_GAIN if any(r.gain is not None for r in tl.strafe)
               else BAKED_VERSION_SWING if any(r.swing is not None for r in tl.strafe)
               else BAKED_VERSION_AUTOBHOP if tl.autobhop
               else BAKED_VERSION_STRAIGHT if any(r.heading is not None for r in tl.strafe)
               else BAKED_VERSION_STRAFE if tl.strafe else BAKED_VERSION)
    body = [
        f"@format {BAKED_FORMAT} {version}",
        f"@name {tl.name}",
        "@hash HASH",
        f"@map {tl.map}",
        f"@ticks {len(inputs)}",
    ]
    body += [f"@setting {k} {_setting_str(v)}" for k, v in tl.settings.items()]
    body += _start_lines(tl.start)
    if tl.autobhop:  # jump held in the air is let go, so landing with it held jumps
        body.append("@autobhop")
    # ticks [from, to): the plugin sets the yaw from the real velocity (the tick lines' yaw is the fallback)
    # with a heading: the plugin picks A or D each tick to keep going that way (straight)
    # with a swing as well: the side is kept until the direction has swung that far past the heading
    body += [f"@strafe {r.start} {r.end} {f6(r.offset)}" + ("" if r.heading is None else f" {f6(r.heading)}")
             + ("" if r.swing is None else f" {f6(r.swing)}") + ("" if r.gain is None else f" {f6(r.gain)}")
             for r in tl.strafe]
    body.append(COLUMNS)
    body += [f"{ti.tick},{f32r(ti.yaw)},{f32r(ti.pitch)},{f32r(ti.forwardmove)},{f32r(ti.sidemove)},{ti.buttons}"
             for ti in inputs]
    body += _view_pitch_lines(tl, len(inputs))
    body += view_yaw_lines(camera or {})
    digest = content_hash(body)
    body[2] = f"@hash {digest}"
    header = ["# TASurf baked timeline, generated by `python -m tasurf bake`.",
              "# Edit the timeline JSON and re-bake; do not edit this file by hand."]
    return "\n".join(header + body) + "\n"


def _view_pitch_lines(tl: Timeline, ticks: int) -> list[str]:
    """`@view_pitch <first tick> <pitch>...`: the pitch replays show on each tick, which the
    plugin sets on the replay bot's eyes after the tick's movement. Not part of the
    hash, so changing it leaves the run, and which runs compare with it, as they were."""
    if tl.view_pitch is None:
        return []
    values = sample(tl.view_pitch, ticks)
    return [f"@view_pitch {t} " + " ".join(f2(v) for v in values[t:t + VIEW_PITCH_PER_LINE])
            for t in range(0, ticks, VIEW_PITCH_PER_LINE)]


def _hashed(line: str) -> bool:
    return not (line.startswith("#") or line.startswith("@hash ") or line.startswith("@view_pitch ")
                or line.startswith("@view_yaw "))


def content_hash(lines: list[str]) -> str:
    """sha256 over every non-comment line except the @hash, @view_pitch and @view_yaw lines, first 16 hex digits."""
    h = hashlib.sha256()
    for line in lines:
        if not _hashed(line):
            continue
        h.update(line.encode("utf-8"))
        h.update(b"\n")
    return h.hexdigest()[:16]


FNV_OFFSET = 0x811C9DC5
FNV_PRIME = 0x01000193


def fnv1a32(data: bytes, h: int = FNV_OFFSET) -> int:
    for b in data:
        h = ((h ^ b) * FNV_PRIME) & 0xFFFFFFFF
    return h


def fnv1a32_hash(lines: list[str]) -> str:
    """The hash the plugin writes into recordings (SourcePawn has no SHA-256):
    FNV-1a 32 over the same lines as content_hash, as "fnv1a32:xxxxxxxx"."""
    h = FNV_OFFSET
    for line in lines:
        if not _hashed(line):
            continue
        h = fnv1a32(line.encode("utf-8") + b"\n", h)
    return f"fnv1a32:{h:08x}"


def summarize(tl: Timeline, inputs: list[TickInput]) -> list[str]:
    out = [f"{tl.name}: map {tl.map}, {tl.length} ticks, start from {tl.start.source}"]
    if tl.settings:
        out.append("  expects " + ", ".join(f"{k}={_setting_str(v)}" for k, v in tl.settings.items()))
    for kr in sorted(tl.keys, key=lambda k: (k.start, k.key)):
        extra = f" value {kr.value:g}" if kr.value is not None else ""
        n = kr.end - kr.start
        out.append(f"  {kr.key:<9} ticks {kr.start}..{kr.end - 1} ({n} tick{'s' if n != 1 else ''}){extra}")
    out.append(f"  yaw keys: " + ", ".join(f"{k.tick}:{k.value:g}({k.interp})" for k in tl.yaw))
    return out


def bake_to_file(tl: Timeline, out_path: Path, camera_for=None) -> tuple[str, list[TickInput]]:
    """camera_for(hash): the camera replays show for the inputs with that hash, or None (see render_baked)."""
    inputs = bake_inputs(tl)
    text = render_baked(tl, inputs)
    camera = camera_for(next(l for l in text.splitlines() if l.startswith("@hash ")).split()[1]) if camera_for else None
    if camera:
        text = render_baked(tl, inputs, camera)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(text, encoding="utf-8", newline="\n")
    return text, inputs


# --------------------------------------------------------------------------
# reading baked files (same rules as plugin/scripting/tasurf/baked.sp)


@dataclass
class BakedFile:
    name: str
    hash: str
    map: str
    settings: dict[str, str]
    start: dict[str, list[str]]
    inputs: list[TickInput]
    strafe: list[tuple] = None  # (from, to, offset), + heading for a straight range, + swing, + gain
    view_pitch: list[float] | None = None  # one per tick, or None (replays show the run's pitch)
    autobhop: bool = False
    view_yaw: dict[int, float] | None = None  # the camera replays show on these ticks (the rest: the yaw played)


def read_baked(text: str) -> BakedFile:
    header: dict[str, str] = {}
    settings: dict[str, str] = {}
    start: dict[str, list[str]] = {}
    inputs: list[TickInput] = []
    strafe: list[tuple] = []
    view_pitch: list[float] = []
    view_yaw: dict[int, float] = {}
    autobhop = False
    lines = text.splitlines()
    for n, raw in enumerate(lines, 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("@"):
            parts = line.split(" ")
            tag = parts[0][1:]
            if tag == "setting":
                settings[parts[1]] = parts[2]
            elif tag == "start":
                start[parts[1]] = parts[2:]
            elif tag == "autobhop" and len(parts) == 1:
                autobhop = True
            elif tag == "strafe" and len(parts) in (4, 5, 6, 7):
                strafe.append((int(parts[1]), int(parts[2])) + tuple(float(p) for p in parts[3:]))
            elif tag == "view_pitch" and len(parts) >= 3:
                if int(parts[1]) != len(view_pitch):
                    raise TimelineError(f"line {n}: expected @view_pitch {len(view_pitch)}, got {parts[1]}")
                view_pitch += [float(p) for p in parts[2:]]
            elif tag == "view_yaw" and len(parts) >= 3:
                first = int(parts[1])
                view_yaw.update((first + i, float(p)) for i, p in enumerate(parts[2:]))
            elif tag in ("format", "name", "hash", "map", "ticks"):
                header[tag] = " ".join(parts[1:])
            else:
                raise TimelineError(f"line {n}: unknown header {parts[0]}")
            continue
        if line.startswith("tick,"):
            continue
        cols = line.split(",")
        if len(cols) not in (6, 10):
            raise TimelineError(f"line {n}: expected 6 or 10 columns, got {len(cols)}")
        tick = int(cols[0])
        if tick != len(inputs):
            raise TimelineError(f"line {n}: expected tick {len(inputs)}, got {tick}")
        if len(cols) == 10:  # recordings: exact float32 bits of yaw, pitch, forwardmove, sidemove
            yaw, pitch, fwd, side = (bits_to_float(c) for c in cols[6:10])
        else:
            yaw, pitch, fwd, side = (float(c) for c in cols[1:5])
        inputs.append(TickInput(tick, yaw, pitch, fwd, side, int(cols[5])))
    versions = (BAKED_VERSION, BAKED_VERSION_STRAFE, BAKED_VERSION_STRAIGHT, BAKED_VERSION_AUTOBHOP, BAKED_VERSION_SWING,
                BAKED_VERSION_GAIN)
    if header.get("format") not in [f"{BAKED_FORMAT} {v}" for v in versions]:
        raise TimelineError(f"not a {BAKED_FORMAT} {BAKED_VERSION}, {BAKED_VERSION_STRAFE}, {BAKED_VERSION_STRAIGHT}, "
                            f"{BAKED_VERSION_AUTOBHOP}, {BAKED_VERSION_SWING} or {BAKED_VERSION_GAIN} file")
    version = int(header["format"].split(" ")[1])
    if strafe and version < BAKED_VERSION_STRAFE:
        raise TimelineError(f"@strafe needs {BAKED_FORMAT} {BAKED_VERSION_STRAFE}")
    if any(len(r) >= 4 for r in strafe) and version < BAKED_VERSION_STRAIGHT:
        raise TimelineError(f"@strafe with a heading needs {BAKED_FORMAT} {BAKED_VERSION_STRAIGHT}")
    if any(len(r) >= 5 for r in strafe) and version < BAKED_VERSION_SWING:
        raise TimelineError(f"@strafe with a swing needs {BAKED_FORMAT} {BAKED_VERSION_SWING}")
    if any(len(r) == 6 for r in strafe) and version < BAKED_VERSION_GAIN:
        raise TimelineError(f"@strafe with a swing's gain needs {BAKED_FORMAT} {BAKED_VERSION_GAIN}")
    if autobhop and version < BAKED_VERSION_AUTOBHOP:
        raise TimelineError(f"@autobhop needs {BAKED_FORMAT} {BAKED_VERSION_AUTOBHOP}")
    if int(header["ticks"]) != len(inputs):
        raise TimelineError(f"@ticks says {header['ticks']} but the file has {len(inputs)} tick lines")
    if view_pitch and len(view_pitch) != len(inputs):
        raise TimelineError(f"@view_pitch has {len(view_pitch)} values for {len(inputs)} ticks")
    if any(not 0 <= t < len(inputs) for t in view_yaw):
        raise TimelineError(f"@view_yaw names a tick past the last ({len(inputs) - 1})")
    expected = fnv1a32_hash(lines) if header["hash"].startswith("fnv1a32:") else content_hash(lines)
    if expected != header["hash"]:
        raise TimelineError("@hash does not match the content (was the file edited by hand?)")
    return BakedFile(header["name"], header["hash"], header["map"], settings, start, inputs, strafe,
                     view_pitch or None, autobhop, view_yaw or None)

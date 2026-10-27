"""Check the Rust back end against the Python one (the reference).

    python rust/parity.py [--exe rust/target/release/tasurf.exe] [--keep]

Everything runs in a throwaway sandbox: a copy of tasurf/, timelines/ and the
plugin sources, plus a copy of the game's data/tasurf folder (baked, runs,
starts) as a fake cstrike folder, so nothing touches the real game or repo.
Each check runs the same thing through both back ends, one after the other in
the same sandbox (reset in between when something writes), and compares:

  bake      stdout, stderr, exit code and the baked bytes: every timeline, and
            ~50 broken variants of one for the error messages
  check     every run CSV
  verify    every timeline's latest runs, --count, --rows-only, error cases
  import    every recording, exact and with --tolerance, error cases
  editor    the same ~300 API calls against both servers: the JSON answers and
            every file they write (saved timelines, baked files, requests)

Answers are compared as JSON text (key order, ints vs floats), ignoring only
request ids, file times and the heartbeat age.
"""

from __future__ import annotations

import argparse
import copy
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_EXE = REPO / "rust" / "target" / "release" / ("tasurf.exe" if os.name == "nt" else "tasurf")
IGNORED_KEYS = {"id", "mtime", "age"}


def game_data_dir() -> Path:
    cstrike = os.environ.get("TASURF_CSTRIKE")
    if not cstrike:
        cfg = json.loads((REPO / "tasurf.local.json").read_text(encoding="utf-8"))
        cstrike = cfg["cstrike_dir"]
    return Path(cstrike) / "addons" / "sourcemod" / "data" / "tasurf"


class Sandbox:
    def __init__(self, root: Path, data: Path):
        self.root, self.data = root, data
        self.gd = root / "cstrike" / "addons" / "sourcemod" / "data" / "tasurf"
        self.reset()

    def reset(self):
        if self.root.exists():
            shutil.rmtree(self.root)
        ignore = shutil.ignore_patterns("__pycache__")
        shutil.copytree(REPO / "tasurf", self.root / "tasurf", ignore=ignore)
        shutil.copytree(REPO / "timelines", self.root / "timelines")
        shutil.copytree(REPO / "plugin" / "scripting", self.root / "plugin" / "scripting")
        shutil.copytree(REPO / "plugin" / "cfg", self.root / "plugin" / "cfg")
        for sub in ("baked", "runs", "starts"):
            shutil.copytree(self.data / sub, self.gd / sub)
        (self.gd / "requests").mkdir(parents=True)
        (self.gd / "requests" / "heartbeat.txt").write_text(
            "version 0.8.0\nmap surf_devs_ksf\nmode edit\nbot ready\nready yes\nplayer alive\nstate idle\ntime 1\n",
            encoding="utf-8", newline="\n")
        self.add_fixtures()

    def add_fixtures(self):
        """Two things the real data may lack, so freeze and splice get past their
        checks: an auto-strafe timeline with a preview CSV of exactly its inputs,
        and a recording that branches off example_surf at tick 40."""
        sys.path.insert(0, str(REPO))
        from tasurf.bake import bake_inputs, f6, fnv1a32_hash, render_baked
        from tasurf.buttons import IN_JUMP, IN_MOVELEFT, IN_MOVERIGHT
        from tasurf.interp import wrap_degrees
        from tasurf.timeline import bits_to_float, float_to_bits, parse_timeline

        starts = [self.gd / "starts"]
        src = self.root / "timelines" / "surf_devs_ksf_rec_2_edit.json"
        runs = sorted(self.gd.glob("runs/surf_devs_ksf_rec_2_edit_*.csv"))
        if src.is_file() and runs:
            doc = json.loads(src.read_text(encoding="utf-8"))
            doc["name"] = "parity_auto"
            L = int(doc["length"])  # the user's own timeline: whatever length it has now
            doc["strafe"] = [{"from": L * 42 // 100, "to": L * 55 // 100},
                             {"from": L * 63 // 100, "to": L * 71 // 100, "offset": 1.5, "heading": 33.5, "swing": 4.5, "gain": 88.5}]  # straight
            doc["autobhop"] = True
            path = self.root / "timelines" / "parity_auto.json"
            path.write_text(json.dumps(doc, indent=2), encoding="utf-8")
            tl = parse_timeline(doc, path=path, start_search_dirs=starts)
            digest = next(l for l in render_baked(tl, bake_inputs(tl)).splitlines() if l.startswith("@hash ")).split()[1]
            run = runs[-1].read_text(encoding="utf-8")
            run = re.sub(r"(?m)^# baked_hash .*$", f"# baked_hash {digest}", run)
            (self.gd / "runs" / "parity_auto.preview.csv").write_text(run, encoding="utf-8", newline="\n")

        src = self.root / "timelines" / "example_surf.json"
        # a view pitch (replays only) on the same inputs: ties and values just under a rounding step for its
        # 2-decimal lines, -0.004 (written 0.00), every interp, and one past 89 (a warning)
        doc = json.loads(src.read_text(encoding="utf-8"))
        doc["name"] = "parity_view"
        doc["view_pitch"] = {"keys": [[0, 0.125, "hold"], [5, 0.375, "hold"], [10, 2.675, "hold"], [15, -0.004, "hold"],
                                      [20, 1.005], [60, 30.0, "smooth"], [120, -12.5, "cubic"], [200, 95.0, "cubic"],
                                      [260, -20.0], [329, 7.77]]}
        (self.root / "timelines" / "parity_view.json").write_text(json.dumps(doc, indent=2), encoding="utf-8")
        # the joint curve (straight lines, rounded at the keys): automatic, integer, fractional and zero widths, a
        # cubic between joints, a slope a smooth curve left (no width: ignored) and a line said with its width
        doc = json.loads(src.read_text(encoding="utf-8"))
        doc["name"] = "parity_joint"
        doc["yaw"] = {"keys": [[0, -90.0, "joint"], [40, -60.0, "joint", None, 6], [70, -110.5, "joint", None, 12.5],
                               [110, -20.0, "cubic"], [160, -45.0, "joint", None, 0], [200, 30.0, "joint", 4.5], [260, 10.0, "joint", -0.75, 8],
                               [329, 55.0, "linear"]]}
        (self.root / "timelines" / "parity_joint.json").write_text(json.dumps(doc, indent=2), encoding="utf-8")
        tl = parse_timeline(json.loads(src.read_text(encoding="utf-8")), path=src, start_search_dirs=starts)
        inputs = bake_inputs(tl)
        head = [l for l in render_baked(tl, inputs).splitlines() if l.startswith(("@setting", "@start"))]
        rows = []
        n = tl.length + 25
        for t in range(n):
            if t < 40:
                ti = inputs[t]
                yaw, pitch, fwd, side, btn = ti.yaw, ti.pitch, ti.forwardmove, ti.sidemove, ti.buttons
            else:
                yaw = wrap_degrees(180.0 - 0.37 * (t - 40))
                pitch, fwd = 1.25, 0.0
                side = 400.0 if (t // 30) % 2 else -250.0
                btn = (IN_MOVERIGHT if side > 0 else IN_MOVELEFT) | (IN_JUMP if t == 70 else 0)
            bits = [float_to_bits(v) for v in (yaw, pitch, fwd, side)]
            dec = [f6(bits_to_float(b)) for b in bits]
            rows.append(f"{t},{','.join(dec)},{btn},{','.join(bits)}")
        body = ["@format tasurf-baked 1", "@name parity_rec", "@hash HASH", f"@map {tl.map}", f"@ticks {n}", *head,
                "tick,yaw,pitch,forwardmove,sidemove,buttons,yaw_bits,pitch_bits,forwardmove_bits,sidemove_bits", *rows]
        body[2] = "@hash " + fnv1a32_hash(body)
        (self.gd / "baked" / "parity_rec.tas").write_text(
            "# TASurf recording, synthetic (rust/parity.py)\n" + "\n".join(body) + "\n", encoding="utf-8", newline="\n")
        for name, fmt, line in (("parity_ab4", "4", "@autobhop"), ("parity_ab3", "3", "@autobhop"), ("parity_abarg", "4", "@autobhop 1")):
            ab = [f"@format tasurf-baked {fmt}", f"@name {name}"] + body[2:5] + [line] + body[5:]
            ab[2] = "@hash " + fnv1a32_hash(ab)
            (self.gd / "baked" / f"{name}.tas").write_text(
                "# TASurf recording, synthetic (rust/parity.py)\n" + "\n".join(ab) + "\n", encoding="utf-8", newline="\n")

        # start positions the main menu has to cope with: one without a map, one broken
        starts = sorted(self.gd.glob("starts/*.json"))
        if starts:
            obj = json.loads(starts[0].read_text(encoding="utf-8"))
            obj.pop("map", None)
            (self.gd / "starts" / "parity_nomap.json").write_text(json.dumps(obj), encoding="utf-8")
        (self.gd / "starts" / "parity_broken.json").write_text('{"format": "tasurf-start", "origin": [1, 2', encoding="utf-8")

    def env(self) -> dict:
        env = dict(os.environ)
        env["TASURF_CSTRIKE"] = str(self.root / "cstrike")
        env["TASURF_ROOT"] = str(self.root)
        env.pop("TASURF_SPCOMP", None)
        env.pop("PYTHONPATH", None)
        return env

    def files(self) -> dict[str, bytes]:
        """Everything under the sandbox that a back end may write."""
        out = {}
        for top in (self.root / "timelines", self.root / "out", self.gd):
            if not top.exists():
                continue
            for p in sorted(top.rglob("*")):
                if p.is_file() and "__pycache__" not in p.parts:
                    out[str(p.relative_to(self.root))] = p.read_bytes()
        return out


class Report:
    def __init__(self):
        self.counts: dict[str, list[int]] = {}
        self.failures: list[str] = []

    def check(self, section: str, label: str, same: bool, detail: str = ""):
        c = self.counts.setdefault(section, [0, 0])
        c[0] += 1
        if same:
            c[1] += 1
        else:
            self.failures.append(f"[{section}] {label}\n{detail}".rstrip())

    def print(self) -> bool:
        for f in self.failures[:40]:
            print("MISMATCH " + f + "\n")
        if len(self.failures) > 40:
            print(f"... {len(self.failures) - 40} more mismatches\n")
        for section, (n, ok) in self.counts.items():
            print(f"{section:<8} {ok}/{n} identical")
        return not self.failures


def first_diff(a: str, b: str, context: int = 160) -> str:
    i = next((k for k, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
    lo = max(0, i - 40)
    return f"  python: {a[lo:i + context]!r}\n  rust:   {b[lo:i + context]!r}"


# --------------------------------------------------------------------------
# command line


def run_cli(sb: Sandbox, backend: str, exe: Path, args: list[str]) -> tuple[int, str, str]:
    cmd = [sys.executable, "-m", "tasurf", *args] if backend == "python" else [str(exe), *args]
    p = subprocess.run(cmd, cwd=sb.root, env=sb.env(), capture_output=True, text=True, encoding="utf-8")
    return p.returncode, p.stdout, p.stderr


def normalise_cli(text: str) -> str:
    text = text.replace("python -m tasurf", "tasurf")
    return re.sub(r"Traceback \(most recent call last\):.*", "<traceback>", text, flags=re.S)


def compare_cli(rep: Report, section: str, sb: Sandbox, exe: Path, args: list[str], writes: bool = False,
                label: str | None = None, clean: Path | None = None):
    results = []
    for backend in ("python", "rust"):
        if writes:
            sb.reset()
        if clean is not None and clean.exists():
            shutil.rmtree(clean)
        before = sb.files()
        code, out, err = run_cli(sb, backend, exe, args)
        after = sb.files()
        changed = {k: v for k, v in after.items() if before.get(k) != v}
        results.append((code, normalise_cli(out), normalise_cli(err), changed))
    (c1, o1, e1, f1), (c2, o2, e2, f2) = results
    problems = []
    if c1 != c2:
        problems.append(f"  exit code: python {c1}, rust {c2}")
    if o1 != o2:
        problems.append("  stdout differs:\n" + first_diff(o1, o2))
    if e1 != e2:
        problems.append("  stderr differs:\n" + first_diff(e1, e2))
    if f1.keys() != f2.keys():
        problems.append(f"  files written: python {sorted(f1)}, rust {sorted(f2)}")
    for k in sorted(f1.keys() & f2.keys()):
        if f1[k] != f2[k]:
            problems.append(f"  {k} differs:\n" + first_diff(f1[k].decode("utf-8", "replace"), f2[k].decode("utf-8", "replace")))
    rep.check(section, label or " ".join(args), not problems, "\n".join(problems))


def broken_timelines(base: dict) -> dict[str, dict]:
    """Variants of a valid timeline, each wrong (or warned about) in one way."""
    out = {}

    def variant(name, fn):
        d = copy.deepcopy(base)
        fn(d)
        out[name] = d

    variant("no_format", lambda d: d.pop("format"))
    variant("bad_format", lambda d: d.update(format="tasurf"))
    variant("version2", lambda d: d.update(version=2))
    variant("version_true", lambda d: d.update(version=True))
    variant("bad_name", lambda d: d.update(name="has space"))
    variant("empty_name", lambda d: d.update(name=""))
    variant("no_map", lambda d: d.pop("map"))
    variant("map_number", lambda d: d.update(map=5))
    variant("bad_map", lambda d: d.update(map="a/b"))
    variant("length0", lambda d: d.update(length=0))
    variant("length_str", lambda d: d.update(length="330"))
    variant("length_frac", lambda d: d.update(length=12.5))
    variant("length_float", lambda d: d.update(length=330.0))
    variant("unknown_top", lambda d: d.update(colour="red", zz=1))
    variant("settings_list", lambda d: d.update(settings=[1, 2]))
    variant("settings_badname", lambda d: d["settings"].update({"sv-x": 1}))
    variant("settings_space", lambda d: d["settings"].update(sv_x="a b"))
    variant("settings_list_value", lambda d: d["settings"].update(sv_x=[1]))
    variant("settings_values", lambda d: d["settings"].update(sv_a=0.1, sv_b=1e20, sv_c=-0.0, sv_d="text", sv_e=2.25))
    variant("start_missing_file", lambda d: d.update(start="no_such_start"))
    variant("start_list", lambda d: d.update(start=[1, 2, 3]))
    variant("start_no_origin", lambda d: d["start"].pop("origin"))
    variant("start_origin_2", lambda d: d["start"].update(origin=[1.0, 2.0]))
    variant("start_origin_str", lambda d: d["start"].update(origin=[1.0, "2", 3.0]))
    variant("start_bits_mismatch", lambda d: d["start"].update(origin_bits=["3F800000", "0", "0"]))
    variant("start_bits_bad", lambda d: d["start"].update(origin_bits=["XYZ", "0", "0"]))
    variant("start_bits_ok", lambda d: d["start"].update(origin=[1.0, 0.0, 0.0], origin_bits=["3f800000", "0", "0"]))
    variant("start_on_ground", lambda d: d["start"].update(on_ground="maybe"))
    variant("start_on_ground_true", lambda d: d["start"].update(on_ground=True))
    variant("start_ground_entity", lambda d: d["start"].update(ground_entity=-1))
    variant("start_ducked", lambda d: d["start"].update(ducked=True))
    variant("start_ducked_str", lambda d: d["start"].update(ducked="no"))
    variant("start_roll", lambda d: d["start"].update(angles=[0.0, 90.0, 5.0]))
    variant("start_extra", lambda d: d["start"].update(health=100, maxspeed=250.0, stamina=1.5, view_offset_z=64.0))
    variant("start_unknown", lambda d: d["start"].update(speed=3))
    variant("start_format", lambda d: d["start"].update(format="nope"))
    variant("start_map_mismatch", lambda d: d["start"].update(map="surf_other"))
    variant("input_zero", lambda d: d.update(input={"move_speed": 0}))
    variant("input_unknown", lambda d: d.update(input={"speed": 1}))
    variant("input_float", lambda d: d.update(input={"move_speed": 450.5}))
    variant("yaw_empty", lambda d: d["yaw"].update(keys=[]))
    variant("yaw_only_comment", lambda d: d["yaw"].update(keys=[{"_c": 1}]))
    variant("yaw_default_bad", lambda d: d["yaw"].update(default_interp="spline"))
    variant("yaw_tick_out", lambda d: d["yaw"]["keys"].append([9999, 1.0]))
    variant("yaw_not_increasing", lambda d: d["yaw"]["keys"].insert(2, [10, 1.0]))
    variant("yaw_bad_interp", lambda d: d["yaw"]["keys"].__setitem__(1, [66, 180.0, "bezier"]))
    variant("yaw_short", lambda d: d["yaw"]["keys"].__setitem__(1, [66]))
    variant("yaw_object_keys", lambda d: d["yaw"].update(keys=[{"tick": 0, "value": 10.0}, {"tick": 50, "value": 20, "interp": "smooth"}, [80, 30.0]]))
    variant("yaw_object_missing", lambda d: d["yaw"].update(keys=[{"tick": 0}]))
    variant("yaw_jump_warning", lambda d: d["yaw"].update(keys=[[0, 0.0], [10, 350.0], [20, -15.5]]))
    variant("pitch_warning", lambda d: d.update(pitch={"keys": [[0, 95.0], [10, -100.25]]}))
    variant("pitch_cubic", lambda d: d.update(pitch={"default_interp": "cubic", "keys": [[0, 5.0], [100, -10.0], [200, 20.0], [300, 0.0]]}))
    variant("yaw_slopes", lambda d: d["yaw"].update(keys=[[0, 180.0, "cubic", 0.0], {"tick": 66, "value": 170.0, "interp": "cubic", "slope": -0.25},
                                                         [130, 140.0, "cubic"], [200, 190.0, "linear", 1.5], [329, 200.0]]))
    variant("yaw_slope_str", lambda d: d["yaw"].update(keys=[[0, 180.0, "cubic", "1"]]))
    variant("yaw_slope_null", lambda d: d["yaw"].update(keys=[{"tick": 0, "value": 180.0, "slope": None}]))
    variant("yaw_key_5", lambda d: d["yaw"].update(keys=[[0, 180.0, "cubic", 1.0, 2.0]]))
    variant("view_pitch_ok", lambda d: d.update(view_pitch={"default_interp": "smooth", "keys": [[0, -3.333], [150, 12.5], [329, 0.005]]}))
    variant("view_pitch_warning", lambda d: d.update(view_pitch={"keys": [[0, -91.5], [10, 89.5]]}))
    variant("view_pitch_empty", lambda d: d.update(view_pitch={"keys": []}))
    variant("view_pitch_out", lambda d: d.update(view_pitch={"keys": [[330, 1.0]]}))
    variant("view_pitch_list", lambda d: d.update(view_pitch=[[0, 1.0]]))
    variant("view_pitch_field", lambda d: d.update(view_pitch={"keys": [[0, 1.0]], "smooth": 2}))
    variant("keys_not_list", lambda d: d.update(keys={"W": 1}))
    variant("keys_bad_name", lambda d: d["keys"].append({"key": "left", "from": 0, "to": 5}))
    variant("keys_bitN", lambda d: d["keys"].extend([["bit6", 0, 5], ["BIT3", 10, 20], ["bit31", 0, 2]]))
    variant("keys_overlap", lambda d: d["keys"].append(["w", 10, 20]))
    variant("keys_value_jump", lambda d: d["keys"].append({"key": "jump", "from": 100, "to": 101, "value": 5}))
    variant("keys_value_neg", lambda d: d["keys"].append({"key": "A", "from": 300, "to": 310, "value": -5}))
    variant("keys_value", lambda d: d["keys"].append({"key": "S", "from": 300, "to": 310, "value": 123.456}))
    variant("keys_from_out", lambda d: d["keys"].append(["duck", 400, 401]))
    variant("keys_to_bad", lambda d: d["keys"].append(["duck", 10, 10]))
    variant("keys_short", lambda d: d["keys"].append(["duck", 10]))
    variant("keys_unknown_field", lambda d: d["keys"].append({"key": "duck", "from": 1, "to": 2, "hold": True}))
    variant("strafe_ok", lambda d: d.update(strafe=[[70, 120], {"from": 140, "to": 200, "offset": -3.5}, {"_c": 0}]))
    variant("strafe_heading", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": -91.25}, {"from": 140, "to": 200, "offset": 2, "heading": 400}]))
    variant("strafe_heading_bad", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": "north"}]))
    variant("strafe_heading_null", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": None}]))
    variant("autobhop", lambda d: d.update(autobhop=True))
    variant("autobhop_off", lambda d: d.update(autobhop=False))
    variant("autobhop_straight", lambda d: d.update(autobhop=True, strafe=[{"from": 70, "to": 120, "heading": 12.5}]))
    variant("autobhop_str", lambda d: d.update(autobhop="yes"))
    variant("autobhop_int", lambda d: d.update(autobhop=1))
    variant("autobhop_null", lambda d: d.update(autobhop=None))
    variant("camera_off", lambda d: d.update(camera_smoothing="off"))
    variant("camera_all", lambda d: d.update(camera_smoothing="all"))
    variant("camera_strafe", lambda d: d.update(camera_smoothing="strafe"))
    variant("camera_bad", lambda d: d.update(camera_smoothing="smooth"))
    variant("camera_bool", lambda d: d.update(camera_smoothing=False))
    variant("swing", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": -40.5, "swing": 3.25}]))
    variant("swing_no_heading", lambda d: d.update(strafe=[{"from": 70, "to": 120, "swing": 3}]))
    variant("swing_zero", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 1, "swing": 0}]))
    variant("swing_big", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 1, "swing": 90.5}]))
    variant("swing_str", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 1, "swing": "wide"}]))
    variant("gain", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": -40.5, "swing": 3.25, "gain": 92.5}]))
    variant("gain_least", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 2, "swing": 8, "gain": 75}]))
    variant("gain_no_swing", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 2, "gain": 90}]))
    variant("gain_low", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 2, "swing": 8, "gain": 74.9}]))
    variant("gain_old_smooth", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 2, "swing": 8, "smooth": 37.5}]))
    variant("gain_and_smooth", lambda d: d.update(strafe=[{"from": 70, "to": 120, "heading": 2, "swing": 8, "gain": 80, "smooth": 0}]))
    variant("turns", lambda d: d.update(turns=[{"from": 90, "to": 130, "rate": -1.5, "shape": "smooth"}, {"from": 20, "to": 60, "rate": 2, "shape": "linear"}]))
    variant("turns_bad_shape", lambda d: d.update(turns=[{"from": 20, "to": 60, "rate": 2, "shape": "auto"}]))
    variant("turns_outside", lambda d: d.update(turns=[{"from": 20, "to": 100000, "rate": 2, "shape": "linear"}]))
    variant("turns_missing", lambda d: d.update(turns=[{"from": 20, "to": 60, "rate": 2}]))
    variant("turns_not_list", lambda d: d.update(turns={"from": 20}))
    variant("strafe_overlap", lambda d: d.update(strafe=[[70, 120], [100, 130]]))
    variant("strafe_offset", lambda d: d.update(strafe=[[70, 120, 100]]))
    variant("strafe_shape", lambda d: d.update(strafe=[[70]]))
    variant("strafe_not_list", lambda d: d.update(strafe=5))
    variant("strafe_to", lambda d: d.update(strafe=[[70, 9999]]))
    variant("top_list", lambda d: (d.clear(), d.update(x=[])))
    return out


def cli_checks(rep: Report, sb: Sandbox, exe: Path):
    timelines = sorted((sb.root / "timelines").glob("*.json"))
    for tl in timelines:
        rel = str(tl.relative_to(sb.root))
        compare_cli(rep, "bake", sb, exe, ["bake", "--no-install", "-o", "out/parity", rel], writes=True)
    compare_cli(rep, "bake", sb, exe, ["bake", *[str(t.relative_to(sb.root)) for t in timelines]], writes=True,
                label="bake all, installing into the (sandbox) game")
    compare_cli(rep, "bake", sb, exe, ["bake", "--no-install", "timelines/nope.json", "timelines/mine.json"], writes=True)

    base = json.loads((REPO / "timelines" / "example_surf.json").read_text(encoding="utf-8"))
    mut = sb.root / "mut"
    for name, doc in broken_timelines(base).items():
        mut.mkdir(exist_ok=True)
        (mut / f"{name}.json").write_text(json.dumps(doc, indent=1), encoding="utf-8")
    (mut / "invalid_json.json").write_text('{"format": ', encoding="utf-8")
    for p in sorted(mut.glob("*.json")):
        rel = str(p.relative_to(sb.root))
        compare_cli(rep, "bake", sb, exe, ["bake", "--no-install", "-o", "out/mut", rel], label=f"bake {p.stem}",
                    clean=sb.root / "out" / "mut")

    runs = sorted((sb.gd / "runs").glob("*.csv"))
    for r in runs:
        compare_cli(rep, "check", sb, exe, ["check", str(r.relative_to(sb.root))])
    names = sorted({re.sub(r"_\d+\.csv$", "", r.name) for r in runs if re.search(r"_\d+\.csv$", r.name)})
    for n in names:
        compare_cli(rep, "verify", sb, exe, ["verify", "--latest", n])
        compare_cli(rep, "verify", sb, exe, ["verify", "--latest", n, "--count", "3", "--rows-only"])
    compare_cli(rep, "verify", sb, exe, ["verify", "--latest", "nope"])
    compare_cli(rep, "verify", sb, exe, ["verify", str(runs[0].relative_to(sb.root))])
    compare_cli(rep, "verify", sb, exe, ["verify", *[str(r.relative_to(sb.root)) for r in runs[:4]]])
    compare_cli(rep, "verify", sb, exe, ["verify", "--rows-only", *[str(r.relative_to(sb.root)) for r in runs[-3:]]])

    recordings = sorted(p.stem for p in (sb.gd / "baked").glob("*.tas")
                        if "@hash fnv1a32:" in p.read_text(encoding="utf-8", errors="replace")[:600])
    for rec in recordings:
        compare_cli(rep, "import", sb, exe, ["import-recording", rec, "--name", "parity_imp"], writes=True)
        compare_cli(rep, "import", sb, exe, ["import-recording", rec, "--name", "parity_tol", "--tolerance", "0.5"],
                    writes=True)
    compare_cli(rep, "import", sb, exe, ["import-recording", "no_such_recording"], writes=True)
    for rec in ("parity_ab4", "parity_ab3", "parity_abarg"):
        compare_cli(rep, "import", sb, exe, ["import-recording", rec, "--name", f"{rec}_imp"], writes=True)
    if recordings:
        compare_cli(rep, "import", sb, exe, ["import-recording", recordings[0], "--name", "mine"], writes=True)
    compare_cli(rep, "import", sb, exe, ["import-recording", "timelines/mine.json", "--name", "parity_bad"], writes=True)


# --------------------------------------------------------------------------
# editor API


def strip_ignored(v):
    if isinstance(v, dict):
        return {k: strip_ignored(x) for k, x in v.items() if k not in IGNORED_KEYS}
    if isinstance(v, list):
        return [strip_ignored(x) for x in v]
    return v


def normalise_file(name: str, data: bytes) -> bytes:
    if name.endswith((".txt", ".tmp")):
        data = re.sub(rb"(?m)^id [0-9a-f]+$", b"id <id>", data)
    return data


class Client:
    def __init__(self, port: int):
        self.base = f"http://127.0.0.1:{port}"

    def call(self, method: str, path: str, body=None) -> tuple[int, str]:
        data = None if body is None else json.dumps(body).encode("utf-8")
        req = urllib.request.Request(self.base + path, data=data, method=method,
                                     headers={"Content-Type": "application/json"} if data else {})
        try:
            with urllib.request.urlopen(req, timeout=120) as r:
                return r.status, r.read().decode("utf-8")
        except urllib.error.HTTPError as e:
            return e.code, e.read().decode("utf-8")


def wait_for_port(port: int, proc: subprocess.Popen):
    for _ in range(200):
        if proc.poll() is not None:
            raise RuntimeError(f"editor exited early: {proc.stdout.read() if proc.stdout else ''}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError(f"editor did not open port {port}")


def edits_of(norm: dict) -> dict:
    return {"length": norm["length"], "yaw": norm["yaw"], "pitch": norm["pitch"], "keys": norm["keys"],
            "strafe": norm.get("strafe") or [], "view_pitch": norm.get("view_pitch"), "turns": norm.get("turns") or []}


def editor_script(c: Client, settle=lambda: None):
    """The calls, generated from the answers (so both back ends get the same
    calls as long as they answer the same). Yields (label, status, text).
    settle(): a request still pending is as if the game took it, so the busy
    rule (a request under a minute old) doesn't depend on how fast a back end is."""
    def call(label, method, path, body=None):
        status, text = c.call(method, path, body)
        return label, status, text

    yield (r := call("list", "GET", "/api/list"))
    listing = json.loads(r[2])
    yield call("game", "GET", "/api/game")
    yield call("open bad name", "GET", "/api/open?name=..%2Fx")
    yield call("open missing", "GET", "/api/open?name=nope")
    yield call("open no name", "GET", "/api/open")
    yield call("unknown get", "GET", "/api/nothing")
    yield call("unknown post", "POST", "/api/nothing", {})
    yield call("request bad", "GET", "/api/request?id=../x")
    yield call("request unknown", "GET", "/api/request?id=abc123")
    for name in listing["timelines"]:
        settle()
        yield call(f"thumb {name}", "GET", f"/api/thumb?name={name}")
        yield (r := call(f"open {name}", "GET", f"/api/open?name={name}"))
        st = json.loads(r[2])
        doc = st["doc"]
        yield call(f"runs {name}", "GET", f"/api/runs?name={name}")
        yield call(f"state {name}", "POST", "/api/state", {"name": name, "doc": doc})
        if st.get("error"):
            continue
        norm = st["norm"]
        L = norm["length"]
        edits = edits_of(norm)
        yield call(f"update {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": edits})
        e2 = copy.deepcopy(edits)
        e2["yaw"] = e2["yaw"] + [[L - 1, 12.5, "smooth"]] if all(k[0] != L - 1 for k in e2["yaw"]) else e2["yaw"]
        e2["keys"] = e2["keys"] + [{"key": "duck", "from": 1, "to": 3}, {"key": "a", "from": 5, "to": 9, "value": 250}]
        e2["strafe"] = [{"from": 2, "to": min(L, 40), "offset": 0}, {"from": min(L - 1, 45), "to": L, "offset": 2.5, "heading": -12.5}]
        yield call(f"update+ {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": e2})
        yield call(f"update pitch {name}", "POST", "/api/update",
                   {"name": name, "doc": doc, "edits": dict(edits, pitch=[[0, 3.0, "linear"], [L // 2, -7.25, "cubic"], [L - 1, 1.0, "linear"]])})
        yield call(f"update no pitch {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, pitch=None)})
        yield call(f"update autobhop {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, autobhop=True)})
        yield call(f"update turns {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, turns=[
            {"from": L // 3, "to": L // 2, "rate": 1.75, "shape": "smooth"}, {"from": 1, "to": 4, "rate": -2, "shape": "linear"}])})
        yield call(f"update no turns {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, turns=[])})
        yield call(f"update swing {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, strafe=[
            {"from": 2, "to": min(L, 40), "offset": 0, "heading": 15.5, "swing": 2.5}])})
        yield call(f"update no autobhop {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, autobhop=False)})
        yield call(f"update bad autobhop {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, autobhop="on")})
        yield call(f"update camera off {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, camera_smoothing="off")})
        yield call(f"update camera all {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, camera_smoothing="all")})
        yield call(f"update camera strafe {name}", "POST", "/api/update",
                   {"name": name, "doc": dict(doc, camera_smoothing="all"), "edits": dict(edits, camera_smoothing="strafe")})
        yield call(f"update bad camera {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, camera_smoothing=0)})
        yield call(f"update view pitch {name}", "POST", "/api/update",
                   {"name": name, "doc": doc, "edits": dict(edits, view_pitch=[[L - 1, 4.0, "linear"], [0, -2.5, "smooth"]])})
        yield call(f"update no view pitch {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, view_pitch=None)})
        yield call(f"update bad view pitch {name}", "POST", "/api/update",
                   {"name": name, "doc": doc, "edits": dict(edits, view_pitch=[[0, 1.0, "bent"]])})
        yield call(f"update empty view pitch {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, view_pitch=[])})
        # keys with their own slopes (the editor's smooth drags write them), through an update and the tick operations
        sl = copy.deepcopy(edits)
        for j, k in enumerate(sl["yaw"]):
            if j % 2 == 0 and j + 1 < len(sl["yaw"]):
                sl["yaw"][j] = [k[0], k[1], "cubic", round(0.37 - 0.11 * j, 4)]
        if all(k[0] != L // 3 for k in sl["yaw"]):
            sl["yaw"] = sorted(sl["yaw"] + [[L // 3, 1.0, "linear", None]], key=lambda k: k[0])
        yield call(f"update slopes {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": sl})
        yield call(f"update bad slope {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, yaw=[[0, 1.0, "cubic", "x"]])})
        for op in ({"op": "insert", "at": L // 2, "count": 5}, {"op": "delete", "from": max(1, L // 4), "to": max(1, L // 4) + 7},
                   {"op": "length", "length": L + 3}, {"op": "turn", "from": 1, "to": min(L - 1, 20), "rate": 1.5, "after": "shift"}):
            yield call(f"op slopes {name} {op['op']}", "POST", "/api/op", {"name": name, "doc": doc, "edits": sl, "op": op})
        # every yaw key a joint (the editor's curve), some with their own widths, slopes smooth curves left (ignored)
        # and a line said with its width, through an update and the operations
        jt = copy.deepcopy(edits)
        widths = {1: [None, 6], 2: [None, 2.5], 3: [None, 0], 4: [3.25], 5: [-1.5], 6: [0.5, 4]}
        jt["yaw"] = [[k[0], k[1], "joint"] + widths.get(j, []) for j, k in enumerate(jt["yaw"])]
        yield call(f"update joints {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": jt})
        yield call(f"update bad joint {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, yaw=[[0, 1.0, "joint", None, -2]])})
        q = max(1, L // 4)
        for i, op in enumerate(({"op": "insert", "at": L // 2, "count": 5}, {"op": "insert", "at": 1, "count": 2},
                                {"op": "insert", "at": jt["yaw"][min(2, len(jt["yaw"]) - 1)][0], "count": 3},
                                {"op": "delete", "from": q, "to": q + 7}, {"op": "delete", "from": 1, "to": 4},
                                {"op": "turn", "from": q, "to": q + 20, "rate": 2.5}, {"op": "turn", "from": q, "to": q + 30, "rate": -1.75, "after": "shift"},
                                {"op": "turn", "from": 2 * q, "to": 2 * q + 15, "rate": 1, "shape": "smooth"}, {"op": "length", "length": L + 4})):
            yield call(f"op joints {name} {i} {op['op']}", "POST", "/api/op", {"name": name, "doc": doc, "edits": jt, "op": op})
        yield call(f"op short key {name}", "POST", "/api/op", {"name": name, "doc": doc, "edits": dict(edits, yaw=[[0, 1.0]]),
                                                               "op": {"op": "insert", "at": 0, "count": 1}})
        yield call(f"update bad {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, yaw=[[0, 1.0, "nope"]])})
        yield call(f"update badkey {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, keys=[{"key": "zz", "from": 0, "to": 1}])})
        yield call(f"update missing {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": dict(edits, keys=[{"key": "W", "to": 1}])})
        yield call(f"update no edits {name}", "POST", "/api/update", {"name": name, "doc": doc})
        q = max(1, L // 4)
        ops = [
            {"op": "insert", "at": 0, "count": 3}, {"op": "insert", "at": L // 2, "count": 5},
            {"op": "insert", "at": L, "count": 2}, {"op": "insert", "at": -1, "count": 2},
            {"op": "insert", "at": 3, "count": 0}, {"op": "delete", "from": 1, "to": 4},
            {"op": "delete", "from": q, "to": q + 7}, {"op": "delete", "from": L - 5, "to": L},
            {"op": "delete", "from": 0, "to": L}, {"op": "delete", "from": 5, "to": 2},
            {"op": "length", "length": L + 10}, {"op": "length", "length": max(1, L - 10)}, {"op": "length", "length": L},
            {"op": "turn", "from": q, "to": q + 20, "rate": 2.5, "keys": {"W": "hold", "D": "release", "jump": "keep"}},
            {"op": "turn", "from": q, "to": q + 30, "rate": -1.75, "shape": "smooth", "after": "shift"},
            {"op": "turn", "from": 2 * q, "to": 2 * q + 15, "rate": 0, "start_yaw": 33.3, "keys": {"duck": "hold"}},
            {"op": "turn", "from": q, "to": 3 * q, "rate": 1, "shape": "auto", "offset": 4, "keys": {"A": "hold"}},
            {"op": "turn", "from": q + 5, "to": q + 9, "rate": 1, "shape": "auto"},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "heading": 87.5, "keys": {"D": "hold", "A": "release"}},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "heading": -33, "offset": -2},
            {"op": "turn", "from": 1, "to": 5, "rate": 0, "shape": "auto", "heading": "east"},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "heading": 12.5, "swing": 3.5},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "swing": 3},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "heading": 1, "swing": 0},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "heading": 22.5, "swing": 7, "gain": 81.25},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "heading": 22.5, "gain": 90},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 0, "shape": "auto", "heading": 22.5, "swing": 7, "gain": 101},
            {"op": "turn", "from": q, "to": 2 * q, "rate": -0.75, "shape": "linear", "after": "shift"},
            {"op": "turn", "from": q, "to": 2 * q, "rate": 1.25, "shape": "smooth"},
            {"op": "turn", "from": 1, "to": 3, "rate": 2, "shape": "linear"},
            {"op": "turn", "from": 3, "to": 2, "rate": 1}, {"op": "turn", "from": 1, "to": 5, "rate": "abc"},
            {"op": "turn", "from": 1, "to": 5, "rate": 1, "shape": "zigzag"},
            {"op": "turn", "from": 1, "to": 5, "rate": 1, "after": "later"},
            {"op": "turn", "from": 1, "to": 5, "rate": 1, "offset": 120, "shape": "auto"},
            {"op": "turn", "from": 1, "to": 5, "rate": 1, "keys": {"W": "press"}},
            {"op": "turn", "from": 1, "to": 5, "rate": 1, "keys": {"left": "hold"}},
            {"op": "turn", "from": 1, "to": 5},
            {"op": "warp"}, {}, {"op": "freeze", "tick": 0}, {"op": "splice", "recording": "../x", "at": 5},
        ]
        for strafe in norm.get("strafe") or []:
            ops.append({"op": "freeze", "tick": strafe["from"]})
        for strafe in norm.get("strafe") or []:
            ops.append({"op": "freeze", "tick": strafe["to"] - 1})
        for rec in listing["recordings"]:
            ops.append({"op": "splice", "recording": rec, "at": 5})
            ops.append({"op": "splice", "recording": rec, "at": 40})
        ops.append({"op": "splice", "recording": "no_such_rec", "at": 5})
        # the curve turns' blocks, carried along by inserts, deletes and turns over them
        blocks = dict(edits, turns=[{"from": 2, "to": q, "rate": 1.5, "shape": "linear"}, {"from": q, "to": 2 * q, "rate": -1, "shape": "smooth"}])
        for i, op in enumerate([{"op": "insert", "at": 3, "count": 4}, {"op": "insert", "at": q + 1, "count": 2},
                                {"op": "delete", "from": q - 2, "to": q + 3}, {"op": "length", "length": q + 1},
                                {"op": "turn", "from": q - 3, "to": q + 3, "rate": 0, "shape": "auto"},
                                {"op": "turn", "from": 3, "to": q + 2, "rate": 0.5, "shape": "smooth"}]):
            yield call(f"op blocks {name} #{i} {op['op']}", "POST", "/api/op", {"name": name, "doc": doc, "edits": blocks, "op": op})
        # A and D set by hand: joined and kept inside the timeline, carried along by the operations like held keys
        L = int(edits["length"])
        hand = dict(edits, ad_by_hand=[{"from": 3, "to": q}, {"from": 1, "to": 4}, {"from": q + 2, "to": q + 3}, {"from": L - 2, "to": L + 9},
                                        {"from": 7, "to": 7}])
        yield call(f"update by hand {name}", "POST", "/api/update", {"name": name, "doc": doc, "edits": hand})
        for i, op in enumerate([{"op": "insert", "at": 2, "count": 3}, {"op": "insert", "at": q + 2, "count": 2},
                                {"op": "delete", "from": 2, "to": q + 1}, {"op": "length", "length": L + 2},
                                {"op": "length", "length": max(1, L // 2)}]):
            yield call(f"op by hand {name} #{i} {op['op']}", "POST", "/api/op", {"name": name, "doc": doc, "edits": hand, "op": op})
        for i, op in enumerate(ops):
            yield (r := call(f"op {name} #{i} {op.get('op')}", "POST", "/api/op", {"name": name, "doc": doc, "edits": edits, "op": op}))
            out = json.loads(r[2])
            if i % 3 == 0 and not out.get("error") and "norm" in out:
                # chain: the next operation on the result of this one
                follow = {"op": "turn", "from": 1, "to": min(out["norm"]["length"] - 1, 30), "rate": 3, "shape": "smooth",
                          "after": "shift", "keys": {"S": "hold"}}
                yield call(f"op {name} #{i} chained", "POST", "/api/op",
                           {"name": name, "doc": out["doc"], "edits": edits_of(out["norm"]), "op": follow})
        yield call(f"save {name}", "POST", "/api/save", {"name": name, "doc": doc})
        yield call(f"save bad {name}", "POST", "/api/save", {"name": name, "doc": dict(doc, length=-1)})
        yield call(f"save badname {name}", "POST", "/api/save", {"name": "a b", "doc": doc})
        yield (r := call(f"preview {name}", "POST", "/api/preview", {"name": name, "doc": doc}))
        pid = json.loads(r[2]).get("id")
        if pid:
            yield call(f"request {name}", "GET", f"/api/request?id={pid}")
        yield call(f"preview bad {name}", "POST", "/api/preview", {"name": name, "doc": dict(doc, version=3)})
        yield call(f"record {name}", "POST", "/api/record", {"name": name, "doc": doc, "at": 5})
        yield call(f"record 0 {name}", "POST", "/api/record", {"name": name, "doc": doc, "at": 0})
        yield call(f"draw {name}", "POST", "/api/draw", {"map": norm["map"], "path": name, "ghost": None, "timeline": name,
                                                        "cursor": 7, "files": "abc123", "box": True})
        yield call(f"draw min {name}", "POST", "/api/draw", {"map": norm["map"]})
        yield call(f"changemap {name}", "POST", "/api/changemap", {"map": norm["map"]})
        yield call(f"changemap bad {name}", "POST", "/api/changemap", {"map": "../" + norm["map"]})
        yield call(f"changemap none {name}", "POST", "/api/changemap", {})
        yield call(f"changemap int {name}", "POST", "/api/changemap", {"map": 5})
    yield call("draw bad name", "POST", "/api/draw", {"map": "a/b"})
    yield call("draw bad cursor", "POST", "/api/draw", {"map": "m", "cursor": -1})
    yield call("draw float cursor", "POST", "/api/draw", {"map": "m", "cursor": 2.5})
    yield call("draw bad files", "POST", "/api/draw", {"map": "m", "files": "a-b"})
    yield call("draw box off", "POST", "/api/draw", {"map": "m", "files": 0, "box": 0})
    yield call("draw no map", "POST", "/api/draw", {})
    yield (r := call("run", "POST", "/api/run", {"name": listing["timelines"][0],
                                                  "doc": json.loads(c.call("GET", f"/api/open?name={listing['timelines'][0]}")[1])["doc"]}))
    rid = json.loads(r[2]).get("id")
    if rid:
        yield call("request run", "GET", f"/api/request?id={rid}")
    yield call("run busy", "POST", "/api/run", {"name": listing["timelines"][0], "doc": {}})
    yield call("preview while busy", "POST", "/api/preview", {"name": listing["timelines"][0], "doc": {}})
    for rec in listing["recordings"]:
        yield call(f"import {rec}", "POST", "/api/import", {"recording": rec, "name": f"imp_{rec}"})
        yield call(f"import again {rec}", "POST", "/api/import", {"recording": rec, "name": f"imp_{rec}"})
    yield call("import missing", "POST", "/api/import", {"recording": "nope", "name": "imp_nope"})
    yield call("import bad name", "POST", "/api/import", {"recording": "x y", "name": "imp"})
    yield call("bad json", "POST", "/api/save", None)

    # the main menu: start positions, new timelines and copies
    yield (r := call("starts", "GET", "/api/starts"))
    for i, st in enumerate(json.loads(r[2])["starts"]):
        body = {"name": f"new_{st['name']}", "start": st["name"]}
        if i % 2:
            body["length"] = 100 + i
        if st.get("map") is None:
            yield call(f"create {st['name']} without map", "POST", "/api/create", body)
            body["map"] = "surf_beginner"
        yield call(f"create from {st['name']}", "POST", "/api/create", body)
    for name in listing["timelines"]:
        yield call(f"copy {name}", "POST", "/api/create", {"name": f"copy_{name}", "copy": name})
    first = listing["timelines"][0]
    for label, body in [
        ("create exists", {"name": first, "copy": first}),
        ("create bad name", {"name": "a b", "start": "x"}),
        ("create name number", {"name": 12, "start": "x"}),
        ("create no start", {"name": "zz"}),
        ("create missing start", {"name": "zz", "start": "nope"}),
        ("create bad start name", {"name": "zz", "start": "../x"}),
        ("create length 0", {"name": "zz", "start": "parity_nomap", "map": "m", "length": 0}),
        ("create length text", {"name": "zz", "start": "parity_nomap", "map": "m", "length": "abc"}),
        ("create length float", {"name": "zz_float", "start": "parity_nomap", "map": "surf_beginner", "length": 99.7}),
        ("create bad map", {"name": "zz", "start": "parity_nomap", "map": "a/b"}),
        ("copy missing", {"name": "zz", "copy": "nope"}),
        ("copy bad", {"name": "zz", "copy": 5}),
        ("create no name", {"start": "x"}),
    ]:
        yield call(label, "POST", "/api/create", body)
    yield (r := call("list after", "GET", "/api/list"))
    for name in json.loads(r[2])["timelines"]:
        if name.startswith(("new_", "copy_", "zz_")):
            yield call(f"open {name}", "GET", f"/api/open?name={name}")


def run_editor(sb: Sandbox, backend: str, exe: Path, port: int):
    sb.reset()
    cmd = ([sys.executable, "-m", "tasurf", "editor", "--port", str(port), "--no-browser"] if backend == "python"
           else [str(exe), "editor", "--port", str(port), "--no-browser"])
    proc = subprocess.Popen(cmd, cwd=sb.root, env=sb.env(), stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    try:
        wait_for_port(port, proc)
        def settle():  # a pending request, aged past the busy rule's minute
            pending = sb.gd / "requests" / "request.txt"
            if pending.exists():
                old = time.time() - 3600
                os.utime(pending, (old, old))

        answers = list(editor_script(Client(port), settle))
    finally:
        proc.terminate()
        proc.wait(timeout=10)
    files = {k: normalise_file(k, v) for k, v in sb.files().items()}
    return answers, files


def editor_checks(rep: Report, sb: Sandbox, exe: Path):
    py_answers, py_files = run_editor(sb, "python", exe, 8781)
    rs_answers, rs_files = run_editor(sb, "rust", exe, 8782)
    if len(py_answers) != len(rs_answers):
        rep.check("editor", "number of calls", False, f"  python {len(py_answers)}, rust {len(rs_answers)}")
    for (label, s1, t1), (label2, s2, t2) in zip(py_answers, rs_answers):
        problems = []
        if label != label2:
            problems.append(f"  the scripts diverged: {label!r} vs {label2!r}")
        if s1 != s2:
            problems.append(f"  status: python {s1}, rust {s2}")
        try:
            j1, j2 = json.dumps(strip_ignored(json.loads(t1))), json.dumps(strip_ignored(json.loads(t2)))
        except json.JSONDecodeError:
            j1, j2 = t1, t2
        if j1 != j2:
            problems.append(first_diff(j1, j2))
        rep.check("editor", label, not problems, "\n".join(problems))
        if label != label2:
            break
    for k in sorted(py_files.keys() | rs_files.keys()):
        a, b = py_files.get(k), rs_files.get(k)
        if a is None or b is None:
            rep.check("files", k, False, f"  only written by {'rust' if a is None else 'python'}")
            continue
        rep.check("files", k, a == b, first_diff(a.decode("utf-8", "replace"), b.decode("utf-8", "replace")) if a != b else "")


def map_checks(rep: Report, sb: Sandbox, exe: Path):
    """The 3D view: every map in the game folder, as both back ends serve it, byte for byte: its
    mesh, its textured data, and every material and texture it uses as the game would find them (the map's
    pakfile, then the game's VPKs and folders). The maps and VPKs are hard links in the sandbox (no copies;
    the servers only read them)."""
    import re
    import struct
    from concurrent.futures import ThreadPoolExecutor
    sb.reset()
    cstrike = game_data_dir().parents[3]

    def link(src: Path, dst: Path):
        dst.parent.mkdir(parents=True, exist_ok=True)
        try:
            os.link(src, dst)
        except OSError:
            shutil.copy2(src, dst)

    names = []
    for src in sorted(list((cstrike / "maps").glob("*.bsp")) + list((cstrike / "download" / "maps").glob("*.bsp"))):
        dst = sb.root / "cstrike" / "download" / "maps" / src.name
        if not dst.exists():
            link(src, dst)
            names.append(src.stem)
    sys.path.insert(0, str(REPO))
    from tests.test_gamefiles import props_map  # the props tests' maps: as they are, compressed, broken
    broken = bytearray(props_map(False))
    struct.pack_into("<i", broken, struct.unpack_from("<i", broken, 8 + 16 * 35)[0] + 4 + 16, 10 ** 6)
    for name, data in [("tasurf_props", props_map(False)), ("tasurf_props_lzma", props_map(True, 76)), ("tasurf_props_broken", bytes(broken))]:
        dst = sb.root / "cstrike" / "download" / "maps" / f"{name}.bsp"
        dst.parent.mkdir(parents=True, exist_ok=True)
        dst.write_bytes(data)
        names.append(name)
    for folder in ("cstrike", "hl2", "platform"):
        for v in (cstrike.parent / folder).glob("*.vpk"):
            link(v, sb.root / folder / v.name)
    procs = []
    try:
        for backend, port in (("python", 8785), ("rust", 8786)):
            cmd = ([sys.executable, "-m", "tasurf", "editor", "--port", str(port), "--no-browser"] if backend == "python"
                   else [str(exe), "editor", "--port", str(port), "--no-browser"])
            procs.append(subprocess.Popen(cmd, cwd=sb.root, env=sb.env(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            wait_for_port(port, procs[-1])

        def get(port, path):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}{path}", timeout=300) as r:
                    return r.status, r.read()
            except urllib.error.HTTPError as e:
                return e.code, e.read()

        def both(path):
            return get(8785, path), get(8786, path)

        def differ(a, b):
            i = next((k for k, (x, y) in enumerate(zip(a[1], b[1])) if x != y), min(len(a[1]), len(b[1])))
            detail = f"  python {a[0]} {len(a[1])} bytes, rust {b[0]} {len(b[1])} bytes, first difference at byte {i}"
            if a[0] != 200 or b[0] != 200:
                detail += f"\n  python {a[1][:200]!r}\n  rust   {b[1][:200]!r}"
            return detail

        q = urllib.parse.quote
        for path in ["/view3d.js", "/predict.js", "/api/mapprops?name=nope", "/api/mapprops", "/api/mapprops?name=..%2Fx",
                     "/api/mapclip?name=nope", "/api/mapclip", "/api/map?name=nope", "/api/map?name=..%2Fx", "/api/map", "/api/maptex?name=nope",
                     "/api/maptex", "/api/maptrig?name=nope", "/api/maptrig", "/api/mapfile?name=nope&path=materials/x.vmt",
                     "/api/mapfile?name=..%2Fx&path=materials/x.vmt"]:
            a, b = both(path)
            rep.check("maps", path, a == b, "" if a == b else differ(a, b))
        if names:  # odd and bad paths, on one map
            for rel in ["", "materials/../cfg/x.vmt", "materials/x.txt", "models/x.mdl", "materials/a:b.vmt", "materials/./x.vmt",
                        "MATERIALS\\DEV\\DEV_MEASUREGENERIC01.VMT", "materials//dev//dev_measuregeneric01.vmt",
                        "materials/dev/dev_measuregeneric01.vtf", "materials/nope/missing.vtf", "materials/caf\u00e9.vmt",
                        "materials/\u4e2d.vmt", "materials/.vmt", "/materials/x.vmt", "models/.mdl", "models/x.vmt",
                        "MODELS\\NOPE\\X.DX90.VTX", "models/../x.vvd"]:
                a, b = both(f"/api/mapfile?name={names[0]}&path={q(rel)}")
                rep.check("mapfiles", f"{names[0]} {rel!r}", a == b, "" if a == b else differ(a, b))

        def mat_path(p, ext):  # as view3d.js asks for them
            p = re.sub(r"/+", "/", p.strip().lower().replace("\\", "/")).lstrip("/")
            p = p if p.startswith("materials/") else "materials/" + p
            return p if p.endswith("." + ext) else f"{p}.{ext}"

        pool = ThreadPoolExecutor(8)
        for name in names:
            t = time.perf_counter(); a = get(8785, f"/api/map?name={name}"); tp = time.perf_counter() - t
            t = time.perf_counter(); b = get(8786, f"/api/map?name={name}"); tr = time.perf_counter() - t
            rep.check("maps", f"{name} ({tp:.1f} s python, {tr:.2f} s rust, {len(a[1]) / 1e6:.1f} MB)", a == b, "" if a == b else differ(a, b))
            a, b = both(f"/api/maptrig?name={name}")
            rep.check("maptrig", f"{name} ({len(a[1]) / 1e3:.0f} kB)", a == b, "" if a == b else differ(a, b))
            a, b = both(f"/api/mapclip?name={name}")
            rep.check("mapclip", f"{name} ({len(a[1]) / 1e3:.0f} kB)", a == b, "" if a == b else differ(a, b))
            a, b = both(f"/api/mapprops?name={name}")
            props = json.loads(a[1]) if a[0] == 200 else {}
            rep.check("mapprops", f"{name} ({len(props.get('props', []))} props, {len(props.get('models', []))} models)", a == b,
                      "" if a == b else differ(a, b))
            batch = [m[:-4] + ext for m in props.get("models", []) for ext in (".mdl", ".vvd", ".dx90.vtx")]  # as view3d.js asks
            same, first_bad = 0, ""
            for rel, (a, b) in zip(batch, pool.map(lambda r: both(f"/api/mapfile?name={name}&path={q(r)}"), batch)):
                same += a == b
                if a != b and not first_bad:
                    first_bad = f"  {rel}\n" + differ(a, b)
            rep.check("mapprops", f"{name}: {same}/{len(batch)} model files", same == len(batch), first_bad)
            t = time.perf_counter(); a = get(8785, f"/api/maptex?name={name}"); tp = time.perf_counter() - t
            t = time.perf_counter(); b = get(8786, f"/api/maptex?name={name}"); tr = time.perf_counter() - t
            rep.check("maptex", f"{name} ({tp:.1f} s python, {tr:.2f} s rust, {len(a[1]) / 1e6:.1f} MB)", a == b, "" if a == b else differ(a, b))
            if a[0] != 200 or a[1][:4] != b"TAST":
                continue
            tex = a[1]
            nv, nt, nm = struct.unpack_from("<III", tex, 8)
            o = 32 + nv * 20
            used = set(struct.unpack_from(f"<{nt}H", tex, o))
            o += (nt * 2 + 3) // 4 * 4
            lens = [struct.unpack_from("<I", tex, o + 24 * k + 20)[0] for k in range(nm)]
            o += 24 * nm
            mats = []
            for ln in lens:
                mats.append(tex[o:o + ln].decode("latin-1"))
                o += ln
            files, same, first_bad = set(), 0, ""
            todo = {mat_path(mats[m], "vmt") for m in used if m < nm}
            while todo:  # the materials, what they include, and their textures
                batch = sorted(todo - files)
                files |= todo
                todo = set()
                for rel, (a, b) in zip(batch, pool.map(lambda r: both(f"/api/mapfile?name={name}&path={q(r)}"), batch)):
                    same += a == b
                    if a != b and not first_bad:
                        first_bad = f"  {rel}\n" + differ(a, b)
                    if a[0] == 200 and rel.endswith(".vmt"):
                        text = re.sub(r"//[^\n]*", "", a[1].decode("latin-1"))
                        for key, ext in (("include", "vmt"), ("$basetexture", "vtf"), ("$basetexture2", "vtf")):
                            for m in re.finditer(r'"?' + re.escape(key) + r'"?\s+"?([^"\s{}]+)', text, re.I):
                                todo.add(mat_path(m.group(1), ext))
            rep.check("mapfiles", f"{name}: {same}/{len(files)} files", same == len(files), first_bad)
        pool.shutdown()
    finally:
        for p in procs:
            p.terminate()
            p.wait(timeout=10)


def write_math_data(path: Path):
    """rust/testdata/cpython_math.txt: math.hypot, degrees(atan2) and radians as
    this Python computes them, for the Rust unit test that checks them bit for bit."""
    import math
    import random
    import struct
    random.seed(7)
    f32 = lambda v: struct.unpack("<f", struct.pack("<f", v))[0]
    bits = lambda v: f"0x{struct.unpack('<Q', struct.pack('<d', v))[0]:016x}"
    cases = []
    for _ in range(400):
        kind = random.random()
        if kind < 0.4:  # surf speeds from float32 velocities
            cases.append((f32(random.uniform(-3500, 3500)), f32(random.uniform(-3500, 3500))))
        elif kind < 0.7:
            cases.append((random.uniform(-1, 1) * 10 ** random.randint(-8, 8),
                          random.uniform(-1, 1) * 10 ** random.randint(-8, 8)))
        else:
            a, r = random.uniform(0, 2 * math.pi), random.uniform(0, 400)
            cases.append((r * math.cos(a), r * math.sin(a)))
    lines = [f"# CPython {sys.version.split()[0]} math results (bits of IEEE doubles), written by rust/parity.py --write-math-data:",
             "# hypot X Y math.hypot(X, Y)   |   angle X Y math.degrees(math.atan2(Y, X)) math.radians(X)"]
    lines += [f"hypot {bits(x)} {bits(y)} {bits(math.hypot(x, y))}" for x, y in cases]
    lines += [f"angle {bits(x)} {bits(y)} {bits(math.degrees(math.atan2(y, x)))} {bits(math.radians(x))}" for x, y in cases[:100]]
    path.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--exe", default=str(DEFAULT_EXE))
    ap.add_argument("--keep", action="store_true", help="keep the sandbox folder")
    ap.add_argument("--only", choices=("cli", "editor", "maps"))
    ap.add_argument("--write-math-data", action="store_true", help="regenerate rust/testdata/cpython_math.txt and exit")
    args = ap.parse_args()
    if args.write_math_data:
        write_math_data(REPO / "rust" / "testdata" / "cpython_math.txt")
        return 0
    exe = Path(args.exe).resolve()
    if not exe.is_file():
        print(f"no Rust build at {exe}; run: cargo build --release (in rust/)")
        return 2
    tmp = Path(tempfile.mkdtemp(prefix="tasurf_parity_"))
    sb = Sandbox(tmp / "sb", game_data_dir())
    rep = Report()
    try:
        if args.only in (None, "cli"):
            cli_checks(rep, sb, exe)
        if args.only in (None, "editor"):
            editor_checks(rep, sb, exe)
        if args.only in (None, "maps"):
            map_checks(rep, sb, exe)
    finally:
        if args.keep:
            print(f"sandbox kept at {tmp}")
        else:
            shutil.rmtree(tmp, ignore_errors=True)
    return 0 if rep.print() else 1


if __name__ == "__main__":
    sys.exit(main())

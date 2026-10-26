"""Browser editor: python -m tasurf editor [timeline]

A small local web server (127.0.0.1 only) serving editor_static/index.html and
a JSON API. Timelines are read from and saved to timelines/. "Run" saves,
bakes, installs the baked file into the game and drops a request file that
the plugin picks up (data/tasurf/requests/request.txt). The result comes back
as requests/<id>.result plus the usual run CSV.

All movement analysis here (best yaw, estimates) uses the reference model in
physics.py; the engine's run CSVs stay the ground truth.
"""

from __future__ import annotations

import json
import math
import shutil
import threading
import time
import uuid
import webbrowser
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from . import config
from .bake import bake_inputs, bake_to_file, read_baked, render_baked
from .camera import camera_yaws
from .bspmap import MapError, find_map, read_clip, read_map, read_props, read_triggers  # 3D view
from .editdoc import (MAX_TICKS, apply_edits, apply_op, format_json, freeze_strafe, import_recording, normalized,
                      save_document, splice_recording, strafe_at)
from .interp import sample
from .physics import best_yaw, check_run, estimate_free_flight, params_from_run
from .gamefiles import GameFiles  # 3D view
from .runcsv import find_runs, read_run
from .timeline import (FORMAT_NAME, FORMAT_VERSION, NAME_RE, TimelineError, bits_to_float, float_to_bits, parse_start,
                       parse_timeline)

STATIC = Path(__file__).resolve().parent / "editor_static"
BUSY = ("the game hasn't picked up the previous run request yet (it needs a map loaded, the replay bot "
        "ready, and no run playing); try again in a moment")
VIEW_PITCH_PLUGIN = (0, 9)  # the first plugin that plays @view_pitch lines (older ones refuse the file)
STRAIGHT_PLUGIN = (0, 11)  # the first that plays straight auto-strafe (@strafe with a heading, baked version 3)
AUTOBHOP_PLUGIN = (0, 12)  # the first that plays auto bhop (@autobhop, baked version 4)
SWING_PLUGIN = (0, 13)  # the first that plays a straight range's swing (baked version 5)
GAIN_PLUGIN = (0, 14)  # the first that plays a swing's gain (smooth turning, baked version 6)
THUMB_POINTS = 240  # at most, along a timeline's path on the start screen (Editor.thumb)
VIEW_YAW_PLUGIN = (0, 15)  # the first that shows @view_yaw lines (older ones refuse the file, so they get none)


class Editor:
    def __init__(self, timelines_dir: Path, cfg: dict, out_dir: Path | None = None):
        self.timelines_dir = timelines_dir
        self.out_dir = out_dir or config.REPO_ROOT / "out" / "baked"
        self.cfg = cfg
        self.data_dir = config.game_data_dir(cfg)
        self.lock = threading.Lock()
        self._run_cache: dict[str, tuple[float, object, object]] = {}
        self._sent: dict[str, float] = {}  # request id -> time sent
        self._maps: dict[str, tuple[float, bytes, bytes | None]] = {}  # 3D view: path -> (mtime, mesh, textured)
        self._files: GameFiles | None = None  # 3D view: the game's materials and textures
        self._replaced: set[str] = set()   # ids replaced by a newer request before the game took them

    # ---- paths ------------------------------------------------------------

    def timeline_path(self, name: str) -> Path:
        if not NAME_RE.match(name):
            raise TimelineError(f"bad timeline name {name!r}")
        return self.timelines_dir / f"{name}.json"

    def search_dirs(self) -> list[Path]:
        return [self.data_dir / "starts"] if self.data_dir else []

    # ---- listing ----------------------------------------------------------

    def list(self) -> dict:
        """Timelines and recordings, with what the main menu shows about each
        (map, length, runs) in "info" and "recording_info", in the same order."""
        timelines = sorted(p.stem for p in self.timelines_dir.glob("*.json"))
        recordings, recording_info = [], []
        if self.data_dir and (self.data_dir / "baked").is_dir():
            for p in sorted((self.data_dir / "baked").glob("*.tas")):
                head = p.read_text(encoding="utf-8", errors="replace")[:600]
                if "@hash fnv1a32:" in head:  # written by tas_record
                    recordings.append(p.stem)
                    recording_info.append(_recording_info(p.stem, head))
        return {"timelines": timelines, "recordings": recordings, "game": self.data_dir is not None,
                "info": [self._timeline_info(n) for n in timelines], "recording_info": recording_info}

    def thumb(self, name: str) -> dict:
        """A timeline's path for its picture on the start screen: from its newest run or preview, x, y and the
        horizontal speed at about THUMB_POINTS ticks, with a null where it teleports (the page draws each stretch).
        points is null before it has run."""
        if not NAME_RE.match(name):
            return {"error": f"bad name {name!r}"}
        tl_name = name  # runs are named after the timeline's own name, which may differ from the file's
        try:
            doc = json.loads((self.timelines_dir / f"{name}.json").read_text(encoding="utf-8"))
            if isinstance(doc, dict) and isinstance(doc.get("name"), str) and NAME_RE.match(doc["name"]):
                tl_name = doc["name"]
        except (OSError, ValueError):
            pass
        files = self._runs(tl_name)
        if self.data_dir and (self.data_dir / "runs" / f"{tl_name}.preview.csv").is_file():
            files = files + [self.data_dir / "runs" / f"{tl_name}.preview.csv"]
        newest, newest_m = None, None
        for f in files:
            m = f.stat().st_mtime
            if newest is None or m > newest_m:
                newest, newest_m = f, m
        if newest is None:
            return {"error": None, "points": None}
        try:
            run = read_run(newest)
            xs, ys, sp = run.floats("x"), run.floats("y"), run.floats("hspeed")
            jump = float(run.header.get("tick_interval", "0.015")) * 3500 * 2.5  # further in a tick than any speed
        except (OSError, ValueError, KeyError) as e:
            return {"error": str(e), "points": None}
        n = len(xs)
        step = max(1, -(-n // THUMB_POINTS))
        keep = list(range(0, n, step))
        if keep and keep[-1] != n - 1:
            keep.append(n - 1)
        points, prev = [], None
        for i in keep:
            if prev is not None and any((xs[t] - xs[t - 1]) * (xs[t] - xs[t - 1]) + (ys[t] - ys[t - 1]) * (ys[t] - ys[t - 1])
                                        > jump * jump for t in range(prev + 1, i + 1)):
                points.append(None)
            points.append([round(xs[i], 1), round(ys[i], 1), round(sp[i], 1)])
            prev = i
        return {"error": None, "file": newest.name, "points": points}

    def _timeline_info(self, name: str) -> dict:
        path = self.timelines_dir / f"{name}.json"
        info = {"name": name, "map": None, "length": None, "runs": 0, "mtime": path.stat().st_mtime}
        try:
            doc = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            info["error"] = "not valid JSON"
            return info
        if isinstance(doc, dict):
            if isinstance(doc.get("map"), str):
                info["map"] = doc["map"]
            if isinstance(doc.get("length"), int) and not isinstance(doc.get("length"), bool):
                info["length"] = doc["length"]
            # runs are named after the timeline's own name, which may differ from the file's
            tl_name = doc.get("name") if isinstance(doc.get("name"), str) and NAME_RE.match(doc["name"]) else name
            info["runs"] = len(self._runs(tl_name))
        return info

    def starts(self) -> dict:
        """The start positions captured in the game (data/tasurf/starts, from tas_setstart and recordings)."""
        out = []
        if self.data_dir and (self.data_dir / "starts").is_dir():
            for p in sorted((self.data_dir / "starts").glob("*.json")):
                entry = {"name": p.stem, "map": None, "yaw": None, "pitch": None, "origin": None,
                         "mtime": p.stat().st_mtime}
                try:
                    st = parse_start(json.loads(p.read_text(encoding="utf-8")), ctx=f"start file {p.name}", source=str(p))
                except (OSError, ValueError) as e:  # TimelineError is a ValueError
                    entry["error"] = str(e)
                else:
                    pitch, yaw = _exact_angles(st)
                    entry.update(map=st.map, yaw=yaw, pitch=pitch, origin=list(st.origin))
                out.append(entry)
        return {"starts": out, "game": self.data_dir is not None}

    def create(self, name, start=None, copy=None, length=None, map_name=None) -> dict:
        """A new timeline: from a start position (copied in, facing its yaw, no keys
        held), or a copy of another timeline under a new name."""
        if not isinstance(name, str) or not NAME_RE.match(name):
            return {"error": "names may only use letters, digits, '_', '-' and '.'"}
        dest = self.timeline_path(name)
        if dest.exists():
            return {"error": f"timelines/{name}.json already exists; pick another name"}
        if copy is not None:
            if not isinstance(copy, str) or not NAME_RE.match(copy):
                return {"error": f"no timeline {copy!r}"}
            try:
                doc = json.loads(self.timeline_path(copy).read_text(encoding="utf-8"))
            except (OSError, ValueError) as e:
                return {"error": str(e)}
            if not isinstance(doc, dict):
                return {"error": f"timelines/{copy}.json is not a timeline"}
            doc["name"] = name
        else:
            if not self.data_dir:
                return {"error": "cstrike_dir is not set in tasurf.local.json, so there are no start positions"}
            if not isinstance(start, str) or not NAME_RE.match(start):
                return {"error": "pick a start position"}
            path = self.data_dir / "starts" / f"{start}.json"
            try:
                obj = json.loads(path.read_text(encoding="utf-8"))
                st = parse_start(obj, ctx=f"start file {path.name}", source=str(path))
            except (OSError, ValueError) as e:
                return {"error": str(e)}
            map_name = st.map or map_name
            if not isinstance(map_name, str) or not NAME_RE.match(map_name):
                return {"error": f"the start position {start} doesn't say which map it's on; pick the map"}
            try:
                length = 660 if length is None else int(length)
            except (TypeError, ValueError):
                return {"error": f"length: expected a number of ticks, got {length!r}"}
            if not 1 <= length <= MAX_TICKS:
                return {"error": f"length: 1 to {MAX_TICKS} ticks"}
            _, yaw = _exact_angles(st)
            doc = {
                "_comment": f"Created in the editor from the start position {start} (copied in, so capturing "
                            f"that start again doesn't change this timeline).",
                "format": FORMAT_NAME, "version": FORMAT_VERSION, "name": name, "map": map_name, "length": length,
                "start": {k: v for k, v in obj.items() if k not in ("format", "version")},
                "yaw": {"default_interp": "cubic", "keys": [[0, yaw, "cubic"]]},  # smooth: no corners in the yaw
                "keys": [],
                "autobhop": True,
            }
        try:
            with self.lock:
                save_document(dest, doc, self.search_dirs())
        except TimelineError as e:
            return {"error": str(e)}
        return {"error": None, "name": name}

    # ---- runs -------------------------------------------------------------

    def _runs(self, name: str) -> list[Path]:
        if not self.data_dir or not (self.data_dir / "runs").is_dir():
            return []
        return find_runs(self.data_dir / "runs", name)

    def _load_run(self, path: Path):
        mtime = path.stat().st_mtime
        cached = self._run_cache.get(str(path))
        if cached and cached[0] == mtime:
            return cached[1], cached[2]
        run = read_run(path)
        rep = check_run(run)
        self._run_cache[str(path)] = (mtime, run, rep)
        return run, rep

    def latest_runs(self, name: str, baseline: str | None = None):
        """Latest run and the one before (ghost). A timeline with no runs of its
        own can name a baseline (e.g. the recording it was imported from)."""
        paths = self._runs(name)
        if not paths and isinstance(baseline, str) and NAME_RE.match(baseline):
            paths = self._runs(baseline)
        latest = self._load_run(paths[-1]) if paths else (None, None)
        ghost = self._load_run(paths[-2])[0] if len(paths) >= 2 else None
        return latest[0], latest[1], ghost, [p.name for p in paths]

    @staticmethod
    def run_payload(run, rep=None) -> dict | None:
        if run is None:
            return None
        r4 = lambda xs: [round(v, 4) for v in xs]
        out = {
            "file": run.path.name if run.path else None,
            "mtime": run.path.stat().st_mtime if run.path and run.path.exists() else 0,
            "status": run.header.get("status"),
            "hash": run.header.get("baked_hash"),
            "tick_interval": float(run.header.get("tick_interval", "0.015")),
            "x": r4(run.floats("x")), "y": r4(run.floats("y")), "z": r4(run.floats("z")),
            "vx": r4(run.floats("vx")), "vy": r4(run.floats("vy")), "vz": r4(run.floats("vz")),
            "hspeed": r4(run.floats("hspeed")),
            "yaw": r4(run.floats("yaw")),  # what was played (auto-strafe ticks: the game's own aim)
            "on_ground": run.ints("on_ground"), "ducked": run.ints("ducked"),
            # for the instant prediction: the inputs played, base velocity (pushes), the movement settings
            "fwd": r4(run.floats("forwardmove")), "side": r4(run.floats("sidemove")), "buttons": run.ints("buttons"),
            "base_vx": r4(run.floats("base_vx")), "base_vy": r4(run.floats("base_vy")), "base_vz": r4(run.floats("base_vz")),
            "move": _move_settings(run),
        }
        if rep is not None:
            out["kinds"] = rep.kinds
        return out

    # ---- analysis of the current (possibly unsaved) timeline ---------------

    def analyse(self, doc: dict, name: str) -> dict:
        path = self.timeline_path(name)
        tl = parse_timeline(doc, path=path, start_search_dirs=self.search_dirs())
        inputs = bake_inputs(tl)
        yaw_u = sample(tl.yaw, tl.length)
        r4 = lambda xs: [round(v, 4) for v in xs]
        preview = {
            "yaw_u": r4(yaw_u),
            "pitch": r4([ti.pitch for ti in inputs]),
            "fwd": [ti.forwardmove for ti in inputs],
            "side": [ti.sidemove for ti in inputs],
            "buttons": [ti.buttons for ti in inputs],
            "view_pitch": r4(sample(tl.view_pitch, tl.length)) if tl.view_pitch is not None else None,
        }
        run, rep, _, _ = self.latest_runs(tl.name, doc.get("_baseline"))
        analysis: dict = {"first_diff": None, "estimate": None, "best": None, "params": None}
        if run is not None and rep is not None:
            analysis["params"] = rep.params.__dict__
            analysis["first_diff"] = _first_difference(inputs, run)
            k = analysis["first_diff"]
            if k is not None:
                strafe = {t: (r.offset, r.heading, r.swing, r.gain) for r in tl.strafe for t in range(r.start, r.end)}
                est = estimate_free_flight(run, rep, k, inputs, strafe=strafe)
                if est:
                    analysis["estimate"] = {
                        "tick": [t for t, _, _ in est],
                        "x": [round(p[0], 4) for _, p, _ in est],
                        "y": [round(p[1], 4) for _, p, _ in est],
                        "z": [round(p[2], 4) for _, p, _ in est],
                        "hspeed": [round(math.hypot(v[0], v[1]), 4) for _, _, v in est],
                    }
            analysis["best"] = _best_yaws(run, rep, inputs, yaw_u)
        text = render_baked(tl, inputs)
        bake_hash = next(l for l in text.splitlines() if l.startswith("@hash ")).split()[1]
        return {"norm": normalized(tl), "preview": preview, "analysis": analysis, "warnings": tl.warnings,
                "hash": bake_hash}

    def open(self, name: str) -> dict:
        path = self.timeline_path(name)
        doc = json.loads(path.read_text(encoding="utf-8"))
        state = self.state(name, doc)
        state["doc"] = doc
        return state

    def state(self, name: str, doc: dict) -> dict:
        try:
            out = self.analyse(doc, name)
            out["error"] = None
        except TimelineError as e:
            out = {"error": str(e)}
        tl_name = (out.get("norm") or {}).get("name", name)
        run, rep, ghost, files = self.latest_runs(tl_name, doc.get("_baseline"))
        out["run"] = self.run_payload(run, rep)
        out["ghost"] = self.run_payload(ghost)
        out["runs"] = files
        out["pv"] = self.preview_payload(tl_name)
        return out

    def preview_payload(self, name: str) -> dict | None:
        """The latest fast engine preview of this timeline (runs/<name>.preview.csv)."""
        if not self.data_dir or not NAME_RE.match(name):
            return None
        path = self.data_dir / "runs" / f"{name}.preview.csv"
        if not path.is_file():
            return None
        run, rep = self._load_run(path)
        out = self.run_payload(run, rep)
        out["mtime"] = path.stat().st_mtime
        return out

    def update(self, name: str, doc: dict, edits: dict) -> dict:
        try:
            new_doc = apply_edits(doc, edits)
            out = self.analyse(new_doc, name)
            out["error"] = None
        except (TimelineError, KeyError, ValueError, TypeError, IndexError) as e:
            return {"error": str(e)}
        out["doc"] = new_doc
        return out

    def op(self, name: str, doc: dict, edits: dict, op: dict) -> dict:
        """A tick operation (insert, delete, length, turn, freeze) on the current
        edits; the answer is an update plus the new normalised timeline."""
        try:
            if op.get("op") == "freeze":
                new_edits, message = self._freeze(name, doc, edits, int(op["tick"]))
            elif op.get("op") == "splice":
                new_edits, message = self._splice(name, doc, edits, str(op["recording"]), int(op["at"]))
            else:
                new_edits, message = apply_op(edits, op)
        except (TimelineError, KeyError, ValueError, TypeError, IndexError) as e:
            return {"error": str(e)}
        out = self.update(name, doc, new_edits)
        if not out.get("error"):
            out["message"] = message
        return out

    def _freeze(self, name: str, doc: dict, edits: dict, tick: int):
        """Auto-strafe range at tick -> keyframes with the yaws the game played
        there, from a preview or run of exactly these inputs."""
        r = strafe_at(edits, tick)
        if r is None:
            raise TimelineError(f"freeze: tick {tick} isn't auto-strafe")
        current = self.analyse(apply_edits(doc, edits), name)
        tl_name = current["norm"]["name"]
        paths = []
        if self.data_dir:
            paths.append(self.data_dir / "runs" / f"{tl_name}.preview.csv")
            paths += list(reversed(self._runs(tl_name)))
        for path in paths:
            if not path.is_file():
                continue
            run, _ = self._load_run(path)
            if run.header.get("baked_hash") != current["hash"] or len(run.rows) < r["to"]:
                continue
            exact = "yaw_bits" in run.columns
            yaws = run.floats("yaw")[r["from"]:r["to"]]
            sides = run.floats("sidemove")[r["from"]:r["to"]] if r.get("heading") is not None else None
            fwds = run.floats("forwardmove")[r["from"]:r["to"]] if r.get("gain") is not None else None
            new = freeze_strafe(edits, r["from"], r["to"], yaws, sides, fwds)
            note = "" if exact else " (from 6-decimal yaws: replay to check, the game's plugin is older than 0.7)"
            return new, f"froze ticks {r['from']}..{r['to'] - 1} into keyframes from {path.name}{note}"
        raise TimelineError("freeze: no preview or run of these exact inputs yet; preview first (P), then freeze")

    def _splice(self, name: str, doc: dict, edits: dict, recording: str, at: int):
        """A recording made from tick `at` of this timeline, joined on from that tick."""
        if not self.data_dir or not NAME_RE.match(recording):
            raise TimelineError("splice: no such recording")
        rec = read_baked((self.data_dir / "baked" / f"{recording}.tas").read_text(encoding="utf-8")).inputs
        tl = parse_timeline(apply_edits(doc, edits), path=self.timeline_path(name), start_search_dirs=self.search_dirs())
        auto = {t for r in tl.strafe for t in range(r.start, r.end)}
        new = splice_recording(edits, at, rec, tl.move_speed, bake_inputs(tl), auto, tl.start.angles[0])
        # check: from `at` on, the new timeline bakes to exactly what was recorded
        baked = bake_inputs(parse_timeline(apply_edits(doc, new), path=self.timeline_path(name),
                                           start_search_dirs=self.search_dirs()))
        differ = sum(1 for a, b in zip(rec[at:], baked[at:])
                     if (float_to_bits(a.yaw), float_to_bits(a.pitch), a.forwardmove, a.sidemove, a.buttons)
                     != (float_to_bits(b.yaw), float_to_bits(b.pitch), b.forwardmove, b.sidemove, b.buttons))
        live = len(rec) - at
        note = "exact" if not differ else f"{differ} tick(s) differ from the recording"
        return new, f"joined your recording on from tick {at}: {live} live tick(s), {len(rec)} in all ({note})"

    def record(self, name: str, doc: dict, at: int) -> dict:
        """Ask the game to play this (unsaved) document on you up to tick `at`, then record you."""
        if not self.data_dir:
            return {"error": "cstrike_dir is not set in tasurf.local.json, so the editor can't reach the game"}
        try:
            tl = parse_timeline(doc, path=self.timeline_path(name), start_search_dirs=self.search_dirs())
        except TimelineError as e:
            return {"error": str(e)}
        if not 1 <= int(at) <= tl.length:
            return {"error": f"record from a tick between 1 and {tl.length} (from tick 0, use tas_record in the game)"}
        if why := self._plugin_too_old(tl):
            return {"error": why}
        if self._busy():
            return {"error": BUSY}
        return self._send_request(tl, f"{tl.name}.record", f"record {int(at)}")

    def save(self, name: str, doc: dict) -> dict:
        with self.lock:
            try:
                save_document(self.timeline_path(name), doc, self.search_dirs())
            except TimelineError as e:
                return {"error": str(e)}
        return {"error": None, "text": format_json(doc)}

    # ---- running in the game -----------------------------------------------

    def run(self, name: str, doc: dict) -> dict:
        if not self.data_dir:
            return {"error": "cstrike_dir is not set in tasurf.local.json, so the editor can't reach the game"}
        if self._busy():
            return {"error": BUSY}
        saved = self.save(name, doc)
        if saved["error"]:
            return saved
        tl = parse_timeline(doc, path=self.timeline_path(name), start_search_dirs=self.search_dirs())
        if why := self._plugin_too_old(tl):
            return {"error": why}
        return self._send_request(tl, tl.name, "run")

    def preview(self, name: str, doc: dict) -> dict:
        """Bake the current (unsaved) document and ask the game for a fast preview."""
        if not self.data_dir:
            return {"error": "cstrike_dir is not set in tasurf.local.json, so the editor can't reach the game"}
        if self._busy():
            return {"error": BUSY}
        try:
            tl = parse_timeline(doc, path=self.timeline_path(name), start_search_dirs=self.search_dirs())
        except TimelineError as e:
            return {"error": str(e)}
        if why := self._plugin_too_old(tl):
            return {"error": why}
        return self._send_request(tl, f"{tl.name}.preview", "preview")

    def _plugin_too_old(self, tl) -> str | None:
        """Why the game's plugin can't play this timeline: one with a view pitch needs 0.9 or
        later, one with straight auto-strafe 0.11, one with auto bhop 0.12. None when it can, or when
        the game hasn't said which version it runs."""
        straight = any(r.heading is not None for r in tl.strafe)
        swing = any(r.swing is not None for r in tl.strafe)
        gain = any(r.gain is not None for r in tl.strafe)
        if tl.view_pitch is None and not straight and not tl.autobhop:
            return None
        version = self.game_status().get("version") or ""
        try:
            have = tuple(int(x) for x in version.split(".")[:2])
        except ValueError:
            return None
        if gain and have < GAIN_PLUGIN:
            return (f"the game's TASurf plugin is {version}, which can't play a swing's gain (0.14 can): build and install "
                    f"it again (python -m tasurf build-plugin, then install; the desktop app's Launch does both), then type  "
                    f"sm plugins reload tasurf  in the game")
        if swing and have < SWING_PLUGIN:
            return (f"the game's TASurf plugin is {version}, which can't play a straight auto-strafe's swing (0.13 can): build "
                    f"and install it again (python -m tasurf build-plugin, then install; the desktop app's Launch does both), "
                    f"then type  sm plugins reload tasurf  in the game")
        if tl.autobhop and have < AUTOBHOP_PLUGIN:
            return (f"the game's TASurf plugin is {version}, which can't play auto bhop (0.12 can): build and "
                    f"install it again (python -m tasurf build-plugin, then install; the desktop app's Launch does both), "
                    f"then type  sm plugins reload tasurf  in the game")
        if straight and have < STRAIGHT_PLUGIN:
            return (f"the game's TASurf plugin is {version}, which can't play straight auto-strafe (0.11 can): build and "
                    f"install it again (python -m tasurf build-plugin, then install; the desktop app's Launch does both), "
                    f"then type  sm plugins reload tasurf  in the game")
        if tl.view_pitch is None or have >= VIEW_PITCH_PLUGIN:
            return None
        return (f"the game's TASurf plugin is {version}, which can't play a view pitch (0.9 can): build and install "
                f"it again (python -m tasurf build-plugin, then install), then type  sm plugins reload tasurf  in the game")

    def _camera(self, tl, digest: str) -> dict[int, float] | None:
        """The camera replays show (camera.py), from a finished run of exactly these inputs (the same hash: the
        preview the editor asks for after an edit, or a run), when the game's plugin can show it: smoothed on the
        auto-strafe ranges, or on the whole run (the timeline's camera_smoothing). None when it's off (a robotic
        look), with nothing to smooth, or before the game has played these inputs (the replay looks along the yaw
        played, as it always did)."""
        ranges = ([(0, tl.length)] if tl.camera_smoothing == "all" else
                  [(r.start, r.end) for r in tl.strafe] if tl.camera_smoothing == "strafe" else [])
        if not ranges or not self.data_dir:
            return None
        try:
            if tuple(int(x) for x in (self.game_status().get("version") or "").split(".")[:2]) < VIEW_YAW_PLUGIN:
                return None
        except ValueError:
            return None
        runs = self.data_dir / "runs"
        for path in [runs / f"{tl.name}.preview.csv"] + self._runs(tl.name)[::-1]:
            if not path.is_file():
                continue
            try:
                run, _ = self._load_run(path)
                yaws = run.floats("yaw")
            except (OSError, ValueError, KeyError):
                continue
            if run.header.get("baked_hash") == digest and run.header.get("status") == "complete" and len(yaws) >= tl.length:
                return camera_yaws(yaws[:tl.length], ranges)
        return None

    def _busy(self) -> bool:
        """A run request the game hasn't picked up yet. The plugin reads one
        request file, so a second one would replace the first. A pending preview
        may be replaced (a newer edit makes it stale anyway), and so may anything
        older than a minute (game closed?)."""
        pending = self.data_dir / "requests" / "request.txt"
        try:
            if time.time() - pending.stat().st_mtime >= 60:
                return False
            return "\npreview\n" not in pending.read_text(encoding="utf-8", errors="replace")
        except OSError:
            return False

    def _send_request(self, tl, baked_name: str, action: str) -> dict:
        with self.lock:
            out = self.out_dir / f"{baked_name}.tas"
            # replays show auto-strafe's camera smoothed, but not a recording's lead-in: you take over from the yaw played
            bake_to_file(tl, out, None if action.startswith("record") else lambda digest: self._camera(tl, digest))
            (self.data_dir / "baked").mkdir(parents=True, exist_ok=True)
            shutil.copyfile(out, self.data_dir / "baked" / out.name)
            req_id = uuid.uuid4().hex[:12]
            requests = self.data_dir / "requests"
            requests.mkdir(parents=True, exist_ok=True)
            pending = requests / "request.txt"
            try:  # a request the game hadn't taken yet is replaced
                old = pending.read_text(encoding="utf-8", errors="replace").split("\n", 1)[0]
                if old.startswith("id "):
                    self._replaced.add(old[3:].strip())
            except OSError:
                pass
            tmp = requests / "request.tmp"
            tmp.write_text(f"id {req_id}\nload {baked_name}\n{action}\n", encoding="utf-8", newline="\n")
            tmp.replace(pending)
            self._sent[req_id] = time.time()
        return {"error": None, "id": req_id}

    def change_map(self, map_name) -> dict:
        """Ask the game to change to this map (the editor opened a timeline on another one): requests/changemap.txt,
        which the plugin takes while no run plays. One older than 30 seconds is dropped, so a game started later
        doesn't act on it. The plugin answers in requests/<id>.result as for a run."""
        if not self.data_dir:
            return {"error": "cstrike_dir is not set in tasurf.local.json, so the editor can't reach the game"}
        if not isinstance(map_name, str) or not NAME_RE.match(map_name):
            return {"error": f"bad map name {map_name!r}"}
        req_id = uuid.uuid4().hex[:12]
        requests = self.data_dir / "requests"
        requests.mkdir(parents=True, exist_ok=True)
        with self.lock:
            tmp = requests / "changemap.tmp"
            tmp.write_text(f"id {req_id}\nmap {map_name}\n", encoding="utf-8", newline="\n")
            tmp.replace(requests / "changemap.txt")
            self._sent[req_id] = time.time()
        return {"error": None, "id": req_id}

    def draw(self, map_name: str, path: str | None, ghost: str | None, timeline: str | None = None,
             cursor: int | None = None, files: str | None = None, box: bool | None = None) -> dict:
        """Tell the game which run CSVs to draw (requests/draw.txt, kept in place so a
        plugin reload or map change draws them again). cursor: the editor's selected
        tick, drawn red up to it and white for a second after it. files: changes
        whenever the CSVs do, so the game reads them again."""
        if not self.data_dir:
            return {"error": "cstrike_dir is not set in tasurf.local.json"}
        for v in (map_name, path, ghost, timeline):
            if v is not None and not NAME_RE.match(v):
                return {"error": f"bad name {v!r}"}
        if cursor is not None and (not isinstance(cursor, int) or cursor < 0):
            return {"error": f"bad cursor {cursor!r}"}
        if files is not None and not str(files).isalnum():
            return {"error": f"bad files stamp {files!r}"}
        requests = self.data_dir / "requests"
        requests.mkdir(parents=True, exist_ok=True)
        lines = [f"id {uuid.uuid4().hex[:12]}", f"map {map_name}", f"path {path or '-'}", f"ghost {ghost or '-'}",
                 f"timeline {timeline or '-'}", f"cursor {'-' if cursor is None else cursor}", f"files {files or '-'}"]
        if box is not None:
            lines.append(f"box {1 if box else 0}")  # the player's box at the cursor (Visuals toolbox)
        with self.lock:
            tmp = requests / "draw.tmp"
            tmp.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
            tmp.replace(requests / "draw.txt")
        return {"error": None}

    def game_status(self) -> dict:
        """What the plugin last reported in requests/heartbeat.txt (written about once a second)."""
        if not self.data_dir:
            return {"ok": False, "detail": "cstrike_dir is not set in tasurf.local.json"}
        beat = self.data_dir / "requests" / "heartbeat.txt"
        if not beat.is_file():
            return {"ok": False, "detail": "the game hasn't reported in: start CS:S and load a map. If it's running, "
                                           "the loaded plugin is too old: type  sm plugins reload tasurf  in its console"}
        age = time.time() - beat.stat().st_mtime
        fields = {}
        for line in beat.read_text(encoding="utf-8", errors="replace").splitlines():
            key, _, value = line.strip().partition(" ")
            if key:
                fields[key] = value
        out = {"version": fields.get("version"), "map": fields.get("map"), "player": fields.get("player"),
               "state": fields.get("state"), "mode": fields.get("mode"), "bot": fields.get("bot"),
               "age": round(age, 1)}
        if age > 5:
            out.update(ok=False, detail="the game isn't responding (closed, loading a map, or the plugin was "
                                        "unloaded); if it's running, type  sm plugins reload tasurf")
        elif "ready" in fields:  # 0.6+: the plugin says whether it can play a request, and why not
            if fields["ready"] != "yes":
                out.update(ok=False, detail=fields.get("why") or "not ready")
            elif out["state"] != "idle":
                out.update(ok=True, detail=f"busy ({out['state']}); a new preview replaces a running one")
            else:
                out.update(ok=True, detail=f"ready: TASurf {out['version']} on {out['map']}")
        elif out["player"] != "alive":
            out.update(ok=False, detail={"none": "no player in the game yet",
                                         "spectator": "you're spectating: join a team",
                                         "dead": "you're dead: respawn"}.get(out["player"], "you need to be alive"))
        elif out["state"] != "idle":
            out.update(ok=True, detail=f"busy ({out['state']}); it will start when that's done")
        else:
            out.update(ok=True, detail=f"ready: TASurf {out['version']} on {out['map']}")
        return out

    def request_status(self, req_id: str) -> dict:
        if not self.data_dir or not req_id.isalnum():
            return {"state": "error", "detail": "no such request"}
        requests = self.data_dir / "requests"
        result = requests / f"{req_id}.result"
        if result.is_file():
            fields = {}
            for line in result.read_text(encoding="utf-8", errors="replace").splitlines():
                key, _, value = line.strip().partition(" ")
                if key:
                    fields[key] = value
            return {"state": "done", **fields}
        pending = requests / "request.txt"
        try:
            text = pending.read_text(encoding="utf-8", errors="replace")
        except OSError:
            text = ""
        if f"id {req_id}\n" in text:
            return {"state": "waiting", "detail": "waiting for the game: " + self.game_status()["detail"]}
        sent = self._sent.get(req_id)
        if req_id in self._replaced:
            return {"state": "done", "status": "superseded", "csv": "-", "detail": "a newer request replaced it"}
        beat = requests / "heartbeat.txt"
        try:
            beat_time = beat.stat().st_mtime
            idle = "\nstate idle\n" in beat.read_text(encoding="utf-8", errors="replace")
        except OSError:
            beat_time, idle = 0.0, False
        if sent is not None and idle and beat_time > sent + 2.5:
            # the game has been idle since well after this was sent, and never answered
            return {"state": "done", "status": "lost", "csv": "-", "detail": "the game dropped the request; try again"}
        return {"state": "running"}

    # ---- the 3D view (see bspmap.py) ---------------------------------

    def map_path(self, name: str) -> Path:
        cstrike = config.cstrike_dir(self.cfg)
        if not NAME_RE.match(name):
            raise MapError(f"bad map name {name!r}")
        if cstrike is None:
            raise MapError("cstrike_dir is not set in tasurf.local.json, so the editor can't find the map")
        path = find_map(cstrike, name)
        if path is None:
            raise MapError(f"no {name}.bsp in cstrike/maps or cstrike/download/maps")
        return path

    def _map(self, name: str, textures: bool) -> tuple[bytes, bytes | None]:
        path = self.map_path(name)
        mtime = path.stat().st_mtime
        cached = self._maps.get(str(path))
        if cached and cached[0] == mtime and (cached[2] is not None or not textures):
            return cached[1], cached[2]
        mesh, textured = read_map(path, textures)
        self._maps = {str(path): (mtime, mesh, textured)}  # one map at a time: they can be tens of MB
        return mesh, textured

    def map_mesh(self, name: str) -> bytes:
        """The map's geometry (TASM)."""
        return self._map(name, False)[0]

    def map_textures(self, name: str) -> bytes:
        """What it takes to draw the map's geometry textured and lit (TAST)."""
        return self._map(name, True)[1] or b""

    def map_triggers(self, name: str) -> bytes:
        """The map's triggers as polygons (TATR)."""
        return read_triggers(self.map_path(name))

    def map_clip(self, name: str) -> bytes:
        """What the player collides with (TACL), for the editor's instant prediction."""
        return read_clip(self.map_path(name))

    def map_props(self, name: str) -> dict:
        """The map's sky, 3D skybox camera and props (bspmap.read_props), for the 3D view."""
        return read_props(self.map_path(name))

    def map_file(self, name: str, rel: str) -> bytes:
        """A material or texture (materials/....vmt or .vtf), or a model's file (models/....mdl, .vvd or .vtx), as
        the game finds it on the map."""
        path = self.map_path(name)
        if self._files is None:
            self._files = GameFiles(config.cstrike_dir(self.cfg))
        return self._files.read(path, rel)

    def import_recording(self, recording: str, name: str) -> dict:
        if not self.data_dir:
            return {"error": "cstrike_dir is not set in tasurf.local.json"}
        if not NAME_RE.match(recording) or not NAME_RE.match(name):
            return {"error": "names may only use letters, digits, '_', '-' and '.'"}
        src = self.data_dir / "baked" / f"{recording}.tas"
        dest = self.timeline_path(name)
        if dest.exists():
            return {"error": f"timelines/{name}.json already exists; pick another name"}
        try:
            report = import_recording(src, dest, name)
        except (TimelineError, OSError, KeyError) as e:
            return {"error": str(e)}
        return {"error": None, "report": report, "name": name}


def _exact_angles(st) -> tuple[float, float]:
    """(pitch, yaw) of a start state, as the exact float32 values when it has them."""
    if st.angles_bits is not None:
        return bits_to_float(st.angles_bits[0]), bits_to_float(st.angles_bits[1])
    return st.angles[0], st.angles[1]


def _recording_info(name: str, head: str) -> dict:
    """Map and length from the start of a recording's baked file."""
    info = {"name": name, "map": None, "ticks": None}
    for line in head.splitlines():
        if line.startswith("@map "):
            info["map"] = line[5:].strip()
        elif line.startswith("@ticks "):
            try:
                info["ticks"] = int(line[7:].strip())
            except ValueError:
                pass
    return info


def _move_settings(run) -> dict | None:
    """The settings the player moves with in this run (from its header), or None when it lacks some."""
    try:
        p, _ = params_from_run(run)
        cvar = lambda name: float(run.header[f"cvar.{name}"].split()[0])
        out = {"dt": p.dt, "gravity": p.gravity, "airaccelerate": p.airaccelerate, "maxspeed": p.maxspeed,
               "maxvelocity": p.maxvelocity, "accelerate": cvar("sv_accelerate"), "friction": cvar("sv_friction"),
               "stopspeed": cvar("sv_stopspeed"), "stepsize": cvar("sv_stepsize"), "bounce": cvar("sv_bounce")}
    except (KeyError, ValueError, IndexError):
        return None
    try:
        out["bunnyhop"] = cvar("sv_enablebunnyhopping")  # 0: a jump cuts the speed to 1.1 x maxspeed
    except (KeyError, ValueError, IndexError):
        pass  # a game without it (the page then assumes no cut)
    return out


def _first_difference(inputs, run) -> int | None:
    """First tick whose input differs from what the run processed (None if the same)."""
    yaw, pitch = run.floats("yaw"), run.floats("pitch")
    fwd, side, btn = run.floats("forwardmove"), run.floats("sidemove"), run.ints("buttons")
    n = min(len(inputs), len(run.rows))
    for t in range(n):
        ti = inputs[t]
        dyaw = abs((ti.yaw - yaw[t] + 180.0) % 360.0 - 180.0)
        if (dyaw > 1e-5 or abs(ti.pitch - pitch[t]) > 1e-5 or abs(ti.forwardmove - fwd[t]) > 1e-4
                or abs(ti.sidemove - side[t]) > 1e-4 or ti.buttons != btn[t]):
            return t
    return None if len(inputs) == len(run.rows) else n


def _best_yaws(run, rep, inputs, yaw_u) -> dict:
    """For each tick airborne in the run: the yaw giving the most speed for the
    held keys, from the run's velocity before that tick."""
    vx, vy, ground = run.floats("vx"), run.floats("vy"), run.ints("on_ground")
    ticks, best, gain = [], [], []
    for t in range(1, min(len(inputs), len(run.rows))):
        ti = inputs[t]
        # ducked air ticks too: ducking crops wish speed on the ground only, and air
        # acceleration is capped at 30 either way
        if ground[t - 1] or ground[t]:
            continue
        res = best_yaw(vx[t - 1], vy[t - 1], ti.forwardmove, ti.sidemove, rep.params, yaw_u[t])
        if res is None:
            continue
        b, s_best, s_cur = res
        ticks.append(t)
        best.append(round(b, 4))
        gain.append(round(s_best - s_cur, 5))
    return {"tick": ticks, "yaw": best, "gain": gain}


class Handler(BaseHTTPRequestHandler):
    editor: Editor  # set on the class by serve()

    def log_message(self, fmt, *args):  # quiet
        pass

    def _send(self, status: int, body: bytes, ctype: str):
        self.send_response(status)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def _json(self, obj, status: int = 200):
        self._send(status, json.dumps(obj).encode("utf-8"), "application/json")

    def do_GET(self):
        url = urlparse(self.path)
        q = {k: v[0] for k, v in parse_qs(url.query).items()}
        try:
            if url.path in ("/", "/index.html"):
                self._send(200, (STATIC / "index.html").read_bytes(), "text/html; charset=utf-8")
            elif url.path in ("/view3d.js", "/predict.js"):  # 3D view, the instant prediction
                self._send(200, (STATIC / url.path[1:]).read_bytes(), "text/javascript; charset=utf-8")
            elif url.path in ("/api/map", "/api/maptex", "/api/maptrig", "/api/mapclip", "/api/mapfile"):  # 3D view
                ed, name = self.editor, q.get("name", "")
                try:
                    body = (ed.map_mesh(name) if url.path == "/api/map" else ed.map_textures(name)
                            if url.path == "/api/maptex" else ed.map_triggers(name)
                            if url.path == "/api/maptrig" else ed.map_clip(name)
                            if url.path == "/api/mapclip" else ed.map_file(name, q.get("path", "")))
                    self._send(200, body, "application/octet-stream")
                except MapError as e:
                    self._json({"error": str(e)}, HTTPStatus.NOT_FOUND)
            elif url.path == "/api/mapprops":  # 3D view: the sky and the props
                try:
                    self._json(self.editor.map_props(q.get("name", "")))
                except MapError as e:
                    self._json({"error": str(e)}, HTTPStatus.NOT_FOUND)
            elif url.path == "/api/list":
                self._json(self.editor.list())
            elif url.path == "/api/open":
                self._json(self.editor.open(q["name"]))
            elif url.path == "/api/thumb":
                self._json(self.editor.thumb(q["name"]))
            elif url.path == "/api/runs":
                name = q["name"]
                self._json({"runs": [p.name for p in self.editor._runs(name)]})
            elif url.path == "/api/request":
                self._json(self.editor.request_status(q.get("id", "")))
            elif url.path == "/api/game":
                self._json(self.editor.game_status())
            elif url.path == "/api/starts":
                self._json(self.editor.starts())
            else:
                self._send(404, b"not found", "text/plain")
        except (TimelineError, KeyError, OSError, json.JSONDecodeError) as e:
            self._json({"error": str(e)}, HTTPStatus.BAD_REQUEST)

    def do_POST(self):
        url = urlparse(self.path)
        try:
            length = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(length) or b"{}")
            ed = self.editor
            if url.path == "/api/update":
                self._json(ed.update(body["name"], body["doc"], body.get("edits", {})))
            elif url.path == "/api/op":
                self._json(ed.op(body["name"], body["doc"], body["edits"], body["op"]))
            elif url.path == "/api/state":
                out = ed.state(body["name"], body["doc"])
                out["doc"] = body["doc"]
                self._json(out)
            elif url.path == "/api/save":
                self._json(ed.save(body["name"], body["doc"]))
            elif url.path == "/api/run":
                self._json(ed.run(body["name"], body["doc"]))
            elif url.path == "/api/preview":
                self._json(ed.preview(body["name"], body["doc"]))
            elif url.path == "/api/record":
                self._json(ed.record(body["name"], body["doc"], int(body["at"])))
            elif url.path == "/api/draw":
                self._json(ed.draw(body["map"], body.get("path"), body.get("ghost"), body.get("timeline"),
                                   body.get("cursor"), body.get("files"), body.get("box")))
            elif url.path == "/api/changemap":
                self._json(ed.change_map(body.get("map")))
            elif url.path == "/api/import":
                self._json(ed.import_recording(body["recording"], body["name"]))
            elif url.path == "/api/create":
                self._json(ed.create(body["name"], body.get("start"), body.get("copy"), body.get("length"), body.get("map")))
            else:
                self._send(404, b"not found", "text/plain")
        except (TimelineError, KeyError, OSError, json.JSONDecodeError) as e:
            self._json({"error": str(e)}, HTTPStatus.BAD_REQUEST)


def make_server(editor: Editor, port: int) -> ThreadingHTTPServer:
    handler = type("BoundHandler", (Handler,), {"editor": editor})
    return ThreadingHTTPServer(("127.0.0.1", port), handler)


def serve(timeline: str | None, port: int, open_browser: bool) -> None:
    cfg = config.load()
    editor = Editor(config.REPO_ROOT / "timelines", cfg)
    server = make_server(editor, port)
    url = f"http://127.0.0.1:{server.server_address[1]}/"
    if timeline:
        url += f"#{Path(timeline).stem}"
    print(f"TASurf editor at {url}  (Ctrl+C to stop)")
    if not editor.data_dir:
        print("note: cstrike_dir is not set in tasurf.local.json; Run and recordings are disabled")
    if open_browser:
        webbrowser.open(url)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()

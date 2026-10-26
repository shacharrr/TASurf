"""Command line: python -m tasurf <command> ...

  bake          timeline JSON -> baked per-tick file (and copy into the game)
  verify        compare run CSVs bit for bit (determinism check)
  check         compare a run CSV against the reference air-movement model
  build-plugin  compile plugin/scripting/tasurf.sp with spcomp
  install       copy the compiled plugin and baked files into the game
  editor        open the browser editor (per-tick editing, path, speed, best yaw)
  import-recording  turn a tas_record recording into an editable timeline
"""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
from pathlib import Path

from . import config
from .bake import bake_to_file, summarize
from .physics import check_run
from .runcsv import compare_runs, find_runs, parse_run
from .timeline import TimelineError, load_timeline

DEFAULT_OUT = config.REPO_ROOT / "out" / "baked"
PLUGIN_SRC = config.REPO_ROOT / "plugin" / "scripting" / "tasurf.sp"
PLUGIN_OUT = config.REPO_ROOT / "plugin" / "plugins" / "tasurf.smx"


def _fail(msg: str) -> int:
    print(f"error: {msg}", file=sys.stderr)
    return 2


def cmd_bake(args, cfg) -> int:
    data_dir = config.game_data_dir(cfg)
    search = [data_dir / "starts"] if data_dir else []
    status = 0
    for tl_path in args.timelines:
        try:
            tl = load_timeline(Path(tl_path), start_search_dirs=search)
            out = Path(args.out_dir) / f"{tl.name}.tas"
            text, inputs = bake_to_file(tl, out)
        except (TimelineError, OSError) as e:
            print(f"error: {tl_path}: {e}", file=sys.stderr)
            status = 2
            continue
        for line in summarize(tl, inputs):
            print(line)
        for w in tl.warnings:
            print(f"  warning: {w}")
        digest = next(l for l in text.splitlines() if l.startswith("@hash ")).split()[1]
        print(f"  wrote {out} (hash {digest})")
        if data_dir and not args.no_install:
            dest = data_dir / "baked" / out.name
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(out, dest)
            print(f"  installed {dest}")
            print(f"  in game: tas_load {tl.name}   then   tas_run")
    return status


def cmd_verify(args, cfg) -> int:
    paths = [Path(p) for p in args.csv]
    if args.latest:
        runs_dir = Path(args.runs_dir) if args.runs_dir else (
            config.game_data_dir(cfg) / "runs" if config.game_data_dir(cfg) else None)
        if runs_dir is None:
            return _fail("--latest needs --runs-dir or cstrike_dir in tasurf.local.json")
        found = find_runs(runs_dir, args.latest)
        if len(found) < 2:
            return _fail(f"need at least 2 runs of {args.latest!r} in {runs_dir}, found {len(found)}")
        paths = found[-args.count:]
    if len(paths) < 2:
        return _fail("give at least two CSV files, or --latest NAME")

    texts = [p.read_text(encoding="utf-8") for p in paths]
    runs = [parse_run(t, p) for t, p in zip(texts, paths)]
    all_same = True
    print(f"reference: {paths[0]} ({len(runs[0].rows)} ticks, status {runs[0].header.get('status', '?')})")
    for p, t, r in zip(paths[1:], texts[1:], runs[1:]):
        cmp = compare_runs(runs[0], r, texts[0], t)
        if args.rows_only and cmp.header_diffs and not any(d.startswith("columns differ") for d in cmp.header_diffs):
            # e.g. a live recording (status "recorded") against its replay (status "complete")
            for d in cmp.header_diffs:
                print(f"  (header, ignored) {d}")
            cmp.header_diffs = []
            cmp.identical = cmp.first_diff_tick is None and cmp.length_note is None
            if cmp.identical:
                print(f"IDENTICAL ROWS: {p.name} matches {paths[0].name} tick for tick")
                continue
        all_same &= cmp.identical
        for line in cmp.lines(paths[0].name, p.name):
            print(line)
    return 0 if all_same else 1


def cmd_check(args, cfg) -> int:
    run = parse_run(Path(args.csv).read_text(encoding="utf-8"), Path(args.csv))
    try:
        rep = check_run(run)
    except KeyError as e:
        return _fail(str(e))
    for line in rep.lines():
        print(line)
    return 0 if not rep.mismatches else 1


def _find_spcomp(args, cfg) -> Path | None:
    candidates = []
    if args.spcomp:
        candidates.append(Path(args.spcomp))
    if config.spcomp(cfg):
        candidates.append(config.spcomp(cfg))
    roots = [config.REPO_ROOT / "third_party" / "sourcemod"]
    if config.cstrike_dir(cfg):
        roots.append(config.cstrike_dir(cfg))
    for root in roots:
        scripting = root / "addons" / "sourcemod" / "scripting"
        for exe in ("spcomp64.exe", "spcomp.exe", "spcomp64", "spcomp"):
            candidates.append(scripting / exe)
    return next((c for c in candidates if c.is_file()), None)


def cmd_build_plugin(args, cfg) -> int:
    spcomp = _find_spcomp(args, cfg)
    if spcomp is None:
        return _fail("spcomp not found. Pass --spcomp PATH, set TASURF_SPCOMP, or extract the SourceMod "
                     "package into third_party/sourcemod (see README).")
    include = spcomp.parent / "include"
    PLUGIN_OUT.parent.mkdir(parents=True, exist_ok=True)
    cmd = [str(spcomp), str(PLUGIN_SRC), f"-i{include}", f"-o{PLUGIN_OUT}"]
    print(" ".join(f'"{c}"' if " " in c else c for c in cmd))
    proc = subprocess.run(cmd, cwd=PLUGIN_SRC.parent)
    if proc.returncode != 0 or not PLUGIN_OUT.is_file():
        return _fail(f"spcomp failed (exit code {proc.returncode})")
    print(f"built {PLUGIN_OUT}")
    return 0


def cmd_install(args, cfg) -> int:
    cstrike = config.cstrike_dir(cfg)
    if cstrike is None:
        return _fail("set cstrike_dir in tasurf.local.json (or TASURF_CSTRIKE)")
    sm = cstrike / "addons" / "sourcemod"
    if not sm.is_dir():
        return _fail(f"{sm} does not exist; install Metamod:Source and SourceMod first (see README)")
    if not PLUGIN_OUT.is_file():
        return _fail(f"{PLUGIN_OUT} does not exist; run: python -m tasurf build-plugin")
    data = config.game_data_dir(cfg)
    for sub in ("baked", "starts", "runs"):
        (data / sub).mkdir(parents=True, exist_ok=True)
    dest = sm / "plugins" / PLUGIN_OUT.name
    shutil.copyfile(PLUGIN_OUT, dest)
    print(f"installed {dest}")
    session_cfg = config.REPO_ROOT / "plugin" / "cfg" / "tasurf_session.cfg"
    shutil.copyfile(session_cfg, cstrike / "cfg" / session_cfg.name)
    print(f"installed {cstrike / 'cfg' / session_cfg.name}")
    if not args.no_baked and DEFAULT_OUT.is_dir():
        for f in sorted(DEFAULT_OUT.glob("*.tas")):
            shutil.copyfile(f, data / "baked" / f.name)
            print(f"installed {data / 'baked' / f.name}")
    print("in game: sm plugins reload tasurf   (or change map)")
    return 0


def cmd_editor(args, cfg) -> int:
    from .editor import serve
    serve(args.timeline, args.port, not args.no_browser)
    return 0


def cmd_import_recording(args, cfg) -> int:
    from .editdoc import import_recording
    src = Path(args.recording)
    if not src.is_file():
        data = config.game_data_dir(cfg)
        cand = data / "baked" / f"{args.recording}.tas" if data else None
        if cand is None or not cand.is_file():
            return _fail(f"no recording {args.recording!r} (give a .tas path or a name from the game's baked folder)")
        src = cand
    name = args.name or src.stem.removesuffix("_rec") + "_edit"
    out = config.REPO_ROOT / "timelines" / f"{name}.json"
    if out.exists() and not args.force:
        return _fail(f"{out} exists (use --name or --force)")
    try:
        rep = import_recording(src, out, name, args.tolerance)
    except (TimelineError, OSError, KeyError) as e:
        return _fail(str(e))
    print(f"wrote {out}: {rep['ticks']} ticks, {rep['yaw_keys']} yaw keys, {rep['pitch_keys']} pitch keys, "
          f"{rep['key_ranges']} key ranges, move speed {rep['move_speed']:g}")
    if rep["exact"]:
        print("  exact: it bakes back to the recorded inputs bit for bit")
    else:
        print(f"  {rep['differing_ticks']} ticks differ from the recording:")
        for line in rep["first_differences"]:
            print(f"    {line}")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="python -m tasurf", description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("bake", help="bake timeline JSON files")
    p.add_argument("timelines", nargs="+")
    p.add_argument("-o", "--out-dir", default=str(DEFAULT_OUT))
    p.add_argument("--no-install", action="store_true", help="don't copy the baked file into the game")
    p.set_defaults(func=cmd_bake)

    p = sub.add_parser("verify", help="compare run CSVs bit for bit")
    p.add_argument("csv", nargs="*")
    p.add_argument("--latest", metavar="NAME", help="compare the newest runs of this timeline from the game's runs folder")
    p.add_argument("--count", type=int, default=2, help="with --latest: how many recent runs (default 2)")
    p.add_argument("--runs-dir")
    p.add_argument("--rows-only", action="store_true",
                   help="compare tick rows only; list header differences without failing "
                        "(e.g. a live recording against its replay)")
    p.set_defaults(func=cmd_verify)

    p = sub.add_parser("check", help="compare a run against the reference air-movement model")
    p.add_argument("csv")
    p.set_defaults(func=cmd_check)

    p = sub.add_parser("build-plugin", help="compile the SourceMod plugin")
    p.add_argument("--spcomp")
    p.set_defaults(func=cmd_build_plugin)

    p = sub.add_parser("install", help="copy plugin + baked files into the game")
    p.add_argument("--no-baked", action="store_true")
    p.set_defaults(func=cmd_install)

    p = sub.add_parser("editor", help="open the browser editor")
    p.add_argument("timeline", nargs="?", help="timeline to open (name or path)")
    p.add_argument("--port", type=int, default=8765, help="0 picks a free port")
    p.add_argument("--no-browser", action="store_true")
    p.set_defaults(func=cmd_editor)

    p = sub.add_parser("import-recording", help="turn a recording into an editable timeline")
    p.add_argument("recording", help="name in the game's baked folder (e.g. surf_beginner_rec) or a .tas path")
    p.add_argument("--name", help="timeline name (default: <recording>_edit)")
    p.add_argument("--tolerance", type=float, default=0.0,
                   help="degrees a yaw/pitch keyframe may deviate to save keys (0 = exact)")
    p.add_argument("--force", action="store_true")
    p.set_defaults(func=cmd_import_recording)

    args = ap.parse_args(argv)
    return args.func(args, config.load())


if __name__ == "__main__":
    sys.exit(main())

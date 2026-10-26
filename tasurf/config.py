"""Local machine configuration (game paths), kept out of version control.

``tasurf.local.json`` in the repo root, e.g.::

    {
      "cstrike_dir": "C:/Program Files (x86)/Steam/steamapps/common/Counter-Strike Source/cstrike",
      "spcomp": "third_party/sourcemod/addons/sourcemod/scripting/spcomp64.exe"
    }

The environment variables TASURF_CSTRIKE and TASURF_SPCOMP override the file.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CONFIG_PATH = REPO_ROOT / "tasurf.local.json"


def load() -> dict:
    cfg: dict = {}
    if CONFIG_PATH.is_file():
        cfg = json.loads(CONFIG_PATH.read_text(encoding="utf-8"))
    if os.environ.get("TASURF_CSTRIKE"):
        cfg["cstrike_dir"] = os.environ["TASURF_CSTRIKE"]
    if os.environ.get("TASURF_SPCOMP"):
        cfg["spcomp"] = os.environ["TASURF_SPCOMP"]
    return cfg


def _path(cfg: dict, key: str) -> Path | None:
    v = cfg.get(key)
    if not v:
        return None
    p = Path(v)
    return p if p.is_absolute() else REPO_ROOT / p


def cstrike_dir(cfg: dict) -> Path | None:
    return _path(cfg, "cstrike_dir")


def spcomp(cfg: dict) -> Path | None:
    return _path(cfg, "spcomp")


def game_data_dir(cfg: dict) -> Path | None:
    """addons/sourcemod/data/tasurf inside the game, where the plugin reads and writes."""
    c = cstrike_dir(cfg)
    return c / "addons" / "sourcemod" / "data" / "tasurf" if c else None

"""Keep the Python side and the SourcePawn plugin in agreement on file formats."""

import json
import re
import tempfile
import unittest
from pathlib import Path

from tasurf.bake import bake_inputs, read_baked, render_baked
from tasurf.timeline import load_timeline

from .helpers import COLUMNS

REPO = Path(__file__).resolve().parent.parent
SP = REPO / "plugin" / "scripting" / "tasurf"

# Exactly the shape Start_WriteJson (start.sp) produces, including -0.000000
# and a sign-bit pattern, as captured mid-air with a velocity.
PLUGIN_START_JSON = """{
  "format": "tasurf-start",
  "version": 1,
  "map": "surf_test",
  "origin": [-63.968750, 3615.968750, 1200.031250],
  "origin_bits": ["C27FE000", "4561FF80", "44960100"],
  "angles": [4.312500, 179.428711, 0.000000],
  "angles_bits": ["408A0000", "43336DC0", "00000000"],
  "velocity": [812.250000, -0.000000, -152.500000],
  "velocity_bits": ["444B1000", "80000000", "C3188000"],
  "base_velocity": [0.000000, 0.000000, 0.000000],
  "base_velocity_flag": false,
  "on_ground": false,
  "ground_entity": 0,
  "ducked": false,
  "view_offset_z": 64.000000,
  "gravity": 0.000000,
  "lagged_movement": 1.000000,
  "velocity_modifier": 1.000000,
  "stamina": 0.000000,
  "health": 100,
  "maxspeed": 250.000000
}
"""


class PluginContractTest(unittest.TestCase):
    def test_csv_columns_match_plugin(self):
        src = (SP / "record.sp").read_text(encoding="utf-8")
        m = re.search(r'#define CSV_COLUMNS "([^"]+)"', src)
        self.assertIsNotNone(m)
        self.assertEqual(m.group(1), COLUMNS)

    def test_plugin_parses_every_start_key_the_baker_writes(self):
        src = (SP / "baked.sp").read_text(encoding="utf-8")
        handled = set(re.findall(r'StrEqual\(parts\[1\], "([a-z_]+)"\)', src))
        with tempfile.TemporaryDirectory() as tmp:
            tl = self._timeline_with_plugin_start(Path(tmp))
            baked = read_baked(render_baked(tl, bake_inputs(tl)))
        self.assertTrue(set(baked.start) <= handled, set(baked.start) - handled)

    def test_plugin_start_file_bakes_exactly(self):
        with tempfile.TemporaryDirectory() as tmp:
            tl = self._timeline_with_plugin_start(Path(tmp))
            baked = read_baked(render_baked(tl, bake_inputs(tl)))
        self.assertEqual(baked.start["origin_bits"], ["C27FE000", "4561FF80", "44960100"])
        self.assertEqual(baked.start["velocity_bits"], ["444B1000", "80000000", "C3188000"])
        self.assertEqual(baked.start["on_ground"], ["no"])
        self.assertEqual(baked.start["gravity"], ["0.000000"])
        self.assertEqual(baked.start["health"], ["100"])
        self.assertEqual(baked.start["maxspeed"], ["250.000000"])
        # pitch defaults to the captured pitch
        self.assertTrue(all(abs(t.pitch - 4.3125) < 1e-9 for t in baked.inputs))

    def _timeline_with_plugin_start(self, tmp: Path):
        (tmp / "cap.json").write_text(PLUGIN_START_JSON)
        tl = {"format": "tasurf-timeline", "version": 1, "name": "c", "map": "surf_test", "length": 3,
              "start": "cap", "yaw": {"keys": [[0, 179.428711]]}}
        (tmp / "c.json").write_text(json.dumps(tl))
        return load_timeline(tmp / "c.json")


if __name__ == "__main__":
    unittest.main()

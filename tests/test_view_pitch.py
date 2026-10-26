"""The view pitch: a track replays show, set on the replay bot's eyes, that never changes the run."""

import copy
import json
import re
import tempfile
import unittest
from pathlib import Path

from tasurf.bake import VIEW_PITCH_PER_LINE, TickInput, bake_inputs, read_baked, render_baked
from tasurf.editdoc import apply_edits, apply_op, normalized, splice_recording
from tasurf.editor import Editor
from tasurf.interp import sample
from tasurf.timeline import TimelineError, float_to_bits, parse_timeline

REPO = Path(__file__).resolve().parent.parent
SP = REPO / "plugin" / "scripting"

BASE = {
    "format": "tasurf-timeline", "version": 1, "name": "vp", "map": "surf_test", "length": 40,
    "start": {"origin": [1.0, 2.0, 3.0], "angles": [4.5, 90.0, 0.0]},
    "yaw": {"keys": [[0, 90.0], [20, 130.0, "smooth"], [39, 100.0]]},
    "pitch": {"keys": [[0, 4.5]]},
    "keys": [{"key": "W", "from": 0, "to": 30}],
}
VIEW = {"keys": [[0, 2.0], [10, 18.25, "smooth"], [25, -12.5], [39, -0.004]]}


def baked(doc):
    tl = parse_timeline(doc)
    return render_baked(tl, bake_inputs(tl))


def edits_of(doc):
    keep = ("length", "yaw", "pitch", "keys", "strafe", "view_pitch")
    return {k: v for k, v in normalized(parse_timeline(doc)).items() if k in keep}


class BakeTest(unittest.TestCase):
    def test_lines_values_and_hash(self):
        plain, viewed = baked(BASE), baked(dict(BASE, view_pitch=VIEW))
        lines = [l for l in viewed.splitlines() if l.startswith("@view_pitch ")]
        self.assertEqual([l.split()[1] for l in lines], ["0", "16", "32"])  # 16 values a line, from tick 0
        self.assertEqual(len(lines[2].split()) - 2, 40 - 32)
        want = sample(parse_timeline(dict(BASE, view_pitch=VIEW)).view_pitch, 40)
        got = [float(v) for l in lines for v in l.split()[2:]]
        self.assertEqual(got, [round(v, 2) + 0.0 for v in want])
        self.assertNotIn("-0.00", viewed)  # -0.004 is written 0.00
        # the run is the same: same tick lines, same hash, so runs still compare and previews stay fresh
        self.assertEqual([l for l in viewed.splitlines() if not l.startswith("@view_pitch ")], plain.splitlines())
        b = read_baked(viewed)
        self.assertEqual(b.view_pitch, got)
        self.assertEqual(b.hash, read_baked(plain).hash)
        self.assertIsNone(read_baked(plain).view_pitch)
        self.assertEqual(VIEW_PITCH_PER_LINE, 16)

    def test_bad_files(self):
        text = baked(dict(BASE, view_pitch=VIEW))
        with self.assertRaises(TimelineError) as e:
            read_baked(text.replace("@view_pitch 16 ", "@view_pitch 17 "))
        self.assertIn("expected @view_pitch 16", str(e.exception))
        with self.assertRaises(TimelineError) as e:
            read_baked("\n".join(l for l in text.splitlines() if not l.startswith("@view_pitch 32 ")) + "\n")
        self.assertIn("32 values for 40 ticks", str(e.exception))
        # it isn't hashed, so editing it by hand is fine; the tick lines still are
        read_baked(text.replace("@view_pitch 0 2.00", "@view_pitch 0 3.00"))
        with self.assertRaises(TimelineError):
            read_baked(text.replace("\n5,", "\n5,1"))

    def test_track_rules(self):
        tl = parse_timeline(dict(BASE, view_pitch={"keys": [[0, 95.0]]}))
        self.assertIn("view_pitch key at tick 0 is 95.0", " ".join(tl.warnings))
        for bad, why in (({"keys": []}, "view_pitch.keys"), ({"keys": [[50, 1.0]]}, "outside the timeline"),
                         ({"keys": [[0, 1.0, "zigzag"]]}, "interp must be")):
            with self.assertRaises(TimelineError) as e:
                parse_timeline(dict(BASE, view_pitch=bad))
            self.assertIn(why, str(e.exception))
        self.assertIsNone(parse_timeline(BASE).view_pitch)


class EditTest(unittest.TestCase):
    def test_edits_set_and_remove_it(self):
        doc = apply_edits(BASE, {"view_pitch": [[0, 1.5, "linear"], [30, 9.0, "linear"]]})
        self.assertEqual(doc["view_pitch"], {"keys": [[0, 1.5, "linear"], [30, 9.0, "linear"]]})
        self.assertEqual(normalized(parse_timeline(doc))["view_pitch"], [[0, 1.5, "linear"], [30, 9.0, "linear"]])
        self.assertIsNone(normalized(parse_timeline(BASE))["view_pitch"])
        self.assertNotIn("view_pitch", apply_edits(doc, {"view_pitch": None}))
        self.assertEqual(apply_edits(doc, {"length": 41})["view_pitch"], doc["view_pitch"])  # left alone

    def test_tick_operations_move_it_with_the_rest(self):
        doc = dict(BASE, view_pitch=VIEW)
        edits = edits_of(doc)
        old = sample(parse_timeline(doc).view_pitch, 40)
        ins, _ = apply_op(edits, {"op": "insert", "at": 12, "count": 5})
        new = sample(parse_timeline(apply_edits(doc, ins)).view_pitch, 45)
        self.assertEqual(new[:12], old[:12])
        self.assertEqual(new[12:17], [old[12]] * 5)
        for t in range(12, 40):
            self.assertAlmostEqual(new[t + 5], old[t], places=9)
        dele, _ = apply_op(edits, {"op": "delete", "from": 5, "to": 15})
        new = sample(parse_timeline(apply_edits(doc, dele)).view_pitch, 30)
        self.assertEqual(new[:5], old[:5])
        for t in range(15, 40):
            self.assertAlmostEqual(new[t - 10], old[t], places=9)
        turn, _ = apply_op(edits, {"op": "turn", "from": 2, "to": 8, "rate": 1.0})
        self.assertEqual(turn["view_pitch"], edits["view_pitch"])
        none, _ = apply_op(edits_of(BASE), {"op": "insert", "at": 3, "count": 2})
        self.assertIsNone(none["view_pitch"])

    def test_a_joined_recording_shows_where_you_looked(self):
        doc = dict(BASE, view_pitch=VIEW)
        cur = bake_inputs(parse_timeline(doc))
        rec = [TickInput(t, ti.yaw, ti.pitch, ti.forwardmove, ti.sidemove, ti.buttons) for t, ti in enumerate(cur[:20])]
        rec += [TickInput(t, 100.0, -3.0 + 0.25 * t, 0.0, 0.0, 0) for t in range(20, 50)]
        new = splice_recording(edits_of(doc), 20, rec, 400.0, cur, set(), 4.5)
        view = sample(parse_timeline(apply_edits(doc, new)).view_pitch, 50)
        self.assertEqual(view[:20], sample(parse_timeline(doc).view_pitch, 20))
        self.assertEqual([float_to_bits(v) for v in view[20:]], [float_to_bits(-3.0 + 0.25 * t) for t in range(20, 50)])
        self.assertIsNone(splice_recording(edits_of(BASE), 20, rec, 400.0, bake_inputs(parse_timeline(BASE)),
                                           set(), 4.5)["view_pitch"])


class EditorTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        (root / "timelines").mkdir()
        self.data = root / "cstrike" / "addons" / "sourcemod" / "data" / "tasurf"
        (self.data / "requests").mkdir(parents=True)
        self.ed = Editor(root / "timelines", {"cstrike_dir": str(root / "cstrike")}, out_dir=root / "out")

    def tearDown(self):
        self.tmp.cleanup()

    def beat(self, version):
        (self.data / "requests" / "heartbeat.txt").write_text(
            f"version {version}\nmap surf_test\nplayer alive\nmode edit\nbot ready\nstate idle\nready yes\ntime 0\n")

    def test_preview_data_and_hash(self):
        doc = dict(BASE, view_pitch=VIEW)
        a, b = self.ed.analyse(BASE, "vp"), self.ed.analyse(doc, "vp")
        self.assertIsNone(a["preview"]["view_pitch"])
        self.assertEqual(len(b["preview"]["view_pitch"]), 40)
        self.assertEqual(b["preview"]["view_pitch"][10], 18.25)
        self.assertEqual(a["hash"], b["hash"])  # the run's inputs didn't change
        self.assertEqual(b["norm"]["view_pitch"], normalized(parse_timeline(doc))["view_pitch"])

    def test_an_older_plugin_is_told_to_update(self):
        doc = dict(BASE, view_pitch=VIEW)
        self.beat("0.8.0")
        for r in (self.ed.run("vp", doc), self.ed.preview("vp", doc), self.ed.record("vp", doc, 5)):
            self.assertIn("plugin is 0.8.0, which can't play a view pitch", r["error"])
        self.assertIsNone(self.ed.preview("vp", BASE)["error"])  # without one, 0.8 plays it as before
        (self.data / "requests" / "request.txt").unlink()
        self.beat("0.9.0")
        self.assertIsNone(self.ed.run("vp", doc)["error"])
        text = (self.data / "baked" / "vp.tas").read_text()
        self.assertEqual(read_baked(text).view_pitch[10], 18.25)
        (self.data / "requests" / "request.txt").unlink()
        (self.data / "requests" / "heartbeat.txt").unlink()  # no game: nothing to check against
        self.assertIsNone(self.ed.preview("vp", doc)["error"])


class PluginTest(unittest.TestCase):
    def test_plugin_reads_and_applies_it(self):
        baked_sp = (SP / "tasurf" / "baked.sp").read_text(encoding="utf-8")
        self.assertIn('strncmp(line, "@view_pitch ", 12) == 0', baked_sp)
        self.assertIn(f"#define VIEW_PITCH_PER_LINE {VIEW_PITCH_PER_LINE} ", baked_sp)
        m = re.search(r"char viewParts\[VIEW_PITCH_PER_LINE \+ (\d+)\]\[(\d+)\]", baked_sp)
        self.assertIsNotNone(m)
        self.assertGreaterEqual(int(m.group(1)), 3)  # the tag, the tick, the values, one to notice too many
        self.assertGreaterEqual(int(m.group(2)), len("-89.00") + 1)
        main = (SP / "tasurf.sp").read_text(encoding="utf-8")
        self.assertIn('"m_angEyeAngles[0]"', main)
        version = re.search(r'#define TAS_VERSION "(\d+)\.(\d+)\.\d+"', main)
        self.assertTrue(version and (int(version[1]), int(version[2])) >= (0, 9), "the plugin plays @view_pitch from 0.9")

    def test_a_long_line_fits_the_plugins_buffer(self):
        doc = dict(BASE, length=64, view_pitch={"keys": [[0, -88.99], [63, -88.99]]})
        longest = max(len(l) for l in baked(doc).splitlines())
        self.assertLess(longest, 512)  # char line[512] in Baked_Load


if __name__ == "__main__":
    unittest.main()

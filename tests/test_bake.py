import copy
import json
import tempfile
import unittest
from pathlib import Path

from tasurf import buttons
from tasurf.bake import bake_inputs, read_baked, render_baked
from tasurf.timeline import TimelineError, bits_to_float, float_to_bits, load_timeline, parse_timeline

REPO = Path(__file__).resolve().parent.parent

BASE = {
    "format": "tasurf-timeline",
    "version": 1,
    "name": "t",
    "map": "surf_test",
    "length": 20,
    "start": {"origin": [1.0, 2.0, 3.0], "angles": [0.0, 90.0, 0.0]},
    "yaw": {"keys": [[0, 90.0]]},
}


def timeline(**changes):
    d = copy.deepcopy(BASE)
    d.update(changes)
    return parse_timeline(d)


class KeyRangeTest(unittest.TestCase):
    def test_to_is_exclusive(self):
        tl = timeline(keys=[{"key": "D", "from": 5, "to": 8}])
        ins = bake_inputs(tl)
        held = [t.tick for t in ins if t.buttons & buttons.IN_MOVERIGHT]
        self.assertEqual(held, [5, 6, 7])
        self.assertEqual([t.sidemove for t in ins[4:9]], [0.0, 400.0, 400.0, 400.0, 0.0])

    def test_list_shorthand(self):
        a = bake_inputs(timeline(keys=[["a", 2, 4]]))
        b = bake_inputs(timeline(keys=[{"key": "moveleft", "from": 2, "to": 4}]))
        self.assertEqual(a, b)
        self.assertEqual(a[2].sidemove, -400.0)

    def test_opposite_keys_cancel_like_a_real_client(self):
        ins = bake_inputs(timeline(keys=[["W", 0, 10], ["S", 5, 10], ["A", 0, 10], ["D", 0, 10]]))
        self.assertEqual(ins[0].forwardmove, 400.0)
        self.assertEqual(ins[5].forwardmove, 0.0)
        self.assertEqual(ins[5].sidemove, 0.0)
        self.assertEqual(ins[5].buttons, buttons.IN_FORWARD | buttons.IN_BACK | buttons.IN_MOVELEFT | buttons.IN_MOVERIGHT)

    def test_value_and_move_speed(self):
        ins = bake_inputs(timeline(input={"move_speed": 450}, keys=[["W", 0, 2], {"key": "D", "from": 0, "to": 2, "value": 123.5}]))
        self.assertEqual((ins[0].forwardmove, ins[0].sidemove), (450.0, 123.5))

    def test_non_movement_buttons(self):
        ins = bake_inputs(timeline(keys=[["jump", 3, 4], ["duck", 3, 6], ["walk", 0, 1]]))
        self.assertEqual(ins[3].buttons, buttons.IN_JUMP | buttons.IN_DUCK)
        self.assertEqual(ins[4].buttons, buttons.IN_DUCK)
        self.assertEqual(ins[0].buttons, buttons.IN_SPEED)
        self.assertEqual(ins[3].forwardmove, 0.0)

    def test_errors(self):
        cases = [
            ([["D", 5, 5]], "must be >"),
            ([["D", 0, 21]], "<= length"),
            ([["D", -1, 3]], "outside the timeline"),
            ([["X", 0, 3]], "unknown key"),
            ([["D", 0, 5], ["d", 4, 8]], "overlaps"),
            ([{"key": "jump", "from": 0, "to": 1, "value": 3}], "only movement keys"),
            ([{"key": "D", "form": 0, "to": 1}], "unknown field"),
        ]
        for keys, msg in cases:
            with self.subTest(keys=keys):
                with self.assertRaisesRegex(TimelineError, msg):
                    timeline(keys=keys)

    def test_adjacent_ranges_of_same_key_are_fine(self):
        ins = bake_inputs(timeline(keys=[["D", 0, 5], ["D", 5, 10]]))
        self.assertTrue(all(t.buttons & buttons.IN_MOVERIGHT for t in ins[:10]))


class TrackTest(unittest.TestCase):
    def test_yaw_is_wrapped_after_interpolating_unwrapped(self):
        tl = timeline(yaw={"keys": [[0, 170.0], [10, 190.0]]})
        yaws = [t.yaw for t in bake_inputs(tl)]
        self.assertEqual(yaws[0], 170.0)
        self.assertEqual(yaws[5], 180.0)  # (-180, 180]: 180 stays 180
        self.assertEqual(yaws[10], -170.0)
        self.assertTrue(all(-180.0 < y <= 180.0 for y in yaws))

    def test_pitch_defaults_to_start_pitch(self):
        d = copy.deepcopy(BASE)
        d["start"]["angles"] = [12.5, 90.0, 0.0]
        self.assertTrue(all(t.pitch == 12.5 for t in bake_inputs(parse_timeline(d))))

    def test_pitch_track(self):
        tl = timeline(pitch={"keys": [{"tick": 0, "value": 0.0}, {"tick": 10, "value": 10.0}]})
        self.assertEqual(bake_inputs(tl)[5].pitch, 5.0)

    def test_track_errors(self):
        for yaw, msg in [
            ({"keys": []}, "non-empty"),
            ({"keys": [[0, 1.0], [0, 2.0]]}, "greater than"),
            ({"keys": [[25, 1.0]]}, "outside"),
            ({"keys": [[0, 1.0, "bezier"]]}, "interp must be"),
            ({"keys": [[0, 1.0]], "default_interp": "nope"}, "default_interp"),
        ]:
            with self.subTest(yaw=yaw):
                with self.assertRaisesRegex(TimelineError, msg):
                    timeline(yaw=yaw)

    def test_long_way_turn_warns(self):
        tl = timeline(yaw={"keys": [[0, 170.0], [10, -170.0]]})
        self.assertTrue(any("190.0" in w for w in tl.warnings), tl.warnings)


class StartTest(unittest.TestCase):
    def test_bits_round_trip(self):
        for x in (0.0, 1.0, -31.0, 3616.0, 0.1, -1234.5678, 1e-7):
            self.assertEqual(float_to_bits(bits_to_float(float_to_bits(x))), float_to_bits(x))
        self.assertEqual(float_to_bits(-0.0), "00000000")

    def test_captured_bits_must_match_decimal(self):
        d = copy.deepcopy(BASE)
        d["start"]["origin_bits"] = [float_to_bits(1.0), float_to_bits(2.0), float_to_bits(3.0)]
        parse_timeline(d)  # consistent: fine
        d["start"]["origin"] = [1.0, 2.0, 4.0]  # edited by hand, bits stale
        with self.assertRaisesRegex(TimelineError, "delete the matching"):
            parse_timeline(d)

    def test_bits_win_in_baked_file(self):
        d = copy.deepcopy(BASE)
        exact = 3.0000002384185791  # next float32 above 3.0
        d["start"]["origin"] = [1.0, 2.0, 3.0]
        d["start"]["origin_bits"] = [float_to_bits(1.0), float_to_bits(2.0), float_to_bits(exact)]
        tl = parse_timeline(d)
        baked = read_baked(render_baked(tl, bake_inputs(tl)))
        self.assertEqual(baked.start["origin_bits"][2], float_to_bits(exact))

    def test_cs_slowdowns_always_reset_to_rested(self):
        # Regression: a hand-written start without stamina let a jump just before
        # tas_run slow the next run's walking (runs diverged from tick 1).
        tl = timeline()
        baked = read_baked(render_baked(tl, bake_inputs(tl)))
        self.assertEqual(baked.start["stamina"], ["0.000000"])
        self.assertEqual(baked.start["velocity_modifier"], ["1.000000"])

    def test_ducked_start_rejected(self):
        d = copy.deepcopy(BASE)
        d["start"]["ducked"] = True
        with self.assertRaisesRegex(TimelineError, "ducked"):
            parse_timeline(d)

    def test_start_map_mismatch(self):
        d = copy.deepcopy(BASE)
        d["start"]["map"] = "surf_other"
        with self.assertRaisesRegex(TimelineError, "captured on map"):
            parse_timeline(d)

    def test_start_file_reference_and_search_dirs(self):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = Path(tmp)
            game_starts = tmp / "game_starts"
            game_starts.mkdir()
            start = {"format": "tasurf-start", "version": 1, "map": "surf_test",
                     "origin": [5.0, 6.0, 7.0], "angles": [0.0, 45.0, 0.0], "on_ground": True,
                     "ground_entity": 0, "maxspeed": 250.0}
            (game_starts / "surf_test.json").write_text(json.dumps(start))
            d = copy.deepcopy(BASE)
            d["start"] = "surf_test"
            tl_path = tmp / "timelines" / "t.json"
            tl_path.parent.mkdir()
            tl_path.write_text(json.dumps(d))
            with self.assertRaisesRegex(TimelineError, "could not find start file"):
                load_timeline(tl_path)
            tl = load_timeline(tl_path, start_search_dirs=[game_starts])
            self.assertEqual(tl.start.origin, (5.0, 6.0, 7.0))
            self.assertEqual(tl.start.on_ground, "yes")
            baked = read_baked(render_baked(tl, bake_inputs(tl)))
            self.assertEqual(baked.start["on_ground"], ["yes"])
            self.assertEqual(baked.start["maxspeed"], ["250.000000"])


class BakedFileTest(unittest.TestCase):
    def test_example_timeline_bakes_and_reads_back(self):
        tl = load_timeline(REPO / "timelines" / "example_surf.json")
        inputs = bake_inputs(tl)
        text = render_baked(tl, inputs)
        baked = read_baked(text)
        self.assertEqual(baked.name, "example_surf")
        self.assertEqual(baked.map, "surf_beginner")
        self.assertEqual(len(baked.inputs), tl.length)
        self.assertEqual(baked.settings["sv_airaccelerate"], "150")
        self.assertEqual(baked.settings["tickrate"], "66.6667")
        for a, b in zip(inputs, baked.inputs):
            # the file holds exactly the float32 the plugin will use
            self.assertEqual(float_to_bits(b.yaw), float_to_bits(a.yaw))
            self.assertEqual(a.buttons, b.buttons)

    def test_baking_is_deterministic(self):
        tl1 = load_timeline(REPO / "timelines" / "example_surf.json")
        tl2 = load_timeline(REPO / "timelines" / "example_surf.json")
        self.assertEqual(render_baked(tl1, bake_inputs(tl1)), render_baked(tl2, bake_inputs(tl2)))

    def test_hash_changes_with_content(self):
        a = timeline(keys=[["D", 0, 5]])
        b = timeline(keys=[["D", 0, 6]])
        ha = read_baked(render_baked(a, bake_inputs(a))).hash
        hb = read_baked(render_baked(b, bake_inputs(b))).hash
        self.assertNotEqual(ha, hb)

    def test_hand_edit_is_detected(self):
        tl = timeline()
        text = render_baked(tl, bake_inputs(tl)).replace("\n3,", "\n3,1", 1)
        with self.assertRaisesRegex(TimelineError, "hash"):
            read_baked(text)

    def test_tick_lines_have_six_columns_and_no_negative_zero(self):
        tl = timeline(yaw={"keys": [[0, -0.0]]})
        text = render_baked(tl, bake_inputs(tl))
        rows = [l for l in text.splitlines() if l and l[0].isdigit()]
        self.assertEqual(len(rows), 20)
        for r in rows:
            self.assertEqual(len(r.split(",")), 6)
            self.assertNotIn("-0.000000", r)

    def test_comments_are_ignored(self):
        tl = timeline(_comment="hi", keys=[{"_comment": "section"}, {"key": "D", "from": 0, "to": 1, "_why": "x"}])
        self.assertEqual(len(tl.keys), 1)

    def test_top_level_typo_rejected(self):
        with self.assertRaisesRegex(TimelineError, "unknown field"):
            timeline(lenght=5)


if __name__ == "__main__":
    unittest.main()

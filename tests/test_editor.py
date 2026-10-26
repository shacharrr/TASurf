"""Editor back end: best yaw, estimates, document edits, recording import, HTTP API."""

import json
import math
import os
import re
import struct
import time
import tempfile
import threading
import unittest
import urllib.request
from pathlib import Path

from tasurf import buttons
from tasurf.bake import TickInput, BakedFile, bake_inputs, read_baked, render_baked
from tasurf.camera import CAM_MARGIN
from tasurf.editdoc import (apply_edits, apply_op, format_json, freeze_strafe, normalized, recording_to_timeline,
                            splice_recording)
from tasurf.editor import Editor, make_server
from tasurf.physics import (AirParams, GroundParams, best_yaw, check_run, estimate_free_flight, ground_accelerate,
                            predict_air_tick, smooth_swing_input, speed_after_air_accel, straight_sidemove, strafe_yaw,
                            swing_close)
from tasurf.runcsv import parse_run
from tasurf.timeline import TimelineError, float_to_bits, parse_timeline

from .helpers import f32, make_csv, row
from .test_physics import SURF, simulate

REPO = Path(__file__).resolve().parent.parent


def f32(v):
    return struct.unpack("<f", struct.pack("<f", v))[0]


def scan_best(vx, vy, fmove, smove, p):
    best = max((speed_after_air_accel(vx, vy, a / 100.0, fmove, smove, p), a / 100.0) for a in range(-18000, 18000))
    return best


class BestYawTest(unittest.TestCase):
    def test_surf_airaccel_strafe_is_perpendicular(self):
        yaw, s_best, s_cur = best_yaw(500.0, 0.0, 0.0, 400.0, SURF, current_yaw=5.0)  # holding D
        self.assertAlmostEqual(yaw, 0.0, places=6)  # look along the velocity, strafe right
        self.assertAlmostEqual(s_best, math.sqrt(500.0 ** 2 + 900.0), places=9)
        self.assertLess(s_cur, s_best)

    def test_matches_brute_force(self):
        low = AirParams(dt=0.015, gravity=800.0, airaccelerate=5.0, maxspeed=250.0, maxvelocity=3500.0)
        for p in (SURF, low):
            for vx, vy, f, s in [(500, 0, 0, 400), (-300, 800, 0, -400), (900, -120, 400, 400), (40, 10, 400, 0),
                                 (1200, 1200, -400, 400)]:
                with self.subTest(airaccel=p.airaccelerate, v=(vx, vy), keys=(f, s)):
                    yaw, s_best, _ = best_yaw(vx, vy, f, s, p, current_yaw=0.0)
                    scan_speed, _ = scan_best(vx, vy, f, s, p)
                    self.assertGreaterEqual(s_best, scan_speed - 1e-7)

    def test_unwrapped_near_current_and_no_keys(self):
        yaw, _, _ = best_yaw(500.0, 0.0, 0.0, 400.0, SURF, current_yaw=725.0)
        self.assertAlmostEqual(yaw, 720.0, places=6)
        self.assertIsNone(best_yaw(500.0, 0.0, 0.0, 0.0, SURF, current_yaw=0.0))


class EstimateTest(unittest.TestCase):
    def test_estimate_follows_new_inputs_from_the_edit(self):
        rows = simulate(30, lambda t: -1.0 * t, 400.0)
        run = parse_run(make_csv(rows))
        rep = check_run(run)
        new = [TickInput(t, -1.0 * t if t < 12 else -2.0 * t, 0.0, 0.0, 400.0, 1024) for t in range(30)]
        est = estimate_free_flight(run, rep, 12, new)
        self.assertEqual([t for t, _, _ in est], list(range(12, 30)))
        # same as stepping the model from the run's exact state at tick 11
        cells = rows[11].split(",")
        pos = tuple(float(c) for c in cells[6:9])
        vel = tuple(float(c) for c in cells[9:12])
        pos, vel = predict_air_tick(pos, vel, new[12].yaw, 0.0, 400.0, rep.params)
        self.assertAlmostEqual(est[0][2][0], vel[0], places=3)

    def test_estimate_stops_at_contact(self):
        rows = simulate(20, lambda t: 0.0, 400.0)
        cells = rows[15].split(",")
        rows[15] = row(15, 0.0, 0.0, 400.0, 1024, tuple(float(c) for c in cells[6:9]),
                       (float(cells[9]), float(cells[10]), 0.0))  # vz clipped: a surface
        run = parse_run(make_csv(rows))
        rep = check_run(run)
        self.assertEqual(rep.kinds[15], "contact")
        new = [TickInput(t, 0.0, 0.0, 0.0, 400.0, 1024) for t in range(20)]
        self.assertEqual(estimate_free_flight(run, rep, 10, new)[-1][0], 14)


BASE = {
    "_comment": "keep me",
    "format": "tasurf-timeline", "version": 1, "name": "ed", "map": "surf_test", "length": 50,
    "settings": {"sv_airaccelerate": 150},
    "start": {"origin": [0.0, 0.0, 0.0], "angles": [0.0, 90.0, 0.0]},
    "yaw": {"default_interp": "linear", "keys": [[0, 90.0], [40, 120.0, "smooth"]]},
    "keys": [{"_comment": "section"}, {"key": "W", "from": 0, "to": 10}, ["d", 12, 20]],
}


class DocumentTest(unittest.TestCase):
    def test_normalized_round_trip_keeps_the_rest(self):
        norm = normalized(parse_timeline(BASE))
        self.assertEqual(norm["yaw"], [[0, 90.0, "linear"], [40, 120.0, "smooth"]])
        self.assertEqual(norm["keys"], [{"key": "forward", "from": 0, "to": 10}, {"key": "moveright", "from": 12, "to": 20}])
        norm["keys"].append({"key": "jump", "from": 10, "to": 11})
        norm["yaw"].append([20, 100.123456789, "cubic"])
        new = apply_edits(BASE, {"yaw": norm["yaw"], "keys": norm["keys"], "length": 60})
        self.assertEqual(new["_comment"], "keep me")
        self.assertEqual(new["settings"], BASE["settings"])
        self.assertEqual(new["keys"][0], {"_comment": "section"})
        self.assertIn({"key": "D", "from": 12, "to": 20}, new["keys"])
        self.assertEqual(new["yaw"]["keys"][1], [20, 100.123456789, "cubic"])  # sorted, not rounded
        self.assertEqual(new["length"], 60)
        parse_timeline(new)

    def test_format_keeps_short_things_on_one_line(self):
        text = format_json(apply_edits(BASE, {}))
        self.assertIn('{ "key": "W", "from": 0, "to": 10 }', text)
        self.assertIn("[40, 120.0, \"smooth\"]", text)
        self.assertEqual(json.loads(text), BASE)


OPS = dict(BASE, length=120, yaw={"keys": [[0, 90.0], [30, 120.0], [60, 100.0, "smooth"], [100, 150.0]]},
           pitch={"keys": [[0, 0.0], [80, 10.0]]},
           keys=[{"key": "W", "from": 0, "to": 25}, {"key": "D", "from": 20, "to": 70}, {"key": "jump", "from": 40, "to": 41}])


def baked_ops(edits):
    return bake_inputs(parse_timeline(apply_edits(OPS, edits)))


def same_input(a, b):
    return (float_to_bits(a.yaw), float_to_bits(a.pitch), a.forwardmove, a.sidemove, a.buttons) == \
           (float_to_bits(b.yaw), float_to_bits(b.pitch), b.forwardmove, b.sidemove, b.buttons)


class TickOpsTest(unittest.TestCase):
    def setUp(self):
        self.edits = {k: v for k, v in normalized(parse_timeline(OPS)).items() if k in ("length", "yaw", "pitch", "keys")}
        self.old = baked_ops(self.edits)

    def test_insert_repeats_the_tick_and_shifts_the_rest(self):
        for at in (0, 15, 30, 45, 90, 110, 119):
            new_edits, _ = apply_op(self.edits, {"op": "insert", "at": at, "count": 7})
            new = baked_ops(new_edits)
            self.assertEqual(len(new), 127)
            for t in range(at):
                self.assertTrue(same_input(new[t], self.old[t]), (at, t))
            for t in range(at, at + 7):
                self.assertTrue(same_input(new[t], self.old[at]), (at, t))
            for t in range(at, 120):
                a, b = new[t + 7], self.old[t]
                self.assertAlmostEqual(a.yaw, b.yaw, places=9)
                self.assertEqual((a.buttons, a.forwardmove, a.sidemove), (b.buttons, b.forwardmove, b.sidemove))
            nxt = min([k[0] for k in self.edits["yaw"] if k[0] > at] + [120])
            for t in range(nxt, 120):  # from the next keyframe on: the same bits
                self.assertTrue(same_input(new[t + 7], self.old[t]), (at, t))

    def test_append_repeats_the_last_tick(self):
        new = baked_ops(apply_op(self.edits, {"op": "length", "length": 130})[0])
        self.assertEqual(len(new), 130)
        for t in range(120, 130):
            self.assertTrue(same_input(new[t], self.old[119]))
        shorter = baked_ops(apply_op(self.edits, {"op": "length", "length": 50})[0])
        for t in range(50):
            self.assertTrue(same_input(shorter[t], self.old[t]))

    def test_delete_removes_ticks(self):
        for a, b in ((0, 5), (10, 35), (28, 32), (55, 101), (100, 120)):
            new = baked_ops(apply_op(self.edits, {"op": "delete", "from": a, "to": b})[0])
            self.assertEqual(len(new), 120 - (b - a))
            for t in range(a):
                self.assertTrue(same_input(new[t], self.old[t]), (a, b, t))
            for t in range(b, 120):
                self.assertAlmostEqual(new[t - (b - a)].yaw, self.old[t].yaw, places=9)
                self.assertEqual(new[t - (b - a)].buttons, self.old[t].buttons)
        with self.assertRaises(TimelineError):
            apply_op(self.edits, {"op": "delete", "from": 0, "to": 120})

    def test_turn(self):
        op = {"op": "turn", "from": 40, "to": 70, "rate": 1.5, "keys": {"A": "hold", "moveright": "release"}}
        new_edits, msg = apply_op(self.edits, op)
        self.assertIn("left", msg)
        self.assertIn("joining the curve smoothly", msg)
        new = baked_ops(new_edits)
        y0 = self.old[40].yaw
        rate = lambda ops, t: wrap(ops[t].yaw - ops[t - 1].yaw)  # the turn on tick t
        # constant speed: the rate in the middle, eased into from the curve's slope and out of it over 7 ticks
        # (a quarter of the turn), so the turn rate never jumps: no corners where the turn meets the curve
        for t in range(48, 64):
            self.assertAlmostEqual(rate(new, t), 1.5, places=4)
        self.assertAlmostEqual(rate(new, 40), rate(self.old, 40), places=4)  # meets the curve before with its slope
        for t in range(41, 72):
            self.assertLess(abs(rate(new, t) - rate(new, t - 1)), 0.35, t)
        for t in range(0, 41):  # before the turn: the yaw as it was
            self.assertAlmostEqual(new[t].yaw, self.old[t].yaw, places=4)
        for t in range(40, 71):
            self.assertTrue(new[t].buttons & buttons.IN_MOVELEFT)
            self.assertFalse(new[t].buttons & buttons.IN_MOVERIGHT)
            self.assertEqual(new[t].sidemove, -400.0)
        self.assertTrue(new[39].buttons & buttons.IN_MOVERIGHT)  # before the turn: untouched
        for t in range(100, 120):  # "keep": later keyframes stay where they were
            self.assertTrue(same_input(new[t], self.old[t]))
        shifted = baked_ops(apply_op(self.edits, dict(op, after="shift"))[0])
        delta = wrap(shifted[70].yaw - self.old[70].yaw)
        self.assertAlmostEqual(shifted[110].yaw, wrap(self.old[110].yaw + delta), places=4)
        self.assertAlmostEqual(rate(shifted, 71), rate(self.old, 71), places=4)  # and leaves with the slope it had
        smooth_edits, smooth_msg = apply_op(self.edits, dict(op, shape="smooth"))
        smooth = baked_ops(smooth_edits)
        self.assertAlmostEqual(smooth[70].yaw, wrap(y0 + 45.0), places=4)  # one curve, with the whole total
        self.assertIn("+45° in total", smooth_msg)
        self.assertAlmostEqual(rate(smooth, 40), rate(self.old, 40), places=4)
        for t in range(41, 72):
            self.assertLess(abs(rate(smooth, t) - rate(smooth, t - 1)), 0.35, t)

    def test_insert_keeps_a_cubic_cubic(self):
        # a part of a cubic is a cubic: cut by a key with the slope there, not written out tick by tick
        doc = dict(OPS, yaw={"keys": [[0, 0.0, "cubic"], [60, 90.0, "cubic"], [119, 0.0, "cubic"]]})
        edits = {k: v for k, v in normalized(parse_timeline(doc)).items() if k in ("length", "yaw", "pitch", "keys")}
        old = bake_inputs(parse_timeline(doc))
        for at in (30, 60, 90):
            out = apply_op(edits, {"op": "insert", "at": at, "count": 10})[0]
            self.assertLessEqual(len(out["yaw"]), 5, at)
            self.assertEqual([k[2] for k in out["yaw"] if k[0] != at][:-1], ["cubic"] * (len(out["yaw"]) - 2), at)
            new = bake_inputs(parse_timeline(apply_edits(doc, out)))
            for t in range(120):
                self.assertAlmostEqual(new[t + 10 if t >= at else t].yaw, old[t].yaw, places=9)

    def test_ops_keep_smooth_and_cubic_curves(self):
        # cutting a smooth segment can't keep its kind: the kept part is written out tick by tick; a cubic one is
        # cut by a key with the slope there
        keys = [[5, 90.0, "cubic"], [20, 120.0, "linear"], [45, 100.0, "cubic"], [70, 160.0, "smooth"],
                [100, 150.0, "cubic"], [110, 140.0, "cubic"]]
        doc = dict(OPS, yaw={"keys": keys})
        tl = parse_timeline(doc)
        edits = {k: v for k, v in normalized(tl).items() if k in ("length", "yaw", "pitch", "keys")}
        old = bake_inputs(tl)
        bits = lambda ti: float_to_bits(ti.yaw)
        for at in range(0, 121, 5):
            new = bake_inputs(parse_timeline(apply_edits(doc, apply_op(edits, {"op": "insert", "at": at, "count": 4})[0])))
            self.assertEqual([bits(t) for t in new[:at]], [bits(t) for t in old[:at]], at)
            for t in range(at, 120):
                self.assertAlmostEqual(new[t + 4].yaw, old[t].yaw, places=9)
        for a, b in ((0, 7), (10, 30), (44, 46), (46, 71), (60, 105), (100, 120)):
            new = bake_inputs(parse_timeline(apply_edits(doc, apply_op(edits, {"op": "delete", "from": a, "to": b})[0])))
            self.assertEqual([bits(t) for t in new[:a]], [bits(t) for t in old[:a]], (a, b))
            for t in range(b, 120):
                self.assertAlmostEqual(new[t - (b - a)].yaw, old[t].yaw, places=9)

    def test_op_endpoint(self):
        ed = Editor(Path(tempfile.mkdtemp()), {})
        (ed.timelines_dir / "ops.json").write_text(json.dumps(OPS))
        out = ed.op("ops", OPS, self.edits, {"op": "insert", "at": 10, "count": 5})
        self.assertIsNone(out["error"])
        self.assertEqual(out["norm"]["length"], 125)
        self.assertEqual(out["doc"]["length"], 125)
        self.assertIn("inserted 5", out["message"])
        self.assertIn("outside", ed.op("ops", OPS, self.edits, {"op": "insert", "at": 500, "count": 5})["error"])


def wrap(v):
    return v - 360.0 * math.ceil((v - 180.0) / 360.0)


class AutoStrafeTest(unittest.TestCase):
    P = AirParams(dt=0.015, gravity=800.0, airaccelerate=150.0, maxspeed=250.0, maxvelocity=3500.0)

    def test_aim_rule(self):
        vx, vy = 600.0 * math.cos(math.radians(40)), 600.0 * math.sin(math.radians(40))
        self.assertAlmostEqual(strafe_yaw(vx, vy, 0.0, -400.0, self.P), 40.0, places=6)    # A: look along the velocity
        self.assertAlmostEqual(strafe_yaw(vx, vy, 0.0, 400.0, self.P), 40.0, places=6)     # D: the same, pushing right
        self.assertAlmostEqual(strafe_yaw(vx, vy, 400.0, -400.0, self.P), 85.0, places=6)  # W+A: 45 left of it
        self.assertAlmostEqual(strafe_yaw(vx, vy, 0.0, -400.0, self.P, offset=5.0), 45.0, places=6)  # harder
        self.assertIsNone(strafe_yaw(vx, vy, 400.0, 0.0, self.P))                         # W alone, no turn
        self.assertAlmostEqual(strafe_yaw(vx, vy, 400.0, 0.0, self.P, turn=-1.0), -50.0, places=6)
        self.assertIsNone(strafe_yaw(vx, vy, 0.0, 0.0, self.P))
        for fmove, smove in ((0.0, -400.0), (0.0, 400.0), (400.0, -400.0), (400.0, 400.0)):
            y = strafe_yaw(vx, vy, fmove, smove, self.P)
            best = best_yaw(vx, vy, fmove, smove, self.P, y)
            self.assertAlmostEqual(speed_after_air_accel(vx, vy, y, fmove, smove, self.P), best[1], places=6)
            self.assertAlmostEqual(best[1], math.hypot(600.0, 30.0), places=6)  # the most there is

    def test_ground_rule_is_the_best_prestrafe(self):
        # friction first, then the most ground acceleration: checked against every yaw
        for ducked in (False, True):
            g = GroundParams(accelerate=5.0, friction=4.0, stopspeed=75.0, ducked=ducked)
            for speed in (30.0, 220.0, 240.0, 260.0, 300.0, 400.0):
                for fm, sm in ((400.0, 400.0), (400.0, -400.0), (0.0, 400.0), (0.0, -400.0)):
                    vx, vy = speed * math.cos(0.3), speed * math.sin(0.3)
                    y = strafe_yaw(vx, vy, fm, sm, self.P, ground=g)
                    got = math.hypot(*ground_accelerate(vx, vy, y, fm, sm, self.P, g))
                    best = max(math.hypot(*ground_accelerate(vx, vy, a / 20.0, fm, sm, self.P, g))
                               for a in range(-3600, 3600))
                    self.assertGreaterEqual(got, best - 1e-6, (ducked, speed, fm, sm))
        g = GroundParams(5.0, 4.0, 75.0)
        self.assertAlmostEqual(strafe_yaw(100.0, 0.0, 400.0, 0.0, self.P, ground=g), 0.0)  # slow: W straight on
        self.assertIsNone(strafe_yaw(400.0, 0.0, 400.0, 0.0, self.P, ground=g))  # fast W alone: which side?
        self.assertIsNone(strafe_yaw(2.0, 0.0, 400.0, 400.0, self.P, ground=g))  # friction stops it

    def test_parse_and_bake(self):
        doc = dict(OPS, strafe=[{"from": 40, "to": 70}, [80, 90, 3.5]])
        tl = parse_timeline(doc)
        self.assertEqual([(r.start, r.end, r.offset) for r in tl.strafe], [(40, 70, 0.0), (80, 90, 3.5)])
        text = render_baked(tl, bake_inputs(tl))
        self.assertIn("@format tasurf-baked 2", text)
        self.assertIn("@strafe 40 70 0.000000\n@strafe 80 90 3.500000\n", text)
        self.assertEqual(read_baked(text).strafe, [(40, 70, 0.0), (80, 90, 3.5)])
        plain = parse_timeline(OPS)
        self.assertIn("@format tasurf-baked 1", render_baked(plain, bake_inputs(plain)))  # unchanged without strafe
        for bad, why in (([[60, 50]], "must be >"), ([[0, 10], [5, 20]], "overlaps"), ([[0, 10, 120]], "offset")):
            with self.assertRaises(TimelineError) as e:
                parse_timeline(dict(OPS, strafe=bad))
            self.assertIn(why, str(e.exception))

    def test_straight_goes_straight_at_the_most_speed(self):
        # A or D picked each tick towards the heading, at the max-gain angle: the heading stays within one tick's turn
        # of the target while the speed grows by the most there is, sqrt(v^2 + 30^2) a tick
        self.assertEqual(straight_sidemove(500.0, 0.0, 400.0, 10.0), -400.0)   # heading to the left: A
        self.assertEqual(straight_sidemove(500.0, 0.0, -400.0, -10.0), 400.0)  # to the right: D
        self.assertEqual(straight_sidemove(500.0, 0.0, 0.0, 10.0), 0.0)        # no side key: nothing to pick
        self.assertEqual(straight_sidemove(0.5, 0.0, 400.0, 10.0), 400.0)      # no speed: the held key stays
        for heading in (0.0, 37.0, 179.0, -120.0):
            v0 = 500.0
            pos, vel = (0.0, 0.0, 0.0), (v0 * math.cos(math.radians(heading)), v0 * math.sin(math.radians(heading)), 0.0)
            sides = set()
            for n in range(1, 301):
                smove = straight_sidemove(vel[0], vel[1], 400.0, heading)
                sides.add(smove)
                yaw = strafe_yaw(vel[0], vel[1], 0.0, smove, self.P)
                speed = math.hypot(vel[0], vel[1])
                pos, vel = predict_air_tick(pos, vel, yaw, 0.0, smove, self.P)
                self.assertAlmostEqual(math.hypot(vel[0], vel[1]), math.hypot(speed, 30.0), places=6)
                err = (math.degrees(math.atan2(vel[1], vel[0])) - heading + 180.0) % 360.0 - 180.0
                self.assertLessEqual(abs(err), math.degrees(math.atan(30.0 / speed)) + 1e-9, (heading, n))
            self.assertEqual(sides, {-400.0, 400.0})  # both keys, as it zigzags
            # the path: within a few units of the line through the start, after 300 ticks (over 2000 units)
            across = -pos[0] * math.sin(math.radians(heading)) + pos[1] * math.cos(math.radians(heading))
            self.assertLess(abs(across), 5.0, heading)

    def test_swing_sweeps_fewer_times(self):
        # with a swing the side is kept until the direction is that far past the heading: far fewer switches than
        # perfect (every tick or two), every tick still gaining the most there is, and the path weaving within
        # about the swing of the heading
        self.assertEqual(straight_sidemove(500.0, 0.0, 400.0, 3.0, 5.0, -400.0), -400.0)  # A, 3° short: keep A
        self.assertEqual(straight_sidemove(500.0, 0.0, 400.0, -3.0, 5.0, -400.0), -400.0)  # A, 3° past: still A
        self.assertEqual(straight_sidemove(500.0, 0.0, 400.0, -6.0, 5.0, -400.0), 400.0)  # A, 6° past: switch to D
        self.assertEqual(straight_sidemove(500.0, 0.0, 400.0, 6.0, 5.0, 400.0), -400.0)   # D, 6° past the other way: A
        self.assertEqual(straight_sidemove(500.0, 0.0, 400.0, 3.0, 5.0, 0.0), -400.0)     # no side before: as perfect
        heading, v0 = 30.0, 800.0
        for swing in (None, 4.0):
            pos, vel, prev, switches = (0.0, 0.0, 0.0), (v0 * math.cos(math.radians(heading)), v0 * math.sin(math.radians(heading)), 0.0), 0.0, 0
            worst = 0.0
            for n in range(400):
                smove = straight_sidemove(vel[0], vel[1], 400.0, heading, swing, prev)
                switches += prev != 0.0 and smove != prev
                yaw = strafe_yaw(vel[0], vel[1], 0.0, smove, self.P)
                speed = math.hypot(vel[0], vel[1])
                pos, vel = predict_air_tick(pos, vel, yaw, 0.0, smove, self.P)
                self.assertAlmostEqual(math.hypot(vel[0], vel[1]), math.hypot(speed, 30.0), places=6)
                worst = max(worst, abs((math.degrees(math.atan2(vel[1], vel[0])) - heading + 180.0) % 360.0 - 180.0))
                prev = smove
            if swing is None:
                perfect = switches
                self.assertGreater(perfect, 150)
            else:
                self.assertLess(switches, perfect / 3)  # ±4° at 800-1000 u/s (about 2° a tick): a switch every 4-5 ticks
                self.assertLess(worst, swing + math.degrees(math.atan(30.0 / v0)) + 1e-6)
                self.assertGreater(worst, swing)

    def test_smooth_swing(self):
        # the turn rate changes by at most a step a tick: at 0% (the smoothest) about what a sine of that size needs,
        # the sweep is the swing, and it keeps about three quarters of the speed gain; at 100% it always turns the
        # most (the most speed); the view is along the velocity
        heading, v0, swing = 30.0, 1500.0, 6.0
        w_max = math.degrees(math.atan(30.0 / v0))
        for close, least_gain in ((0.0, 0.7), (0.5, 0.9), (1.0, 0.999)):
            pos, vel = (0.0, 0.0, 0.0), (v0 * math.cos(math.radians(heading)), v0 * math.sin(math.radians(heading)), 0.0)
            prev, prev_dir, dirs, gains = 0.0, None, [], []
            for n in range(600):
                fm, sm, yaw = smooth_swing_input(vel[0], vel[1], prev_dir, prev, 400.0, heading, swing, close)
                self.assertAlmostEqual(yaw, math.degrees(math.atan2(vel[1], vel[0])))  # looking along the velocity
                self.assertAlmostEqual(math.hypot(fm, sm), 400.0, places=6)  # as hard as the held key
                speed, prev_dir = math.hypot(vel[0], vel[1]), math.degrees(math.atan2(vel[1], vel[0]))
                pos, vel = predict_air_tick(pos, vel, yaw, fm, sm, self.P)
                gains.append((math.hypot(vel[0], vel[1]) ** 2 - speed ** 2) / 900.0)
                dirs.append(math.degrees(math.atan2(vel[1], vel[0])) - heading)
                prev = sm
            later, rate = dirs[100:], [dirs[i] - dirs[i - 1] for i in range(101, len(dirs))]
            self.assertGreater(sum(gains[100:]) / len(gains[100:]), least_gain, close)
            self.assertLess(max(abs(d) for d in later), swing + 0.05, close)
            if close == 0.0:
                self.assertGreater(max(abs(d) for d in later), swing - 0.05)  # it sweeps all of it
                biggest = max(abs(rate[i] - rate[i - 1]) for i in range(1, len(rate)))
                self.assertLess(biggest, 1.1 * w_max * w_max / swing)  # no corners: a sine's change, about 0.22°/tick
        self.assertIsNone(smooth_swing_input(0.5, 0.0, None, 0.0, 400.0, 0.0, 6.0, 0.0))  # no speed: as straight
        self.assertIsNone(smooth_swing_input(500.0, 0.0, None, 0.0, 0.0, 0.0, 6.0, 0.0))  # no side key

    def test_swing_gain_parses_bakes_turns_and_freezes(self):
        tl = parse_timeline(dict(OPS, strafe=[{"from": 40, "to": 70, "heading": 87.5, "swing": 6, "gain": 90}]))
        self.assertEqual(tl.strafe[0].gain, 90.0)
        text = render_baked(tl, bake_inputs(tl))
        self.assertIn("@format tasurf-baked 6", text)
        self.assertIn("@strafe 40 70 0.000000 87.500000 6.000000 90.000000\n", text)
        self.assertEqual(read_baked(text).strafe, [(40, 70, 0.0, 87.5, 6.0, 90.0)])
        with self.assertRaises(TimelineError) as e:
            read_baked(text.replace("@format tasurf-baked 6", "@format tasurf-baked 5"))
        self.assertIn("@strafe with a swing's gain needs tasurf-baked 6", str(e.exception))
        self.assertEqual(normalized(tl)["strafe"][0]["gain"], 90.0)
        # the first version's smooth (how close to perfect) reads as the gain it keeps: 0 is the smoothest, 75%
        old = parse_timeline(dict(OPS, strafe=[{"from": 40, "to": 70, "heading": 87.5, "swing": 6, "smooth": 0}]))
        self.assertEqual(old.strafe[0].gain, 75.0)
        self.assertAlmostEqual(swing_close(75.0), 0.0)
        self.assertAlmostEqual(swing_close(100.0), 1.0)
        for bad, why in (({"from": 40, "to": 70, "heading": 1, "gain": 90}, "only with a swing"),
                         ({"from": 40, "to": 70, "heading": 1, "swing": 6, "gain": 70}, "75 to 100")):
            with self.assertRaises(TimelineError) as e:
                parse_timeline(dict(OPS, strafe=[bad]))
            self.assertIn(why, str(e.exception))
        edits = {k: v for k, v in normalized(parse_timeline(OPS)).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        new, msg = apply_op(edits, {"op": "turn", "from": 30, "to": 49, "rate": 0, "shape": "auto", "heading": 10.0,
                                    "swing": 6, "gain": 92.5})
        self.assertEqual(new["strafe"], [{"from": 30, "to": 50, "offset": 0.0, "heading": 10.0, "swing": 6.0, "gain": 92.5}])
        self.assertIn("keeping 92.5% of the speed gain", msg)
        # Freeze: every tick's A or D, and W, as hard as the game pressed them
        sides = [-399.0, -399.5, 400.0, 398.0]
        fwds = [28.5, 17.0, 0.0, 39.0]
        frozen = freeze_strafe(new, 30, 34, [5.0] * 4, sides, fwds)
        played = baked_ops(frozen)[30:34]
        self.assertEqual([ti.sidemove for ti in played], sides)
        self.assertEqual([ti.forwardmove for ti in played], fwds)

    def test_swing_parses_bakes_and_turns(self):
        doc = dict(OPS, strafe=[{"from": 40, "to": 70, "heading": 87.5, "swing": 6}])
        tl = parse_timeline(doc)
        self.assertEqual(tl.strafe[0].swing, 6.0)
        text = render_baked(tl, bake_inputs(tl))
        self.assertIn("@format tasurf-baked 5", text)
        self.assertIn("@strafe 40 70 0.000000 87.500000 6.000000\n", text)
        self.assertEqual(read_baked(text).strafe, [(40, 70, 0.0, 87.5, 6.0)])
        with self.assertRaises(TimelineError) as e:
            read_baked(text.replace("@format tasurf-baked 5", "@format tasurf-baked 4"))
        self.assertIn("@strafe with a swing needs tasurf-baked 5", str(e.exception))
        self.assertEqual(normalized(tl)["strafe"][0], {"from": 40, "to": 70, "offset": 0.0, "heading": 87.5, "swing": 6.0})
        for bad, why in (({"from": 40, "to": 70, "swing": 6}, "only for a straight range"),
                         ({"from": 40, "to": 70, "heading": 1, "swing": 0}, "more than 0"),
                         ({"from": 40, "to": 70, "heading": 1, "swing": 91}, "at most 90")):
            with self.assertRaises(TimelineError) as e:
                parse_timeline(dict(OPS, strafe=[bad]))
            self.assertIn(why, str(e.exception))
        edits = {k: v for k, v in normalized(parse_timeline(OPS)).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        new, msg = apply_op(edits, {"op": "turn", "from": 30, "to": 49, "rate": 0, "shape": "auto", "heading": 10.0, "swing": 4})
        self.assertEqual(new["strafe"], [{"from": 30, "to": 50, "offset": 0.0, "heading": 10.0, "swing": 4.0}])
        self.assertIn("swinging ±4°", msg)
        with self.assertRaises(TimelineError):
            apply_op(edits, {"op": "turn", "from": 30, "to": 49, "rate": 0, "shape": "auto", "swing": 4})  # not straight

    def test_curve_turns_are_named_blocks(self):
        # a curve turn is named in turns (the editor's block: edit it again, remove it); ops carry it along
        edits = {k: v for k, v in normalized(parse_timeline(OPS)).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        new, _ = apply_op(edits, {"op": "turn", "from": 40, "to": 69, "rate": -1.25, "shape": "smooth"})
        self.assertEqual(new["turns"], [{"from": 40, "to": 70, "rate": -1.25, "shape": "smooth"}])
        moved = apply_op(new, {"op": "insert", "at": 10, "count": 5})[0]
        self.assertEqual(moved["turns"], [{"from": 45, "to": 75, "rate": -1.25, "shape": "smooth"}])
        cut = apply_op(new, {"op": "delete", "from": 60, "to": 80})[0]
        self.assertEqual(cut["turns"], [{"from": 40, "to": 60, "rate": -1.25, "shape": "smooth"}])
        # a turn over part of it cuts it; auto-strafe too
        over = apply_op(new, {"op": "turn", "from": 60, "to": 79, "rate": 2, "shape": "linear"})[0]
        self.assertEqual(over["turns"], [{"from": 40, "to": 60, "rate": -1.25, "shape": "smooth"},
                                         {"from": 60, "to": 80, "rate": 2.0, "shape": "linear"}])
        auto = apply_op(new, {"op": "turn", "from": 30, "to": 49, "rate": 0, "shape": "auto"})[0]
        self.assertEqual(auto["turns"], [{"from": 50, "to": 70, "rate": -1.25, "shape": "smooth"}])
        # into the timeline and back: not baked, so the run (and its hash) is as without them
        doc = apply_edits(OPS, new)
        self.assertEqual(doc["turns"], [{"from": 40, "to": 70, "rate": -1.25, "shape": "smooth"}])
        tl = parse_timeline(doc)
        self.assertEqual(normalized(tl)["turns"], doc["turns"])
        plain = dict(doc); plain.pop("turns")
        self.assertEqual(render_baked(tl, bake_inputs(tl)), render_baked(parse_timeline(plain), bake_inputs(parse_timeline(plain))))
        self.assertNotIn("turns", apply_edits(doc, {"turns": []}))
        with self.assertRaises(TimelineError) as e:
            parse_timeline(dict(OPS, turns=[{"from": 40, "to": 70, "rate": 1, "shape": "auto"}]))
        self.assertIn("turns[0].shape", str(e.exception))

    def test_straight_ranges_parse_bake_and_turn(self):
        doc = dict(OPS, strafe=[{"from": 40, "to": 70, "heading": 87.5}, [80, 90, 3.5]])
        tl = parse_timeline(doc)
        self.assertEqual([(r.start, r.end, r.offset, r.heading) for r in tl.strafe], [(40, 70, 0.0, 87.5), (80, 90, 3.5, None)])
        text = render_baked(tl, bake_inputs(tl))
        self.assertIn("@format tasurf-baked 3", text)
        self.assertIn("@strafe 40 70 0.000000 87.500000\n@strafe 80 90 3.500000\n", text)
        self.assertEqual(read_baked(text).strafe, [(40, 70, 0.0, 87.5), (80, 90, 3.5)])
        self.assertEqual(normalized(tl)["strafe"][0], {"from": 40, "to": 70, "offset": 0.0, "heading": 87.5})
        self.assertEqual(apply_edits(OPS, {"strafe": normalized(tl)["strafe"]})["strafe"][0], {"from": 40, "to": 70, "heading": 87.5})
        with self.assertRaises(TimelineError) as e:
            parse_timeline(dict(OPS, strafe=[{"from": 40, "to": 70, "heading": "north"}]))
        self.assertIn("heading", str(e.exception))
        # the Turn operation makes one; a touching range with another heading stays apart
        edits = {k: v for k, v in normalized(parse_timeline(OPS)).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        new, msg = apply_op(edits, {"op": "turn", "from": 30, "to": 49, "rate": 0, "shape": "auto", "heading": 10.0,
                                    "keys": {"D": "hold", "A": "release"}})
        new, _ = apply_op(new, {"op": "turn", "from": 50, "to": 59, "rate": 0, "shape": "auto", "heading": 20.0})
        self.assertEqual([(r["from"], r["to"], r.get("heading")) for r in new["strafe"]], [(30, 50, 10.0), (50, 60, 20.0)])
        self.assertIn("straight", msg)

    def test_autobhop_parses_bakes_and_edits(self):
        tl = parse_timeline(dict(OPS, autobhop=True))
        self.assertTrue(tl.autobhop)
        text = render_baked(tl, bake_inputs(tl))
        self.assertIn("@format tasurf-baked 4", text)
        self.assertEqual(text.count("\n@autobhop\n"), 1)
        self.assertTrue(read_baked(text).autobhop)
        # off: the file is as before, byte for byte (and so its hash)
        plain = parse_timeline(OPS)
        self.assertFalse(plain.autobhop)
        self.assertEqual(render_baked(parse_timeline(dict(OPS, autobhop=False)), bake_inputs(plain)),
                         render_baked(plain, bake_inputs(plain)))
        self.assertNotIn("@autobhop", render_baked(plain, bake_inputs(plain)))
        # with a straight range too, still version 4
        both = parse_timeline(dict(OPS, autobhop=True, strafe=[{"from": 40, "to": 70, "heading": 87.5}]))
        self.assertEqual(read_baked(render_baked(both, bake_inputs(both))).strafe, [(40, 70, 0.0, 87.5)])
        with self.assertRaises(TimelineError) as e:
            parse_timeline(dict(OPS, autobhop="yes"))
        self.assertIn("autobhop", str(e.exception))
        with self.assertRaises(TimelineError) as e:
            read_baked(text.replace("@format tasurf-baked 4", "@format tasurf-baked 3"))
        self.assertIn("@autobhop needs tasurf-baked 4", str(e.exception))
        # the editor: written only when on
        self.assertTrue(normalized(tl)["autobhop"])
        self.assertNotIn("autobhop", normalized(plain))
        on = apply_edits(OPS, {"autobhop": True})
        self.assertIs(on["autobhop"], True)
        self.assertNotIn("autobhop", apply_edits(on, {"autobhop": False}))
        self.assertTrue(apply_edits(on, {"length": 50})["autobhop"])  # other edits keep it

    def test_plugin_autobhop(self):
        sp = REPO / "plugin" / "scripting"
        main = (sp / "tasurf.sp").read_text(encoding="utf-8")
        self.assertIn('StrEqual(parts[0], "@autobhop") && n == 1 && version >= 4', (sp / "tasurf" / "baked.sp").read_text(encoding="utf-8"))
        self.assertIn("if (g_AutoBhop && AutoBhop_Apply(client, buttons))", main)
        self.assertIn("g_LastButtons = buttons;", main)
        # auto-strafe's "does this tick jump" looks at the buttons played, which auto bhop let go in the air
        self.assertIn("(g_LastButtons & IN_JUMP)", (sp / "tasurf" / "strafe.sp").read_text(encoding="utf-8"))
        self.assertIn('CreateConVar("tas_autobhop", "1"', (sp / "tasurf" / "autobhop.sp").read_text(encoding="utf-8"))

    def test_camera_smoothing_setting(self):
        doc = dict(OPS, strafe=[{"from": 30, "to": 40}])
        self.assertEqual(parse_timeline(doc).camera_smoothing, "strafe")  # auto-strafe unless the timeline says otherwise
        self.assertNotIn("camera_smoothing", normalized(parse_timeline(doc)))
        for mode in ("all", "off"):
            other = apply_edits(doc, {"camera_smoothing": mode})
            self.assertEqual(other["camera_smoothing"], mode)
            self.assertEqual(parse_timeline(other).camera_smoothing, mode)
            self.assertEqual(normalized(parse_timeline(other))["camera_smoothing"], mode)
            self.assertNotIn("camera_smoothing", apply_edits(other, {"camera_smoothing": "strafe"}))  # only when not the default
            self.assertEqual(apply_edits(other, {"length": 50})["camera_smoothing"], mode)  # other edits keep it
            # it's only the view: the baked run and its hash are the same
            self.assertEqual(render_baked(parse_timeline(doc), bake_inputs(parse_timeline(doc))),
                             render_baked(parse_timeline(other), bake_inputs(parse_timeline(other))))
        for bad in ("smooth", False, None):
            with self.assertRaises(TimelineError):
                parse_timeline(dict(doc, camera_smoothing=bad))
            with self.assertRaises(TimelineError):
                apply_edits(doc, {"camera_smoothing": bad})

    def test_plugin_shows_the_baked_camera(self):
        sp = REPO / "plugin" / "scripting"
        main = (sp / "tasurf.sp").read_text(encoding="utf-8")
        baked = (sp / "tasurf" / "baked.sp").read_text(encoding="utf-8")
        self.assertIn('strncmp(line, "@view_yaw ", 10) == 0', baked)
        self.assertIn("ti.hasViewYaw = true;", baked)
        # what replays show (not while recording); the movement keeps the command's yaw
        self.assertIn("float camYaw = ti.hasViewYaw && !g_Recording ? ti.viewYaw : angles[1];", main)
        self.assertIn("view[1] = camYaw;", main)
        self.assertIn("#define VIEW_YAW_PER_LINE 16", baked)
        # the page's 3D view and yaw row show the same camera: the same rule's constants
        from tasurf import camera
        page = (REPO / "tasurf" / "editor_static" / "index.html").read_text(encoding="utf-8")
        self.assertIn(f"const CAM = {{ half: {camera.CAM_HALF}, margin: {camera.CAM_MARGIN}, break: {camera.CAM_BREAK:g} }};", page)
        self.assertEqual(camera.VIEW_YAW_PER_LINE, 16)

    def test_plugin_blocks_fall_damage(self):
        sp = REPO / "plugin" / "scripting"
        fall = (sp / "tasurf" / "falldamage.sp").read_text(encoding="utf-8")
        self.assertIn('CreateConVar("tas_fall_damage", "0"', fall)  # off unless asked for, as on surf servers
        self.assertIn("if ((damagetype & DMG_FALL) && !g_cvFallDamage.BoolValue)", fall)
        self.assertIn("FallDamage_OnClientPutInServer(client);", (sp / "tasurf.sp").read_text(encoding="utf-8"))
        self.assertIn('"tas_fall_damage"', (sp / "tasurf" / "settings.sp").read_text(encoding="utf-8"))  # runs record it

    def test_freezing_straight_writes_the_keys_it_played(self):
        edits = {k: v for k, v in normalized(parse_timeline(dict(OPS, strafe=[{"from": 30, "to": 40, "heading": 0.0}]))).items()
                 if k in ("length", "yaw", "pitch", "keys", "strafe")}
        sides = [400.0, -400.0, -400.0, 400.0, 400.0, 400.0, -400.0, 400.0, -400.0, 400.0]
        new = freeze_strafe(edits, 30, 40, [5.0] * 10, sides)
        self.assertEqual(new["strafe"], [])
        played = [ti.sidemove for ti in baked_ops(new)[30:40]]
        self.assertEqual(played, sides)
        self.assertEqual([ti.sidemove for ti in baked_ops(new)[40:70]], [400.0] * 30)  # D as before after the range

    def test_ops_carry_strafe(self):
        edits = {k: v for k, v in normalized(parse_timeline(OPS)).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        auto, msg = apply_op(edits, {"op": "turn", "from": 40, "to": 69, "shape": "auto", "rate": 0, "offset": 2,
                                     "keys": {"A": "hold", "D": "release"}})
        self.assertIn("auto-strafe", msg)
        self.assertEqual(auto["strafe"], [{"from": 40, "to": 70, "offset": 2.0}])
        self.assertEqual(auto["yaw"], edits["yaw"])  # the curve stays, as the fallback
        moved = apply_op(auto, {"op": "insert", "at": 10, "count": 5})[0]
        self.assertEqual(moved["strafe"], [{"from": 45, "to": 75, "offset": 2.0}])
        cut = apply_op(auto, {"op": "delete", "from": 50, "to": 60})[0]
        self.assertEqual(cut["strafe"], [{"from": 40, "to": 60, "offset": 2.0}])
        manual = apply_op(auto, {"op": "turn", "from": 50, "to": 59, "rate": 1.0})[0]
        self.assertEqual(manual["strafe"], [{"from": 40, "to": 50, "offset": 2.0}, {"from": 60, "to": 70, "offset": 2.0}])
        doc = apply_edits(OPS, auto)
        self.assertEqual(doc["strafe"], [{"from": 40, "to": 70, "offset": 2.0}])
        self.assertNotIn("strafe", apply_edits(doc, {"strafe": []}))

    def test_plugin_reads_strafe(self):
        sp = REPO / "plugin" / "scripting" / "tasurf"
        baked = (sp / "baked.sp").read_text(encoding="utf-8")
        self.assertIn('StrEqual(parts[0], "@strafe") && ((n == 4 && version >= 2) || (n == 5 && version >= 3) || (n == 6 && version >= 5)', baked)
        self.assertIn('StrEqual(parts[2], "2")', baked)
        self.assertIn('StrEqual(parts[2], "3")', baked)  # straight ranges (a heading)
        self.assertIn("bool AutoStrafe_Side(", (sp / "strafe.sp").read_text(encoding="utf-8"))
        self.assertIn("ti.strafeSwing > 0.0 && g_LastSidemove != 0.0", (sp / "strafe.sp").read_text(encoding="utf-8"))  # the swing
        self.assertIn("bool AutoStrafe_Smooth(", (sp / "strafe.sp").read_text(encoding="utf-8"))  # the smooth swing
        self.assertIn("(n == 7 && version >= 6)", baked)
        strafe = (sp / "strafe.sp").read_text(encoding="utf-8")
        self.assertIn("#define AIR_SPEED_CAP 30.0", strafe)  # the cap physics.py uses
        for cvar in ("sv_airaccelerate", "sv_accelerate", "sv_friction", "sv_stopspeed", "sv_maxspeed"):
            self.assertIn(f'"{cvar}"', strafe)  # read from the game, never assumed
        self.assertIn("#define DUCK_SPEED_MULTIPLIER 0.33333334", strafe)  # the multiplier physics.py uses
        record = (sp / "record.sp").read_text(encoding="utf-8")
        self.assertIn(",yaw_bits,pitch_bits\"", record)  # freeze needs the exact yaws

    def test_freeze_plays_the_same_yaws(self):
        root = Path(tempfile.mkdtemp())
        (root / "timelines").mkdir()
        data = root / "cstrike" / "addons" / "sourcemod" / "data" / "tasurf"
        (data / "runs").mkdir(parents=True)
        ed = Editor(root / "timelines", {"cstrike_dir": str(root / "cstrike")}, out_dir=root / "out")
        doc = dict(OPS, strafe=[{"from": 40, "to": 70}])
        edits = {k: v for k, v in normalized(parse_timeline(doc)).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        self.assertIn("preview first", ed.op("ops", doc, edits, {"op": "freeze", "tick": 50})["error"])
        # what the game played: the timeline's yaw, except odd float32 aims on the auto-strafe ticks
        played = [f32(ti.yaw) for ti in bake_inputs(parse_timeline(doc))]
        for t in range(40, 70):
            played[t] = f32(wrap(150.0 + 7.3 * (t - 40) + 0.001 * t * t))  # crosses 180
        rows = [row(t, played[t], 0.0, 0.0, 0, (t, 0.0, 0.0), (300.0, 0.0, 0.0)) for t in range(120)]
        h = ed.analyse(doc, "ops")["hash"]
        (data / "runs" / "ed.preview.csv").write_text(make_csv(rows, {"name": "ed", "baked_hash": h}))
        out = ed.op("ops", doc, edits, {"op": "freeze", "tick": 50})
        self.assertIsNone(out["error"], out.get("error"))
        self.assertIn("froze ticks 40..69", out["message"])
        self.assertEqual(out["norm"]["strafe"], [])
        frozen = bake_inputs(parse_timeline(out["doc"]))
        old = bake_inputs(parse_timeline(doc))
        for t in range(120):
            want = played[t] if 40 <= t < 70 else old[t].yaw
            self.assertEqual(float_to_bits(frozen[t].yaw), float_to_bits(want), t)
        self.assertIn("isn't auto-strafe", ed.op("ops", doc, edits, {"op": "freeze", "tick": 10})["error"])


def recording_text(name, inputs):
    """A baked file holding these exact inputs, as tas_record writes them (decimals that round-trip)."""
    from tasurf.bake import content_hash, f32r
    body = ["@format tasurf-baked 1", f"@name {name}", "@hash HASH", "@map surf_test", f"@ticks {len(inputs)}",
            "@start origin 0.000000 0.000000 0.000000", "@start angles 0.000000 90.000000 0.000000",
            "tick,yaw,pitch,forwardmove,sidemove,buttons"]
    body += [f"{t},{f32r(ti.yaw)},{f32r(ti.pitch)},{f32r(ti.forwardmove)},{f32r(ti.sidemove)},{ti.buttons}"
             for t, ti in enumerate(inputs)]
    body[2] = f"@hash {content_hash(body)}"
    return "\n".join(body) + "\n"


class SpliceTest(unittest.TestCase):
    """Record from a tick: the timeline up to it, the recording from it on."""

    def setUp(self):
        self.doc = dict(OPS, strafe=[{"from": 40, "to": 50}])
        tl = parse_timeline(self.doc)
        self.edits = {k: v for k, v in normalized(tl).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        self.old = bake_inputs(tl)
        self.at = 60
        rec = []
        for t, ti in enumerate(self.old[:self.at]):  # the timeline as the game played it
            yaw = f32(wrap(ti.yaw + 3.7)) if 40 <= t < 50 else f32(ti.yaw)  # auto-strafe ticks: the game's aim
            rec.append(TickInput(t, yaw, f32(ti.pitch), ti.forwardmove, ti.sidemove, ti.buttons))
        for i in range(70):  # then you: turning through 180, keys, a slow walk, the scoreboard bit
            t = self.at + i
            left = (i // 10) % 2 == 0
            b = (buttons.IN_MOVELEFT if left else buttons.IN_MOVERIGHT) | (buttons.IN_DUCK if i > 30 else 0)
            b |= buttons.IN_JUMP if i == 5 else 0
            b |= (1 << 16) if 20 <= i < 24 else 0
            side = (-400.0 if left else 400.0) if i < 50 else (-250.0 if left else 250.0)
            fwd = 400.0 if 40 <= i < 45 else 0.0
            b |= buttons.IN_FORWARD if fwd else 0
            rec.append(TickInput(t, f32(wrap(150.0 + 1.3 * i + 0.01 * i * i)), f32(5.0 - 0.1 * i), fwd, side, b))
        self.rec = rec

    def bits(self, ti):
        return float_to_bits(ti.yaw), float_to_bits(ti.pitch), ti.forwardmove, ti.sidemove, ti.buttons

    def test_prefix_kept_and_recording_exact(self):
        auto = set(range(40, 50))
        new = splice_recording(self.edits, self.at, self.rec, 400.0, self.old, auto, 0.0)
        self.assertEqual(new["length"], 130)
        self.assertEqual(new["strafe"], [{"from": 40, "to": 50, "offset": 0.0}])  # the part you edited stays editable
        baked = bake_inputs(parse_timeline(apply_edits(self.doc, new)))
        for t in range(self.at):
            self.assertEqual(self.bits(baked[t]), self.bits(self.old[t]), t)
        for t in range(self.at, 130):
            self.assertEqual(self.bits(baked[t]), self.bits(self.rec[t]), t)

    def test_refuses_a_changed_timeline(self):
        changed = list(self.rec)
        ti = changed[20]
        changed[20] = TickInput(20, ti.yaw, ti.pitch, ti.forwardmove, ti.sidemove, ti.buttons ^ buttons.IN_JUMP)
        with self.assertRaises(TimelineError) as e:
            splice_recording(self.edits, self.at, changed, 400.0, self.old, set(range(40, 50)), 0.0)
        self.assertIn("tick 20 differs", str(e.exception))
        with self.assertRaises(TimelineError):  # the auto-strafe yaw only counts as the game's own on those ticks
            splice_recording(self.edits, self.at, self.rec, 400.0, self.old, set(), 0.0)

    def test_record_request_and_splice_op(self):
        root = Path(tempfile.mkdtemp())
        (root / "timelines").mkdir()
        data = root / "cstrike" / "addons" / "sourcemod" / "data" / "tasurf"
        (data / "baked").mkdir(parents=True)
        ed = Editor(root / "timelines", {"cstrike_dir": str(root / "cstrike")}, out_dir=root / "out")
        r = ed.record("ops", self.doc, self.at)
        self.assertIsNone(r["error"])
        self.assertEqual((data / "requests" / "request.txt").read_text(), f"id {r['id']}\nload ed.record\nrecord 60\n")
        self.assertIn("between 1 and", ed.record("ops", self.doc, 0)["error"])
        (data / "baked" / "ed_from60.tas").write_text(recording_text("ed_from60", self.rec))
        out = ed.op("ops", self.doc, self.edits, {"op": "splice", "recording": "ed_from60", "at": 60})
        self.assertIsNone(out["error"], out.get("error"))
        self.assertIn("70 live tick(s), 130 in all (exact)", out["message"])
        self.assertEqual(out["norm"]["length"], 130)
        src = (REPO / "plugin" / "scripting" / "tasurf" / "remote.sp").read_text(encoding="utf-8")
        self.assertIn('strncmp(line, "record ", 7)', src)
        self.assertIn('"baked %s"', src)


class ImportTest(unittest.TestCase):
    def test_recording_import_is_exact(self):
        # yaw crossing +-180 and back, turn keys, both W and S held, a walk-speed range, a raw bit
        inputs, yaw = [], 170.0
        for t in range(300):
            yaw += 0.73 if t < 150 else -0.91
            y = f32(((yaw + 180.0) % 360.0) - 180.0 if ((yaw + 180.0) % 360.0) - 180.0 != -180.0 else 180.0)
            b, f, s = 0, 0.0, 0.0
            if 10 <= t < 120:
                b |= buttons.IN_FORWARD; f = 400.0
            if 100 <= t < 110:
                b |= buttons.IN_BACK; f = 0.0  # W+S
            if 130 <= t < 200:
                b |= buttons.IN_MOVERIGHT | buttons.IN_SPEED; s = 208.0
            if t % 7 == 0:
                b |= buttons.IN_LEFT
            if t == 50:
                b |= 1 << 16  # scoreboard
            inputs.append(TickInput(t, y, f32(3.5 + 0.01 * t), f, s, b))
        start = {"origin": ["1.000000", "2.000000", "3.000000"], "origin_bits": ["3F800000", "40000000", "40400000"],
                 "angles": ["0", "0", "0"], "angles_bits": ["00000000", "00000000", "00000000"],
                 "velocity": ["0", "0", "0"], "velocity_bits": ["00000000", "00000000", "00000000"],
                 "on_ground": ["yes"], "ground_entity": ["0"], "stamina": ["0.000000"], "maxspeed": ["260.000000"]}
        baked = BakedFile("rec", "fnv1a32:00000000", "surf_test", {"tickrate": "66.6667"}, start, inputs)
        doc, report = recording_to_timeline(baked, "rec_edit")
        self.assertTrue(report["exact"], report)
        self.assertEqual(doc["_baseline"], "rec")
        self.assertLess(report["yaw_keys"], 300)  # the linear stretches collapse
        keys = {r["key"] for r in doc["keys"]}
        self.assertTrue({"W", "S", "D", "walk", "turnleft", "bit16"} <= keys, keys)
        tl = parse_timeline(doc)
        for a, b in zip(inputs, bake_inputs(tl)):
            self.assertEqual(float_to_bits(a.yaw), float_to_bits(b.yaw))
            self.assertEqual(a.buttons, b.buttons)


class SlopeTest(unittest.TestCase):
    """A key's own slope: in the timeline, through the editor's edits and tick operations."""

    DOC = dict(OPS, yaw={"keys": [[0, 90.0], [30, 120.0, "cubic", 0.75], {"tick": 60, "value": 100.0, "interp": "cubic", "slope": -0.5},
                                  [100, 150.0]]})

    def test_parse_and_normalise(self):
        tl = parse_timeline(self.DOC)
        self.assertEqual([k.slope for k in tl.yaw], [None, 0.75, -0.5, None])
        self.assertEqual(normalized(tl)["yaw"], [[0, 90.0, "linear"], [30, 120.0, "cubic", 0.75], [60, 100.0, "cubic", -0.5],
                                                 [100, 150.0, "linear"]])
        for bad, why in (([[0, 1.0, "cubic", "x"]], "keys[0][3]"), ([[0, 1.0, "cubic", 1.0, 2]], "expected [tick, value]"),
                         ([{"tick": 0, "value": 1.0, "slope": None}], "keys[0].slope")):
            with self.assertRaises(TimelineError) as e:
                parse_timeline(dict(OPS, yaw={"keys": bad}))
            self.assertIn(why, str(e.exception))

    def test_edits_and_operations_keep_slopes(self):
        edits = {k: v for k, v in normalized(parse_timeline(self.DOC)).items() if k in ("length", "yaw", "pitch", "keys", "strafe")}
        doc = apply_edits(self.DOC, {"yaw": [[0, 90.0, "linear"], [30, 121.0, "cubic", 1], [100, 150.0, "linear", None]]})
        self.assertEqual(doc["yaw"]["keys"], [[0, 90.0, "linear"], [30, 121.0, "cubic", 1.0], [100, 150.0, "linear"]])
        old = bake_inputs(parse_timeline(self.DOC))
        moved, _ = apply_op(edits, {"op": "insert", "at": 5, "count": 4})
        self.assertIn([34, 120.0, "cubic", 0.75], moved["yaw"])  # moved with its slope
        new = bake_inputs(parse_timeline(apply_edits(self.DOC, moved)))
        for t in range(5, 120):
            self.assertAlmostEqual(new[t + 4].yaw, old[t].yaw, places=9)
        cut, _ = apply_op(edits, {"op": "delete", "from": 70, "to": 80})
        new = bake_inputs(parse_timeline(apply_edits(self.DOC, cut)))
        for t in range(80, 120):
            self.assertAlmostEqual(new[t - 10].yaw, old[t].yaw, places=9)
        with self.assertRaises(TimelineError) as e:
            apply_op(dict(edits, yaw=[[0, 1.0]]), {"op": "insert", "at": 0, "count": 1})
        self.assertIn("a key is [tick, value, interp]", str(e.exception))


class ApiTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        (root / "timelines").mkdir()
        (root / "timelines" / "ed.json").write_text(json.dumps(BASE))
        self.data = root / "cstrike" / "addons" / "sourcemod" / "data" / "tasurf"
        (self.data / "runs").mkdir(parents=True)
        self.editor = Editor(root / "timelines", {"cstrike_dir": str(root / "cstrike")}, out_dir=root / "out")
        self.server = make_server(self.editor, 0)
        self.base = f"http://127.0.0.1:{self.server.server_address[1]}"
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.tmp.cleanup()

    def call(self, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(self.base + path, data=data, headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(req, timeout=10) as r:
            return json.loads(r.read())

    def test_open_edit_save_run(self):
        self.assertEqual(self.call("/api/list")["timelines"], ["ed"])
        state = self.call("/api/open?name=ed")
        self.assertIsNone(state["error"])
        self.assertEqual(len(state["preview"]["yaw_u"]), 50)
        self.assertIsNone(state["run"])

        norm = state["norm"]
        norm["keys"].append({"key": "jump", "from": 30, "to": 31})
        up = self.call("/api/update", {"name": "ed", "doc": state["doc"], "edits": {"keys": norm["keys"]}})
        self.assertIsNone(up["error"])
        self.assertEqual(up["preview"]["buttons"][30], buttons.IN_JUMP)

        bad = self.call("/api/update", {"name": "ed", "doc": state["doc"], "edits": {"length": 5}})
        self.assertIn("outside the timeline", bad["error"])

        self.assertIsNone(self.call("/api/save", {"name": "ed", "doc": up["doc"]})["error"])
        saved = json.loads((Path(self.tmp.name) / "timelines" / "ed.json").read_text())
        self.assertIn({"key": "jump", "from": 30, "to": 31}, saved["keys"])

        run = self.call("/api/run", {"name": "ed", "doc": up["doc"]})
        self.assertIsNone(run["error"])
        req = (self.data / "requests" / "request.txt").read_text()
        self.assertEqual(req, f"id {run['id']}\nload ed\nrun\n")
        self.assertTrue((self.data / "baked" / "ed.tas").is_file())
        self.assertEqual(self.call(f"/api/request?id={run['id']}")["state"], "waiting")
        # the plugin takes the request and later answers
        (self.data / "requests" / "request.txt").unlink()
        self.assertEqual(self.call(f"/api/request?id={run['id']}")["state"], "running")
        (self.data / "requests" / f"{run['id']}.result").write_text("status complete\ncsv ed_001.csv\ndetail x y\n")
        done = self.call(f"/api/request?id={run['id']}")
        self.assertEqual((done["state"], done["status"], done["csv"], done["detail"]), ("done", "complete", "ed_001.csv", "x y"))

    def test_preview_request_and_freshness(self):
        state = self.call("/api/open?name=ed")
        norm = state["norm"]
        norm["keys"].append({"key": "jump", "from": 30, "to": 31})
        up = self.call("/api/update", {"name": "ed", "doc": state["doc"], "edits": {"keys": norm["keys"]}})
        self.assertNotEqual(up["hash"], state["hash"])  # the edit changes what would be baked

        pv = self.call("/api/preview", {"name": "ed", "doc": up["doc"]})
        self.assertIsNone(pv["error"])
        self.assertEqual((self.data / "requests" / "request.txt").read_text(), f"id {pv['id']}\nload ed.preview\npreview\n")
        self.assertTrue((self.data / "baked" / "ed.preview.tas").is_file())
        saved = json.loads((Path(self.tmp.name) / "timelines" / "ed.json").read_text())
        self.assertNotIn({"key": "jump", "from": 30, "to": 31}, saved["keys"])  # previews don't save

        # a newer preview replaces one the game hasn't taken yet
        pv2 = self.call("/api/preview", {"name": "ed", "doc": up["doc"]})
        self.assertIsNone(pv2["error"])
        self.assertEqual(self.call(f"/api/request?id={pv['id']}")["status"], "superseded")
        self.assertEqual(self.call(f"/api/request?id={pv2['id']}")["state"], "waiting")
        # so does a run; but a pending run isn't replaced
        run = self.call("/api/run", {"name": "ed", "doc": up["doc"]})
        self.assertIsNone(run["error"])
        self.assertIn("hasn't picked up", self.call("/api/preview", {"name": "ed", "doc": up["doc"]})["error"])
        (self.data / "requests" / "request.txt").unlink()  # the game took the run

        # a request the game took but never answered, while it says it's idle, was lost
        beat = self.data / "requests" / "heartbeat.txt"
        beat.write_text("version 0.6.0\nmap surf_test\nstate idle\nready yes\ntime 0\n")
        self.assertEqual(self.call(f"/api/request?id={run['id']}")["state"], "running")  # heartbeat too recent
        later = time.time() + 5
        os.utime(beat, (later, later))
        self.assertEqual(self.call(f"/api/request?id={run['id']}")["status"], "lost")

        # the plugin answers with runs/ed.preview.csv carrying the baked hash
        rows = simulate(50, lambda t: 90.0, 400.0)
        (self.data / "runs" / "ed.preview.csv").write_text(make_csv(rows, {"name": "ed", "baked_hash": up["hash"]}))
        st = self.call("/api/state", {"name": "ed", "doc": up["doc"]})
        self.assertEqual(st["pv"]["hash"], st["hash"])  # fresh
        self.assertEqual(len(st["pv"]["x"]), 50)
        self.assertEqual(st["runs"], [])  # previews are not numbered runs
        stale = self.call("/api/state", {"name": "ed", "doc": state["doc"]})
        self.assertNotEqual(stale["pv"]["hash"], stale["hash"])

    def test_runs_bake_the_auto_strafe_camera(self):
        """Once the game has played these inputs (a preview or a run with the same hash), runs and previews bake its
        camera: the yaw it played on auto-strafe, smoothed with the ticks either side (@view_yaw, not hashed). Only
        for a plugin that shows it (0.15), and not for a recording's lead-in."""
        doc = dict(BASE, strafe=[{"from": 10, "to": 30}])
        h = self.call("/api/state", {"name": "ed", "doc": doc})["hash"]
        beat = self.data / "requests" / "heartbeat.txt"
        beat.parent.mkdir(parents=True)
        beat.write_text("version 0.15.0\nmap surf_test\nstate idle\nready yes\ntime 0\n")
        pending = self.data / "requests" / "request.txt"

        def baked(path, name="ed"):
            r = self.call(path, {"name": "ed", "doc": doc, "at": 40})
            self.assertIsNone(r["error"])
            pending.unlink()
            return read_baked((self.data / "baked" / f"{name}.tas").read_text())

        self.assertIsNone(baked("/api/run").view_yaw)  # the game hasn't played these inputs yet
        rows = simulate(50, lambda t: 90.0 + ((1.0 if t % 2 else -1.0) if 10 <= t < 30 else 0.0), 400.0)
        (self.data / "runs" / "ed_001.csv").write_text(make_csv(rows, {"name": "ed", "baked_hash": "0" * 16}))
        self.assertIsNone(baked("/api/run").view_yaw)  # other inputs
        (self.data / "runs" / "ed.preview.csv").write_text(make_csv(rows, {"name": "ed", "baked_hash": h}))
        run = baked("/api/run")
        self.assertEqual(run.hash, h)  # the same run
        self.assertEqual(min(run.view_yaw), max(0, 10 - CAM_MARGIN + 1))
        for t in range(14, 26):
            self.assertAlmostEqual(run.view_yaw[t], 90.0, delta=0.1)  # zigzagging ±1° played, straight shown
        self.assertIsNotNone(baked("/api/preview", "ed.preview").view_yaw)
        doc["camera_smoothing"] = "off"  # the robotic look: the yaw played (same inputs, same hash)
        self.assertEqual(self.call("/api/state", {"name": "ed", "doc": doc})["hash"], h)
        self.assertIsNone(baked("/api/run").view_yaw)
        doc["camera_smoothing"] = "all"  # the whole run: every tick
        whole = baked("/api/run").view_yaw
        self.assertEqual(sorted(whole), list(range(50)))
        self.assertAlmostEqual(whole[20], 90.0, delta=0.1)
        doc.pop("camera_smoothing")
        self.assertIsNone(baked("/api/record", "ed.record").view_yaw)  # you take over from the yaw played
        beat.write_text("version 0.14.0\nmap surf_test\nstate idle\nready yes\ntime 0\n")
        self.assertIsNone(baked("/api/run").view_yaw)  # 0.14 refuses a file with @view_yaw lines

    def test_change_map_request(self):
        """Opening a timeline on another map asks the game to change to it: requests/changemap.txt, which the plugin
        takes before any run request (so a preview for the new timeline waits for the new map)."""
        r = self.call("/api/changemap", {"map": "surf_test"})
        self.assertIsNone(r["error"])
        self.assertEqual((self.data / "requests" / "changemap.txt").read_text(), f"id {r['id']}\nmap surf_test\n")
        for bad in ("../surf_test", "a b", 5, None):
            self.assertIn("bad map name", self.call("/api/changemap", {"map": bad})["error"])
        src = (REPO / "plugin" / "scripting" / "tasurf" / "remote.sp").read_text(encoding="utf-8")
        self.assertIn('"%s/requests/changemap.txt"', src)
        self.assertIn("ForceChangeLevel(map,", src)
        self.assertIn("IsMapValid(map)", src)  # a map the game doesn't have is an error, not a change
        self.assertLess(src.index("if (Remote_CheckChangeMap(target))"), src.index('"%s/requests/request.txt"'))

    def test_thumb(self):
        """The start screen's picture of a timeline: its newest run's path, thinned to at most THUMB_POINTS points,
        broken where it teleports."""
        self.assertEqual(self.call("/api/thumb?name=ed")["points"], None)  # not run yet
        rows = simulate(600, lambda t: 90.0, 400.0)
        jumped = [r.split(",") for r in rows]
        for cells in jumped[400:]:  # a teleport at tick 400: 5000 units on in x (the exact bits are what's read)
            x = f32(float(cells[6]) + 5000)
            cells[6], cells[19] = f"{x:.6f}", float_to_bits(x)
        (self.data / "runs" / "ed_001.csv").write_text(make_csv([",".join(c) for c in jumped], {"name": "ed"}))
        t = self.call("/api/thumb?name=ed")
        self.assertEqual(t["file"], "ed_001.csv")
        pts = t["points"]
        self.assertLessEqual(len(pts), 242)
        self.assertEqual(pts.count(None), 1)
        self.assertEqual(pts[0][:2], [round(float(jumped[0][6]), 1), round(float(jumped[0][7]), 1)])
        self.assertEqual(pts[-1][:2], [round(float(jumped[-1][6]), 1), round(float(jumped[-1][7]), 1)])  # the last tick too
        self.assertIn("bad name", self.call("/api/thumb?name=../x")["error"])

    def test_draw_request(self):
        r = self.call("/api/draw", {"map": "surf_test", "path": "ed.preview.csv", "ghost": "ed_003.csv",
                                    "timeline": "ed", "cursor": 120, "files": "3fa9c2"})
        self.assertIsNone(r["error"])
        lines = (self.data / "requests" / "draw.txt").read_text().splitlines()
        self.assertTrue(lines[0].startswith("id "))
        self.assertEqual(lines[1:], ["map surf_test", "path ed.preview.csv", "ghost ed_003.csv",
                                     "timeline ed", "cursor 120", "files 3fa9c2"])
        first_id = lines[0]
        self.call("/api/draw", {"map": "surf_test", "path": "ed_004.csv", "ghost": None})
        lines = (self.data / "requests" / "draw.txt").read_text().splitlines()
        self.assertNotEqual(lines[0], first_id)  # a new id makes the plugin redraw
        self.assertEqual(lines[3:], ["ghost -", "timeline -", "cursor -", "files -"])
        self.assertIn("bad name", self.call("/api/draw", {"map": "surf_test", "path": "../x.csv"})["error"])
        self.assertIn("bad cursor", self.call("/api/draw", {"map": "surf_test", "path": "a.csv", "cursor": -3})["error"])
        self.call("/api/draw", {"map": "surf_test", "path": "a.csv", "box": False})  # the Visuals toolbox's in-game box
        self.assertEqual((self.data / "requests" / "draw.txt").read_text().splitlines()[-1], "box 0")
        src = (REPO / "plugin" / "scripting" / "tasurf" / "remote.sp").read_text(encoding="utf-8")
        for needle in ('"%s/requests/draw.txt"', 'strncmp(line, "map ", 4)', 'strncmp(line, "path ", 5)',
                       'strncmp(line, "ghost ", 6)', 'strncmp(line, "timeline ", 9)', 'strncmp(line, "cursor ", 7)',
                       'strncmp(line, "files ", 6)', 'strncmp(line, "box ", 4)'):
            self.assertIn(needle, src)

    def test_game_status_from_heartbeat(self):
        g = self.call("/api/game")
        self.assertFalse(g["ok"])
        self.assertIn("sm plugins reload tasurf", g["detail"])  # no heartbeat: old plugin or no game
        beat = self.data / "requests" / "heartbeat.txt"
        beat.parent.mkdir(parents=True, exist_ok=True)
        beat.write_text("version 0.4.1\nmap surf_test\nplayer spectator\nstate idle\ntime 0\n")
        self.assertIn("join a team", self.call("/api/game")["detail"])
        beat.write_text("version 0.4.1\nmap surf_test\nplayer alive\nstate idle\ntime 0\n")
        g = self.call("/api/game")
        self.assertTrue(g["ok"])
        self.assertEqual((g["version"], g["map"]), ("0.4.1", "surf_test"))
        run = self.call("/api/run", {"name": "ed", "doc": json.loads((Path(self.tmp.name) / "timelines" / "ed.json").read_text())})
        self.assertIn("ready", self.call(f"/api/request?id={run['id']}")["detail"])
        old = time.time() - 30
        os.utime(beat, (old, old))
        self.assertIn("isn't responding", self.call("/api/game")["detail"])
        # 0.6+: the plugin says itself whether it's ready, and why not
        beat.write_text("version 0.6.0\nmap surf_test\nplayer alive\nmode record\nbot ready\nstate idle\n"
                        "ready no\nwhy you're in record mode: tas_mode edit\ntime 0\n")
        g = self.call("/api/game")
        self.assertFalse(g["ok"])
        self.assertEqual((g["detail"], g["mode"], g["bot"]), ("you're in record mode: tas_mode edit", "record", "ready"))
        beat.write_text("version 0.6.0\nmap surf_test\nplayer dead\nmode edit\nbot ready\nstate idle\nready yes\ntime 0\n")
        self.assertTrue(self.call("/api/game")["ok"])  # the bot plays; you don't need to be alive

    def test_analysis_with_a_run(self):
        rows = simulate(50, lambda t: 90.0, 400.0)
        (self.data / "runs" / "ed_001.csv").write_text(make_csv(rows, {"name": "ed"}))
        state = self.call("/api/open?name=ed")
        self.assertEqual(state["run"]["file"], "ed_001.csv")
        self.assertEqual(len(state["run"]["kinds"]), 50)
        a = state["analysis"]
        self.assertEqual(a["first_diff"], 0)  # the run's inputs differ from this timeline right away
        self.assertTrue(a["best"]["tick"])

    def test_main_menu_list_starts_and_create(self):
        listing = self.call("/api/list")
        self.assertEqual(listing["info"][0]["map"], "surf_test")
        self.assertEqual((listing["info"][0]["name"], listing["info"][0]["length"], listing["info"][0]["runs"]), ("ed", 50, 0))

        starts = self.data / "starts"
        starts.mkdir()
        start = {"format": "tasurf-start", "version": 1, "map": "surf_test", "origin": [1.0, 2.0, 3.0],
                 "angles": [10.0, 89.12001037597656, 0.0], "angles_bits": ["41200000", "42B23D72", "00000000"],
                 "on_ground": True}
        (starts / "spot.json").write_text(json.dumps(start))
        (starts / "broken.json").write_text("{")
        got = self.call("/api/starts")["starts"]
        self.assertEqual([s["name"] for s in got], ["broken", "spot"])
        self.assertIn("Expecting", got[0]["error"])
        self.assertEqual((got[1]["map"], got[1]["yaw"], got[1]["pitch"]), ("surf_test", 89.12001037597656, 10.0))

        made = self.call("/api/create", {"name": "fresh", "start": "spot", "length": 120})
        self.assertIsNone(made["error"])
        doc = json.loads((Path(self.tmp.name) / "timelines" / "fresh.json").read_text())
        self.assertEqual((doc["map"], doc["length"], doc["keys"]), ("surf_test", 120, []))
        self.assertEqual(doc["yaw"]["keys"], [[0, 89.12001037597656, "cubic"]])  # smooth from the start
        self.assertNotIn("format", doc["start"])  # the start is copied in
        tl = parse_timeline(doc)
        self.assertEqual(float_to_bits(bake_inputs(tl)[0].yaw), "42B23D72")

        self.assertIn("already exists", self.call("/api/create", {"name": "fresh", "start": "spot"})["error"])
        self.assertIn("letters", self.call("/api/create", {"name": "a b", "start": "spot"})["error"])
        self.assertIn("1 to", self.call("/api/create", {"name": "zero", "start": "spot", "length": 0})["error"])
        self.assertIn("No such file", self.call("/api/create", {"name": "nostart", "start": "nope"})["error"])

        copied = self.call("/api/create", {"name": "ed_copy", "copy": "ed"})
        self.assertIsNone(copied["error"])
        doc = json.loads((Path(self.tmp.name) / "timelines" / "ed_copy.json").read_text())
        self.assertEqual((doc["name"], doc["_comment"]), ("ed_copy", "keep me"))
        self.assertEqual(self.call("/api/list")["timelines"], ["ed", "ed_copy", "fresh"])

    def test_plugin_request_format_matches(self):
        src = (REPO / "plugin" / "scripting" / "tasurf" / "remote.sp").read_text(encoding="utf-8")
        for needle in ('"%s/requests/request.txt"', 'strncmp(line, "id ", 3)', 'strncmp(line, "load ", 5)',
                       'StrEqual(line, "run")', 'StrEqual(line, "preview")', '"%s/requests/%s.result"',
                       '"status %s"', '"csv %s"', '"detail %s"'):
            self.assertIn(needle, src)
        preview = (REPO / "plugin" / "scripting" / "tasurf" / "preview.sp").read_text(encoding="utf-8")
        self.assertIn('"%s/runs/%s.preview.csv"', preview)
        self.assertIn('"superseded"', preview)  # a preview a newer request replaced
        for needle in ('"ready %s"', '"why %s"', '"mode %s"', '"bot %s"', '"state idle"', '"version %s"', '"map %s"'):
            self.assertIn(needle, src)  # the heartbeat fields game_status reads


if __name__ == "__main__":
    unittest.main()

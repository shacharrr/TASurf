"""The camera replays show on auto-strafe (tasurf/camera.py) and its @view_yaw lines."""

import math
import unittest

from tasurf.bake import bake_inputs, read_baked, render_baked
from tasurf.camera import CAM_MARGIN, camera_yaws, view_yaw_lines
from tasurf.timeline import parse_timeline


def zigzag(n, ranges, base=lambda t: 90.0, amp=1.0):
    """Yaws that zigzag by amp either side of base on the ranges (auto-strafe at the most speed), base elsewhere."""
    return [base(t) + (amp if t % 2 else -amp) if any(a <= t < b for a, b in ranges) else base(t) for t in range(n)]


def shake(ys, lo, hi):  # how far a tick strays from the line through its neighbours
    return max(abs(ys[t] - (ys[t - 1] + ys[t + 1]) / 2) for t in range(lo, hi))


class CameraTest(unittest.TestCase):
    def test_smooths_the_zigzag_with_no_lag(self):
        ranges = [(40, 100)]
        ys = zigzag(140, ranges)
        cam = camera_yaws(ys, ranges)
        full = [cam.get(t, ys[t]) for t in range(len(ys))]
        self.assertAlmostEqual(shake(ys, 45, 95), 2.0)
        self.assertLess(shake(full, 45, 95), 0.01)
        for t in range(50, 90):
            self.assertAlmostEqual(full[t], 90.0, delta=0.01)  # centred: right on the way it goes
        # it's only on the ranges and the margins either side, easing in and out
        self.assertEqual(min(cam), 40 - CAM_MARGIN + 1)
        self.assertEqual(max(cam), 99 + CAM_MARGIN - 1)
        self.assertAlmostEqual(cam[40 - CAM_MARGIN + 1], ys[40 - CAM_MARGIN + 1], delta=0.01)

    def test_follows_a_turn(self):
        ranges = [(20, 80)]
        ys = zigzag(100, ranges, base=lambda t: 10.0 + 0.8 * t)  # turning steadily while it strafes
        cam = camera_yaws(ys, ranges)
        for t in range(30, 70):
            self.assertAlmostEqual(cam[t], 10.0 + 0.8 * t, delta=0.01)

    def test_across_minus_180(self):
        ranges = [(10, 50)]
        ys = [((y + 180.0) % 360.0) - 180.0 for y in zigzag(60, ranges, base=lambda t: 179.5)]
        cam = camera_yaws(ys, ranges)
        for t in range(15, 45):
            self.assertLess(abs(((cam[t] - 179.5) + 180.0) % 360.0 - 180.0), 0.05)
            self.assertTrue(-180.0 <= cam[t] < 180.0)

    def test_not_across_a_teleport(self):
        ranges = [(10, 70)]
        ys = zigzag(80, ranges, base=lambda t: 0.0 if t < 40 else 120.0)  # a teleport turns it 120° at tick 40
        cam = camera_yaws(ys, ranges)
        # each side averages only its own ticks (one-sided there, so a little of the zigzag is left), not 60° between
        self.assertAlmostEqual(cam[39], 0.0, delta=0.2)
        self.assertAlmostEqual(cam[40], 120.0, delta=0.2)

    def test_lines(self):
        cam = {t: 0.5 * t for t in list(range(3, 40)) + [50, 51]}
        lines = view_yaw_lines(cam)
        self.assertEqual([l.split()[1] for l in lines], ["3", "19", "35", "50"])
        self.assertEqual(lines[0].split()[2:5], ["1.5000", "2.0000", "2.5000"])
        self.assertEqual(len(lines[0].split()), 18)
        self.assertEqual(view_yaw_lines({0: -0.00001}), ["@view_yaw 0 0.0000"])

    def test_baked_lines_are_not_hashed(self):
        tl = parse_timeline({"format": "tasurf-timeline", "version": 1, "name": "c", "map": "surf_test", "length": 60,
                             "start": {"origin": [0.0, 0.0, 0.0], "angles": [0.0, 90.0, 0.0]},
                             "yaw": {"keys": [[0, 90.0]]}, "keys": [{"key": "moveleft", "from": 0, "to": 60}],
                             "strafe": [{"from": 10, "to": 40}]})
        inputs = bake_inputs(tl)
        plain = render_baked(tl, inputs)
        cam = camera_yaws(zigzag(60, [(10, 40)]), [(10, 40)])
        with_cam = render_baked(tl, inputs, cam)
        self.assertIn("@view_yaw ", with_cam)
        b0, b1 = read_baked(plain), read_baked(with_cam)
        self.assertEqual(b0.hash, b1.hash)
        self.assertIsNone(b0.view_yaw)
        self.assertEqual(sorted(b1.view_yaw), sorted(cam))
        for t, v in cam.items():
            self.assertTrue(math.isclose(b1.view_yaw[t], v, abs_tol=5e-5))


if __name__ == "__main__":
    unittest.main()

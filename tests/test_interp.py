import math
import unittest

from tasurf.interp import Key, joint_half, joint_slopes, sample, value_at, wrap_degrees


class WrapTest(unittest.TestCase):
    def test_range(self):
        for a in (-1080.0, -540.0, -360.0, -180.0, -0.5, 0.0, 179.999, 180.0, 359.0, 360.0, 540.0, 3600.25):
            w = wrap_degrees(a)
            self.assertGreater(w, -180.0)
            self.assertLessEqual(w, 180.0)
            self.assertAlmostEqual(math.remainder(w - a, 360.0), 0.0, places=9)

    def test_values(self):
        self.assertEqual(wrap_degrees(190.0), -170.0)
        self.assertEqual(wrap_degrees(180.0), 180.0)
        self.assertEqual(wrap_degrees(-180.0), 180.0)
        self.assertEqual(wrap_degrees(540.0), 180.0)
        self.assertEqual(wrap_degrees(370.0), 10.0)
        self.assertEqual(wrap_degrees(-190.0), 170.0)

    def test_in_range_float32_values_are_unchanged_bit_for_bit(self):
        import struct
        f32 = lambda v: struct.unpack("<f", struct.pack("<f", v))[0]
        # recorded yaws are float32; a turn offset of 360*k must not change them
        for a in map(f32, (179.98348999023438, -179.98348999023438, 89.12001037597656, 0.0001234, -2.5e-05, 180.0)):
            self.assertEqual(wrap_degrees(a), a)
            self.assertEqual(wrap_degrees(a + 720.0), a)
            self.assertEqual(wrap_degrees(a - 1080.0), a)


class InterpTest(unittest.TestCase):
    def test_before_first_and_after_last_hold(self):
        keys = [Key(10, 5.0), Key(20, 15.0)]
        self.assertEqual(value_at(keys, 0), 5.0)
        self.assertEqual(value_at(keys, 25), 15.0)

    def test_linear(self):
        keys = [Key(0, 0.0, "linear"), Key(10, 100.0)]
        self.assertEqual(sample(keys, 11), [float(10 * i) for i in range(11)])

    def test_hold_changes_exactly_at_next_key(self):
        keys = [Key(0, 1.0, "hold"), Key(5, 2.0)]
        self.assertEqual(sample(keys, 7), [1.0] * 5 + [2.0, 2.0])

    def test_smooth_endpoints_and_midpoint(self):
        keys = [Key(0, 0.0, "smooth"), Key(10, 10.0)]
        self.assertEqual(value_at(keys, 0), 0.0)
        self.assertEqual(value_at(keys, 10), 10.0)
        self.assertAlmostEqual(value_at(keys, 5), 5.0)
        # eases in: the first step is smaller than linear
        self.assertLess(value_at(keys, 1), 1.0)

    def test_cubic_passes_through_keys(self):
        keys = [Key(0, 0.0, "cubic"), Key(10, 30.0, "cubic"), Key(25, 10.0, "cubic"), Key(40, 50.0)]
        for k in keys:
            self.assertAlmostEqual(value_at(keys, k.tick), k.value)

    def test_cubic_on_a_line_stays_on_the_line(self):
        keys = [Key(0, 0.0, "cubic"), Key(10, 20.0, "cubic"), Key(30, 60.0)]
        for t in range(31):
            self.assertAlmostEqual(value_at(keys, t), 2.0 * t)

    def test_unwrapped_interpolation_never_wraps(self):
        # 350 -> 370 turns +20 through 360, not -340
        keys = [Key(0, 350.0, "linear"), Key(20, 370.0)]
        vals = sample(keys, 21)
        self.assertTrue(all(b > a for a, b in zip(vals, vals[1:])))
        self.assertEqual(vals[10], 360.0)

    def test_mixed_modes_per_segment(self):
        keys = [Key(0, 0.0, "hold"), Key(4, 4.0, "linear"), Key(8, 8.0)]
        self.assertEqual(sample(keys, 9), [0.0, 0.0, 0.0, 0.0, 4.0, 5.0, 6.0, 7.0, 8.0])

    def test_a_keys_own_slope_joins_a_line_without_a_corner(self):
        # a line of slope 2 up to tick 10, then a cubic that starts with that slope: no corner at 10
        keys = [Key(0, 0.0, "linear"), Key(10, 20.0, "cubic", 2.0), Key(20, 20.0, "linear", 0.0), Key(30, 20.0)]
        d = lambda keys, t: value_at(keys, t + 1) - value_at(keys, t)
        self.assertAlmostEqual(d(keys, 9), 2.0)
        self.assertGreater(d(keys, 10), 1.6)  # the cubic leaves with the line's slope (it then bends to level off)
        self.assertAlmostEqual(value_at(keys, 20), 20.0)
        # without the slope, the estimate (from the neighbours: 20 / 20 = 1 per tick) leaves a corner
        plain = [Key(0, 0.0, "linear"), Key(10, 20.0, "cubic"), Key(20, 20.0, "linear"), Key(30, 20.0)]
        self.assertLess(d(plain, 10), 1.0)

    def test_slope_is_the_hermite_tangent(self):
        keys = [Key(0, 0.0, "cubic", 3.0), Key(10, 10.0, "cubic", -1.0)]
        # cubic Hermite with m0 = 3, m1 = -1 (per tick) over 10 ticks
        for t in range(11):
            s = t / 10
            want = (-2 * s**3 + 3 * s**2) * 10 + (s**3 - 2 * s**2 + s) * 30 + (s**3 - s**2) * -10
            self.assertAlmostEqual(value_at(keys, t), want)


class JointTest(unittest.TestCase):
    """joint: straight lines (a steady turn rate) with rounded joints at the keys, through every key exactly."""

    V = [Key(0, 0.0, "joint"), Key(40, -80.0, "joint"), Key(80, 0.0, "joint"), Key(100, 30.0, "joint")]

    def rates(self, keys, n):
        v = sample(keys, n)
        return v, [v[t + 1] - v[t] for t in range(n - 1)]

    def test_through_every_key_with_straight_parts_and_a_smooth_joint(self):
        v, r = self.rates(self.V, 101)
        self.assertEqual([v[k.tick] for k in self.V], [k.value for k in self.V])
        self.assertEqual(joint_half(self.V, 1), 10.0)  # a quarter of the shorter segment beside it
        self.assertEqual(len({round(x, 9) for x in r[:30]}), 1)  # a steady rate up to the joint at 30 .. 50
        self.assertEqual(len({round(x, 9) for x in r[50:70]}), 1)
        self.assertLess(r[0], -2.0)  # tilted past key to key (-2) so the rounded joint reaches -80
        self.assertTrue(all(r[t] < r[t + 1] + 1e-12 for t in range(29, 50)))  # the rate turns round evenly
        self.assertLess(max(abs(r[t + 1] - r[t]) for t in range(99)), 0.35)  # no corner anywhere
        self.assertEqual(min(v), -80.0)  # never past the key

    def test_value_at_and_sample_agree_bit_for_bit(self):
        keys = self.V + [Key(130, 10.0, "cubic"), Key(150, 40.0, "joint"), Key(160, 0.0, "linear")]
        v = sample(keys, 170)
        self.assertEqual([value_at(keys, t) for t in range(170)], v)

    def test_a_width_of_zero_is_a_corner_and_a_wider_one_rounds_more(self):
        sharp = [Key(0, 0.0, "joint"), Key(40, -80.0, "joint", joint=0.0), Key(80, 0.0, "joint")]
        _, r = self.rates(sharp, 81)
        self.assertEqual((r[38], r[39], r[40], r[41]), (-2.0, -2.0, 2.0, 2.0))  # key to key, a corner at 40
        wide = [Key(0, 0.0, "joint"), Key(40, -80.0, "joint", joint=40.0), Key(80, 0.0, "joint")]
        _, rw = self.rates(wide, 81)
        _, rn = self.rates(self.V[:3], 81)
        self.assertLess(max(abs(rw[t + 1] - rw[t]) for t in range(79)), max(abs(rn[t + 1] - rn[t]) for t in range(79)))

    def test_moving_a_key_reshapes_the_curve_near_it_only(self):
        keys = [Key(10 * i, float((-1) ** i * 20 + i), "joint") for i in range(30)]
        moved = list(keys)
        moved[25] = Key(250, keys[25].value + 15.0, "joint")
        a, b = sample(keys, 300), sample(moved, 300)
        self.assertEqual(a[:150], b[:150])
        self.assertNotEqual(a[240:250], b[240:250])

    def test_a_cubic_meets_a_joint_line_without_a_corner(self):
        keys = [Key(0, 0.0, "joint"), Key(30, 60.0, "cubic"), Key(60, 60.0, "joint"), Key(90, 0.0, "linear")]
        _, r = self.rates(keys, 91)
        self.assertEqual(r[28], 2.0)  # the line, into the cubic at 30 (key to key the cubic would leave flat)
        self.assertLess(abs(r[30] - r[29]), 0.1)
        self.assertLess(abs(r[60] - r[59]), 0.1)

    def test_a_slope_a_smooth_curve_left_on_a_joint_key_is_ignored(self):
        # a key that was a smooth curve keeps its slope when it becomes a joint; only a width makes a slope its line
        clean = [Key(0, 2.6, "cubic", 11.3), Key(5, 62.5, "joint"), Key(15, 36.8, "joint"), Key(25, 37.5, "joint"), Key(30, 37.0, "linear")]
        left = [clean[0], Key(5, 62.5, "joint", 11.98), Key(15, 36.8, "joint", -2.57), Key(25, 37.5, "joint", 11.98), clean[4]]
        v = sample(left, 31)
        self.assertEqual(v, sample(clean, 31))
        self.assertTrue(all(v[t] > v[t + 1] for t in range(5, 12)))  # down from 62.5 towards 36.8
        self.assertGreater(min(v[5:25]), 35.5)  # rounding past the key a little, no dip (the slopes made one to ~20)
        said = [clean[0], Key(5, 62.5, "joint", -1.0, 2.5)] + clean[2:]
        self.assertEqual(joint_slopes(said, 1, 1)[1], -1.0)


if __name__ == "__main__":
    unittest.main()

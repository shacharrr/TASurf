import math
import unittest

from tasurf.interp import Key, sample, value_at, wrap_degrees


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


if __name__ == "__main__":
    unittest.main()

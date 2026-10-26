import math
import unittest

from tasurf.physics import AirParams, air_accelerate, check_run, predict_air_tick, wish
from tasurf.runcsv import parse_run

from .helpers import make_csv, row

SURF = AirParams(dt=0.015, gravity=800.0, airaccelerate=150.0, maxspeed=250.0, maxvelocity=3500.0)


class ReferencePhysicsTest(unittest.TestCase):
    def test_perpendicular_wish_adds_30_in_quadrature(self):
        # yaw 0, holding D: wish direction is the right vector (0, -1), perpendicular to +x velocity
        wishdir, wishspeed = wish(0.0, 0.0, 400.0, SURF.maxspeed)
        self.assertAlmostEqual(wishdir[0], 0.0)
        self.assertAlmostEqual(wishdir[1], -1.0)
        self.assertEqual(wishspeed, 250.0)
        for speed in (0.0, 30.0, 500.0, 3000.0):
            vx, vy = air_accelerate(speed, 0.0, wishdir, wishspeed, SURF.airaccelerate, SURF.dt)
            self.assertAlmostEqual(math.hypot(vx, vy), math.sqrt(speed ** 2 + 900.0), places=9)

    def test_low_airaccelerate_limits_the_gain(self):
        wishdir, wishspeed = wish(0.0, 0.0, 400.0, 250.0)
        vx, vy = air_accelerate(500.0, 0.0, wishdir, wishspeed, airaccelerate=5.0, dt=0.015)
        self.assertAlmostEqual(math.hypot(vx, vy), math.sqrt(500.0 ** 2 + (5.0 * 250.0 * 0.015) ** 2))

    def test_no_gain_when_already_fast_along_wishdir(self):
        wishdir, wishspeed = wish(0.0, 400.0, 0.0, 250.0)  # forward = +x
        self.assertEqual(air_accelerate(500.0, 0.0, wishdir, wishspeed, 150.0, 0.015), (500.0, 0.0))

    def test_forward_and_side_are_clamped_to_maxspeed(self):
        _, wishspeed = wish(30.0, 400.0, 400.0, 250.0)
        self.assertAlmostEqual(wishspeed, 250.0)

    def test_gravity_split_in_halves(self):
        pos, vel = predict_air_tick((0.0, 0.0, 0.0), (0.0, 0.0, 0.0), 0.0, 0.0, 0.0, SURF)
        self.assertAlmostEqual(vel[2], -800.0 * 0.015)
        self.assertAlmostEqual(pos[2], -0.5 * 800.0 * 0.015 * 0.015)


def simulate(n, yaw_fn, side, pos=(0.0, 0.0, 500.0), vel=(600.0, 0.0, 0.0), event=None):
    """Rows for n free-flight ticks produced by the reference model itself (with float32 rounding).
    event(t, pos, vel) -> (pos, vel) can alter the state after tick t, like a map trigger would."""
    from .helpers import f32
    rows = [row(0, yaw_fn(0), 0.0, side, 1024, pos, vel)]
    pos = tuple(f32(c) for c in pos)
    vel = tuple(f32(c) for c in vel)
    for t in range(1, n):
        pos, vel = predict_air_tick(pos, vel, yaw_fn(t), 0.0, side, SURF)
        if event:
            pos, vel = event(t, pos, vel)
        pos = tuple(f32(c) for c in pos)
        vel = tuple(f32(c) for c in vel)
        rows.append(row(t, yaw_fn(t), 0.0, side, 1024, pos, vel))
    return rows


class CheckRunTest(unittest.TestCase):
    def test_model_generated_run_matches(self):
        rows = simulate(40, lambda t: -1.5 * (t - 1), 400.0)
        rep = check_run(parse_run(make_csv(rows)))
        self.assertEqual(rep.air_ticks, 39)
        self.assertEqual(rep.free_flight, 39)
        self.assertEqual(rep.matched, 39, rep.mismatches)
        self.assertGreaterEqual(rep.perpendicular, 1)  # tick 1: yaw 0 with velocity along +x
        self.assertLess(rep.perpendicular_max_err, 1e-3)

    def test_tampered_tick_is_reported(self):
        rows = simulate(20, lambda t: -1.0 * t, 400.0)
        cells = rows[10].split(",")
        tampered = row(10, float(cells[1]), 0.0, 400.0, 1024,
                       tuple(float(c) for c in cells[6:9]),
                       (float(cells[9]) + 5.0, float(cells[10]), float(cells[11])))
        rows[10] = tampered
        rep = check_run(parse_run(make_csv(rows)))
        self.assertTrue(rep.mismatches)
        self.assertTrue(rep.mismatches[0].startswith("tick 10:"), rep.mismatches)

    def test_teleport_is_not_a_model_mismatch(self):
        def teleport(t, pos, vel):  # map teleport at tick 10: position jumps, velocity kept
            return ((pos[0] + 3000.0, pos[1], pos[2]), vel) if t == 10 else (pos, vel)
        rep = check_run(parse_run(make_csv(simulate(20, lambda t: -1.0 * t, 400.0, event=teleport))))
        self.assertEqual(len(rep.teleports), 1)
        self.assertTrue(rep.teleports[0].startswith("tick 10:"))
        self.assertEqual((rep.mismatches, rep.events), ([], []))

    def test_speed_reset_is_a_map_event(self):
        def stop(t, pos, vel):  # trigger zeroes horizontal speed at tick 10
            return (pos, (0.0, 0.0, vel[2])) if t == 10 else (pos, vel)
        rep = check_run(parse_run(make_csv(simulate(20, lambda t: -1.0 * t, 400.0, event=stop))))
        self.assertEqual(len(rep.events), 1)
        self.assertTrue(rep.events[0].startswith("tick 10:"))
        self.assertEqual((rep.mismatches, rep.teleports), ([], []))

    def test_ground_and_contact_ticks_are_skipped(self):
        rows = simulate(5, lambda t: 0.0, 400.0)
        cells = rows[3].split(",")
        rows[3] = row(3, 0.0, 0.0, 400.0, 1024, tuple(float(c) for c in cells[6:9]),
                      (float(cells[9]), float(cells[10]), 0.0), on_ground=1, ground_entity=0)
        rep = check_run(parse_run(make_csv(rows)))
        self.assertEqual(rep.air_ticks, 2)  # ticks 1 and 2; 3 and 4 touch the ground row


if __name__ == "__main__":
    unittest.main()

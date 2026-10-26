import tempfile
import unittest
from pathlib import Path

from tasurf.runcsv import compare_runs, find_runs, parse_run

from .helpers import make_csv, row


def sample_rows(n=5, z_nudge_at=None):
    rows = []
    for t in range(n):
        z = 100.0 - t * 0.5
        if t == z_nudge_at:
            z = 100.0 - t * 0.5 + 1e-5  # a one-or-two-ulp difference
        rows.append(row(t, 90.0, 0.0, 400.0, 1024, (10.0 * t, 0.0, z), (600.0, 30.0, -8.0 * t)))
    return rows


class CompareTest(unittest.TestCase):
    def test_identical(self):
        text = make_csv(sample_rows())
        cmp = compare_runs(parse_run(text), parse_run(text), text, text)
        self.assertTrue(cmp.identical)
        self.assertTrue(cmp.byte_identical)

    def test_bit_level_difference_is_found(self):
        a = make_csv(sample_rows())
        b = make_csv(sample_rows(z_nudge_at=3))
        cmp = compare_runs(parse_run(a), parse_run(b), a, b)
        self.assertFalse(cmp.identical)
        self.assertEqual(cmp.first_diff_tick, 3)
        self.assertEqual(cmp.differing_ticks, 1)
        self.assertTrue(any(c.startswith("z_bits:") for c in cmp.first_diff_cells), cmp.first_diff_cells)
        report = "\n".join(cmp.lines("a.csv", "b.csv"))
        self.assertIn("first differing tick: 3", report)

    def test_header_and_length_differences(self):
        a = make_csv(sample_rows(5))
        b = make_csv(sample_rows(4), header={"cvar.sv_airaccelerate": "1000", "status": "stopped at tick 4"})
        cmp = compare_runs(parse_run(a), parse_run(b), a, b)
        self.assertFalse(cmp.identical)
        self.assertTrue(any("cvar.sv_airaccelerate: 150 vs 1000" in d for d in cmp.header_diffs))
        self.assertEqual(cmp.length_note, "5 vs 4 ticks")

    def test_exact_floats_come_from_bits(self):
        run = parse_run(make_csv(sample_rows(z_nudge_at=2)))
        self.assertNotEqual(run.floats("z")[2], 99.0)  # decimal column says 99.000000
        self.assertEqual(run.header["status"], "complete")

    def test_find_runs_orders_by_run_number(self):
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            for name in ("ex_002.csv", "ex_010.csv", "ex_001.csv", "ex_other_001.csv", "ex_003.meta.txt"):
                (d / name).write_text("x")
            self.assertEqual([p.name for p in find_runs(d, "ex")], ["ex_001.csv", "ex_002.csv", "ex_010.csv"])


if __name__ == "__main__":
    unittest.main()

"""Recordings written by tas_record: 10-column baked files with an FNV-1a hash."""

import re
import unittest
from pathlib import Path

from tasurf.bake import FNV_OFFSET, FNV_PRIME, fnv1a32, fnv1a32_hash, read_baked
from tasurf.timeline import TimelineError, bits_to_float, float_to_bits

SP = Path(__file__).resolve().parent.parent / "plugin" / "scripting" / "tasurf" / "recorder.sp"


def plugin_style_recording(inputs, name="surf_test_rec"):
    """A recording laid out exactly as Recording_WriteBaked writes it."""
    head = ["@format tasurf-baked 1", f"@name {name}", "@map surf_test", f"@ticks {len(inputs)}",
            "@setting tickrate 66.6667", "@setting sv_airaccelerate 150",
            "@start origin 1.000000 2.000000 3.000000", "@start origin_bits 3F800000 40000000 40400000",
            "@start angles 4.312500 89.120010 0.000000", "@start angles_bits 408A0000 42B23D72 00000000",
            "@start velocity 0.000000 0.000000 0.000000", "@start velocity_bits 00000000 00000000 00000000",
            "@start base_velocity 0.000000 0.000000 0.000000", "@start base_velocity_flag 0",
            "@start on_ground yes", "@start ground_entity 0", "@start ducked 0", "@start gravity 1.000000",
            "@start lagged_movement 1.000000", "@start velocity_modifier 1.000000", "@start stamina 0.000000",
            "@start health 100", "@start maxspeed 260.000000",
            "tick,yaw,pitch,forwardmove,sidemove,buttons,yaw_bits,pitch_bits,forwardmove_bits,sidemove_bits"]
    ticks = [f"{i},{y:.6f},{p:.6f},{f:.6f},{s:.6f},{b},{float_to_bits(y)},{float_to_bits(p)},"
             f"{float_to_bits(f)},{float_to_bits(s)}" for i, (y, p, f, s, b) in enumerate(inputs)]
    digest = fnv1a32_hash(head + ticks)
    lines = ["# TASurf recording, written by tas_record: live inputs with their exact float bits.",
             "# Replay it with tas_load / tas_run; do not edit by hand.",
             head[0], head[1], f"@hash {digest}", *head[2:], *ticks]
    return "\r\n".join(lines) + "\r\n"  # SourceMod writes text-mode files: CRLF on Windows


class Fnv1aTest(unittest.TestCase):
    def test_standard_vectors(self):
        self.assertEqual(fnv1a32(b""), 0x811C9DC5)
        self.assertEqual(fnv1a32(b"a"), 0xE40C292C)
        self.assertEqual(fnv1a32(b"foobar"), 0xBF9CF968)

    def test_plugin_uses_the_same_constants(self):
        src = SP.read_text(encoding="utf-8")
        init = int(re.search(r"#define FNV_INIT\s+(-?\d+)", src).group(1))
        prime = int(re.search(r"#define FNV_PRIME\s+(-?\d+)", src).group(1))
        self.assertEqual(init & 0xFFFFFFFF, FNV_OFFSET)
        self.assertEqual(prime, FNV_PRIME)
        self.assertIn('"fnv1a32:%08x"', src)


class RecordingFileTest(unittest.TestCase):
    # small-angle yaw/pitch need more than 6 decimals; the bits columns carry them exactly
    INPUTS = [(0.000123, -1.5e-05, 400.0, 0.0, 8), (89.12001037597656, 21.29598045, 0.0, -400.0, 512 | 2)]

    def test_round_trip_uses_exact_bits(self):
        baked = read_baked(plugin_style_recording(self.INPUTS))
        self.assertTrue(baked.hash.startswith("fnv1a32:"))
        for got, (y, p, f, s, b) in zip(baked.inputs, self.INPUTS):
            self.assertEqual(float_to_bits(got.yaw), float_to_bits(y))
            self.assertEqual(float_to_bits(got.pitch), float_to_bits(p))
            self.assertEqual(got.yaw, bits_to_float(float_to_bits(y)))  # not the 6-decimal rounding
            self.assertEqual((got.forwardmove, got.sidemove, got.buttons), (f, s, b))
        self.assertNotEqual(baked.inputs[0].yaw, 0.000123)  # float32, not the decimal

    def test_edit_is_detected(self):
        text = plugin_style_recording(self.INPUTS).replace(",8,", ",9,", 1)
        with self.assertRaisesRegex(TimelineError, "hash"):
            read_baked(text)

    def test_column_line_matches_plugin(self):
        src = SP.read_text(encoding="utf-8")
        self.assertIn('"tick,yaw,pitch,forwardmove,sidemove,buttons,yaw_bits,pitch_bits,forwardmove_bits,sidemove_bits"', src)


if __name__ == "__main__":
    unittest.main()

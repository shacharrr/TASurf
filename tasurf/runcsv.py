"""Read run CSVs written by the plugin and compare runs bit for bit.

A run CSV has ``# key value`` header lines, one column-name line, then one
row per tick (see docs/file-formats.md). Float state columns are written both
as decimals and as raw float32 bits (``*_bits`` columns), so a comparison of
the text is a bit-exact comparison of the simulation.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

from .timeline import bits_to_float


@dataclass
class RunCsv:
    path: Path | None
    header: dict[str, str]
    columns: list[str]
    rows: list[list[str]]

    def col(self, name: str) -> int:
        try:
            return self.columns.index(name)
        except ValueError:
            raise KeyError(f"{self.path}: no column {name!r}") from None

    def floats(self, name: str) -> list[float]:
        """Column values as exact floats (from the *_bits column when there is one)."""
        bits_name = f"{name}_bits"
        if bits_name in self.columns:
            i = self.col(bits_name)
            return [bits_to_float(r[i]) for r in self.rows]
        i = self.col(name)
        return [float(r[i]) for r in self.rows]

    def ints(self, name: str) -> list[int]:
        i = self.col(name)
        return [int(r[i]) for r in self.rows]


def parse_run(text: str, path: Path | None = None) -> RunCsv:
    header: dict[str, str] = {}
    columns: list[str] | None = None
    rows: list[list[str]] = []
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line:
            continue
        if line.startswith("#"):
            body = line[1:].strip()
            key, _, value = body.partition(" ")
            header[key] = value.strip()
            continue
        cells = line.split(",")
        if columns is None:
            columns = cells
            continue
        if len(cells) != len(columns):
            raise ValueError(f"{path or 'csv'} line {n}: {len(cells)} cells, expected {len(columns)}")
        rows.append(cells)
    if columns is None:
        raise ValueError(f"{path or 'csv'}: no column line found")
    return RunCsv(path, header, columns, rows)


def read_run(path: Path) -> RunCsv:
    return parse_run(path.read_text(encoding="utf-8"), path)


@dataclass
class Comparison:
    identical: bool
    byte_identical: bool
    header_diffs: list[str] = field(default_factory=list)
    first_diff_tick: int | None = None
    first_diff_cells: list[str] = field(default_factory=list)
    differing_ticks: int = 0
    length_note: str | None = None

    def lines(self, name_a: str, name_b: str) -> list[str]:
        if self.byte_identical:
            return [f"IDENTICAL: {name_b} is byte-identical to {name_a}"]
        if self.identical:
            return [f"IDENTICAL: {name_b} matches {name_a} (same header and rows; whitespace differs)"]
        out = [f"DIFFERENT: {name_b} vs {name_a}"]
        for d in self.header_diffs:
            out.append(f"  header  {d}")
        if self.length_note:
            out.append(f"  length  {self.length_note}")
        if self.first_diff_tick is not None:
            out.append(f"  first differing tick: {self.first_diff_tick} "
                       f"({self.differing_ticks} differing ticks in total)")
            for c in self.first_diff_cells:
                out.append(f"    {c}")
        return out


def _cell_note(col: str, a: str, b: str) -> str:
    if col.endswith("_bits"):
        try:
            return f"{col}: {a} ({bits_to_float(a)!r}) vs {b} ({bits_to_float(b)!r})"
        except ValueError:
            pass
    return f"{col}: {a} vs {b}"


def compare_runs(a: RunCsv, b: RunCsv, raw_a: str | None = None, raw_b: str | None = None) -> Comparison:
    if raw_a is not None and raw_b is not None and raw_a == raw_b:
        return Comparison(identical=True, byte_identical=True)

    cmp = Comparison(identical=False, byte_identical=False)
    for key in sorted(set(a.header) | set(b.header)):
        va, vb = a.header.get(key), b.header.get(key)
        if va != vb:
            cmp.header_diffs.append(f"{key}: {va if va is not None else '<missing>'} vs "
                                    f"{vb if vb is not None else '<missing>'}")
    if a.columns != b.columns:
        cmp.header_diffs.append(f"columns differ: {','.join(a.columns)} vs {','.join(b.columns)}")
        return cmp

    n = min(len(a.rows), len(b.rows))
    for i in range(n):
        ra, rb = a.rows[i], b.rows[i]
        if ra != rb:
            cmp.differing_ticks += 1
            if cmp.first_diff_tick is None:
                cmp.first_diff_tick = int(ra[0]) if ra[0].lstrip("-").isdigit() else i
                cmp.first_diff_cells = [_cell_note(col, x, y)
                                        for col, x, y in zip(a.columns, ra, rb) if x != y]
    if len(a.rows) != len(b.rows):
        cmp.length_note = f"{len(a.rows)} vs {len(b.rows)} ticks"
    cmp.identical = not cmp.header_diffs and cmp.first_diff_tick is None and cmp.length_note is None
    return cmp


_RUN_RE = re.compile(r"^(?P<name>.+)_(?P<idx>\d+)\.csv$")


def find_runs(runs_dir: Path, name: str) -> list[Path]:
    """Run CSVs for a timeline name, oldest first (by the run number in the file name)."""
    found = []
    for p in runs_dir.glob(f"{name}_*.csv"):
        m = _RUN_RE.match(p.name)
        if m and m.group("name") == name:
            found.append((int(m.group("idx")), p))
    return [p for _, p in sorted(found)]

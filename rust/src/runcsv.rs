//! Read run CSVs written by the plugin and compare runs bit for bit (tasurf/runcsv.py).
//!
//! A run CSV has `# key value` header lines, one column-name line, then one row
//! per tick. Float state columns are written both as decimals and as raw float32
//! bits (`*_bits` columns), so comparing the text compares the simulation exactly.

use std::path::{Path, PathBuf};

use crate::bake::put;
use crate::py;

#[derive(Clone, Debug)]
pub struct RunCsv {
    pub path: Option<PathBuf>,
    pub header: Vec<(String, String)>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl RunCsv {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.header.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn has_column(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c == name)
    }

    pub fn col(&self, name: &str) -> Result<usize, String> {
        self.columns.iter().position(|c| c == name).ok_or_else(|| {
            let p = self.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "None".into());
            py::repr_str(&format!("{}: no column {}", p, py::repr_str(name))) // str(KeyError(...))
        })
    }

    /// Column values as exact floats (from the *_bits column when there is one).
    pub fn floats(&self, name: &str) -> Result<Vec<f64>, String> {
        let bits_name = format!("{}_bits", name);
        if self.has_column(&bits_name) {
            let i = self.col(&bits_name)?;
            return self.rows.iter().map(|r| py::bits_to_float(&r[i])).collect();
        }
        let i = self.col(name)?;
        self.rows.iter().map(|r| py::parse_float(&r[i])).collect()
    }

    pub fn ints(&self, name: &str) -> Result<Vec<i64>, String> {
        let i = self.col(name)?;
        self.rows.iter().map(|r| py::parse_int(&r[i])).collect()
    }
}

pub fn parse_run(text: &str, path: Option<&Path>) -> Result<RunCsv, String> {
    let where_ = || path.map(|p| p.display().to_string()).unwrap_or_else(|| "csv".into());
    let mut header: Vec<(String, String)> = Vec::new();
    let mut columns: Option<Vec<String>> = None;
    let mut rows: Vec<Vec<String>> = Vec::new();
    for (idx, raw) in py::splitlines(text).into_iter().enumerate() {
        let n = idx + 1;
        let line = py::strip(raw);
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix('#') {
            let body = py::strip(rest);
            let (key, value) = py::partition(body, " ");
            put(&mut header, key, py::strip(value).to_string());
            continue;
        }
        let cells: Vec<String> = line.split(',').map(str::to_string).collect();
        match &columns {
            None => {
                columns = Some(cells);
                continue;
            }
            Some(cols) => {
                if cells.len() != cols.len() {
                    return Err(format!("{} line {}: {} cells, expected {}", where_(), n, cells.len(), cols.len()));
                }
            }
        }
        rows.push(cells);
    }
    match columns {
        None => Err(format!("{}: no column line found", where_())),
        Some(columns) => Ok(RunCsv { path: path.map(Path::to_path_buf), header, columns, rows }),
    }
}

pub fn read_run(path: &Path) -> Result<RunCsv, String> {
    let text = std::fs::read_to_string(path).map_err(|e| crate::io_error(&e, path))?;
    parse_run(&text, Some(path))
}

#[derive(Clone, Debug, Default)]
pub struct Comparison {
    pub identical: bool,
    pub byte_identical: bool,
    pub header_diffs: Vec<String>,
    pub first_diff_tick: Option<i64>,
    pub first_diff_cells: Vec<String>,
    pub differing_ticks: i64,
    pub length_note: Option<String>,
}

impl Comparison {
    pub fn lines(&self, name_a: &str, name_b: &str) -> Vec<String> {
        if self.byte_identical {
            return vec![format!("IDENTICAL: {} is byte-identical to {}", name_b, name_a)];
        }
        if self.identical {
            return vec![format!("IDENTICAL: {} matches {} (same header and rows; whitespace differs)", name_b, name_a)];
        }
        let mut out = vec![format!("DIFFERENT: {} vs {}", name_b, name_a)];
        for d in &self.header_diffs {
            out.push(format!("  header  {}", d));
        }
        if let Some(l) = &self.length_note {
            out.push(format!("  length  {}", l));
        }
        if let Some(t) = self.first_diff_tick {
            out.push(format!("  first differing tick: {} ({} differing ticks in total)", t, self.differing_ticks));
            for c in &self.first_diff_cells {
                out.push(format!("    {}", c));
            }
        }
        out
    }
}

fn cell_note(col: &str, a: &str, b: &str) -> String {
    if col.ends_with("_bits") {
        if let (Ok(fa), Ok(fb)) = (py::bits_to_float(a), py::bits_to_float(b)) {
            return format!("{}: {} ({}) vs {} ({})", col, a, py::repr(fa), b, py::repr(fb));
        }
    }
    format!("{}: {} vs {}", col, a, b)
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

pub fn compare_runs(a: &RunCsv, b: &RunCsv, raw_a: Option<&str>, raw_b: Option<&str>) -> Comparison {
    if let (Some(x), Some(y)) = (raw_a, raw_b) {
        if x == y {
            return Comparison { identical: true, byte_identical: true, ..Default::default() };
        }
    }
    let mut cmp = Comparison::default();
    let mut keys: Vec<&str> = a.header.iter().chain(b.header.iter()).map(|(k, _)| k.as_str()).collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let (va, vb) = (a.get(key), b.get(key));
        if va != vb {
            cmp.header_diffs.push(format!("{}: {} vs {}", key, va.unwrap_or("<missing>"), vb.unwrap_or("<missing>")));
        }
    }
    if a.columns != b.columns {
        cmp.header_diffs.push(format!("columns differ: {} vs {}", a.columns.join(","), b.columns.join(",")));
        return cmp;
    }
    let n = a.rows.len().min(b.rows.len());
    for i in 0..n {
        let (ra, rb) = (&a.rows[i], &b.rows[i]);
        if ra != rb {
            cmp.differing_ticks += 1;
            if cmp.first_diff_tick.is_none() {
                let t0 = ra[0].trim_start_matches('-');
                cmp.first_diff_tick = Some(if is_digits(t0) { py::parse_int(&ra[0]).unwrap_or(i as i64) } else { i as i64 });
                cmp.first_diff_cells = a
                    .columns
                    .iter()
                    .zip(ra.iter().zip(rb.iter()))
                    .filter(|(_, (x, y))| x != y)
                    .map(|(col, (x, y))| cell_note(col, x, y))
                    .collect();
            }
        }
    }
    if a.rows.len() != b.rows.len() {
        cmp.length_note = Some(format!("{} vs {} ticks", a.rows.len(), b.rows.len()));
    }
    cmp.identical = cmp.header_diffs.is_empty() && cmp.first_diff_tick.is_none() && cmp.length_note.is_none();
    cmp
}

/// Run CSVs for a timeline name, oldest first (by the run number in the file name).
pub fn find_runs(runs_dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found: Vec<(u128, PathBuf)> = Vec::new();
    let entries = match std::fs::read_dir(runs_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    for entry in entries.flatten() {
        let fname = entry.file_name().to_string_lossy().to_string();
        // ^(?P<name>.+)_(?P<idx>\d+)\.csv$ with a greedy name
        let stem = match fname.strip_suffix(".csv") {
            Some(s) => s,
            None => continue,
        };
        let us = match stem.rfind('_') {
            Some(i) => i,
            None => continue,
        };
        let (base, idx) = (&stem[..us], &stem[us + 1..]);
        if base.is_empty() || base != name || !is_digits(idx) {
            continue;
        }
        if let Ok(i) = idx.parse::<u128>() {
            found.push((i, runs_dir.join(&fname)));
        }
    }
    found.sort();
    found.into_iter().map(|(_, p)| p).collect()
}

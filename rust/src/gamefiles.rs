//! Find a map's materials and textures the way the game does, for the 3D view's
//! textured look (tasurf/gamefiles.py, byte for byte).
//!
//! The game looks for a file first in the map's own pakfile (a zip inside the .bsp),
//! then along the search paths of CS:S's gameinfo.txt, in its order: cstrike/custom/*
//! (folders and VPKs, alphabetically), the VPK archives, the loose files of cstrike/,
//! hl2/ and platform/, and last cstrike/download/. Pakfile entries are stored or
//! LZMA-compressed, the two kinds the game reads. Paths are matched case-insensitively
//! (ASCII) and with either kind of slash.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const LUMP_PAKFILE: usize = 40;
const VPKS: [&str; 7] = [
    "cstrike/cstrike_english",
    "cstrike/cstrike_pak",
    "hl2/hl2_textures",
    "hl2/hl2_sound_vo_english",
    "hl2/hl2_sound_misc",
    "hl2/hl2_misc",
    "platform/platform_misc",
];
const LOOSE: [&str; 3] = ["cstrike", "hl2", "platform"];

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// data[a:b] with Python's clamping.
fn slice(data: &[u8], a: usize, b: usize) -> &[u8] {
    let (a, b) = (a.min(data.len()), b.min(data.len()));
    if a < b {
        &data[a..b]
    } else {
        &[]
    }
}

fn lower_key(name: &[u8]) -> Vec<u8> {
    name.iter().map(|&c| if c == b'\\' { b'/' } else { c.to_ascii_lowercase() }).collect()
}

fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// The request's path as the game would look it up, or the error if it isn't a material or texture.
pub fn normal_path(rel: &str) -> Result<Vec<u8>, String> {
    let mut b: Vec<u8> = Vec::with_capacity(rel.len());
    if rel.chars().all(|c| (c as u32) <= 0xFF) {
        for c in rel.chars() {
            let c = if c == '\\' { b'/' } else { (c as u32 as u8).to_ascii_lowercase() };
            if !(c == b'/' && b.last() == Some(&b'/')) {
                b.push(c); // the game ignores doubled slashes
            }
        }
    }
    let under = |dir: &[u8], exts: &[&[u8]]| b.starts_with(dir) && b.len() >= dir.len() + 1 + 4 && exts.iter().any(|e| b.ends_with(e));
    let ok = (under(b"materials/", &[b".vmt", b".vtf"]) || under(b"models/", &[b".mdl", b".vvd", b".vtx"]))
        && b.iter().all(|&c| c >= 0x20 && c != b':')
        && b.split(|&c| c == b'/').all(|part| !part.is_empty() && part != b"." && part != b"..");
    if !ok {
        return Err(format!("bad file path {} (a materials/... .vmt or .vtf file, or a models/... .mdl, .vvd or .vtx)", crate::py::repr_str(rel)));
    }
    Ok(b)
}

/// A VPK archive's directory: the entries' places, by lower-case path.
struct Vpk {
    path: PathBuf,
    tree: Vec<u8>,
    data_start: usize,
    entries: HashMap<Vec<u8>, (u16, u32, u32, usize, u16)>, // (archive, offset, length, preload at, size)
}

impl Vpk {
    fn open(path: &Path) -> Vpk {
        let empty = Vpk { path: path.to_path_buf(), tree: Vec::new(), data_start: 0, entries: HashMap::new() };
        let d = match std::fs::read(path) {
            Ok(d) => d,
            Err(_) => return empty,
        };
        if d.len() < 12 {
            return empty;
        }
        let (sig, version, tree_size) = (u32_at(&d, 0), u32_at(&d, 4), u32_at(&d, 8));
        if sig != 0x55AA1234 || !(version == 1 || version == 2) {
            return empty;
        }
        let head = if version == 1 { 12 } else { 28 };
        let mut entries = HashMap::new();
        let mut pos = head;
        // None: a broken directory (the whole archive is then left out, as Python does)
        let parsed = (|| -> Option<()> {
            let cstr = |pos: &mut usize| -> Option<Vec<u8>> {
                let end = *pos + d.get(*pos..)?.iter().position(|&c| c == 0)?;
                let s = d[*pos..end].to_vec();
                *pos = end + 1;
                Some(s)
            };
            loop {
                let ext = cstr(&mut pos)?;
                if ext.is_empty() {
                    break;
                }
                loop {
                    let folder = cstr(&mut pos)?;
                    if folder.is_empty() {
                        break;
                    }
                    loop {
                        let name = cstr(&mut pos)?;
                        if name.is_empty() {
                            break;
                        }
                        if pos + 18 > d.len() {
                            return None;
                        }
                        let preload = u16_at(&d, pos + 4);
                        let (archive, offset, length) = (u16_at(&d, pos + 6), u32_at(&d, pos + 8), u32_at(&d, pos + 12));
                        pos += 18;
                        let mut full = Vec::new();
                        if folder != b" " {
                            full.extend_from_slice(&folder);
                            full.push(b'/');
                        }
                        full.extend_from_slice(&name);
                        if ext != b" " {
                            full.push(b'.');
                            full.extend_from_slice(&ext);
                        }
                        entries.entry(lower_key(&full)).or_insert((archive, offset, length, pos, preload));
                        pos += preload as usize;
                    }
                }
            }
            Some(())
        })();
        if parsed.is_none() {
            return empty;
        }
        Vpk { path: path.to_path_buf(), data_start: head + tree_size as usize, tree: d, entries }
    }

    fn read(&self, key: &[u8]) -> Option<Vec<u8>> {
        let &(archive, offset, length, at, preload) = self.entries.get(key)?;
        let mut data = slice(&self.tree, at, at + preload as usize).to_vec();
        if length > 0 {
            if archive == 0x7FFF {
                // in this file, after the directory
                let a = self.data_start + offset as usize;
                data.extend_from_slice(slice(&self.tree, a, a + length as usize));
            } else {
                let name: Vec<char> = self.path.file_name()?.to_string_lossy().chars().collect();
                let cut = if name.ends_with(&['_', 'd', 'i', 'r', '.', 'v', 'p', 'k']) { 8 } else { 4 }; // as Python slices
                let base: String = name[..name.len().saturating_sub(cut)].iter().collect();
                let mut f = std::fs::File::open(self.path.with_file_name(format!("{}_{:03}.vpk", base, archive))).ok()?;
                f.seek(SeekFrom::Start(offset as u64)).ok()?;
                let mut part = Vec::with_capacity(length as usize);
                f.take(length as u64).read_to_end(&mut part).ok()?;
                data.extend_from_slice(&part);
            }
            if data.len() != preload as usize + length as usize {
                return None;
            }
        }
        Some(data)
    }
}

type ZipIndex = HashMap<Vec<u8>, (u16, usize, usize, usize)>; // (method, data offset, compressed size, size)

/// A zip's entries by lower-case path.
fn zip_index(pak: &[u8]) -> ZipIndex {
    let mut out = ZipIndex::new();
    let start = pak.len().saturating_sub(65557);
    let end = match (start..pak.len().saturating_sub(3)).rev().find(|&i| &pak[i..i + 4] == b"PK\x05\x06") {
        Some(e) if e + 22 <= pak.len() => e,
        _ => return out,
    };
    let count = u16_at(pak, end + 10);
    let mut p = u32_at(pak, end + 16) as usize;
    for _ in 0..count {
        if slice(pak, p, p + 4) != b"PK\x01\x02" || p + 46 > pak.len() {
            break;
        }
        let method = u16_at(pak, p + 10);
        let (csize, size) = (u32_at(pak, p + 20) as usize, u32_at(pak, p + 24) as usize);
        let (nlen, xlen, clen) = (u16_at(pak, p + 28) as usize, u16_at(pak, p + 30) as usize, u16_at(pak, p + 32) as usize);
        let local = u32_at(pak, p + 42) as usize;
        let name = slice(pak, p + 46, p + 46 + nlen).to_vec();
        p += 46 + nlen + xlen + clen;
        if slice(pak, local, local + 4) != b"PK\x03\x04" || local + 30 > pak.len() {
            continue;
        }
        let (lnlen, lxlen) = (u16_at(pak, local + 26) as usize, u16_at(pak, local + 28) as usize);
        out.entry(lower_key(&name)).or_insert((method, local + 30 + lnlen + lxlen, csize, size));
    }
    out
}

fn zip_read(pak: &[u8], entry: (u16, usize, usize, usize)) -> Option<Vec<u8>> {
    let (method, at, csize, size) = entry;
    let raw = slice(pak, at, at.saturating_add(csize));
    if raw.len() != csize {
        return None;
    }
    if method == 0 {
        return if csize == size { Some(raw.to_vec()) } else { None };
    }
    if method == 14 && csize >= 9 {
        // LZMA: 2 bytes version, 2 bytes properties size (5), the properties, the stream
        if u16_at(raw, 2) != 5 {
            return None;
        }
        // of a known size: lzma-rs stops there, so an end marker after it is ignored (as Python does)
        let mut alone = raw[4..9].to_vec();
        alone.extend_from_slice(&(size as u64).to_le_bytes());
        alone.extend_from_slice(&raw[9..]);
        let mut out = Vec::with_capacity(size);
        lzma_rs::lzma_decompress(&mut std::io::Cursor::new(alone), &mut out).ok()?;
        if out.len() < size {
            return None;
        }
        out.truncate(size);
        return Some(out);
    }
    None
}

enum Place {
    Vpk(Vpk),
    Dir(PathBuf),
}

/// The game's files under the CS:S folder (the one holding cstrike/), and one map's pakfile at a time.
pub struct GameFiles {
    game: PathBuf,
    paths: Mutex<Option<std::sync::Arc<Vec<Place>>>>,
    pak: Mutex<Option<(PathBuf, f64, std::sync::Arc<(Vec<u8>, ZipIndex)>)>>,
}

impl GameFiles {
    pub fn new(cstrike_dir: &Path) -> GameFiles {
        GameFiles {
            game: cstrike_dir.parent().map(Path::to_path_buf).unwrap_or_default(),
            paths: Mutex::new(None),
            pak: Mutex::new(None),
        }
    }

    fn search(&self) -> std::sync::Arc<Vec<Place>> {
        let mut guard = self.paths.lock().unwrap();
        if let Some(p) = guard.as_ref() {
            return p.clone();
        }
        let mut paths = Vec::new();
        let custom = self.game.join("cstrike").join("custom");
        if custom.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&custom).map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
            entries.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
            for p in entries {
                let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                let numbered = name.len() >= 8
                    && name.ends_with(".vpk")
                    && name.as_bytes()[name.len() - 8] == b'_'
                    && name.as_bytes()[name.len() - 7..name.len() - 4].iter().all(u8::is_ascii_digit);
                if p.is_dir() {
                    paths.push(Place::Dir(p));
                } else if p.extension().map(|e| e.to_string_lossy().to_lowercase() == "vpk").unwrap_or(false) && !numbered {
                    paths.push(Place::Vpk(Vpk::open(&p)));
                }
            }
        }
        for v in VPKS {
            let p = self.game.join(format!("{}_dir.vpk", v));
            if p.is_file() {
                paths.push(Place::Vpk(Vpk::open(&p)));
            }
        }
        for d in LOOSE {
            paths.push(Place::Dir(self.game.join(d)));
        }
        paths.push(Place::Dir(self.game.join("cstrike").join("download")));
        let arc = std::sync::Arc::new(paths);
        *guard = Some(arc.clone());
        arc
    }

    fn pakfile(&self, bsp: &Path) -> Result<std::sync::Arc<(Vec<u8>, ZipIndex)>, String> {
        let mtime = crate::editor::mtime(bsp).ok_or_else(|| format!("can't read {}", bsp.display()))?;
        let mut guard = self.pak.lock().unwrap();
        if let Some((p, t, pak)) = guard.as_ref() {
            if p == bsp && *t == mtime {
                return Ok(pak.clone());
            }
        }
        let mut pak = Vec::new();
        if let Ok(mut f) = std::fs::File::open(bsp) {
            let mut head = Vec::with_capacity(1036);
            if (&mut f).take(1036).read_to_end(&mut head).is_ok() && head.len() == 1036 && &head[..4] == b"VBSP" {
                let h = 8 + 16 * LUMP_PAKFILE;
                let ofs = i32::from_le_bytes([head[h], head[h + 1], head[h + 2], head[h + 3]]).max(0) as u64;
                let length = i32::from_le_bytes([head[h + 4], head[h + 5], head[h + 6], head[h + 7]]).max(0) as u64;
                let mut raw = Vec::new();
                if f.seek(SeekFrom::Start(ofs)).is_ok() && f.take(length).read_to_end(&mut raw).is_ok() {
                    pak = crate::bspmap::unlzma(&raw).unwrap_or_default();
                }
            }
        }
        let index = zip_index(&pak);
        let arc = std::sync::Arc::new((pak, index));
        *guard = Some((bsp.to_path_buf(), mtime, arc.clone())); // one map at a time: pakfiles can be tens of MB
        Ok(arc)
    }

    /// The file at rel (materials/....vmt or .vtf) as the game would find it while on the map at bsp.
    pub fn read(&self, bsp: &Path, rel: &str) -> Result<Vec<u8>, String> {
        let key = normal_path(rel)?;
        let pak = self.pakfile(bsp)?;
        let search = self.search();
        if let Some(&entry) = pak.1.get(&key) {
            if let Some(data) = zip_read(&pak.0, entry) {
                return Ok(data);
            }
        }
        for place in search.iter() {
            match place {
                Place::Vpk(v) => {
                    if let Some(data) = v.read(&key) {
                        return Ok(data);
                    }
                }
                Place::Dir(d) => {
                    let p = d.join(latin1(&key));
                    if p.is_file() {
                        if let Ok(data) = std::fs::read(&p) {
                            return Ok(data);
                        }
                    }
                }
            }
        }
        Err(format!("{} isn't in the map or the game's files", latin1(&key)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_as_the_game_looks_them_up() {
        assert_eq!(normal_path(r"MATERIALS\Dev//Dev_Measure.VMT").unwrap(), b"materials/dev/dev_measure.vmt");
        assert_eq!(normal_path("materials/caf\u{e9}.vtf").unwrap(), b"materials/caf\xe9.vtf");
        assert_eq!(normal_path(r"models\Props\Crate.dx90.VTX").unwrap(), b"models/props/crate.dx90.vtx");
        for bad in ["", "materials/../cfg/x.vmt", "materials/./x.vmt", "materials/x.txt", "models/x.txt", "models/.mdl", "/materials/x.vmt",
                    "c:/materials/x.vmt", "materials/a:b.vmt", "materials/.vmt", "materials/\u{4e2d}.vmt"] {
            assert!(normal_path(bad).is_err(), "{}", bad);
        }
    }

    #[test]
    fn lzma_entries_with_and_without_an_end_marker() {
        let data = b"lzma ".repeat(100);
        let mut packed = Vec::new();
        lzma_rs::lzma_compress(&mut &data[..], &mut packed).unwrap(); // props, size (unknown), stream, end marker
        let mut raw = vec![9, 4, 5, 0];
        raw.extend_from_slice(&packed[..5]);
        raw.extend_from_slice(&packed[13..]);
        let mut pak = vec![0u8; 3];
        pak.extend_from_slice(&raw);
        assert_eq!(zip_read(&pak, (14, 3, raw.len(), data.len())).unwrap(), data);
        assert!(zip_read(&pak[..pak.len() - 20], (14, 3, raw.len() - 20, data.len())).is_none()); // cut short
        assert_eq!(zip_read(b"abc", (0, 0, 3, 3)).unwrap(), b"abc");
        assert!(zip_read(b"abc", (8, 0, 3, 3)).is_none()); // deflate: the game doesn't read it either
    }
}

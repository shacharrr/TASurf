//! Read a Source map (.bsp) as a triangle mesh, for the editor's 3D view (tasurf/bspmap.py).
//!
//! The faces of the world and of brush entities, and displacements, as triangles;
//! tool faces (nodraw, sky, triggers, hints, skips) and trigger models left out. The
//! mesh, the data for drawing it textured and lit, and the triggers as polygons are
//! the same little-endian binary as Python writes, byte for byte (the formats are
//! described in tasurf/bspmap.py):
//!   b"TASM", u32 version (1), u32 vertex count, u32 index count,
//!   f32 mins[3], f32 maxs[3], f32 positions[3 * vertices], u32 indices[...]
//! Triangles are wound counter-clockwise seen from the front of their surface.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const LUMP_ENTITIES: usize = 0;
const LUMP_PLANES: usize = 1;
const LUMP_TEXDATA: usize = 2;
const LUMP_VERTEXES: usize = 3;
const LUMP_TEXINFO: usize = 6;
const LUMP_FACES: usize = 7;
const LUMP_LIGHTING: usize = 8;
const LUMP_EDGES: usize = 12;
const LUMP_SURFEDGES: usize = 13;
const LUMP_MODELS: usize = 14;
const LUMP_DISPINFO: usize = 26;
const LUMP_DISP_VERTS: usize = 33;
const LUMP_STRING_DATA: usize = 43;
const LUMP_STRING_TABLE: usize = 44;
const LUMP_LIGHTING_HDR: usize = 53;
const LUMP_FACES_HDR: usize = 58;
const LUMP_NODES: usize = 5;
const LUMP_LEAFS: usize = 10;
const LUMP_LEAFBRUSHES: usize = 17;
const LUMP_BRUSHES: usize = 18;
const LUMP_BRUSHSIDES: usize = 19;
const LUMP_GAME_LUMP: usize = 35;
/// The game lump of the static props.
const SPRP: i32 = i32::from_be_bytes(*b"sprp");
const ZONES: [&str; 5] = ["func_buyzone", "func_bomb_target", "func_hostage_rescue", "func_vip_safetyzone", "func_escapezone"];

/// texinfo flags of faces that aren't drawn in the game: SKY2D, SKY, TRIGGER, NODRAW, HINT, SKIP
const HIDDEN: i32 = 0x2 | 0x4 | 0x40 | 0x80 | 0x100 | 0x200;
const MESH_VERSION: u32 = 1;
const TEX_VERSION: u32 = 1;
const TRIG_VERSION: u32 = 1;
const CLIP_VERSION: u32 = 1;
const NO_MATERIAL: u16 = 0xFFFF;
/// a side of a face's lightmap; the game's are at most 128, so more is a broken map
const MAX_LUXELS: i64 = 1024;

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn i32_at(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32_at(b: &[u8], o: usize) -> f64 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]) as f64
}

/// The indices of a Python slice [a:b] of a sequence of n (negative ends count from the back).
fn py_range(n: usize, a: i64, b: i64) -> std::ops::Range<usize> {
    let n = n as i64;
    let at = |x: i64| if x < 0 { (x + n).max(0) } else { x.min(n) };
    let (a, b) = (at(a), at(b));
    a as usize..b.max(a) as usize
}

/// data[a:b] with Python's clamping.
fn slice(data: &[u8], a: i64, b: i64) -> &[u8] {
    let n = data.len() as i64;
    let a = a.clamp(0, n) as usize;
    let b = b.clamp(0, n) as usize;
    if a < b {
        &data[a..b]
    } else {
        &[]
    }
}

fn lump(data: &[u8], index: usize) -> Result<Vec<u8>, String> {
    let h = 8 + 16 * index;
    let (ofs, length) = (i32_at(data, h) as i64, i32_at(data, h + 4) as i64);
    unlzma(slice(data, ofs, ofs + length))
}

/// A lump as the map compiler stored it: as it is, or LZMA-compressed.
pub fn unlzma(raw: &[u8]) -> Result<Vec<u8>, String> {
    if raw.len() < 4 || &raw[..4] != b"LZMA" {
        return Ok(raw.to_vec());
    }
    // Valve's LZMA lump: "LZMA", actual size, compressed size, 5 property bytes, then the stream
    if raw.len() < 17 {
        return Err("truncated LZMA lump".into());
    }
    let (actual, compressed) = (u32_at(raw, 4) as u64, u32_at(raw, 8) as i64);
    let mut alone = raw[12..17].to_vec();
    alone.extend_from_slice(&actual.to_le_bytes());
    alone.extend_from_slice(slice(raw, 17, 17 + compressed));
    let mut out = Vec::with_capacity(actual as usize);
    lzma_rs::lzma_decompress(&mut std::io::Cursor::new(alone), &mut out).map_err(|e| format!("{:?}", e))?;
    out.truncate(actual as usize);
    Ok(out)
}

/// The lump as records of `size` bytes (Python's struct.iter_unpack, which wants whole records).
fn records(data: &[u8], index: usize, size: usize) -> Result<Vec<u8>, String> {
    let l = lump(data, index)?;
    if l.len() % size != 0 {
        return Err(format!("iterative unpacking requires a buffer of a multiple of {} bytes", size));
    }
    Ok(l)
}

pub fn is_trigger(classname: &str) -> bool {
    classname.starts_with("trigger_") || ZONES.contains(&classname)
}

/// A brush entity: its origin (its faces are stored relative to it), class and name.
type BrushEntity = ([f64; 3], String, String);

/// Brush entities by model number.
fn brush_entities(text: &[char]) -> std::collections::HashMap<usize, BrushEntity> {
    let mut out = std::collections::HashMap::new();
    let mut ent: Vec<(String, String)> = Vec::new();
    let mut key: Option<String> = None;
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if c == '{' || c == '}' {
            if c == '}' {
                let get = |k: &str| ent.iter().rev().find(|(a, _)| a == k).map(|(_, v)| v.clone());
                let model = get("model").unwrap_or_default();
                if let Some(num) = model.strip_prefix('*') {
                    if !num.is_empty() && num.chars().all(|d| d.is_ascii_digit()) {
                        let origin_text = get("origin").unwrap_or_else(|| "0 0 0".into());
                        let parts: Result<Vec<f64>, _> = origin_text.split_whitespace().map(str::parse::<f64>).collect();
                        let origin = match parts {
                            Ok(p) if p.len() == 3 => [p[0], p[1], p[2]],
                            _ => [0.0; 3],
                        };
                        if let Ok(n) = num.parse::<usize>() {
                            out.insert(n, (origin, get("classname").unwrap_or_default(), get("targetname").unwrap_or_default()));
                        }
                    }
                }
            }
            ent.clear();
            key = None;
            i += 1;
        } else if c == '"' {
            match text[i + 1..].iter().position(|&ch| ch == '"') {
                Some(len) => {
                    let s: String = text[i + 1..i + 1 + len].iter().collect();
                    match key.take() {
                        None => key = Some(s),
                        Some(k) => ent.push((k, s)),
                    }
                    i += len + 2;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    out
}

pub fn read_mesh(path: &Path) -> Result<Vec<u8>, String> {
    Ok(read_map(path, false)?.0)
}

/// The mesh, and with textures what it takes to draw it textured and lit (TAST).
pub fn read_map(path: &Path, textures: bool) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let data = std::fs::read(path).map_err(|e| crate::io_error(&e, path))?;
    if data.len() < 1036 || &data[..4] != b"VBSP" {
        return Err(format!("{}: not a Source map", name));
    }
    let version = i32_at(&data, 4);
    if version != 19 && version != 20 {
        return Err(format!("{}: map version {} isn't supported (CS:S maps are 19 or 20)", name, version));
    }
    build(&data, textures).map_err(|e| format!("{}: {}", name, e))
}

/// What the textured data is made from.
struct TexIn {
    texdata: Vec<u8>,
    strtab: Vec<u8>,
    strdata: Vec<u8>,
    light: Vec<u8>,
    kind: u32,
    lfaces: Option<Vec<u8>>, // the faces' light offsets (None: the faces lump's own)
}

fn build(data: &[u8], textures: bool) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    let read = |e: String| format!("can't read the map ({})", e);
    let verts = records(data, LUMP_VERTEXES, 12).map_err(read)?;
    let edges = records(data, LUMP_EDGES, 4).map_err(read)?;
    let surfedges = records(data, LUMP_SURFEDGES, 4).map_err(read)?;
    let texinfo = records(data, LUMP_TEXINFO, 72).map_err(read)?;
    let faces = records(data, LUMP_FACES, 56).map_err(read)?;
    let models = records(data, LUMP_MODELS, 48).map_err(read)?;
    let dispinfo = records(data, LUMP_DISPINFO, 176).map_err(read)?;
    let dispverts = records(data, LUMP_DISP_VERTS, 20).map_err(read)?;
    let ent_text: Vec<char> = lump(data, LUMP_ENTITIES).map_err(read)?.iter().map(|&b| b as char).collect();
    let ents = brush_entities(&ent_text);
    let tex = if textures {
        let texdata = records(data, LUMP_TEXDATA, 32).map_err(read)?;
        let strtab = records(data, LUMP_STRING_TABLE, 4).map_err(read)?;
        let strdata = lump(data, LUMP_STRING_DATA).map_err(read)?;
        // the faces' light: LDR if the map has it, else HDR (its offsets then are in the HDR faces' copy)
        let (mut light, mut kind, mut lfaces) = (lump(data, LUMP_LIGHTING).map_err(read)?, 1, None);
        if light.is_empty() {
            light = lump(data, LUMP_LIGHTING_HDR).map_err(read)?;
            kind = 2;
            let hdr = records(data, LUMP_FACES_HDR, 56).map_err(read)?;
            if hdr.len() == faces.len() {
                lfaces = Some(hdr);
            }
        }
        if light.is_empty() {
            kind = 0;
        }
        Some(TexIn { texdata, strtab, strdata, light, kind, lfaces })
    } else {
        None
    };

    let bad = || "the map's geometry is inconsistent".to_string();
    let n_flags = texinfo.len() / 72;
    let vert = |i: usize| -> Result<[f64; 3], String> {
        if i >= verts.len() / 12 {
            return Err(bad());
        }
        Ok([f32_at(&verts, 12 * i), f32_at(&verts, 12 * i + 4), f32_at(&verts, 12 * i + 8)])
    };
    let corner = |se: i64| -> Result<[f64; 3], String> {
        let e = se.unsigned_abs() as usize;
        if e >= edges.len() / 4 {
            return Err(bad());
        }
        vert(u16_at(&edges, 4 * e + if se >= 0 { 0 } else { 2 }) as usize)
    };

    let mut pos: Vec<f64> = Vec::new();
    let mut idx: Vec<u32> = Vec::new();
    // textures: per vertex the texture position (texels), the lightmap position (luxels; -0.5 without a
    // lightmap) and the blend; per triangle its material; per face (first vertex, vertices, lightmap item)
    let (mut uv, mut lux, mut blend): (Vec<f64>, Vec<f64>, Vec<f64>) = (Vec::new(), Vec::new(), Vec::new());
    let mut tri_mat: Vec<u16> = Vec::new();
    let mut face_items: Vec<(usize, usize, usize)> = Vec::new();
    let mut items: Vec<(i64, i64, i64)> = vec![(2, 2, -1)]; // lightmaps to pack: (width, height, offset); 0: the white
    for mi in 0..models.len() / 48 {
        if mi > 0 && ents.get(&mi).map(|e| is_trigger(&e.1)).unwrap_or(false) {
            continue; // the game never draws triggers
        }
        let o = if mi > 0 { ents.get(&mi).map(|e| e.0).unwrap_or([0.0; 3]) } else { [0.0; 3] };
        let (first, count) = (i32_at(&models, 48 * mi + 40) as i64, i32_at(&models, 48 * mi + 44) as i64);
        for fi in py_range(faces.len() / 56, first, first + count) {
            let f = 56 * fi as usize;
            let (firstedge, numedges) = (i32_at(&faces, f + 4) as i64, i16_at(&faces, f + 8) as i64);
            let (ti, disp) = (i16_at(&faces, f + 10) as i64, i16_at(&faces, f + 12) as i64);
            if ti < 0 || ti as usize >= n_flags || i32_at(&texinfo, 72 * ti as usize + 64) & HIDDEN != 0 || numedges < 3 {
                continue;
            }
            // a face's corners run clockwise seen from its front, and the game draws it in that order: that's
            // its front, whatever its plane and side flag say (Source's side flag repeats the plane number's
            // last bit, and the plane already faces the face's way; a few odd faces disagree with their plane)
            let mut poly: Vec<[f64; 3]> = Vec::with_capacity(numedges as usize);
            for k in 0..numedges {
                let s = firstedge + k;
                if s < 0 || s as usize >= surfedges.len() / 4 {
                    return Err(bad());
                }
                poly.push(corner(i32_at(&surfedges, 4 * s as usize) as i64)?);
            }
            let base = (pos.len() / 3) as u32;
            let first_tri = idx.len();
            let (mut tv, mut lv) = ([0.0f64; 8], [0.0f64; 8]);
            let (mut mat, mut lit, mut item) = (NO_MATERIAL, false, 0usize);
            let (mut smin, mut tmin, mut ls, mut lt) = (0i64, 0i64, 0i64, 0i64);
            if let Some(t) = &tex {
                let r = 72 * ti as usize;
                for k in 0..8 {
                    tv[k] = f32_at(&texinfo, r + 4 * k);
                    lv[k] = f32_at(&texinfo, r + 32 + 4 * k);
                }
                let td = i32_at(&texinfo, r + 68) as i64;
                if td >= 0 && td < ((t.texdata.len() / 32) as i64).min(NO_MATERIAL as i64) {
                    mat = td as u16;
                }
                let lf_bytes: &[u8] = t.lfaces.as_deref().unwrap_or(&faces);
                let lf = f;
                let lofs = i32_at(lf_bytes, lf + 20) as i64;
                smin = i32_at(lf_bytes, lf + 28) as i64;
                tmin = i32_at(lf_bytes, lf + 32) as i64;
                ls = i32_at(lf_bytes, lf + 36) as i64;
                lt = i32_at(lf_bytes, lf + 40) as i64;
                let (w, h) = (ls + 1, lt + 1);
                lit = t.kind > 0
                    && lofs >= 0
                    && lf_bytes[lf + 16] != 255
                    && 0 < w
                    && w <= MAX_LUXELS
                    && 0 < h
                    && h <= MAX_LUXELS
                    && lofs + 4 * w * h <= t.light.len() as i64;
                if lit {
                    item = items.len();
                    items.push((w + 2, h + 2, lofs));
                }
            }
            if disp >= 0 && numedges == 4 {
                let d = disp as usize;
                if d >= dispinfo.len() / 176 {
                    return Err(bad());
                }
                let di = 176 * d;
                let (sx, sy, sz) = (f32_at(&dispinfo, di), f32_at(&dispinfo, di + 4), f32_at(&dispinfo, di + 8));
                let (vstart, power) = (i32_at(&dispinfo, di + 12) as i64, i32_at(&dispinfo, di + 20));
                if !(2..=4).contains(&power) {
                    return Err(format!("a displacement of power {} (2 to 4 are valid)", power));
                }
                // the corner nearest the displacement's start position comes first
                let dist2 = |p: &[f64; 3]| (p[0] - sx) * (p[0] - sx) + (p[1] - sy) * (p[1] - sy) + (p[2] - sz) * (p[2] - sz);
                let mut k0 = 0;
                for k in 1..4 {
                    if dist2(&poly[k]) < dist2(&poly[k0]) {
                        k0 = k;
                    }
                }
                let c: Vec<[f64; 3]> = (0..4).map(|k| poly[(k0 + k) % 4]).collect();
                let size = (1usize << power) + 1;
                for i in 0..size {
                    let t = i as f64 / (size - 1) as f64;
                    let a: Vec<f64> = (0..3).map(|j| c[0][j] + (c[1][j] - c[0][j]) * t).collect();
                    let b: Vec<f64> = (0..3).map(|j| c[3][j] + (c[2][j] - c[3][j]) * t).collect();
                    for j in 0..size {
                        let u = j as f64 / (size - 1) as f64;
                        let vi = vstart + (i * size + j) as i64;
                        if vi < 0 || vi as usize >= dispverts.len() / 20 {
                            return Err(bad());
                        }
                        let dv = 20 * vi as usize;
                        let (vx, vy, vz, dist) = (f32_at(&dispverts, dv), f32_at(&dispverts, dv + 4), f32_at(&dispverts, dv + 8), f32_at(&dispverts, dv + 12));
                        // undisplaced
                        let (fx, fy, fz) = (a[0] + (b[0] - a[0]) * u, a[1] + (b[1] - a[1]) * u, a[2] + (b[2] - a[2]) * u);
                        pos.push(fx + vx * dist + o[0]);
                        pos.push(fy + vy * dist + o[1]);
                        pos.push(fz + vz * dist + o[2]);
                        if tex.is_some() {
                            uv.push(fx * tv[0] + fy * tv[1] + fz * tv[2] + tv[3]);
                            uv.push(fx * tv[4] + fy * tv[5] + fz * tv[6] + tv[7]);
                            if lit {
                                lux.push(ls as f64 * u);
                                lux.push(lt as f64 * t);
                            } else {
                                lux.push(-0.5);
                                lux.push(-0.5);
                            }
                            blend.push(f32_at(&dispverts, dv + 16));
                        }
                    }
                }
                // the grid's cells as the game splits them, all facing where the base face does (the grid runs
                // along its corners, so in its order a cell is clockwise from the front): a part bent past
                // square to the face, such as a sculpted ramp's far side, faces where the rest does
                let s = size as u32;
                for i in 0..s - 1 {
                    for j in 0..s - 1 {
                        let a = base + i * s + j;
                        let (b, cc, d) = (a + 1, a + s, a + s + 1);
                        if (i + j) % 2 == 0 {
                            idx.extend_from_slice(&[a, b, cc, b, d, cc]);
                        } else {
                            idx.extend_from_slice(&[a, d, cc, a, b, d]);
                        }
                    }
                }
            } else {
                for p in &poly {
                    pos.push(p[0] + o[0]);
                    pos.push(p[1] + o[1]);
                    pos.push(p[2] + o[2]);
                    if tex.is_some() {
                        let (x, y, z) = (p[0], p[1], p[2]);
                        uv.push(x * tv[0] + y * tv[1] + z * tv[2] + tv[3]);
                        uv.push(x * tv[4] + y * tv[5] + z * tv[6] + tv[7]);
                        if lit {
                            lux.push(x * lv[0] + y * lv[1] + z * lv[2] + lv[3] - smin as f64);
                            lux.push(x * lv[4] + y * lv[5] + z * lv[6] + lv[7] - tmin as f64);
                        } else {
                            lux.push(-0.5);
                            lux.push(-0.5);
                        }
                        blend.push(0.0);
                    }
                }
                for k in 1..poly.len() as u32 - 1 {
                    // a fan, turned to run counter-clockwise from the front
                    idx.extend_from_slice(&[base, base + k + 1, base + k]);
                }
            }
            if tex.is_some() {
                tri_mat.extend(std::iter::repeat(mat).take((idx.len() - first_tri) / 3));
                face_items.push((base as usize, pos.len() / 3 - base as usize, item));
            }
        }
    }

    let p32: Vec<f32> = pos.iter().map(|&v| v as f32).collect();
    let (mut mins, mut maxs) = ([0.0f32; 3], [0.0f32; 3]);
    if !p32.is_empty() {
        for j in 0..3 {
            let axis = p32.iter().skip(j).step_by(3);
            mins[j] = axis.clone().fold(f32::INFINITY, |m, &v| if v < m { v } else { m });
            maxs[j] = axis.fold(f32::NEG_INFINITY, |m, &v| if v > m { v } else { m });
        }
    }
    let mut out = Vec::with_capacity(40 + p32.len() * 4 + idx.len() * 4);
    out.extend_from_slice(b"TASM");
    for v in [MESH_VERSION, (p32.len() / 3) as u32, idx.len() as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in mins.iter().chain(maxs.iter()).chain(p32.iter()) {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for v in &idx {
        out.extend_from_slice(&v.to_le_bytes());
    }
    let textured = tex.map(|t| textured(pos.len() / 3, &uv, &lux, &blend, &tri_mat, &face_items, &items, &t));
    Ok((out, textured))
}

/// What the player collides with (TACL, see bspmap.py): every brush of every model, with its planes, and
/// the entity lump.
pub fn read_clip(path: &Path) -> Result<Vec<u8>, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let data = std::fs::read(path).map_err(|e| crate::io_error(&e, path))?;
    if data.len() < 1036 || &data[..4] != b"VBSP" {
        return Err(format!("{}: not a Source map", name));
    }
    let version = i32_at(&data, 4);
    if version != 19 && version != 20 {
        return Err(format!("{}: map version {} isn't supported (CS:S maps are 19 or 20)", name, version));
    }
    clip_brushes(&data).map_err(|e| format!("{}: {}", name, e))
}

fn clip_brushes(data: &[u8]) -> Result<Vec<u8>, String> {
    let read = |e: String| format!("can't read the map ({})", e);
    let planes = records(data, LUMP_PLANES, 20).map_err(read)?;
    let nodes = records(data, LUMP_NODES, 32).map_err(read)?;
    let leaf_size = if i32_at(data, 8 + 16 * LUMP_LEAFS + 8) == 0 { 56 } else { 32 };
    let leaves = records(data, LUMP_LEAFS, leaf_size).map_err(read)?;
    let leafbrushes = records(data, LUMP_LEAFBRUSHES, 2).map_err(read)?;
    let brushes = records(data, LUMP_BRUSHES, 12).map_err(read)?;
    let sides = records(data, LUMP_BRUSHSIDES, 8).map_err(read)?;
    let models = records(data, LUMP_MODELS, 48).map_err(read)?;
    let text = lump(data, LUMP_ENTITIES).map_err(read)?;

    let bad = || "the map's brushes are inconsistent".to_string();
    let (mut table, mut out): (Vec<u32>, Vec<f64>) = (Vec::new(), Vec::new());
    for mi in 0..models.len() / 48 {
        // the brushes of the leaves under its head node
        let mut found = std::collections::BTreeSet::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![i32_at(&models, 48 * mi + 36) as i64];
        while let Some(n) = stack.pop() {
            if n < 0 {
                let leaf = (-1 - n) as usize;
                if leaf >= leaves.len() / leaf_size {
                    return Err(bad());
                }
                let (first, count) = (u16_at(&leaves, leaf * leaf_size + 24) as i64, u16_at(&leaves, leaf * leaf_size + 26) as i64);
                for k in py_range(leafbrushes.len() / 2, first, first + count) {
                    found.insert(u16_at(&leafbrushes, 2 * k) as usize);
                }
            } else if seen.insert(n) {
                if n as usize >= nodes.len() / 32 {
                    return Err(bad());
                }
                let at = 32 * n as usize;
                stack.push(i32_at(&nodes, at + 4) as i64);
                stack.push(i32_at(&nodes, at + 8) as i64);
            }
        }
        for b in found {
            if b >= brushes.len() / 12 {
                return Err(bad());
            }
            let (first_side, n_sides) = (i32_at(&brushes, 12 * b) as i64, i32_at(&brushes, 12 * b + 4) as i64);
            let mut bs = Vec::new();
            for sd in py_range(sides.len() / 8, first_side, first_side + n_sides) {
                let i = u16_at(&sides, 8 * sd) as usize;
                if i >= planes.len() / 20 {
                    return Err(bad());
                }
                bs.push([f32_at(&planes, 20 * i), f32_at(&planes, 20 * i + 4), f32_at(&planes, 20 * i + 8), f32_at(&planes, 20 * i + 12)]);
            }
            table.extend_from_slice(&[(out.len() / 4) as u32, bs.len() as u32, u32_at(&brushes, 12 * b + 8), mi as u32]);
            for p in bs {
                out.extend_from_slice(&p);
            }
        }
    }
    let mut res = Vec::new();
    res.extend_from_slice(b"TACL");
    for v in [CLIP_VERSION, (table.len() / 4) as u32, (out.len() / 4) as u32, text.len() as u32] {
        res.extend_from_slice(&v.to_le_bytes());
    }
    for v in &table {
        res.extend_from_slice(&v.to_le_bytes());
    }
    put_f32s(&mut res, &out);
    res.extend_from_slice(&text);
    pad4(&mut res);
    Ok(res)
}

fn put_f32s(out: &mut Vec<u8>, vals: &[f64]) {
    for &v in vals {
        out.extend_from_slice(&(v as f32).to_le_bytes());
    }
}

fn pad4(out: &mut Vec<u8>) {
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

#[allow(clippy::too_many_arguments)]
fn textured(
    nverts: usize,
    uv: &[f64],
    lux: &[f64],
    blend: &[f64],
    tri_mat: &[u16],
    face_items: &[(usize, usize, usize)],
    items: &[(i64, i64, i64)],
    t: &TexIn,
) -> Vec<u8> {
    // the lightmaps in one atlas: rows ("shelves") of them, tallest first, in an atlas a power of two wide
    let area: i64 = items.iter().map(|&(w, h, _)| w * h).sum();
    let widest = items.iter().map(|&(w, _, _)| w).max().unwrap_or(0);
    let mut aw: i64 = 256;
    while (aw * aw < area || aw < widest) && aw < 8192 {
        aw *= 2;
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&k| (-items[k].1, k));
    let (mut px, mut py) = (vec![0i64; items.len()], vec![0i64; items.len()]);
    let (mut x, mut y, mut shelf) = (0i64, 0i64, 0i64);
    for &k in &order {
        let (w, h) = (items[k].0, items[k].1);
        if x + w > aw {
            x = 0;
            y += shelf;
            shelf = 0;
        }
        px[k] = x;
        py[k] = y;
        x += w;
        shelf = shelf.max(h);
    }
    let ah = y + shelf;
    let mut atlas = vec![0u8; (aw * ah * 4) as usize];
    for r in 0..2 {
        let o = (((py[0] + r) * aw + px[0]) * 4) as usize;
        atlas[o..o + 8].copy_from_slice(&[255, 255, 255, 0, 255, 255, 255, 0]);
    }
    for k in 1..items.len() {
        let (w, h, lofs) = ((items[k].0 - 2) as usize, (items[k].1 - 2) as usize, items[k].2 as usize);
        for r in 0..h + 2 {
            // with the edge luxels around it again, so filtering never reads past it
            let a = lofs + 4 * w * (r.max(1) - 1).min(h - 1);
            let row = &t.light[a..a + 4 * w];
            let o = (((py[k] + r as i64) * aw + px[k]) * 4) as usize;
            atlas[o..o + 4].copy_from_slice(&row[..4]);
            atlas[o + 4..o + 4 + 4 * w].copy_from_slice(row);
            atlas[o + 4 + 4 * w..o + 8 + 4 * w].copy_from_slice(&row[4 * w - 4..]);
        }
    }
    let mut lc = vec![0.0f64; lux.len()];
    for &(first, n, item) in face_items {
        let (x0, y0) = ((px[item] + 1) as f64, (py[item] + 1) as f64);
        for v in (2 * first..2 * (first + n)).step_by(2) {
            lc[v] = (x0 + lux[v] + 0.5) / aw as f64;
            lc[v + 1] = (y0 + lux[v + 1] + 0.5) / ah as f64;
        }
    }

    let n_td = t.texdata.len() / 32;
    let (mut table, mut names) = (Vec::with_capacity(24 * n_td), Vec::new());
    for k in 0..n_td {
        let r = 32 * k;
        let name_id = i32_at(&t.texdata, r + 12) as i64;
        let mut name: &[u8] = &[];
        if name_id >= 0 && (name_id as usize) < t.strtab.len() / 4 {
            let o = i32_at(&t.strtab, 4 * name_id as usize) as i64;
            if o >= 0 && (o as usize) < t.strdata.len() {
                let o = o as usize;
                let end = t.strdata[o..].iter().position(|&c| c == 0).map(|p| o + p).unwrap_or(t.strdata.len());
                name = &t.strdata[o..end];
            }
        }
        for c in 0..3 {
            table.extend_from_slice(&(f32_at(&t.texdata, r + 4 * c) as f32).to_le_bytes());
        }
        table.extend_from_slice(&i32_at(&t.texdata, r + 16).to_le_bytes());
        table.extend_from_slice(&i32_at(&t.texdata, r + 20).to_le_bytes());
        table.extend_from_slice(&(name.len() as u32).to_le_bytes());
        names.extend_from_slice(name);
    }
    let mut out = Vec::with_capacity(32 + nverts * 20 + tri_mat.len() * 2 + table.len() + names.len() + atlas.len() + 8);
    out.extend_from_slice(b"TAST");
    for v in [TEX_VERSION, nverts as u32, tri_mat.len() as u32, n_td as u32, aw as u32, ah as u32, t.kind] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    put_f32s(&mut out, uv);
    put_f32s(&mut out, &lc);
    put_f32s(&mut out, blend);
    for m in tri_mat {
        out.extend_from_slice(&m.to_le_bytes());
    }
    pad4(&mut out);
    out.extend_from_slice(&table);
    out.extend_from_slice(&names);
    pad4(&mut out);
    out.extend_from_slice(&atlas);
    out
}

/// The map's triggers as polygons (TATR).
/// The map's entities, each its keys and values (a key given twice: the last).
fn entities(text: &[char]) -> Vec<HashMap<String, String>> {
    let mut out = Vec::new();
    let mut ent = HashMap::new();
    let mut key: Option<String> = None;
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if c == '{' || c == '}' {
            if c == '}' {
                out.push(std::mem::take(&mut ent));
            }
            ent.clear();
            key = None;
            i += 1;
        } else if c == '"' {
            match text[i + 1..].iter().position(|&ch| ch == '"') {
                Some(len) => {
                    let s: String = text[i + 1..i + 1 + len].iter().collect();
                    match key.take() {
                        None => key = Some(s),
                        Some(k) => {
                            ent.insert(k, s);
                        }
                    }
                    i += len + 2;
                }
                None => i += 1,
            }
        } else {
            i += 1;
        }
    }
    out
}

/// n numbers, as an entity's origin or angles writes them; None unless exactly that many plain finite ones.
fn numbers(text: Option<&String>, n: usize) -> Option<Vec<f64>> {
    let parts: Vec<&str> = text.map(|t| t.split(crate::py::is_space).filter(|p| !p.is_empty()).collect()).unwrap_or_default();
    if parts.len() != n || !parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit() || "eE.+-".contains(c))) {
        return None;
    }
    let out: Vec<f64> = parts.iter().map(|p| p.parse::<f64>()).collect::<Result<_, _>>().ok()?;
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// What the map shows besides its brushes, for the 3D view (bspmap.py's read_props): its sky, its 3D
/// skybox's camera and its props, static and prop_* entities, as the same JSON.
pub fn read_props(path: &Path) -> Result<Value, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let data = std::fs::read(path).map_err(|e| crate::io_error(&e, path))?;
    if data.len() < 1036 || &data[..4] != b"VBSP" {
        return Err(format!("{}: not a Source map", name));
    }
    let version = i32_at(&data, 4);
    if version != 19 && version != 20 {
        return Err(format!("{}: map version {} isn't supported (CS:S maps are 19 or 20)", name, version));
    }
    props(&data).ok_or_else(|| format!("{}: the map's props are inconsistent", name))
}

#[derive(Default)]
struct Props {
    models: Vec<String>,
    index: HashMap<String, usize>,
    rows: Vec<Value>,
}

impl Props {
    fn add(&mut self, model: &str, xyz: &[f64], angles: &[f64], skin: i64) {
        let mut n = crate::py::strip(model).to_lowercase().replace('\\', "/");
        while n.contains("//") {
            n = n.replace("//", "/");
        }
        let m = match self.index.get(&n) {
            Some(&m) => m,
            None => {
                self.index.insert(n.clone(), self.models.len());
                self.models.push(n);
                self.models.len() - 1
            }
        };
        let mut row = vec![json!(m)];
        row.extend(xyz.iter().chain(angles).map(|&v| crate::py::num(crate::py::round(v, 2))));
        row.push(json!(skin));
        self.rows.push(Value::Array(row));
    }
}

fn props(data: &[u8]) -> Option<Value> {
    let mut out = Props::default();
    let glump = lump(data, LUMP_GAME_LUMP).ok()?;
    let count = if glump.len() >= 4 { i32_at(&glump, 0) } else { 0 };
    for k in 0..count.max(0) as usize {
        let h = 4 + 16 * k;
        if h + 16 > glump.len() {
            return None;
        }
        let (gid, ofs, length) = (i32_at(&glump, h), i32_at(&glump, h + 8) as i64, i32_at(&glump, h + 12) as i64);
        if gid != SPRP || ofs < 0 || length < 0 {
            continue;
        }
        let compressed = slice(data, ofs, ofs + 4) == b"LZMA";
        let sp = unlzma(if compressed { slice(data, ofs, data.len() as i64) } else { slice(data, ofs, ofs + length) }).ok()?;
        let int_at = |o: usize| (o + 4 <= sp.len()).then(|| i32_at(&sp, o));
        let nd = int_at(0)?;
        if nd < 0 || nd as usize > sp.len() / 128 {
            return None;
        }
        let nd = nd as usize;
        let names: Vec<String> = (0..nd)
            .map(|i| {
                let b = slice(&sp, 4 + 128 * i as i64, 4 + 128 * (i as i64 + 1));
                b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())].iter().map(|&c| c as char).collect()
            })
            .collect();
        let mut o = 4 + 128 * nd;
        let nl = int_at(o)?;
        o += 4 + 2 * nl.max(0) as usize;
        let n = int_at(o)?;
        o += 4;
        let size = if n > 0 { (sp.len() - o) / n as usize } else { 0 };
        if size >= 56 {
            // each: origin, angles, its model (the dictionary's), ..., skin
            for i in 0..n as usize {
                let e = o + i * size;
                let xyz = [f32_at(&sp, e), f32_at(&sp, e + 4), f32_at(&sp, e + 8)];
                let angles = [f32_at(&sp, e + 12), f32_at(&sp, e + 16), f32_at(&sp, e + 20)];
                let (kind, skin) = (u16_at(&sp, e + 24) as usize, i32_at(&sp, e + 32));
                if kind < nd && xyz.iter().chain(&angles).all(|v| v.is_finite()) {
                    out.add(&names[kind], &xyz, &angles, skin as i64);
                }
            }
        }
    }
    let text: Vec<char> = lump(data, LUMP_ENTITIES).ok()?.iter().map(|&b| b as char).collect();
    let (mut sky, mut sky_camera) = (String::new(), Value::Null);
    for ent in entities(&text) {
        let cls = ent.get("classname").map(String::as_str).unwrap_or("");
        let model = ent.get("model").map(String::as_str).unwrap_or("");
        if cls == "worldspawn" {
            sky = ent.get("skyname").cloned().unwrap_or_default();
        } else if cls == "sky_camera" && sky_camera.is_null() {
            let scale = ent.get("scale").cloned().unwrap_or_else(|| "16".into());
            if let Some(o3) = numbers(ent.get("origin"), 3) {
                let sc = numbers(Some(&scale), 1).map(|s| s[0]).filter(|&s| s > 0.0).unwrap_or(16.0);
                let mut v: Vec<Value> = o3.iter().map(|&x| crate::py::num(crate::py::round(x, 2))).collect();
                v.push(crate::py::num(sc));
                sky_camera = Value::Array(v);
            }
        } else if cls.starts_with("prop_")
            && model.to_lowercase().ends_with(".mdl")
            && ent.get("rendermode").map(String::as_str) != Some("10")
            && ent.get("StartDisabled").map(String::as_str) != Some("1")
        {
            if let Some(o3) = numbers(ent.get("origin"), 3) {
                let skin = crate::py::strip(ent.get("skin").map(String::as_str).unwrap_or("0"));
                let digits = skin.strip_prefix(['+', '-']).unwrap_or(skin);
                let skin = if (1..=9).contains(&digits.len()) && digits.chars().all(|c| c.is_ascii_digit()) { skin.parse().unwrap_or(0) } else { 0 };
                out.add(model, &o3, &numbers(ent.get("angles"), 3).unwrap_or(vec![0.0; 3]), skin);
            }
        }
    }
    Some(json!({"error": null, "sky": sky, "sky_camera": sky_camera, "models": out.models, "props": out.rows}))
}

pub fn read_triggers(path: &Path) -> Result<Vec<u8>, String> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let data = std::fs::read(path).map_err(|e| crate::io_error(&e, path))?;
    if data.len() < 1036 || &data[..4] != b"VBSP" {
        return Err(format!("{}: not a Source map", name));
    }
    let version = i32_at(&data, 4);
    if version != 19 && version != 20 {
        return Err(format!("{}: map version {} isn't supported (CS:S maps are 19 or 20)", name, version));
    }
    triggers(&data).map_err(|e| format!("{}: {}", name, e))
}

fn triggers(data: &[u8]) -> Result<Vec<u8>, String> {
    let read = |e: String| format!("can't read the map ({})", e);
    let planes = records(data, LUMP_PLANES, 20).map_err(read)?;
    let nodes = records(data, LUMP_NODES, 32).map_err(read)?;
    // leaves: 56 bytes in version 0 of the lump (with their ambient light), 32 after
    let leaf_size = if i32_at(data, 8 + 16 * LUMP_LEAFS + 8) == 0 { 56 } else { 32 };
    let leaves = records(data, LUMP_LEAFS, leaf_size).map_err(read)?;
    let leafbrushes = records(data, LUMP_LEAFBRUSHES, 2).map_err(read)?;
    let brushes = records(data, LUMP_BRUSHES, 12).map_err(read)?;
    let sides = records(data, LUMP_BRUSHSIDES, 8).map_err(read)?;
    let models = records(data, LUMP_MODELS, 48).map_err(read)?;
    let ent_text: Vec<char> = lump(data, LUMP_ENTITIES).map_err(read)?.iter().map(|&b| b as char).collect();
    let ents = brush_entities(&ent_text);

    let bad = || "the map's triggers are inconsistent".to_string();
    let plane = |i: usize| -> Result<[f64; 4], String> {
        if i >= planes.len() / 20 {
            return Err(bad());
        }
        Ok([f32_at(&planes, 20 * i), f32_at(&planes, 20 * i + 4), f32_at(&planes, 20 * i + 8), f32_at(&planes, 20 * i + 12)])
    };
    let mut keys: Vec<usize> = ents.keys().copied().collect();
    keys.sort_unstable();
    let (mut heads, mut counts, mut pos): (Vec<(usize, usize, Vec<u8>, Vec<u8>)>, Vec<u32>, Vec<f64>) = (Vec::new(), Vec::new(), Vec::new());
    for mi in keys {
        let (origin, cls, tname) = &ents[&mi];
        if !is_trigger(cls) || !(0 < mi && mi < models.len() / 48) {
            continue;
        }
        // the model's brushes: those of the leaves under its head node
        let mut found = std::collections::BTreeSet::new();
        let mut seen = std::collections::HashSet::new();
        let mut stack = vec![i32_at(&models, 48 * mi + 36) as i64];
        while let Some(n) = stack.pop() {
            if n < 0 {
                let leaf = (-1 - n) as usize;
                if leaf >= leaves.len() / leaf_size {
                    return Err(bad());
                }
                let (first, count) = (u16_at(&leaves, leaf * leaf_size + 24) as i64, u16_at(&leaves, leaf * leaf_size + 26) as i64);
                for k in py_range(leafbrushes.len() / 2, first, first + count) {
                    found.insert(u16_at(&leafbrushes, 2 * k) as usize);
                }
            } else if seen.insert(n) {
                if n as usize >= nodes.len() / 32 {
                    return Err(bad());
                }
                let at = 32 * n as usize;
                stack.push(i32_at(&nodes, at + 4) as i64);
                stack.push(i32_at(&nodes, at + 8) as i64);
            }
        }
        let first_poly = counts.len();
        for b in found {
            if b >= brushes.len() / 12 {
                return Err(bad());
            }
            let (first_side, n_sides) = (i32_at(&brushes, 12 * b) as i64, i32_at(&brushes, 12 * b + 4) as i64);
            let mut bp = Vec::new();
            for sd in py_range(sides.len() / 8, first_side, first_side + n_sides) {
                if i16_at(&sides, 8 * sd + 6) == 0 {
                    bp.push(plane(u16_at(&sides, 8 * sd) as usize)?);
                }
            }
            for (i, p) in bp.iter().enumerate() {
                let mut w = base_winding(p);
                for (j, other) in bp.iter().enumerate() {
                    if j != i && !w.is_empty() {
                        w = clip(w, other);
                    }
                }
                if w.len() >= 3 {
                    counts.push(w.len() as u32);
                    for v in &w {
                        pos.extend_from_slice(&[v[0] + origin[0], v[1] + origin[1], v[2] + origin[2]]);
                    }
                }
            }
        }
        let latin1 = |s: &str| s.chars().map(|c| c as u32 as u8).collect::<Vec<u8>>();
        heads.push((first_poly, counts.len() - first_poly, latin1(cls), latin1(tname)));
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"TATR");
    for v in [TRIG_VERSION, heads.len() as u32, counts.len() as u32, (pos.len() / 3) as u32] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    for (f, n, c, nm) in &heads {
        for v in [*f as u32, *n as u32, c.len() as u32, nm.len() as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    for (_, _, c, nm) in &heads {
        out.extend_from_slice(c);
        out.extend_from_slice(nm);
    }
    pad4(&mut out);
    for c in &counts {
        out.extend_from_slice(&c.to_le_bytes());
    }
    put_f32s(&mut out, &pos);
    Ok(out)
}

/// bigger than any map: a plane's first polygon, before the brush's other planes cut it
const BOX: f64 = 131072.0;
const ON_EPSILON: f64 = 0.1;

/// A huge square on the plane (as the map compiler makes it).
fn base_winding(p: &[f64; 4]) -> Vec<[f64; 3]> {
    let (nx, ny, nz, d) = (p[0], p[1], p[2], p[3]);
    let n = [nx, ny, nz];
    let mut axis = 0;
    for i in 1..3 {
        if n[i].abs() > n[axis].abs() {
            axis = i;
        }
    }
    let (mut ux, mut uy, mut uz) = if axis < 2 { (0.0, 0.0, 1.0) } else { (1.0, 0.0, 0.0) };
    let v = ux * nx + uy * ny + uz * nz;
    (ux, uy, uz) = (ux - v * nx, uy - v * ny, uz - v * nz);
    let length = (ux * ux + uy * uy + uz * uz).sqrt();
    (ux, uy, uz) = (ux / length * BOX, uy / length * BOX, uz / length * BOX);
    let (rx, ry, rz) = (uy * nz - uz * ny, uz * nx - ux * nz, ux * ny - uy * nx); // up x normal
    let (ox, oy, oz) = (nx * d, ny * d, nz * d);
    vec![
        [ox - rx + ux, oy - ry + uy, oz - rz + uz],
        [ox + rx + ux, oy + ry + uy, oz + rz + uz],
        [ox + rx - ux, oy + ry - uy, oz + rz - uz],
        [ox - rx - ux, oy - ry - uy, oz - rz - uz],
    ]
}

/// The part of the polygon behind the plane (inside the brush).
fn clip(w: Vec<[f64; 3]>, p: &[f64; 4]) -> Vec<[f64; 3]> {
    let (nx, ny, nz, d) = (p[0], p[1], p[2], p[3]);
    let dists: Vec<f64> = w.iter().map(|q| q[0] * nx + q[1] * ny + q[2] * nz - d).collect();
    let sides: Vec<i32> = dists.iter().map(|&e| if e > ON_EPSILON { 1 } else if e < -ON_EPSILON { -1 } else { 0 }).collect();
    if !sides.contains(&1) {
        return w;
    }
    if !sides.contains(&-1) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..w.len() {
        if sides[i] <= 0 {
            out.push(w[i]);
        }
        let k = (i + 1) % w.len();
        if sides[i] == 0 || sides[k] == 0 || sides[k] == sides[i] {
            continue;
        }
        let t = dists[i] / (dists[i] - dists[k]);
        let n = [nx, ny, nz];
        let mut mid = [0.0; 3];
        for j in 0..3 {
            mid[j] = if n[j] == 1.0 { d } else if n[j] == -1.0 { -d } else { w[i][j] + t * (w[k][j] - w[i][j]) };
        }
        out.push(mid);
    }
    out
}

/// The map's .bsp in cstrike/maps or cstrike/download/maps (custom maps from servers).
pub fn find_map(cstrike_dir: &Path, name: &str) -> Option<PathBuf> {
    [cstrike_dir.join("maps"), cstrike_dir.join("download").join("maps")]
        .into_iter()
        .map(|d| d.join(format!("{}.bsp", name)))
        .find(|p| p.is_file())
}

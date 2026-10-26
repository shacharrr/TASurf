"""Read a Source map (.bsp) as a triangle mesh, for the editor's 3D view.

The editor serves it (/api/map, /api/maptex, /api/maptrig, /api/mapclip, /api/mapprops); rust/src/bspmap.rs
writes the same bytes. The instant prediction (editor_static/predict.js) uses the map's brushes from here too.

Only what it takes to see the map: the faces of the world and of brush
entities (func_brush, func_door, ...), and displacements, as triangles. Tool
faces (nodraw, sky, triggers, hints, skips) are left out, and so are the
models of triggers (trigger_*, buy zones, ...), which the game never draws even
where an old compiler kept their faces. Lumps the map compiler stored
LZMA-compressed are unpacked. The format:
https://developer.valvesoftware.com/wiki/BSP_(Source)

The mesh is little-endian binary:
  b"TASM", u32 version (1), u32 vertex count, u32 index count,
  f32 mins[3], f32 maxs[3], f32 positions[3 * vertices], u32 indices[...]
Each face gets its own vertices (flat shading needs no sharing). Triangles are
wound counter-clockwise seen from the front of their surface, so a viewer can
leave out the backs (and see through walls from behind them).

For the textured look, the same vertices and triangles, in the same order, have
more (little-endian binary too):
  b"TAST", u32 version (1), u32 vertex count, u32 triangle count, u32 material
  count, u32 atlas width, u32 atlas height, u32 lighting (0 none: draw
  fullbright, 1 from the map's LDR lighting, 2 from its HDR lighting),
  f32 texcoords[2 * vertices]    in texels: divide by the texture's size
  f32 lightcoords[2 * vertices]  in the lightmap atlas, 0 to 1
  f32 blend[vertices]            displacements' alpha, 0 ($basetexture) to 255
                                 ($basetexture2); 0 off displacements
  u16 material[triangles]        the triangle's texdata (65535: none), then
                                 zeros to a multiple of 4 bytes
  per material (texdata): f32 reflectivity[3] (its average colour, linear),
                          i32 width, i32 height (its texture's, at compile
                          time), u32 name length
  the names, one after another (as in the map: materials/<name>.vmt), then
  zeros to a multiple of 4 bytes
  u8 atlas[width * height * 4]   every face's lightmap as the map stores it
                                 (r, g, b, signed exponent: r * 2^e / 255 is
                                 linear light), each with a border of its edge
                                 luxels; a 2 x 2 block of white (1.0) for the
                                 faces without one
A lightmap is the first one the face has (its first light style; with bump
maps, the first of the four is the unbumped one). Displacements' lightmaps are
laid over the displacement's grid: t along the edge from the start corner, s
along the other one.

The triggers, for drawing them as volumes (little-endian binary):
  b"TATR", u32 version (1), u32 trigger count, u32 polygon count, u32 vertex
  count,
  per trigger: u32 first polygon, u32 polygon count, u32 class length, u32 name
               length (its classname and targetname)
  the classes and names, one after another, then zeros to a multiple of 4 bytes
  u32 vertices[polygons]  each polygon's vertex count
  f32 positions[3 * vertices]
A trigger is a brush entity whose class starts with trigger_, or a CS:S zone
(func_buyzone, func_bomb_target, ...), by model number. Its polygons
are its brushes' sides, from the brushes' planes (as the game's collision sees
the trigger, whether or not the compiler kept faces for it): each side's plane
cut by the brush's other planes, bevel planes left out.

What the player collides with, for the editor's instant prediction of the path
(little-endian binary): every brush of every model, as the game's collision sees
it (its planes, bevels included, which is what a moving box needs), and the
entities that say which models are what:
  b"TACL", u32 version (1), u32 brush count, u32 plane count, u32 entity text
  length,
  per brush: u32 first plane, u32 plane count, u32 contents, u32 model
  f32 planes[4 * planes]   normal x, y, z and distance, the brush's sides in order
  the entity lump as the map has it, then zeros to a multiple of 4 bytes
Brushes come model by model (0 is the world), each model's in order. A brush
entity's brushes are relative to its origin, as its faces are.
"""

from __future__ import annotations

import lzma
import math
import re
import struct
from array import array
from pathlib import Path

LUMP_ENTITIES, LUMP_PLANES, LUMP_TEXDATA, LUMP_VERTEXES, LUMP_TEXINFO, LUMP_FACES = 0, 1, 2, 3, 6, 7
LUMP_LIGHTING, LUMP_EDGES, LUMP_SURFEDGES, LUMP_MODELS, LUMP_DISPINFO, LUMP_DISP_VERTS = 8, 12, 13, 14, 26, 33
LUMP_STRING_DATA, LUMP_STRING_TABLE, LUMP_LIGHTING_HDR, LUMP_FACES_HDR = 43, 44, 53, 58
LUMP_NODES, LUMP_LEAFS, LUMP_LEAFBRUSHES, LUMP_BRUSHES, LUMP_BRUSHSIDES = 5, 10, 17, 18, 19
LUMP_GAME_LUMP = 35
SPRP = int.from_bytes(b"sprp", "big")  # the game lump of the static props
ZONES = {"func_buyzone", "func_bomb_target", "func_hostage_rescue", "func_vip_safetyzone", "func_escapezone"}
FACE = "<HBBihhhh4Bif2i2iiHHI"

# texinfo flags of faces that aren't drawn in the game: SKY2D, SKY, TRIGGER, NODRAW, HINT, SKIP
HIDDEN = 0x2 | 0x4 | 0x40 | 0x80 | 0x100 | 0x200
MESH_VERSION = 1
TEX_VERSION = 1
TRIG_VERSION = 1
CLIP_VERSION = 1
NO_MATERIAL = 0xFFFF
MAX_LUXELS = 1024  # a side of a face's lightmap; the game's are at most 128, so more is a broken map


class MapError(ValueError):
    pass


def _lump(data: bytes, index: int) -> bytes:
    ofs, length = struct.unpack_from("<ii", data, 8 + 16 * index)
    return _unlzma(data[ofs:ofs + length])


def _unlzma(raw: bytes) -> bytes:
    """A lump as the map compiler stored it: as it is, or LZMA-compressed."""
    if raw[:4] != b"LZMA":
        return raw
    # Valve's LZMA lump: "LZMA", actual size, compressed size, 5 property bytes, then the stream
    actual, compressed = struct.unpack_from("<II", raw, 4)
    return _lzma(raw[12:17], actual, raw[17:17 + compressed])


def _lzma(props: bytes, size: int, stream: bytes) -> bytes:
    """An LZMA stream (with its 5 property bytes) of a known size; whatever follows that many bytes, such as
    an end marker, is ignored (liblzma's own reader of sized streams rejects end markers)."""
    if len(props) < 5 or props[0] >= 9 * 5 * 5:
        raise lzma.LZMAError("Invalid or unsupported options")
    lc, lp, pb = props[0] % 9, props[0] // 9 % 5, props[0] // 45
    filters = [{"id": lzma.FILTER_LZMA1, "lc": lc, "lp": lp, "pb": pb, "dict_size": struct.unpack_from("<I", props, 1)[0]}]
    out = lzma.LZMADecompressor(lzma.FORMAT_RAW, filters=filters).decompress(stream, max_length=size)
    if len(out) < size:
        raise lzma.LZMAError("Compressed data ended before the end-of-stream marker was reached")
    return out


def is_trigger(classname: str) -> bool:
    return classname.startswith("trigger_") or classname in ZONES


def _brush_entities(text: str) -> dict[int, tuple[tuple[float, float, float], str, str]]:
    """Brush entities by model number: their origin (their faces are stored relative to it), class and name."""
    out: dict[int, tuple[tuple[float, float, float], str, str]] = {}
    ent: dict[str, str] = {}
    key = None
    for m in re.finditer(r'"([^"]*)"|([{}])', text):
        if m.group(2) == "{":
            ent, key = {}, None
        elif m.group(2) == "}":
            model = ent.get("model", "")
            if model.startswith("*") and model[1:].isdigit():
                try:
                    origin = tuple(float(v) for v in ent.get("origin", "0 0 0").split())
                except ValueError:
                    origin = ()
                out[int(model[1:])] = (origin if len(origin) == 3 else (0.0, 0.0, 0.0),  # type: ignore[assignment]
                                       ent.get("classname", ""), ent.get("targetname", ""))
            ent, key = {}, None
        elif key is None:
            key = m.group(1)
        else:
            ent[key] = m.group(1)
            key = None
    return out


def read_mesh(path: Path) -> bytes:
    return read_map(path)[0]


def read_map(path: Path, textures: bool = False) -> tuple[bytes, bytes | None]:
    """The mesh, and with textures what it takes to draw it textured and lit (the TAST data above)."""
    data = path.read_bytes()
    if len(data) < 1036:
        raise MapError(f"{path.name}: not a Source map")
    ident, version = struct.unpack_from("<4si", data, 0)
    if ident != b"VBSP":
        raise MapError(f"{path.name}: not a Source map")
    if version not in (19, 20):
        raise MapError(f"{path.name}: map version {version} isn't supported (CS:S maps are 19 or 20)")
    try:
        verts = [v for v in struct.iter_unpack("<3f", _lump(data, LUMP_VERTEXES))]
        edges = [e for e in struct.iter_unpack("<2H", _lump(data, LUMP_EDGES))]
        surfedges = [s for (s,) in struct.iter_unpack("<i", _lump(data, LUMP_SURFEDGES))]
        texinfo = [t for t in struct.iter_unpack("<16fii", _lump(data, LUMP_TEXINFO))]
        faces = [f for f in struct.iter_unpack(FACE, _lump(data, LUMP_FACES))]
        models = [m for m in struct.iter_unpack("<9f3i", _lump(data, LUMP_MODELS))]
        dispinfo = [d for d in struct.iter_unpack("<3fiii152x", _lump(data, LUMP_DISPINFO))]
        dispverts = [d for d in struct.iter_unpack("<5f", _lump(data, LUMP_DISP_VERTS))]
        ents = _brush_entities(_lump(data, LUMP_ENTITIES).decode("latin-1"))
        if textures:
            texdata = [t for t in struct.iter_unpack("<3fi4i", _lump(data, LUMP_TEXDATA))]
            strtab = [o for (o,) in struct.iter_unpack("<i", _lump(data, LUMP_STRING_TABLE))]
            strdata = _lump(data, LUMP_STRING_DATA)
            # the faces' light: LDR if the map has it, else HDR (its offsets then are in the HDR faces' copy)
            light, kind, lfaces = _lump(data, LUMP_LIGHTING), 1, faces
            if not light:
                light, kind = _lump(data, LUMP_LIGHTING_HDR), 2
                hdr_faces = [f for f in struct.iter_unpack(FACE, _lump(data, LUMP_FACES_HDR))]
                if len(hdr_faces) == len(faces):
                    lfaces = hdr_faces
            if not light:
                kind = 0
    except (struct.error, lzma.LZMAError) as e:
        raise MapError(f"{path.name}: can't read the map ({e})") from None

    pos: list[float] = []
    idx: list[int] = []
    # textures: per vertex the texture position (texels), the lightmap position (luxels; -0.5 without a
    # lightmap) and the blend; per triangle its material; per face (first vertex, vertices, lightmap item)
    uv: list[float] = []
    lux: list[float] = []
    blend: list[float] = []
    tri_mat: list[int] = []
    face_items: list[tuple[int, int, int]] = []
    items: list[tuple[int, int, int]] = [(2, 2, -1)]  # lightmaps to pack: (width, height, offset); 0: the white

    def corner(se: int) -> tuple[float, float, float]:
        e = edges[se] if se >= 0 else edges[-se]
        return verts[e[0] if se >= 0 else e[1]]

    try:
        for mi, model in enumerate(models):
            if mi and mi in ents and is_trigger(ents[mi][1]):
                continue  # the game never draws triggers
            ox, oy, oz = ents[mi][0] if mi and mi in ents else (0.0, 0.0, 0.0)
            first, count = model[10], model[11]
            for fi in range(len(faces))[first:first + count]:
                f = faces[fi]
                firstedge, numedges, ti, disp = f[3], f[4], f[5], f[6]
                if ti < 0 or ti >= len(texinfo) or texinfo[ti][16] & HIDDEN or numedges < 3:
                    continue
                # a face's corners run clockwise seen from its front, and the game draws it in that order: that's
                # its front, whatever its plane and side flag say (Source's side flag repeats the plane number's
                # last bit, and the plane already faces the face's way; a few odd faces disagree with their plane)
                poly = [corner(surfedges[firstedge + k]) for k in range(numedges)]
                base, first_tri = len(pos) // 3, len(idx)
                if textures:
                    tv, lv, td = texinfo[ti][0:8], texinfo[ti][8:16], texinfo[ti][17]
                    mat = td if 0 <= td < min(len(texdata), NO_MATERIAL) else NO_MATERIAL
                    lf = lfaces[fi]
                    lofs, smin, tmin, w, h = lf[12], lf[14], lf[15], lf[16] + 1, lf[17] + 1
                    lit = (kind > 0 and lofs >= 0 and lf[8] != 255 and 0 < w <= MAX_LUXELS and 0 < h <= MAX_LUXELS
                           and lofs + 4 * w * h <= len(light))
                    item = len(items) if lit else 0
                    if lit:
                        items.append((w + 2, h + 2, lofs))
                if disp >= 0 and numedges == 4:
                    sx, sy, sz, vstart, _, power = dispinfo[disp]
                    if not 2 <= power <= 4:
                        raise MapError(f"{path.name}: a displacement of power {power} (2 to 4 are valid)")
                    # the corner nearest the displacement's start position comes first
                    k0 = min(range(4), key=lambda k: (poly[k][0] - sx) ** 2 + (poly[k][1] - sy) ** 2 + (poly[k][2] - sz) ** 2)
                    c = poly[k0:] + poly[:k0]
                    size = (1 << power) + 1
                    for i in range(size):
                        t = i / (size - 1)
                        ax, ay, az = (c[0][j] + (c[1][j] - c[0][j]) * t for j in range(3))
                        bx, by, bz = (c[3][j] + (c[2][j] - c[3][j]) * t for j in range(3))
                        for j in range(size):
                            u = j / (size - 1)
                            vx, vy, vz, dist, alpha = dispverts[vstart + i * size + j]
                            fx, fy, fz = ax + (bx - ax) * u, ay + (by - ay) * u, az + (bz - az) * u  # undisplaced
                            pos += (fx + vx * dist + ox, fy + vy * dist + oy, fz + vz * dist + oz)
                            if textures:
                                uv += (fx * tv[0] + fy * tv[1] + fz * tv[2] + tv[3], fx * tv[4] + fy * tv[5] + fz * tv[6] + tv[7])
                                lux += (lf[16] * u, lf[17] * t) if lit else (-0.5, -0.5)
                                blend.append(alpha)
                    # the grid's cells as the game splits them, all facing where the base face does (the grid runs
                    # along its corners, so in its order a cell is clockwise from the front): a part bent past
                    # square to the face, such as a sculpted ramp's far side, faces where the rest does
                    for i in range(size - 1):
                        for j in range(size - 1):
                            a = base + i * size + j
                            b, cc, d = a + 1, a + size, a + size + 1
                            idx += (a, b, cc, b, d, cc) if (i + j) % 2 == 0 else (a, d, cc, a, b, d)
                else:
                    for x, y, z in poly:
                        pos += (x + ox, y + oy, z + oz)
                        if textures:
                            uv += (x * tv[0] + y * tv[1] + z * tv[2] + tv[3], x * tv[4] + y * tv[5] + z * tv[6] + tv[7])
                            lux += ((x * lv[0] + y * lv[1] + z * lv[2] + lv[3] - smin, x * lv[4] + y * lv[5] + z * lv[6] + lv[7] - tmin)
                                    if lit else (-0.5, -0.5))
                            blend.append(0.0)
                    for k in range(1, numedges - 1):  # a fan, turned to run counter-clockwise from the front
                        idx += (base, base + k + 1, base + k)
                if textures:
                    tri_mat += [mat] * ((len(idx) - first_tri) // 3)
                    face_items.append((base, len(pos) // 3 - base, item))
    except IndexError:
        raise MapError(f"{path.name}: the map's geometry is inconsistent") from None

    p32 = array("f", pos)
    if p32:
        mins = [min(p32[j::3]) for j in range(3)]
        maxs = [max(p32[j::3]) for j in range(3)]
    else:
        mins = maxs = [0.0, 0.0, 0.0]
    i32 = array("I", idx)
    if p32.itemsize != 4 or i32.itemsize != 4:
        raise MapError("this Python's float/int arrays aren't 32-bit")
    head = b"TASM" + struct.pack("<III6f", MESH_VERSION, len(p32) // 3, len(i32), *mins, *maxs)
    if BIG_ENDIAN:  # the mesh is little-endian
        p32.byteswap()
        i32.byteswap()
    mesh = head + p32.tobytes() + i32.tobytes()
    if not textures:
        return mesh, None
    return mesh, _textured(len(pos) // 3, uv, lux, blend, tri_mat, face_items, items, light, kind, texdata, strtab, strdata)


BIG_ENDIAN = struct.pack("=I", 1) != struct.pack("<I", 1)


def _le(a: array) -> bytes:
    if BIG_ENDIAN:
        a.byteswap()
    return a.tobytes()


def _pad4(b: bytes) -> bytes:
    return b + b"\0" * (-len(b) % 4)


def _textured(nverts: int, uv: list[float], lux: list[float], blend: list[float], tri_mat: list[int],
              face_items: list[tuple[int, int, int]], items: list[tuple[int, int, int]], light: bytes, kind: int,
              texdata: list[tuple], strtab: list[int], strdata: bytes) -> bytes:
    # the lightmaps in one atlas: rows ("shelves") of them, tallest first, in an atlas a power of two wide
    area = sum(w * h for w, h, _ in items)
    widest = max(w for w, _, _ in items)
    aw = 256
    while (aw * aw < area or aw < widest) and aw < 8192:
        aw *= 2
    px, py = [0] * len(items), [0] * len(items)
    x = y = shelf = 0
    for k in sorted(range(len(items)), key=lambda k: (-items[k][1], k)):
        w, h = items[k][0], items[k][1]
        if x + w > aw:
            x, y, shelf = 0, y + shelf, 0
        px[k], py[k] = x, y
        x += w
        shelf = max(shelf, h)
    ah = y + shelf
    atlas = bytearray(aw * ah * 4)
    for r in range(2):
        o = ((py[0] + r) * aw + px[0]) * 4
        atlas[o:o + 8] = b"\xff\xff\xff\x00" * 2
    for k in range(1, len(items)):
        w, h, lofs = items[k][0] - 2, items[k][1] - 2, items[k][2]
        for r in range(h + 2):  # with the edge luxels around it again, so filtering never reads past it
            a = lofs + 4 * w * min(max(r - 1, 0), h - 1)
            row = light[a:a + 4 * w]
            o = ((py[k] + r) * aw + px[k]) * 4
            atlas[o:o + 4 * w + 8] = row[:4] + row + row[-4:]
    lc = [0.0] * len(lux)
    for first, n, item in face_items:
        x0, y0 = px[item] + 1, py[item] + 1
        for v in range(2 * first, 2 * (first + n), 2):
            lc[v] = (x0 + lux[v] + 0.5) / aw
            lc[v + 1] = (y0 + lux[v + 1] + 0.5) / ah

    table, names = [], []
    for r, g, b, name_id, w, h, _vw, _vh in texdata:
        name = b""
        if 0 <= name_id < len(strtab) and 0 <= strtab[name_id] < len(strdata):
            o = strtab[name_id]
            end = strdata.find(b"\0", o)
            name = strdata[o:end if end >= 0 else len(strdata)]
        table.append(struct.pack("<3f2iI", r, g, b, w, h, len(name)))
        names.append(name)
    head = b"TAST" + struct.pack("<7I", TEX_VERSION, nverts, len(tri_mat), len(texdata), aw, ah, kind)
    return (head + _le(array("f", uv)) + _le(array("f", lc)) + _le(array("f", blend)) + _pad4(_le(array("H", tri_mat)))
            + b"".join(table) + _pad4(b"".join(names)) + bytes(atlas))


def _entities(text: str) -> list[dict[str, str]]:
    """The map's entities, each its keys and values (a key given twice: the last)."""
    out, ent, key = [], {}, None
    for m in re.finditer(r'"([^"]*)"|([{}])', text):
        if m.group(2) == "{":
            ent, key = {}, None
        elif m.group(2) == "}":
            out.append(ent)
            ent, key = {}, None
        elif key is None:
            key = m.group(1)
        else:
            ent[key] = m.group(1)
            key = None
    return out


def _numbers(text: str | None, n: int) -> list[float] | None:
    """n numbers, as an entity's origin or angles writes them; None unless exactly that many plain finite ones."""
    parts = (text or "").split()
    if len(parts) != n or not all(re.fullmatch(r"[0-9eE.+-]+", p) for p in parts):
        return None
    try:
        out = [float(p) for p in parts]
    except ValueError:
        return None
    return out if all(math.isfinite(v) for v in out) else None


def read_props(path: Path) -> dict:
    """What the map shows besides its brushes, for the 3D view: its sky (worldspawn's skyname: the six textures
    around everything), where its 3D skybox's camera is (sky_camera: origin and scale, or None) and its props,
    the models placed on it: the static ones (compiled into the map: the game lump's sprp) and the prop_*
    entities (prop_dynamic, prop_physics, ...; not those drawn as nothing, rendermode 10, or hidden at the start,
    StartDisabled). models: their files, lower case; props: [model, x, y, z, pitch, yaw, roll, skin] each, to two
    decimals."""
    data = path.read_bytes()
    if len(data) < 1036 or data[:4] != b"VBSP":
        raise MapError(f"{path.name}: not a Source map")
    version, = struct.unpack_from("<i", data, 4)
    if version not in (19, 20):
        raise MapError(f"{path.name}: map version {version} isn't supported (CS:S maps are 19 or 20)")
    models: list[str] = []
    index: dict[str, int] = {}
    props: list[list] = []

    def model_of(name: str) -> int:
        n = re.sub(r"/+", "/", name.strip().lower().replace("\\", "/"))
        if n not in index:
            index[n] = len(models)
            models.append(n)
        return index[n]

    def add(model: int, xyz, angles, skin: int):
        props.append([model, *(round(v, 2) for v in xyz), *(round(v, 2) for v in angles), skin])

    try:
        glump = _lump(data, LUMP_GAME_LUMP)
        count = struct.unpack_from("<i", glump, 0)[0] if len(glump) >= 4 else 0
        for k in range(max(0, count)):
            gid, _, _, ofs, length = struct.unpack_from("<iHHii", glump, 4 + 16 * k)
            if gid != SPRP or ofs < 0 or length < 0:
                continue
            sp = _unlzma(data[ofs:] if data[ofs:ofs + 4] == b"LZMA" else data[ofs:ofs + length])
            nd = struct.unpack_from("<i", sp, 0)[0]
            if not 0 <= nd <= len(sp) // 128:
                raise struct.error("the model dictionary runs past the end")
            names = [sp[4 + 128 * i:4 + 128 * (i + 1)].split(b"\0")[0].decode("latin-1") for i in range(nd)]
            o = 4 + 128 * nd
            nl = struct.unpack_from("<i", sp, o)[0]
            o += 4 + 2 * max(0, nl)
            n = struct.unpack_from("<i", sp, o)[0]
            o += 4
            size = (len(sp) - o) // n if n > 0 else 0
            for i in range(n if size >= 56 else 0):  # each: origin, angles, its model (the dictionary's), ..., skin
                e = o + i * size
                xyz, angles = struct.unpack_from("<3f", sp, e), struct.unpack_from("<3f", sp, e + 12)
                kind, skin = struct.unpack_from("<H", sp, e + 24)[0], struct.unpack_from("<i", sp, e + 32)[0]
                if kind < nd and all(math.isfinite(v) for v in xyz + angles):
                    add(model_of(names[kind]), xyz, angles, skin)
        ents = _entities(_lump(data, LUMP_ENTITIES).decode("latin-1"))
    except (struct.error, lzma.LZMAError):
        raise MapError(f"{path.name}: the map's props are inconsistent") from None
    sky, sky_camera = "", None
    for ent in ents:
        cls = ent.get("classname", "")
        if cls == "worldspawn":
            sky = ent.get("skyname", "")
        elif cls == "sky_camera" and sky_camera is None:
            o3, sc = _numbers(ent.get("origin"), 3), _numbers(ent.get("scale", "16"), 1)
            if o3:
                sky_camera = [*(round(v, 2) for v in o3), sc[0] if sc and sc[0] > 0 else 16.0]
        elif (cls.startswith("prop_") and ent.get("model", "").lower().endswith(".mdl") and ent.get("rendermode") != "10"
              and ent.get("StartDisabled") != "1"):
            o3 = _numbers(ent.get("origin"), 3)
            if o3:
                skin = ent.get("skin", "0").strip()
                add(model_of(ent["model"]), o3, _numbers(ent.get("angles"), 3) or [0.0, 0.0, 0.0],
                    int(skin) if re.fullmatch(r"[+-]?[0-9]{1,9}", skin) else 0)
    return {"error": None, "sky": sky, "sky_camera": sky_camera, "models": models, "props": props}


def read_triggers(path: Path) -> bytes:
    """The map's triggers as polygons (the TATR data above)."""
    data = path.read_bytes()
    if len(data) < 1036 or data[:4] != b"VBSP":
        raise MapError(f"{path.name}: not a Source map")
    version, = struct.unpack_from("<i", data, 4)
    if version not in (19, 20):
        raise MapError(f"{path.name}: map version {version} isn't supported (CS:S maps are 19 or 20)")
    try:
        planes = [p[:4] for p in struct.iter_unpack("<4fi", _lump(data, LUMP_PLANES))]
        nodes = [n[1:3] for n in struct.iter_unpack("<3i6h2H2h", _lump(data, LUMP_NODES))]
        # leaves: 56 bytes in version 0 of the lump (with their ambient light), 32 after
        leaf_size = 56 if struct.unpack_from("<i", data, 8 + 16 * LUMP_LEAFS + 8)[0] == 0 else 32
        leaves = [struct.unpack_from("<2H", leaf, 24) for (leaf,) in struct.iter_unpack(f"<{leaf_size}s", _lump(data, LUMP_LEAFS))]
        leafbrushes = [b for (b,) in struct.iter_unpack("<H", _lump(data, LUMP_LEAFBRUSHES))]
        brushes = [b[:2] for b in struct.iter_unpack("<3i", _lump(data, LUMP_BRUSHES))]
        sides = [s for s in struct.iter_unpack("<Hhhh", _lump(data, LUMP_BRUSHSIDES))]
        models = [m for m in struct.iter_unpack("<9f3i", _lump(data, LUMP_MODELS))]
        ents = _brush_entities(_lump(data, LUMP_ENTITIES).decode("latin-1"))
    except (struct.error, lzma.LZMAError) as e:
        raise MapError(f"{path.name}: can't read the map ({e})") from None

    heads, counts, pos = [], [], []
    try:
        for mi, (origin, cls, name) in sorted(ents.items()):
            if not is_trigger(cls) or not 0 < mi < len(models):
                continue
            # the model's brushes: those of the leaves under its head node
            found, stack, seen = set(), [models[mi][9]], set()
            while stack:
                n = stack.pop()
                if n < 0:
                    first, count = leaves[-1 - n]
                    found.update(leafbrushes[first:first + count])
                elif n not in seen:
                    seen.add(n)
                    stack += nodes[n]
            first_poly = len(counts)
            for b in sorted(found):
                first_side, n_sides = brushes[b]
                bp = [planes[sd[0]] for sd in sides[first_side:first_side + n_sides] if not sd[3]]
                for i, (nx, ny, nz, d) in enumerate(bp):
                    w = _base_winding(nx, ny, nz, d)
                    for j, other in enumerate(bp):
                        if j != i and w:
                            w = _clip(w, *other)
                    if len(w) >= 3:
                        counts.append(len(w))
                        for x, y, z in w:
                            pos += (x + origin[0], y + origin[1], z + origin[2])
            heads.append((first_poly, len(counts) - first_poly, cls.encode("latin-1"), name.encode("latin-1")))
    except IndexError:
        raise MapError(f"{path.name}: the map's triggers are inconsistent") from None
    strings = b"".join(c + n for _, _, c, n in heads)
    return (b"TATR" + struct.pack("<4I", TRIG_VERSION, len(heads), len(counts), len(pos) // 3)
            + b"".join(struct.pack("<4I", f, n, len(c), len(nm)) for f, n, c, nm in heads)
            + _pad4(strings) + _le(array("I", counts)) + _le(array("f", pos)))


def read_clip(path: Path) -> bytes:
    """What the player collides with (the TACL data above)."""
    data = path.read_bytes()
    if len(data) < 1036 or data[:4] != b"VBSP":
        raise MapError(f"{path.name}: not a Source map")
    version, = struct.unpack_from("<i", data, 4)
    if version not in (19, 20):
        raise MapError(f"{path.name}: map version {version} isn't supported (CS:S maps are 19 or 20)")
    try:
        planes = [p[:4] for p in struct.iter_unpack("<4fi", _lump(data, LUMP_PLANES))]
        nodes = [n[1:3] for n in struct.iter_unpack("<3i6h2H2h", _lump(data, LUMP_NODES))]
        leaf_size = 56 if struct.unpack_from("<i", data, 8 + 16 * LUMP_LEAFS + 8)[0] == 0 else 32
        leaves = [struct.unpack_from("<2H", leaf, 24) for (leaf,) in struct.iter_unpack(f"<{leaf_size}s", _lump(data, LUMP_LEAFS))]
        leafbrushes = [b for (b,) in struct.iter_unpack("<H", _lump(data, LUMP_LEAFBRUSHES))]
        brushes = [b for b in struct.iter_unpack("<3i", _lump(data, LUMP_BRUSHES))]
        sides = [s for s in struct.iter_unpack("<Hhhh", _lump(data, LUMP_BRUSHSIDES))]
        models = [m for m in struct.iter_unpack("<9f3i", _lump(data, LUMP_MODELS))]
        text = _lump(data, LUMP_ENTITIES)
    except (struct.error, lzma.LZMAError) as e:
        raise MapError(f"{path.name}: can't read the map ({e})") from None

    table, out = array("I"), array("f")
    try:
        for mi, model in enumerate(models):
            found, stack, seen = set(), [model[9]], set()  # the brushes of the leaves under its head node
            while stack:
                n = stack.pop()
                if n < 0:
                    first, count = leaves[-1 - n]
                    found.update(leafbrushes[first:first + count])
                elif n not in seen:
                    seen.add(n)
                    stack += nodes[n]
            for b in sorted(found):
                first_side, n_sides, contents = brushes[b]
                bs = [planes[sd[0]] for sd in sides[first_side:first_side + n_sides]]
                table += array("I", (len(out) // 4, len(bs), contents & 0xFFFFFFFF, mi))
                for plane in bs:
                    out += array("f", plane)
    except IndexError:
        raise MapError(f"{path.name}: the map's brushes are inconsistent") from None
    return (b"TACL" + struct.pack("<4I", CLIP_VERSION, len(table) // 4, len(out) // 4, len(text))
            + _le(table) + _le(out) + _pad4(text))


BOX = 131072.0  # bigger than any map: a plane's first polygon, before the brush's other planes cut it
ON_EPSILON = 0.1


def _base_winding(nx: float, ny: float, nz: float, d: float) -> list[tuple[float, float, float]]:
    """A huge square on the plane (as the map compiler makes it)."""
    n = (nx, ny, nz)
    axis = max(range(3), key=lambda i: abs(n[i]))
    ux, uy, uz = (0.0, 0.0, 1.0) if axis < 2 else (1.0, 0.0, 0.0)
    v = ux * nx + uy * ny + uz * nz
    ux, uy, uz = ux - v * nx, uy - v * ny, uz - v * nz
    length = math.sqrt(ux * ux + uy * uy + uz * uz)
    ux, uy, uz = ux / length * BOX, uy / length * BOX, uz / length * BOX
    rx, ry, rz = uy * nz - uz * ny, uz * nx - ux * nz, ux * ny - uy * nx  # up x normal
    ox, oy, oz = nx * d, ny * d, nz * d
    return [(ox - rx + ux, oy - ry + uy, oz - rz + uz), (ox + rx + ux, oy + ry + uy, oz + rz + uz),
            (ox + rx - ux, oy + ry - uy, oz + rz - uz), (ox - rx - ux, oy - ry - uy, oz - rz - uz)]


def _clip(w: list[tuple[float, float, float]], nx: float, ny: float, nz: float, d: float) -> list[tuple[float, float, float]]:
    """The part of the polygon behind the plane (inside the brush)."""
    dists = [x * nx + y * ny + z * nz - d for x, y, z in w]
    sides = [1 if e > ON_EPSILON else -1 if e < -ON_EPSILON else 0 for e in dists]
    if 1 not in sides:
        return w
    if -1 not in sides:
        return []
    out = []
    for i, p in enumerate(w):
        if sides[i] <= 0:
            out.append(p)
        k = (i + 1) % len(w)
        if sides[i] == 0 or sides[k] == 0 or sides[k] == sides[i]:
            continue
        q, t = w[k], dists[i] / (dists[i] - dists[k])
        out.append(tuple(d if c == 1.0 else -d if c == -1.0 else p[j] + t * (q[j] - p[j])  # type: ignore[misc]
                         for j, c in enumerate((nx, ny, nz))))
    return out


def find_map(cstrike_dir: Path, name: str) -> Path | None:
    """The map's .bsp in cstrike/maps or cstrike/download/maps (custom maps from servers)."""
    for sub in ("maps", "download/maps"):
        p = cstrike_dir / sub / f"{name}.bsp"
        if p.is_file():
            return p
    return None

"""The 3D view's textured look and triggers: the game's files (pakfile, VPKs, folders), a map's textured
data and its triggers."""

import io
import lzma
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path

from tasurf.bspmap import MapError, read_clip, read_map, read_props, read_triggers
from tasurf.gamefiles import GameFiles, normal_path

FACE = "<HBBihhhh4Bif2i2iiHHI"


def make_bsp(lumps: dict[int, bytes], versions: dict[int, int] | None = None) -> bytes:
    """A map file: the header (version 20) and the given lumps (of the given lump versions), one after another."""
    head, body = bytearray(1036), bytearray()
    head[:8] = b"VBSP" + struct.pack("<i", 20)
    for index, data in lumps.items():
        struct.pack_into("<iii", head, 8 + 16 * index, 1036 + len(body), len(data), (versions or {}).get(index, 0))
        body += data + b"\0" * (-len(data) % 4)
    return bytes(head + body)


def vpk_dir(entries: list[tuple[str, bytes, int, int, int]], inline: bytes = b"") -> bytes:
    """A version 2 VPK directory: entries (path, preload, archive, offset, length), then the inline data."""
    groups: dict[str, dict[str, list]] = {}
    for path, preload, archive, offset, length in entries:
        folder, _, file = path.rpartition("/")
        name, _, ext = file.rpartition(".")
        groups.setdefault(ext, {}).setdefault(folder or " ", []).append((name, preload, archive, offset, length))
    tree = bytearray()
    for ext, folders in groups.items():
        tree += ext.encode() + b"\0"
        for folder, files in folders.items():
            tree += folder.encode() + b"\0"
            for name, preload, archive, offset, length in files:
                tree += name.encode() + b"\0" + struct.pack("<IHHIIH", 0, len(preload), archive, offset, length, 0xFFFF) + preload
            tree += b"\0"
        tree += b"\0"
    tree += b"\0"
    return struct.pack("<III", 0x55AA1234, 2, len(tree)) + b"\0" * 16 + bytes(tree) + inline


def zip_of(files: dict[str, tuple[bytes, int]]) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for name, (data, method) in files.items():
            z.writestr(zipfile.ZipInfo(name), data, compress_type=method)
    return buf.getvalue()


class NormalPathTest(unittest.TestCase):
    def test_paths_as_the_game_looks_them_up(self):
        self.assertEqual(normal_path("MATERIALS\\Dev//Dev_Measure.VMT"), b"materials/dev/dev_measure.vmt")
        self.assertEqual(normal_path("materials/caf\u00e9.vtf"), b"materials/caf\xe9.vtf")

    def test_models_files_too(self):
        self.assertEqual(normal_path(r"models\Props\Crate.MDL"), b"models/props/crate.mdl")
        self.assertEqual(normal_path("models/props/crate.vvd"), b"models/props/crate.vvd")
        self.assertEqual(normal_path("models/props/crate.dx90.vtx"), b"models/props/crate.dx90.vtx")

    def test_only_materials_and_textures_under_materials(self):
        for bad in ["", "materials/../cfg/x.vmt", "materials/./x.vmt", "materials/x.txt", "models/x.txt", "models/.mdl",
                    "models/../x.mdl", "materials/x.mdl", "models/x.vmt",
                    "/materials/x.vmt", "c:/materials/x.vmt", "materials/a:b.vmt", "materials/.vmt", "materials/\u4e2d.vmt"]:
            with self.assertRaises(MapError, msg=bad):
                normal_path(bad)


class GameFilesTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = Path(self.tmp.name)
        self.cstrike = root / "cstrike"

        def put(rel: str, data: bytes):
            p = root / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(data)

        pak = zip_of({"materials/a.vmt": (b"from the pakfile", zipfile.ZIP_STORED),
                      "Materials/Packed.VTF": (b"lzma " * 100, zipfile.ZIP_LZMA)})
        put("cstrike/download/maps/m.bsp", make_bsp({40: pak}))
        put("cstrike/custom/mine/materials/b.vmt", b"from custom")
        put("cstrike/cstrike_pak_dir.vpk", vpk_dir([("materials/b.vmt", b"", 0x7FFF, 0, 8),
                                                   ("materials/c.vmt", b"pre", 0, 3, 4),
                                                   ("materials/a.vmt", b"", 0x7FFF, 8, 3)], inline=b"from vpkvpk"))
        put("cstrike/cstrike_pak_000.vpk", b"...load")
        put("cstrike/materials/d.vmt", b"loose")
        put("cstrike/download/materials/d.vmt", b"downloaded d")
        put("cstrike/download/materials/e.vmt", b"downloaded e")
        self.files = GameFiles(self.cstrike)
        self.bsp = self.cstrike / "download" / "maps" / "m.bsp"

    def tearDown(self):
        self.tmp.cleanup()

    def test_the_games_search_order(self):
        read = lambda rel: self.files.read(self.bsp, rel)
        self.assertEqual(read("materials/a.vmt"), b"from the pakfile")  # the map's own files come first
        self.assertEqual(read("materials/packed.vtf"), b"lzma " * 100)  # LZMA in the pakfile, any case
        self.assertEqual(read("materials/b.vmt"), b"from custom")  # custom/ before the VPKs
        self.assertEqual(read("materials/c.vmt"), b"preload")  # a VPK entry's preload, then its archive's part
        self.assertEqual(read("MATERIALS\\D.VMT"), b"loose")  # loose files before download/
        self.assertEqual(read("materials/e.vmt"), b"downloaded e")
        with self.assertRaisesRegex(MapError, "materials/nope.vmt isn't in the map or the game's files"):
            read("materials/nope.vmt")


def square_map(light: bytes = b"", hdr: bool = False, trigger: bool = False) -> bytes:
    """One 64 x 64 face on z = 0 facing up, textured with dev/test (64 x 32), its lightmap 3 x 3 luxels. Its corners
    run clockwise seen from above, as a compiler writes a face: (0, 0), (0, 64), (64, 64), (64, 0)."""
    verts = struct.pack("<12f", 0, 0, 0, 64, 0, 0, 64, 64, 0, 0, 64, 0)
    edges = struct.pack("<10H", 0, 0, 0, 1, 1, 2, 2, 3, 3, 0)
    texinfo = struct.pack("<16fii", 1, 0, 0, 0, 0, 1, 0, 0, 1 / 32, 0, 0, 0, 0, 1 / 32, 0, 0, 0, 0)
    face = lambda lightofs: struct.pack(FACE, 0, 0, 0, 0, 4, 0, -1, -1, 0, 255, 255, 255, lightofs, 0.0, 0, 0, 2, 2, 0, 0, 0, 0)
    lumps = {0: b'{"classname" "worldspawn"}\0', 1: struct.pack("<4fi", 0, 0, 1, 0, 2), 2: struct.pack("<3fi4i", 0.5, 0.25, 0.125, 0, 64, 32, 64, 32),
             3: verts, 6: texinfo, 7: face(0), 12: edges, 13: struct.pack("<4i", -4, -3, -2, -1),
             14: struct.pack("<9f3i", 0, 0, 0, 64, 64, 0, 0, 0, 0, 0, 0, 1), 43: b"dev/test\0", 44: struct.pack("<i", 0)}
    if hdr:
        lumps.update({53: b"\0" * 8 + light, 58: face(8)})  # the HDR faces' copy says where its light is
    else:
        lumps[8] = light
    if trigger:  # a 64 x 32 x 16 box, placed at x = 100, whose model kept a face (as old compilers did)
        lumps[0] = b'{"classname" "worldspawn"}\n{"classname" "trigger_teleport" "targetname" "tp1" "model" "*1" "origin" "100 0 0"}\0'
        lumps[1] += b"".join(struct.pack("<4fi", *n, d, 0) for n, d in [((1, 0, 0), 64), ((-1, 0, 0), 0), ((0, 1, 0), 32),
                                                                        ((0, -1, 0), 0), ((0, 0, 1), 16), ((0, 0, -1), 0)])
        lumps[7] = face(0) * 2
        lumps[14] += struct.pack("<9f3i", 0, 0, 0, 64, 32, 16, 0, 0, 0, -1, 1, 1)  # its head node: leaf 0
        lumps[10] = struct.pack("<ihh6h4Hh2x", 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, -1)  # leaf 0: brush 0
        lumps[17] = struct.pack("<H", 0)
        lumps[18] = struct.pack("<3i", 0, 7, 0)
        lumps[19] = b"".join(struct.pack("<Hhhh", plane, 0, -1, bevel) for plane, bevel in [(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (1, 1)])
        return make_bsp(lumps, {10: 1})
    return make_bsp(lumps)


def read_textured(tex: bytes) -> dict:
    nv, nt, nm, aw, ah, kind = struct.unpack_from("<6I", tex, 8)
    o = 32
    uv = struct.unpack_from(f"<{2 * nv}f", tex, o); o += 8 * nv
    lc = struct.unpack_from(f"<{2 * nv}f", tex, o); o += 8 * nv
    blend = struct.unpack_from(f"<{nv}f", tex, o); o += 4 * nv
    mat = struct.unpack_from(f"<{nt}H", tex, o); o += (2 * nt + 3) // 4 * 4
    table = [struct.unpack_from("<3f2iI", tex, o + 24 * k) for k in range(nm)]; o += 24 * nm
    names = []
    for e in table:
        names.append(tex[o:o + e[5]]); o += e[5]
    o += -o % 4
    return dict(nv=nv, nt=nt, aw=aw, ah=ah, kind=kind, uv=uv, lc=lc, blend=blend, mat=mat, table=table, names=names,
                atlas=tex[o:], size=len(tex) - o)


class TexturedMapTest(unittest.TestCase):
    LIGHT = b"".join(bytes((10 + k, 20 + k, 30 + k, 0)) for k in range(9))  # luxel (s, t) = k = 3 t + s

    def read(self, data: bytes) -> dict:
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            p.write_bytes(data)
            mesh, tex = read_map(p, textures=True)
            self.assertEqual(read_map(p)[0], mesh)  # the mesh is the same either way
        self.assertEqual(tex[:4], b"TAST")
        return read_textured(tex)

    def luxel(self, t: dict, x: int, y: int) -> bytes:
        o = (y * t["aw"] + x) * 4
        return t["atlas"][o:o + 4]

    def test_texture_and_lightmap_positions(self):
        t = self.read(square_map(self.LIGHT))
        self.assertEqual((t["nv"], t["nt"], t["kind"], t["mat"]), (4, 2, 1, (0, 0)))
        self.assertEqual(t["uv"], (0, 0, 0, 64, 64, 64, 64, 0))  # in texels
        self.assertEqual((t["aw"], t["ah"], t["size"]), (256, 5, 256 * 5 * 4))
        # the face's 3 x 3 luxels with a 1-luxel border at (0, 0): luxel s lies at texel 1 + s
        for v, (s, u) in enumerate([(0, 0), (0, 2), (2, 2), (2, 0)]):
            self.assertAlmostEqual(t["lc"][2 * v], (1 + s + 0.5) / 256, places=6)
            self.assertAlmostEqual(t["lc"][2 * v + 1], (1 + u + 0.5) / 5, places=6)
        L = lambda k: self.LIGHT[4 * k:4 * k + 4]
        self.assertEqual(self.luxel(t, 1, 1), L(0))
        self.assertEqual(self.luxel(t, 3, 3), L(8))
        self.assertEqual([self.luxel(t, x, 0) for x in range(5)], [L(0), L(0), L(1), L(2), L(2)])  # the border repeats the edge
        self.assertEqual(self.luxel(t, 4, 4), L(8))
        self.assertEqual(self.luxel(t, 5, 0), b"\xff\xff\xff\x00")  # the white block, for faces without a lightmap
        self.assertEqual(t["names"], [b"dev/test"])
        self.assertEqual(t["table"][0][3:5], (64, 32))

    def test_hdr_only_lighting_uses_the_hdr_faces(self):
        t = self.read(square_map(self.LIGHT, hdr=True))
        self.assertEqual(t["kind"], 2)
        self.assertEqual(self.luxel(t, 1, 1), self.LIGHT[:4])

    def test_no_lighting_is_fullbright(self):
        t = self.read(square_map())
        self.assertEqual(t["kind"], 0)
        white = [k for k in range(0, t["aw"] * t["ah"] * 4, 4) if t["atlas"][k:k + 4] == b"\xff\xff\xff\x00"]
        x, y = white[0] // 4 % t["aw"], white[0] // 4 // t["aw"]
        for v in range(4):  # every vertex between the white block's four luxels
            self.assertAlmostEqual(t["lc"][2 * v], (x + 1) / t["aw"], places=6)
            self.assertAlmostEqual(t["lc"][2 * v + 1], (y + 1) / t["ah"], places=6)


class FrontTest(unittest.TestCase):
    def test_a_face_is_seen_from_where_its_corners_run_clockwise(self):
        """A floor (plane up, side 0) and a ceiling over it (the plane's flipped twin, odd, side 1, as compilers write
        it): each comes out counter-clockwise from where it faces, the ceiling from below, not turned again."""
        verts = struct.pack("<24f", 0, 0, 0, 64, 0, 0, 64, 64, 0, 0, 64, 0, 0, 0, 64, 64, 0, 64, 64, 64, 64, 0, 64, 64)
        edges = struct.pack("<18H", 0, 0, 0, 1, 1, 2, 2, 3, 3, 0, 4, 5, 5, 6, 6, 7, 7, 4)
        texinfo = struct.pack("<16fii", 1, 0, 0, 0, 0, 1, 0, 0, 1 / 32, 0, 0, 0, 0, 1 / 32, 0, 0, 0, 0)
        face = lambda plane, side, first: struct.pack(FACE, plane, side, 0, first, 4, 0, -1, -1, 0, 255, 255, 255, -1, 0.0, 0, 0, 2, 2, 0, 0, 0, 0)
        lumps = {0: b'{"classname" "worldspawn"}\0', 1: struct.pack("<4fi4fi", 0, 0, 1, 0, 2, 0, 0, -1, 0, 2),
                 2: struct.pack("<3fi4i", 0.5, 0.5, 0.5, 0, 64, 32, 64, 32), 3: verts, 6: texinfo, 7: face(0, 0, 0) + face(1, 1, 4), 12: edges,
                 13: struct.pack("<8i", -4, -3, -2, -1, 5, 6, 7, 8),  # floor clockwise from above, ceiling clockwise from below
                 14: struct.pack("<9f3i", 0, 0, 0, 64, 64, 64, 0, 0, 0, 0, 0, 2), 43: b"dev/test\0", 44: struct.pack("<i", 0)}
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            p.write_bytes(make_bsp(lumps))
            mesh = read_map(p)[0]
        nv, ni = struct.unpack_from("<II", mesh, 8)
        pos = struct.unpack_from(f"<{3 * nv}f", mesh, 40)
        idx = struct.unpack_from(f"<{ni}I", mesh, 40 + 12 * nv)
        up = lambda t: ((pos[3 * t[1]] - pos[3 * t[0]]) * (pos[3 * t[2] + 1] - pos[3 * t[0] + 1])
                        - (pos[3 * t[1] + 1] - pos[3 * t[0] + 1]) * (pos[3 * t[2]] - pos[3 * t[0]])) > 0
        tris = [idx[k:k + 3] for k in range(0, ni, 3)]
        self.assertEqual(len(tris), 4)
        self.assertEqual([up(t) for t in tris], [True, True, False, False])  # the floor seen from above, the ceiling from below


class DisplacementTest(unittest.TestCase):
    def test_a_fold_faces_where_the_rest_does(self):
        """A displacement faces one way as a whole, where its base face does (as the game draws it): its last row
        pushed back past the start, so that strip faces down, keeps the order of the rest, not turned up."""
        verts = struct.pack("<12f", 0, 0, 0, 64, 0, 0, 64, 64, 0, 0, 64, 0)
        edges = struct.pack("<10H", 0, 0, 0, 1, 1, 2, 2, 3, 3, 0)
        texinfo = struct.pack("<16fii", 1, 0, 0, 0, 0, 1, 0, 0, 1 / 32, 0, 0, 0, 0, 1 / 32, 0, 0, 0, 0)
        face = struct.pack(FACE, 0, 0, 0, 0, 4, 0, 0, -1, 0, 255, 255, 255, -1, 0.0, 0, 0, 2, 2, 0, 0, 0, 0)  # displacement 0
        size = 5  # power 2
        disp = struct.pack("<3fiii", 0, 0, 0, 0, 0, 2) + b"\0" * 152
        # rows along y (corner 0 to 1): the last one is pushed 100 back in y and 20 up: it folds over the row before
        dverts = b"".join(struct.pack("<5f", 0, -100 / 101.98, 20 / 101.98, 101.98, 0) if i == size - 1 else struct.pack("<5f", 0, 0, 1, 0, 0)
                          for i in range(size) for _ in range(size))
        lumps = {0: b'{"classname" "worldspawn"}\0', 1: struct.pack("<4fi", 0, 0, 1, 0, 2), 2: struct.pack("<3fi4i", 0.5, 0.5, 0.5, 0, 64, 32, 64, 32),
                 3: verts, 6: texinfo, 7: face, 12: edges, 13: struct.pack("<4i", -4, -3, -2, -1), 26: disp, 33: dverts,
                 14: struct.pack("<9f3i", 0, 0, 0, 64, 64, 0, 0, 0, 0, 0, 0, 1), 43: b"dev/test\0", 44: struct.pack("<i", 0)}
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            p.write_bytes(make_bsp(lumps))
            mesh = read_map(p)[0]
        nv, ni = struct.unpack_from("<II", mesh, 8)
        pos = struct.unpack_from(f"<{3 * nv}f", mesh, 40)
        idx = struct.unpack_from(f"<{ni}I", mesh, 40 + 12 * nv)
        want = []  # the same order in every cell (counter-clockwise from above, where the face looks)
        for i in range(size - 1):
            for j in range(size - 1):
                a = i * size + j
                b, cc, dd = a + 1, a + size, a + size + 1
                want += [a, b, cc, b, dd, cc] if (i + j) % 2 == 0 else [a, dd, cc, a, b, dd]
        self.assertEqual(list(idx), want)
        up = lambda t: ((pos[3 * t[1]] - pos[3 * t[0]]) * (pos[3 * t[2] + 1] - pos[3 * t[0] + 1])
                        - (pos[3 * t[1] + 1] - pos[3 * t[0] + 1]) * (pos[3 * t[2]] - pos[3 * t[0]])) > 0
        tris = [idx[k:k + 3] for k in range(0, ni, 3)]
        self.assertTrue(any(not up(t) for t in tris[-2 * (size - 1):]))  # the folded strip does face down now
        self.assertTrue(all(up(t) for t in tris[:2 * (size - 1)]))


class TriggersTest(unittest.TestCase):
    def test_a_box_trigger_from_its_brush(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            p.write_bytes(square_map(trigger=True))
            data = read_triggers(p)
            mesh = read_map(p)[0]
        self.assertEqual(struct.unpack_from("<2I", mesh, 8), (4, 6))  # the trigger's own face isn't in the map
        self.assertEqual(data[:4], b"TATR")
        n_trig, n_poly, n_vert = struct.unpack_from("<3I", data, 8)
        self.assertEqual((n_trig, n_poly, n_vert), (1, 6, 24))  # its six sides; the bevel side adds none
        self.assertEqual(struct.unpack_from("<4I", data, 20), (0, 6, 16, 3))
        self.assertEqual(data[36:55], b"trigger_teleporttp1")
        o = 56
        self.assertEqual(struct.unpack_from("<6I", data, o), (4,) * 6)
        pos = struct.unpack_from("<72f", data, o + 24)
        corners = {pos[i:i + 3] for i in range(0, 72, 3)}
        self.assertEqual(corners, {(x, y, z) for x in (100, 164) for y in (0, 32) for z in (0, 16)})  # moved to its origin
        for k in range(6):  # each side flat, on one of the box's planes
            side = [pos[3 * (4 * k + i):3 * (4 * k + i) + 3] for i in range(4)]
            self.assertTrue(any(len({v[axis] for v in side}) == 1 for axis in range(3)))


def clip_map() -> bytes:
    """A world of one solid box (64 x 64 x 16, under z = 0) and a trigger_teleport box (as square_map's) at x = 100."""
    planes = [((0, 0, 1), 0), ((1, 0, 0), 64), ((-1, 0, 0), 0), ((0, 1, 0), 32), ((0, -1, 0), 0), ((0, 0, 1), 16),
              ((0, 0, -1), 0), ((0, 1, 0), 64), ((0, 0, -1), 16)]
    leaf = lambda first: struct.pack("<ihh6h4Hh2x", 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, first, 1, -1)
    lumps = {
        0: b'{"classname" "worldspawn"}\n{"classname" "trigger_teleport" "target" "dest" "model" "*1" "origin" "100 0 0"}\0',
        1: b"".join(struct.pack("<4fi", *n, d, 0) for n, d in planes),
        5: struct.pack("<3i6h2H2h", 0, -2, -2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0),  # node 0: leaf 1 both sides
        10: leaf(0) + leaf(1),  # leaf 0: brush 0 (the trigger's), leaf 1: brush 1 (the world's)
        14: struct.pack("<9f3i", 0, 0, -16, 64, 64, 0, 0, 0, 0, 0, 0, 0) + struct.pack("<9f3i", 0, 0, 0, 64, 32, 16, 0, 0, 0, -1, 0, 0),
        17: struct.pack("<2H", 0, 1),
        18: struct.pack("<3i", 0, 7, 0) + struct.pack("<3i", 7, 6, 1),
        19: b"".join(struct.pack("<Hhhh", plane, 0, -1, bevel)
                     for plane, bevel in [(1, 0), (2, 0), (3, 0), (4, 0), (5, 0), (6, 0), (1, 1), (0, 0), (8, 0), (1, 0), (2, 0), (7, 0), (4, 0)]),
    }
    return make_bsp(lumps, {10: 1})


class ClipTest(unittest.TestCase):
    def test_every_brush_with_its_planes_and_the_entities(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            p.write_bytes(clip_map())
            data = read_clip(p)
        self.assertEqual(data[:4], b"TACL")
        version, n_brush, n_plane, n_text = struct.unpack_from("<4I", data, 4)
        self.assertEqual((version, n_brush, n_plane), (1, 2, 13))
        # the world's model first, then the trigger's; contents as the map says (1: solid)
        self.assertEqual(struct.unpack_from("<8I", data, 20), (0, 6, 1, 0, 6, 7, 0, 1))
        planes = struct.unpack_from(f"<{4 * n_plane}f", data, 52)
        self.assertEqual(planes[:8], (0, 0, 1, 0, 0, 0, -1, 16))  # the world box's top and bottom
        self.assertEqual(planes[-4:], (1, 0, 0, 64))  # the trigger's bevel side is there too
        o = 52 + 16 * n_plane
        self.assertTrue(data[o:o + n_text].startswith(b'{"classname" "worldspawn"}'))
        self.assertEqual(len(data) % 4, 0)

    def test_broken_maps(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            p.write_bytes(square_map())  # no node for the world's head node
            with self.assertRaises(MapError) as e:
                read_clip(p)
            self.assertIn("brushes are inconsistent", str(e.exception))
            p.write_bytes(b"VBSP" + struct.pack("<i", 21) + bytes(1028))
            with self.assertRaises(MapError) as e:
                read_clip(p)
            self.assertIn("version 21", str(e.exception))


def valve_lzma(data: bytes) -> bytes:
    """data as the map compiler compresses a lump: "LZMA", its size, the stream's size, 5 property bytes, the stream."""
    alone = lzma.compress(data, format=lzma.FORMAT_ALONE)  # properties (5), size (8), stream
    return b"LZMA" + struct.pack("<II", len(data), len(alone) - 13) + alone[:5] + alone[13:]


def props_map(compress: bool, entry: int = 64) -> bytes:
    """A map with two static props (entries of `entry` bytes) of one model and three prop entities: the static prop
    lump stored as it is or compressed."""
    names = [b"Models\\Props\\Crate.mdl", b"models/tree.mdl"]
    sp = struct.pack("<i", len(names)) + b"".join(n.ljust(128, b"\0") for n in names)
    sp += struct.pack("<i3H", 3, 0, 1, 2)  # the leaves they're in
    sp += struct.pack("<i", 3)
    for xyz, angles, kind, skin in [((1, 2, 3.125), (0, 90, 0), 1, 0), ((-5, 6.004, 7), (10, 20, 30), 1, 2), ((0, 0, 0), (0, 0, 0), 9, 0)]:
        e = bytearray(entry)  # the last one's model isn't in the dictionary
        struct.pack_into("<6fHHHBBi", e, 0, *xyz, *angles, kind, 0, 0, 6, 0, skin)
        sp += bytes(e)
    stored = valve_lzma(sp) if compress else sp
    lumps = {0: b'{"classname" "worldspawn" "skyname" "sky_day01_01"}\n'
                b'{"classname" "sky_camera" "origin" "0 0 -12000" "scale" "16"}\n'
                b'{"classname" "prop_dynamic" "model" "models/Props/crate.mdl" "origin" "8 9 10" "angles" "0 45 0" "skin" "1"}\n'
                b'{"classname" "prop_physics" "model" "models/barrel.mdl" "origin" "1 1 1"}\n'
                b'{"classname" "prop_dynamic" "model" "models/hidden.mdl" "origin" "1 1 1" "rendermode" "10"}\n'
                b'{"classname" "prop_dynamic" "model" "models/later.mdl" "origin" "1 1 1" "StartDisabled" "1"}\n'
                b'{"classname" "func_brush" "model" "*1" "origin" "1 1 1"}\0'}
    bsp = bytearray(make_bsp(lumps))
    at = len(bsp) + 4 + 16  # the game lump: its directory (one entry), then the static props' data
    glump = struct.pack("<i", 1) + struct.pack("<iHHii", int.from_bytes(b"sprp", "big"), 0, 6, at, len(stored)) + stored
    struct.pack_into("<iii", bsp, 8 + 16 * 35, len(bsp), len(glump), 0)
    return bytes(bsp + glump)


class PropsTest(unittest.TestCase):
    def test_the_sky_and_the_props(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            for compress in (False, True):
                for entry in (64, 76):  # sizes of different versions of the lump
                    p.write_bytes(props_map(compress, entry))
                    got = read_props(p)
                    self.assertEqual(got, {
                        "error": None, "sky": "sky_day01_01", "sky_camera": [0.0, 0.0, -12000.0, 16.0],
                        "models": ["models/tree.mdl", "models/props/crate.mdl", "models/barrel.mdl"],
                        "props": [[0, 1.0, 2.0, 3.12, 0.0, 90.0, 0.0, 0], [0, -5.0, 6.0, 7.0, 10.0, 20.0, 30.0, 2],
                                  [1, 8.0, 9.0, 10.0, 0.0, 45.0, 0.0, 1], [2, 1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0]]}, (compress, entry))

    def test_broken_maps(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "m.bsp"
            p.write_bytes(square_map())  # no game lump: no static props
            self.assertEqual(read_props(p), {"error": None, "sky": "", "sky_camera": None, "models": [], "props": []})
            bsp = bytearray(props_map(False))
            ofs, _, _ = struct.unpack_from("<iii", bsp, 8 + 16 * 35)
            struct.pack_into("<i", bsp, ofs + 4 + 16, 10 ** 6)  # a dictionary longer than the lump
            p.write_bytes(bytes(bsp))
            with self.assertRaises(MapError) as e:
                read_props(p)
            self.assertIn("props are inconsistent", str(e.exception))


if __name__ == "__main__":
    unittest.main()

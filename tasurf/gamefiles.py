"""Find a map's materials and textures the way the game does, for the 3D view's
textured look.

The game looks for a file first in the map's own pakfile (a zip inside the .bsp,
where custom maps carry their textures), then along the search paths of CS:S's
gameinfo.txt, in its order: cstrike/custom/* (folders and VPKs, alphabetically),
the VPK archives (cstrike_english, cstrike_pak, hl2_textures, hl2_sound_vo_english,
hl2_sound_misc, hl2_misc, platform_misc), the loose files of cstrike/, hl2/ and
platform/, and last cstrike/download/ (what servers sent). Pakfile entries are
stored or LZMA-compressed, the two kinds the game reads.

Paths are matched as the game matches them: case-insensitively (ASCII) and with
either kind of slash.
"""

from __future__ import annotations

import lzma
import re
import struct
import threading
from pathlib import Path

from .bspmap import MapError, _lzma, _unlzma

LUMP_PAKFILE = 40
VPKS = ["cstrike/cstrike_english", "cstrike/cstrike_pak", "hl2/hl2_textures", "hl2/hl2_sound_vo_english",
        "hl2/hl2_sound_misc", "hl2/hl2_misc", "platform/platform_misc"]
LOOSE = ["cstrike", "hl2", "platform"]
PATH_RE = re.compile(rb"materials/[^\x00-\x1f:]+\.(vmt|vtf)|models/[^\x00-\x1f:]+\.(mdl|vvd|vtx)")


def normal_path(rel: str) -> bytes:
    """The request's path as the game would look it up, or MapError if it isn't a material or texture, or a
    model's file (the 3D view's props: .mdl, .vvd, .vtx)."""
    try:
        b = re.sub(rb"/+", b"/", rel.encode("latin-1").lower().replace(b"\\", b"/"))  # the game ignores doubled slashes
    except UnicodeEncodeError:
        b = b""
    if not PATH_RE.fullmatch(b) or any(part in (b"", b".", b"..") for part in b.split(b"/")):
        raise MapError(f"bad file path {rel!r} (a materials/... .vmt or .vtf file, or a models/... .mdl, .vvd or .vtx)")
    return b


class _Vpk:
    """A VPK archive's directory: the entries' places, by lower-case path."""

    def __init__(self, path: Path):
        self.path = path
        self.entries: dict[bytes, tuple[int, int, int, int, int]] = {}  # (archive, offset, length, preload at, size)
        self.tree = b""
        d = path.read_bytes()
        sig, version, tree_size = struct.unpack_from("<III", d, 0)
        if sig != 0x55AA1234 or version not in (1, 2):
            return
        head = 12 if version == 1 else 28
        self.tree, self.data_start = d, head + tree_size
        pos = head

        def cstr() -> bytes:
            nonlocal pos
            end = d.index(b"\0", pos)
            s, pos = d[pos:end], end + 1
            return s

        while ext := cstr():
            while folder := cstr():
                while name := cstr():
                    _crc, preload, archive, offset, length, _end = struct.unpack_from("<IHHIIH", d, pos)
                    pos += 18
                    full = (folder + b"/" if folder != b" " else b"") + name + (b"." + ext if ext != b" " else b"")
                    self.entries.setdefault(full.lower().replace(b"\\", b"/"), (archive, offset, length, pos, preload))
                    pos += preload

    def read(self, key: bytes) -> bytes | None:
        e = self.entries.get(key)
        if e is None:
            return None
        archive, offset, length, at, preload = e
        data = self.tree[at:at + preload]
        if length:
            if archive == 0x7FFF:  # in this file, after the directory
                data += self.tree[self.data_start + offset:self.data_start + offset + length]
            else:
                name = self.path.name
                base = name[:-len("_dir.vpk")] if name.endswith("_dir.vpk") else name[:-len(".vpk")]
                with open(self.path.with_name(f"{base}_{archive:03d}.vpk"), "rb") as f:
                    f.seek(offset)
                    data += f.read(length)
            if len(data) != preload + length:
                return None
        return data


def _zip_index(pak: bytes) -> dict[bytes, tuple[int, int, int, int]]:
    """A zip's entries by lower-case path: (method, data offset, compressed size, size)."""
    out: dict[bytes, tuple[int, int, int, int]] = {}
    end = pak.rfind(b"PK\x05\x06", max(0, len(pak) - 65557))
    if end < 0 or end + 22 > len(pak):
        return out
    count, _cd_size, p = struct.unpack_from("<HII", pak, end + 10)
    for _ in range(count):
        if pak[p:p + 4] != b"PK\x01\x02" or p + 46 > len(pak):
            break
        method, = struct.unpack_from("<H", pak, p + 10)
        csize, size, nlen, xlen, clen = struct.unpack_from("<IIHHH", pak, p + 20)
        local, = struct.unpack_from("<I", pak, p + 42)
        name = pak[p + 46:p + 46 + nlen]
        p += 46 + nlen + xlen + clen
        if pak[local:local + 4] != b"PK\x03\x04" or local + 30 > len(pak):
            continue
        lnlen, lxlen = struct.unpack_from("<HH", pak, local + 26)
        out.setdefault(name.lower().replace(b"\\", b"/"), (method, local + 30 + lnlen + lxlen, csize, size))
    return out


def _zip_read(pak: bytes, entry: tuple[int, int, int, int]) -> bytes | None:
    method, at, csize, size = entry
    raw = pak[at:at + csize]
    if len(raw) != csize:
        return None
    if method == 0:
        return raw if csize == size else None
    if method == 14 and csize >= 9:  # LZMA: 2 bytes version, 2 bytes properties size (5), the properties, the stream
        psize, = struct.unpack_from("<H", raw, 2)
        if psize != 5:
            return None
        try:
            return _lzma(raw[4:9], size, raw[9:])
        except lzma.LZMAError:
            return None
    return None


class GameFiles:
    """The game's files under the CS:S folder (the one holding cstrike/), and one map's pakfile at a time."""

    def __init__(self, cstrike_dir: Path):
        self.game = cstrike_dir.parent
        self.lock = threading.Lock()
        self._paths: list[_Vpk | Path] | None = None
        self._pak: tuple[str, float, bytes, dict] | None = None

    def _search(self) -> list[_Vpk | Path]:
        if self._paths is None:
            paths: list[_Vpk | Path] = []
            custom = self.game / "cstrike" / "custom"
            if custom.is_dir():
                for p in sorted(custom.iterdir(), key=lambda p: p.name.lower()):
                    if p.is_dir():
                        paths.append(p)
                    elif p.suffix.lower() == ".vpk" and not re.search(r"_\d{3}\.vpk$", p.name.lower()):
                        paths.append(self._vpk(p))
            for v in VPKS:
                p = self.game / f"{v}_dir.vpk"
                if p.is_file():
                    paths.append(self._vpk(p))
            paths += [self.game / d for d in LOOSE]
            paths.append(self.game / "cstrike" / "download")
            self._paths = paths
        return self._paths

    @staticmethod
    def _vpk(p: Path) -> _Vpk:
        try:
            return _Vpk(p)
        except (OSError, ValueError, struct.error):
            v = _Vpk.__new__(_Vpk)
            v.path, v.entries, v.tree = p, {}, b""
            return v

    def _pakfile(self, bsp: Path) -> tuple[bytes, dict]:
        mtime = bsp.stat().st_mtime
        if self._pak and self._pak[0] == str(bsp) and self._pak[1] == mtime:
            return self._pak[2], self._pak[3]
        with open(bsp, "rb") as f:
            head = f.read(1036)
            pak = b""
            if len(head) == 1036 and head[:4] == b"VBSP":
                ofs, length = struct.unpack_from("<ii", head, 8 + 16 * LUMP_PAKFILE)
                f.seek(max(0, ofs))
                try:
                    pak = _unlzma(f.read(max(0, length)))
                except (lzma.LZMAError, struct.error):
                    pak = b""
        index = _zip_index(pak)
        self._pak = (str(bsp), mtime, pak, index)  # one map at a time: pakfiles can be tens of MB
        return pak, index

    def read(self, bsp: Path, rel: str) -> bytes:
        """The file at rel (materials/....vmt or .vtf, models/....mdl, .vvd or .vtx) as the game would find it while
        on the map at bsp."""
        key = normal_path(rel)
        with self.lock:
            pak, index = self._pakfile(bsp)
            search = self._search()
        if key in index:
            data = _zip_read(pak, index[key])
            if data is not None:
                return data
        for place in search:
            if isinstance(place, _Vpk):
                try:
                    data = place.read(key)
                except OSError:
                    data = None
                if data is not None:
                    return data
            else:
                p = place / key.decode("latin-1")
                try:
                    if p.is_file():
                        return p.read_bytes()
                except OSError:
                    pass
        raise MapError(f"{key.decode('latin-1')} isn't in the map or the game's files")

"""Source engine button bits (IN_* from in_buttons.h) and timeline key names."""

IN_ATTACK = 1 << 0
IN_JUMP = 1 << 1
IN_DUCK = 1 << 2
IN_FORWARD = 1 << 3
IN_BACK = 1 << 4
IN_USE = 1 << 5
IN_LEFT = 1 << 7  # +left: client-side turning; the server only sees the flag
IN_RIGHT = 1 << 8
IN_MOVELEFT = 1 << 9
IN_MOVERIGHT = 1 << 10
IN_ATTACK2 = 1 << 11
IN_RELOAD = 1 << 13
IN_SPEED = 1 << 17  # +speed, which is "walk" in CS:S

# canonical key name -> (button bit, forwardmove sign, sidemove sign)
KEYS = {
    "forward": (IN_FORWARD, +1, 0),
    "back": (IN_BACK, -1, 0),
    "moveleft": (IN_MOVELEFT, 0, -1),
    "moveright": (IN_MOVERIGHT, 0, +1),
    "jump": (IN_JUMP, 0, 0),
    "duck": (IN_DUCK, 0, 0),
    "walk": (IN_SPEED, 0, 0),
    "use": (IN_USE, 0, 0),
    "attack": (IN_ATTACK, 0, 0),
    "attack2": (IN_ATTACK2, 0, 0),
    "reload": (IN_RELOAD, 0, 0),
    "turnleft": (IN_LEFT, 0, 0),
    "turnright": (IN_RIGHT, 0, 0),
}

# Accepted spellings (case-insensitive). "left"/"right" are deliberately not
# aliases of anything: in Source those names mean the +left/+right turn keys,
# which are spelled turnleft/turnright here to avoid confusion with A/D.
ALIASES = {
    "w": "forward",
    "s": "back",
    "a": "moveleft",
    "d": "moveright",
    "crouch": "duck",
    "speed": "walk",
}

MOVEMENT_KEYS = frozenset(k for k, (_, f, s) in KEYS.items() if f or s)
_NAMED_BITS = {bit: name for name, (bit, _, _) in KEYS.items()}


def canonical_key(name: str) -> str | None:
    """The canonical key name for a user-written key, or None. Any button bit
    without a name can be written as bitN (N = 0..31)."""
    low = name.strip().lower()
    low = ALIASES.get(low, low)
    if low in KEYS:
        return low
    if low.startswith("bit") and low[3:].isdigit() and 0 <= int(low[3:]) <= 31:
        bit = 1 << int(low[3:])
        return _NAMED_BITS.get(bit, f"bit{int(low[3:])}")
    return None


def key_info(key: str) -> tuple[int, int, int]:
    """(button bit, forwardmove sign, sidemove sign) for a canonical key name."""
    if key in KEYS:
        return KEYS[key]
    return 1 << int(key[3:]), 0, 0


def key_for_bit(bit_index: int) -> str:
    return _NAMED_BITS.get(1 << bit_index, f"bit{bit_index}")

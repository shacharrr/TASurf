//! Source engine button bits (IN_* from in_buttons.h) and timeline key names (tasurf/buttons.py).

pub const IN_ATTACK: i64 = 1 << 0;
pub const IN_JUMP: i64 = 1 << 1;
pub const IN_DUCK: i64 = 1 << 2;
pub const IN_FORWARD: i64 = 1 << 3;
pub const IN_BACK: i64 = 1 << 4;
pub const IN_USE: i64 = 1 << 5;
pub const IN_LEFT: i64 = 1 << 7; // +left: client-side turning; the server only sees the flag
pub const IN_RIGHT: i64 = 1 << 8;
pub const IN_MOVELEFT: i64 = 1 << 9;
pub const IN_MOVERIGHT: i64 = 1 << 10;
pub const IN_ATTACK2: i64 = 1 << 11;
pub const IN_RELOAD: i64 = 1 << 13;
pub const IN_SPEED: i64 = 1 << 17; // +speed, which is "walk" in CS:S

/// canonical key name -> (button bit, forwardmove sign, sidemove sign), in the Python order
pub const KEYS: [(&str, i64, i64, i64); 13] = [
    ("forward", IN_FORWARD, 1, 0),
    ("back", IN_BACK, -1, 0),
    ("moveleft", IN_MOVELEFT, 0, -1),
    ("moveright", IN_MOVERIGHT, 0, 1),
    ("jump", IN_JUMP, 0, 0),
    ("duck", IN_DUCK, 0, 0),
    ("walk", IN_SPEED, 0, 0),
    ("use", IN_USE, 0, 0),
    ("attack", IN_ATTACK, 0, 0),
    ("attack2", IN_ATTACK2, 0, 0),
    ("reload", IN_RELOAD, 0, 0),
    ("turnleft", IN_LEFT, 0, 0),
    ("turnright", IN_RIGHT, 0, 0),
];

/// Accepted spellings (case-insensitive). "left"/"right" are deliberately not aliases.
pub const ALIASES: [(&str, &str); 6] =
    [("w", "forward"), ("s", "back"), ("a", "moveleft"), ("d", "moveright"), ("crouch", "duck"), ("speed", "walk")];

pub fn is_movement_key(key: &str) -> bool {
    matches!(key, "forward" | "back" | "moveleft" | "moveright")
}

fn named_bit(bit: i64) -> Option<&'static str> {
    KEYS.iter().find(|k| k.1 == bit).map(|k| k.0)
}

/// The canonical key name for a user-written key, or None. Any button bit without
/// a name can be written as bitN (N = 0..31).
pub fn canonical_key(name: &str) -> Option<String> {
    let low = name.trim().to_lowercase();
    let low = ALIASES.iter().find(|a| a.0 == low).map(|a| a.1.to_string()).unwrap_or(low);
    if KEYS.iter().any(|k| k.0 == low) {
        return Some(low);
    }
    if let Some(n) = low.strip_prefix("bit") {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(i) = n.parse::<u32>() {
                if i <= 31 {
                    return Some(named_bit(1 << i).map(str::to_string).unwrap_or_else(|| format!("bit{}", i)));
                }
            }
        }
    }
    None
}

/// (button bit, forwardmove sign, sidemove sign) for a canonical key name.
pub fn key_info(key: &str) -> (i64, i64, i64) {
    if let Some(k) = KEYS.iter().find(|k| k.0 == key) {
        return (k.1, k.2, k.3);
    }
    let i: u32 = key[3..].parse().expect("canonical bitN key");
    (1 << i, 0, 0)
}

pub fn key_for_bit(bit_index: u32) -> String {
    named_bit(1 << bit_index).map(str::to_string).unwrap_or_else(|| format!("bit{}", bit_index))
}

/// The known names, for the "unknown key" message: sorted(set(KEYS) | set(ALIASES)).
pub fn known_names() -> Vec<&'static str> {
    let mut v: Vec<&str> = KEYS.iter().map(|k| k.0).chain(ALIASES.iter().map(|a| a.0)).collect();
    v.sort();
    v.dedup();
    v
}

/// The editor's display names (W A S D) for the movement keys (editdoc.DISPLAY_NAMES).
pub fn display_name(key: &str) -> String {
    match key {
        "forward" => "W",
        "back" => "S",
        "moveleft" => "A",
        "moveright" => "D",
        other => other,
    }
    .to_string()
}

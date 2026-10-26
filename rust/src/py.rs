//! Python-compatible numbers and text, so this back end writes byte for byte what
//! the Python one (tasurf/*.py) wrote: baked files, saved timelines, JSON, messages.
//!
//! Rust's fixed-precision float formatting rounds exactly like Python's (correctly
//! rounded, ties to even), and its shortest round-trip digits are Python's repr
//! digits; only the layout differs (`1e-05`, `90.0`), which `repr` handles.

use serde_json::Value;

/// `repr(x)` / `str(x)` for a float.
pub fn repr(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let (neg, digits, exp) = shortest(x);
    let decpt = exp + 1; // x = 0.DIGITS * 10^decpt
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if decpt <= -4 || decpt > 16 {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.abs()));
    } else if decpt <= 0 {
        out.push_str("0.");
        out.push_str(&"0".repeat((-decpt) as usize));
        out.push_str(&digits);
    } else if decpt as usize >= digits.len() {
        out.push_str(&digits);
        out.push_str(&"0".repeat(decpt as usize - digits.len()));
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// Sign, shortest round-trip digits and decimal exponent (x = d.ddd * 10^exp),
/// as Python's repr picks them: of the shortest digit strings that read back as
/// x, the one nearest x, ties to even. Rust's shortest form finds the length but
/// breaks an exact tie upwards (226.10855102539063 for ...0625), so the digits
/// are taken from the correctly rounded form of that length when it reads back.
fn shortest(x: f64) -> (bool, String, i32) {
    let split = |s: String| -> (bool, String, i32) {
        let (mant, exp) = s.split_once('e').expect("LowerExp has an exponent");
        let neg = mant.starts_with('-');
        let digits: String = mant.trim_start_matches('-').chars().filter(|c| *c != '.').collect();
        let digits = match digits.trim_end_matches('0') {
            "" => "0".to_string(),
            d => d.to_string(),
        };
        (neg, digits, exp.parse().expect("integer exponent"))
    };
    let short = split(format!("{:e}", x));
    let rounded = format!("{:.*e}", short.1.len() - 1, x);
    if rounded.parse::<f64>().ok() == Some(x) {
        return split(rounded);
    }
    short
}

/// `f"{x:.{places}f}"`
pub fn fixed(x: f64, places: usize) -> String {
    format!("{:.*}", places, x)
}

/// `f"{x:+.{places}f}"`
pub fn fixed_signed(x: f64, places: usize) -> String {
    format!("{:+.*}", places, x)
}

/// `round(x, ndigits)`: correctly rounded, ties to even, as a float again.
pub fn round(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    fixed(x, ndigits).parse().expect("formatted float parses")
}

/// `round(x)`: to an integer, ties to even.
pub fn round_int(x: f64) -> f64 {
    x.round_ties_even()
}

/// `f"{x:g}"`: 6 significant digits, trailing zeros removed.
pub fn g(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let s = format!("{:.5e}", x); // the value rounded to 6 significant digits
    let (mant, exp) = s.split_once('e').expect("LowerExp has an exponent");
    let exp: i32 = exp.parse().expect("integer exponent");
    let neg = mant.starts_with('-');
    let digits: String = mant.trim_start_matches('-').chars().filter(|c| *c != '.').collect();
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if (-4..6).contains(&exp) {
        let body = if exp < 0 {
            format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
        } else {
            let point = (exp + 1) as usize;
            format!("{}.{}", &digits[..point], &digits[point..])
        };
        let body = body.trim_end_matches('0').trim_end_matches('.');
        out.push_str(body);
    } else {
        let mut m = format!("{}.{}", &digits[..1], &digits[1..]);
        m = m.trim_end_matches('0').trim_end_matches('.').to_string();
        out.push_str(&m);
        out.push('e');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.abs()));
    }
    out
}

/// `f"{x:+g}"`
pub fn g_signed(x: f64) -> String {
    let s = g(x);
    if s.starts_with('-') {
        s
    } else {
        format!("+{}", s)
    }
}

/// Python's float `a % b`: the result takes the sign of b.
pub fn fmod(a: f64, b: f64) -> f64 {
    let m = a % b;
    if m != 0.0 {
        if (b < 0.0) != (m < 0.0) {
            m + b
        } else {
            m
        }
    } else {
        0.0f64.copysign(b)
    }
}

const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;
const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

/// `math.degrees`
pub fn degrees(x: f64) -> f64 {
    x * RAD_TO_DEG
}

/// `math.radians`
pub fn radians(x: f64) -> f64 {
    x * DEG_TO_RAD
}

/// `math.hypot(x, y)`: CPython 3.12's own algorithm (lossless scaling, exact
/// squares with fma, a differential correction), not the C library's hypot.
pub fn hypot(x: f64, y: f64) -> f64 {
    let (ax, ay) = (x.abs(), y.abs());
    if ax.is_infinite() || ay.is_infinite() {
        return f64::INFINITY;
    }
    if ax.is_nan() || ay.is_nan() {
        return f64::NAN;
    }
    let mut v = [ax, ay];
    vector_norm(&mut v, ax.max(ay))
}

fn vector_norm(vec: &mut [f64], max: f64) -> f64 {
    if max == 0.0 || vec.len() <= 1 {
        return max;
    }
    let max_e = frexp_exp(max);
    if max_e < -1023 {
        for v in vec.iter_mut() {
            *v /= f64::MIN_POSITIVE; // subnormals to normals
        }
        return f64::MIN_POSITIVE * vector_norm(vec, max / f64::MIN_POSITIVE);
    }
    let scale = pow2(-max_e);
    let (mut csum, mut frac1, mut frac2) = (1.0f64, 0.0f64, 0.0f64);
    for &v in vec.iter() {
        let x = v * scale; // lossless scaling
        let (hi, lo) = dl_mul(x, x); // lossless squaring
        let (shi, slo) = dl_fast_sum(csum, hi); // lossless addition
        csum = shi;
        frac1 += lo;
        frac2 += slo;
    }
    let mut h = (csum - 1.0 + (frac1 + frac2)).sqrt();
    let (hi, lo) = dl_mul(-h, h);
    let (shi, slo) = dl_fast_sum(csum, hi);
    csum = shi;
    frac1 += lo;
    frac2 += slo;
    let x = csum - 1.0 + (frac1 + frac2);
    h += x / (2.0 * h); // differential correction
    h / scale
}

fn dl_mul(x: f64, y: f64) -> (f64, f64) {
    let z = x * y;
    (z, x.mul_add(y, -z))
}

fn dl_fast_sum(a: f64, b: f64) -> (f64, f64) {
    let x = a + b;
    (x, (a - x) + b)
}

/// The exponent frexp gives: x = m * 2^e with 0.5 <= m < 1.
fn frexp_exp(x: f64) -> i32 {
    let bits = x.to_bits();
    let e = ((bits >> 52) & 0x7ff) as i32;
    if e == 0 {
        // subnormal: scale up by 2^64 first
        return frexp_exp(x * pow2(64)) - 64;
    }
    e - 1022
}

/// 2^k for k in the normal range.
fn pow2(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

/// float32 bits of x as 8 upper-case hex digits (`float_to_bits`), -0.0 as 0.0.
pub fn float_to_bits(x: f64) -> String {
    format!("{:08X}", ((x + 0.0) as f32).to_bits())
}

/// The float32 value of 8 hex digits (`bits_to_float`), as a double.
pub fn bits_to_float(hex: &str) -> Result<f64, String> {
    u32::from_str_radix(hex.trim(), 16)
        .map(|b| f32::from_bits(b) as f64)
        .map_err(|_| format!("invalid literal for int() with base 16: {}", repr_str(hex)))
}

/// x rounded to float32 and back (`_f32`).
pub fn f32_round(x: f64) -> f64 {
    ((x + 0.0) as f32) as f64
}

/// Python's `float(s)` for the strings this project writes (decimals, inf, nan).
pub fn parse_float(s: &str) -> Result<f64, String> {
    let t = s.trim();
    t.parse::<f64>().map_err(|_| format!("could not convert string to float: {}", repr_str(s)))
}

/// Python's `int(s)` for decimal strings.
pub fn parse_int(s: &str) -> Result<i64, String> {
    let t = s.trim();
    t.parse::<i64>().map_err(|_| format!("invalid literal for int() with base 10: {}", repr_str(s)))
}

/// `s.splitlines()`: lines without their line breaks, no empty last line.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let brk = matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !brk {
            continue;
        }
        out.push(&s[start..i]);
        let mut end = i + c.len_utf8();
        if c == '\r' {
            if let Some((j, '\n')) = chars.peek().copied() {
                chars.next();
                end = j + 1;
            }
        }
        start = end;
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

/// `s.strip()`.
pub fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// `str.isspace()` for one character.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

/// `s.partition(sep)` without the separator.
pub fn partition<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str) {
    match s.find(sep) {
        Some(i) => (&s[..i], &s[i + sep.len()..]),
        None => (s, ""),
    }
}

/// `repr(s)` for a str.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr(v)` for a value that came from JSON (None, True, 'text', [1, 2.0], {'k': v}).
pub fn repr_value(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Number(n) => number_text(n),
        Value::String(s) => repr_str(s),
        Value::Array(a) => format!("[{}]", a.iter().map(repr_value).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => format!(
            "{{{}}}",
            o.iter().map(|(k, v)| format!("{}: {}", repr_str(k), repr_value(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// `type(v).__name__` for a JSON value.
pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) => if n.is_f64() { "float" } else { "int" },
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn number_text(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        i.to_string()
    } else if let Some(u) = n.as_u64() {
        u.to_string()
    } else {
        repr(n.as_f64().unwrap_or(f64::NAN))
    }
}

// --------------------------------------------------------------------------
// JSON text as Python's json module writes it

/// `json.dumps(v)` (ensure_ascii) or `json.dumps(v, ensure_ascii=False)`.
pub fn dumps(v: &Value, ensure_ascii: bool) -> String {
    let mut out = String::new();
    write_json(v, ensure_ascii, &mut out);
    out
}

fn write_json(v: &Value, ascii: bool, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_f64() {
                let x = n.as_f64().unwrap_or(f64::NAN);
                out.push_str(&if x.is_nan() {
                    "NaN".to_string()
                } else if x.is_infinite() {
                    if x > 0.0 { "Infinity".to_string() } else { "-Infinity".to_string() }
                } else {
                    repr(x)
                });
            } else {
                out.push_str(&number_text(n));
            }
        }
        Value::String(s) => write_str(s, ascii, out),
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_json(x, ascii, out);
            }
            out.push(']');
        }
        Value::Object(o) => {
            out.push('{');
            for (i, (k, x)) in o.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_str(k, ascii, out);
                out.push_str(": ");
                write_json(x, ascii, out);
            }
            out.push('}');
        }
    }
}

fn write_str(s: &str, ascii: bool, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if ascii && (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for unit in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", unit));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A float as a JSON number that dumps like a Python float (`400.0`, not `400`).
pub fn num(x: f64) -> Value {
    serde_json::Number::from_f64(x).map(Value::Number).unwrap_or(Value::Null)
}

// --------------------------------------------------------------------------
// json.loads: the same document, and the same message for a broken one

/// `json.loads(text)`. A document Python rejects fails with Python's message
/// ("Expecting ',' delimiter: line 3 column 5 (char 20)"), so the errors for a
/// hand-edited file read the same as before.
pub fn loads(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| json_error(text).unwrap_or_else(|| e.to_string()))
}

/// The JSONDecodeError message Python's json module (its C scanner) gives for
/// text, or None when it would accept it (NaN, 1e400, lone surrogates: those are
/// the only things it reads that serde_json does not).
pub fn json_error(text: &str) -> Option<String> {
    let s: Vec<char> = text.chars().collect();
    let at = |msg: &str, pos: usize| -> String {
        let before = &s[..pos.min(s.len())];
        let line = before.iter().filter(|c| **c == '\n').count() + 1;
        let col = match before.iter().rposition(|c| *c == '\n') {
            Some(nl) => pos - nl,
            None => pos + 1,
        };
        format!("{}: line {} column {} (char {})", msg, line, col, pos)
    };
    if s.first() == Some(&'\u{feff}') {
        return Some(at("Unexpected UTF-8 BOM (decode using utf-8-sig)", 0));
    }
    let start = skip_ws(&s, 0);
    match scan_value(&s, start) {
        Err((msg, pos)) => Some(at(msg, pos)),
        Ok(end) => {
            let end = skip_ws(&s, end);
            if end != s.len() {
                Some(at("Extra data", end))
            } else {
                None
            }
        }
    }
}

type Scan = Result<usize, (&'static str, usize)>;

fn skip_ws(s: &[char], mut i: usize) -> usize {
    while i < s.len() && matches!(s[i], ' ' | '\t' | '\n' | '\r') {
        i += 1;
    }
    i
}

fn word_at(s: &[char], i: usize, word: &str) -> bool {
    let w: Vec<char> = word.chars().collect();
    // the C scanner wants one more character after the word's first one (`idx + n < length`)
    i + w.len() - 1 < s.len() && s[i..i + w.len()] == w[..]
}

fn scan_value(s: &[char], i: usize) -> Scan {
    if i >= s.len() {
        return Err(("Expecting value", i));
    }
    match s[i] {
        '"' => return scan_string(s, i + 1),
        '{' => return scan_object(s, i + 1),
        '[' => return scan_array(s, i + 1),
        'n' if word_at(s, i, "null") => return Ok(i + 4),
        't' if word_at(s, i, "true") => return Ok(i + 4),
        'f' if word_at(s, i, "false") => return Ok(i + 5),
        'N' if word_at(s, i, "NaN") => return Ok(i + 3),
        'I' if word_at(s, i, "Infinity") => return Ok(i + 8),
        '-' if word_at(s, i, "-Infinity") => return Ok(i + 9),
        _ => {}
    }
    scan_number(s, i)
}

fn scan_number(s: &[char], start: usize) -> Scan {
    let digit = |i: usize| i < s.len() && s[i].is_ascii_digit();
    let end_idx = s.len() - 1;
    let mut i = start;
    if s[i] == '-' {
        i += 1;
        if i > end_idx {
            return Err(("Expecting value", start));
        }
    }
    if ('1'..='9').contains(&s[i]) {
        i += 1;
        while digit(i) {
            i += 1;
        }
    } else if s[i] == '0' {
        i += 1;
    } else {
        return Err(("Expecting value", start));
    }
    if i < end_idx && s[i] == '.' && digit(i + 1) {
        i += 2;
        while digit(i) {
            i += 1;
        }
    }
    if i < end_idx && (s[i] == 'e' || s[i] == 'E') {
        let e_start = i;
        i += 1;
        if i < end_idx && (s[i] == '-' || s[i] == '+') {
            i += 1;
        }
        while digit(i) {
            i += 1;
        }
        if !s[i - 1].is_ascii_digit() {
            i = e_start;
        }
    }
    Ok(i)
}

fn scan_string(s: &[char], mut end: usize) -> Scan {
    let begin = end - 1;
    let hex = |c: char| c.is_ascii_hexdigit();
    loop {
        let mut next = end;
        while next < s.len() && s[next] != '"' && s[next] != '\\' {
            if (s[next] as u32) <= 0x1f {
                return Err(("Invalid control character at", next));
            }
            next += 1;
        }
        if next >= s.len() {
            return Err(("Unterminated string starting at", begin));
        }
        if s[next] == '"' {
            return Ok(next + 1);
        }
        next += 1; // past the backslash
        if next == s.len() {
            return Err(("Unterminated string starting at", begin));
        }
        if s[next] != 'u' {
            end = next + 1;
            if !matches!(s[next], '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't') {
                return Err(("Invalid \\escape", end - 2));
            }
            continue;
        }
        next += 1;
        end = next + 4;
        if end >= s.len() {
            return Err(("Invalid \\uXXXX escape", next - 1));
        }
        if !s[next..end].iter().all(|c| hex(*c)) {
            return Err(("Invalid \\uXXXX escape", end - 5));
        }
        let c = u32::from_str_radix(&s[next..end].iter().collect::<String>(), 16).unwrap_or(0);
        // a surrogate pair \ud83d\ude00
        if (0xD800..0xDC00).contains(&c) && end + 6 < s.len() && s[end] == '\\' && s[end + 1] == 'u' {
            let second = end + 2;
            if !s[second..second + 4].iter().all(|c| hex(*c)) {
                return Err(("Invalid \\uXXXX escape", end + 1));
            }
            let c2 = u32::from_str_radix(&s[second..second + 4].iter().collect::<String>(), 16).unwrap_or(0);
            if (0xDC00..0xE000).contains(&c2) {
                end += 6;
            }
        }
    }
}

fn scan_object(s: &[char], i: usize) -> Scan {
    let mut i = skip_ws(s, i);
    if i >= s.len() || s[i] != '}' {
        loop {
            if i >= s.len() || s[i] != '"' {
                return Err(("Expecting property name enclosed in double quotes", i));
            }
            i = skip_ws(s, scan_string(s, i + 1)?);
            if i >= s.len() || s[i] != ':' {
                return Err(("Expecting ':' delimiter", i));
            }
            i = skip_ws(s, scan_value(s, skip_ws(s, i + 1))?);
            if i < s.len() && s[i] == '}' {
                break;
            }
            if i >= s.len() || s[i] != ',' {
                return Err(("Expecting ',' delimiter", i));
            }
            i = skip_ws(s, i + 1);
        }
    }
    Ok(i + 1)
}

fn scan_array(s: &[char], i: usize) -> Scan {
    let mut i = skip_ws(s, i);
    if i >= s.len() || s[i] != ']' {
        loop {
            i = skip_ws(s, scan_value(s, i)?);
            if i < s.len() && s[i] == ']' {
                break;
            }
            if i >= s.len() || s[i] != ',' {
                return Err(("Expecting ',' delimiter", i));
            }
            i = skip_ws(s, i + 1);
        }
    }
    Ok(i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_matches_python() {
        let cases = [
            (1e16, "1e+16"), (1e15, "1000000000000000.0"), (0.0001, "0.0001"), (0.00001, "1e-05"),
            (92.75032806396484, "92.75032806396484"), (400.0, "400.0"), (-0.0, "-0.0"), (1.5e-7, "1.5e-07"),
            (123456789012345680.0, "1.2345678901234568e+17"), (0.1, "0.1"), (-2.5, "-2.5"), (1e100, "1e+100"),
            (226.108551025390625, "226.10855102539062"), (31.542007446289062, "31.542007446289062"),
            (-697.2927856445312, "-697.2927856445312"), (1e20, "1e+20"), (5e-324, "5e-324"),
        ];
        for (x, want) in cases {
            assert_eq!(repr(x), want, "{x}");
        }
    }

    #[test]
    fn g_matches_python() {
        let cases = [(800.0, "800"), (150.0, "150"), (0.015, "0.015"), (66.6666, "66.6666"), (1234567.0, "1.23457e+06"),
                     (0.00001234, "1.234e-05"), (2.5, "2.5"), (-45.0, "-45"), (100000.0, "100000"), (1e6, "1e+06")];
        for (x, want) in cases {
            assert_eq!(g(x), want, "{x}");
        }
        assert_eq!(g_signed(45.0), "+45");
    }

    #[test]
    fn fixed_rounds_ties_to_even() {
        assert_eq!(fixed(0.125, 2), "0.12");
        assert_eq!(fixed(0.0078125, 6), "0.007812");
        assert_eq!(round(2.675, 2), 2.67); // 2.675 is just below the tie in binary
    }

    #[test]
    fn python_mod() {
        assert_eq!(fmod(-10.0, 360.0), 350.0);
        assert_eq!(fmod(10.0, -360.0), -350.0);
    }

    #[test]
    fn splitlines_like_python() {
        assert_eq!(splitlines("a\nb\r\nc\rd\n"), ["a", "b", "c", "d"]);
        assert_eq!(splitlines("a\n\nb"), ["a", "", "b"]);
        assert!(splitlines("").is_empty());
    }

    #[test]
    fn json_errors_like_python() {
        let cases = [
            ("", "Expecting value: line 1 column 1 (char 0)"),
            ("{", "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ("[", "Expecting value: line 1 column 2 (char 1)"),
            ("{\"format\": ", "Expecting value: line 1 column 12 (char 11)"),
            ("[1,]", "Expecting value: line 1 column 4 (char 3)"),
            ("{\"a\" 1}", "Expecting ':' delimiter: line 1 column 6 (char 5)"),
            ("{a: 1}", "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ("[1 2]", "Expecting ',' delimiter: line 1 column 4 (char 3)"),
            ("\"abc", "Unterminated string starting at: line 1 column 1 (char 0)"),
            ("[1] x", "Extra data: line 1 column 5 (char 4)"),
            ("\"a\u{1}\"", "Invalid control character at: line 1 column 3 (char 2)"),
            ("\"\\q\"", "Invalid \\escape: line 1 column 2 (char 1)"),
            ("[\"\\u12\"]", "Invalid \\uXXXX escape: line 1 column 4 (char 3)"),
            ("[\"\\u12G4\"]", "Invalid \\uXXXX escape: line 1 column 4 (char 3)"),
            ("-", "Expecting value: line 1 column 1 (char 0)"),
            ("01", "Extra data: line 1 column 2 (char 1)"),
            ("{\"a\":1,}", "Expecting property name enclosed in double quotes: line 1 column 8 (char 7)"),
            ("\u{feff}{}", "Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)"),
            ("[nul]", "Expecting value: line 1 column 2 (char 1)"),
            ("{\"a\":1 \"b\":2}", "Expecting ',' delimiter: line 1 column 8 (char 7)"),
            ("\n\n  {\"x\": [1, 2,\n  ]}", "Expecting value: line 4 column 3 (char 19)"),
            ("[1e]", "Expecting ',' delimiter: line 1 column 3 (char 2)"),
            ("[-]", "Expecting value: line 1 column 2 (char 1)"),
            ("tru", "Expecting value: line 1 column 1 (char 0)"),
            ("[1.]", "Expecting ',' delimiter: line 1 column 3 (char 2)"),
            ("  ", "Expecting value: line 1 column 3 (char 2)"),
            ("{\"a\": }", "Expecting value: line 1 column 7 (char 6)"),
            ("{\"\u{e9}\": [1, 2", "Expecting ',' delimiter: line 1 column 12 (char 11)"),
            ("[1, 2", "Expecting ',' delimiter: line 1 column 6 (char 5)"),
            ("{\"a\": 1", "Expecting ',' delimiter: line 1 column 8 (char 7)"),
            ("\"abc\\", "Unterminated string starting at: line 1 column 1 (char 0)"),
            ("{\"a\":1}}", "Extra data: line 1 column 8 (char 7)"),
            ("[]]", "Extra data: line 1 column 3 (char 2)"),
            ("nulls", "Extra data: line 1 column 5 (char 4)"),
            ("{\"a\"", "Expecting ':' delimiter: line 1 column 5 (char 4)"),
            ("{\"a\":", "Expecting value: line 1 column 6 (char 5)"),
            ("[1,", "Expecting value: line 1 column 4 (char 3)"),
            ("{\"k\": \"v\",", "Expecting property name enclosed in double quotes: line 1 column 11 (char 10)"),
            ("{,}", "Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ("[,]", "Expecting value: line 1 column 2 (char 1)"),
            ("{\"a\":1, \"b\"}", "Expecting ':' delimiter: line 1 column 12 (char 11)"),
        ];
        for (text, want) in cases {
            assert_eq!(json_error(text).as_deref(), Some(want), "{:?}", text);
            assert_eq!(loads(text).unwrap_err(), want, "{:?}", text);
        }
        for ok in ["\"\\ud800\"", "[NaN, Infinity, -Infinity]", "[1e400]", "{\"a\": [1, 2.5e-3, true, null, \"\\u00e9\\ud83d\\ude00\"]}"] {
            assert_eq!(json_error(ok), None, "{:?}", ok);
        }
    }

    #[test]
    fn math_matches_cpython_bit_for_bit() {
        let f = |s: &str| f64::from_bits(u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap());
        let mut n = 0;
        for line in include_str!("../testdata/cpython_math.txt").lines().filter(|l| !l.starts_with('#')) {
            let p: Vec<&str> = line.split_whitespace().collect();
            let (x, y) = (f(p[1]), f(p[2]));
            match p[0] {
                "hypot" => assert_eq!(hypot(x, y).to_bits(), f(p[3]).to_bits(), "{}", line),
                "angle" => {
                    assert_eq!(degrees(y.atan2(x)).to_bits(), f(p[3]).to_bits(), "{}", line);
                    assert_eq!(radians(x).to_bits(), f(p[4]).to_bits(), "{}", line);
                }
                other => panic!("unknown case {}", other),
            }
            n += 1;
        }
        assert_eq!(n, 500);
    }

    #[test]
    fn hypot_basic() {
        assert_eq!(hypot(3.0, 4.0), 5.0);
        assert_eq!(hypot(0.0, -7.5), 7.5);
    }

    #[test]
    fn dumps_like_python() {
        let v: Value = serde_json::from_str(r#"{"a": [1, 2.0, "x\"y"], "b": null, "c": true, "é": 1e-5}"#).unwrap();
        assert_eq!(dumps(&v, false), r#"{"a": [1, 2.0, "x\"y"], "b": null, "c": true, "é": 1e-05}"#);
        assert_eq!(dumps(&v, true), r#"{"a": [1, 2.0, "x\"y"], "b": null, "c": true, "\u00e9": 1e-05}"#);
    }
}

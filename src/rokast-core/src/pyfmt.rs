//! Python's `repr()` of strings and string lists: the messages rokast shows were written with `{x!r}`, and the
//! port keeps them byte for byte (tests/parity).

/// `repr(s)` for a str: single quotes unless the text has a single quote and no double quote.
pub fn repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::with_capacity(s.len() + 2);
    out.push(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

/// `repr(list_of_str)`.
pub fn repr_list<S: AsRef<str>>(items: &[S]) -> String {
    let parts: Vec<String> = items.iter().map(|s| repr(s.as_ref())).collect();
    format!("[{}]", parts.join(", "))
}

/// `str(value)` for a JSON value the way an f-string prints the Python object it came from (None, True, 1, text).
pub fn py_str(v: Option<&serde_json::Value>) -> String {
    use serde_json::Value;
    match v {
        None | Some(Value::Null) => "None".into(),
        Some(Value::Bool(b)) => (if *b { "True" } else { "False" }).into(),
        Some(Value::String(s)) => s.clone(),
        Some(other) => other.to_string(),
    }
}

/// `json.dumps(v)` with Python's defaults: ", " and ": " separators, non-ASCII as \uXXXX. Keeps `v`'s key order.
pub fn dumps(v: &serde_json::Value) -> String {
    use serde_json::Value;
    match v {
        Value::String(s) => {
            let mut out = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    '\u{8}' => out.push_str("\\b"),
                    '\u{c}' => out.push_str("\\f"),
                    ' '..='~' | '\u{7f}' => out.push(c),
                    c => {
                        let mut buf = [0u16; 2];
                        for u in c.encode_utf16(&mut buf) {
                            out.push_str(&format!("\\u{u:04x}"));
                        }
                    }
                }
            }
            out.push('"');
            out
        }
        Value::Array(a) => format!("[{}]", a.iter().map(dumps).collect::<Vec<_>>().join(", ")),
        Value::Object(o) => {
            let items: Vec<String> =
                o.iter().map(|(k, v)| format!("{}: {}", dumps(&Value::from(k.as_str())), dumps(v))).collect();
            format!("{{{}}}", items.join(", "))
        }
        other => other.to_string(), // null, true/false, numbers print the same
    }
}

/// Python's `str.isprintable()` for one character, close enough for device names: no control, format, separator
/// (other than the ASCII space) or private-use characters.
pub fn is_printable(c: char) -> bool {
    let u = c as u32;
    if c == ' ' {
        return true;
    }
    if c.is_control() || c.is_whitespace() {
        return false;
    }
    !matches!(u, 0xAD | 0x600..=0x605 | 0x61C | 0x6DD | 0x70F | 0x180E | 0x200B..=0x200F | 0x202A..=0x202E
        | 0x2060..=0x2064 | 0x2066..=0x206F | 0xFEFF | 0xFFF9..=0xFFFB | 0xE000..=0xF8FF | 0xD800..=0xDFFF
        | 0xF0000..=0x10FFFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_matches_python() {
        assert_eq!(repr("DP-3"), "'DP-3'");
        assert_eq!(repr("Casper's"), "\"Casper's\"");
        assert_eq!(repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(repr("x\ny\\"), "'x\\ny\\\\'");
        assert_eq!(repr_list(&["A", "B"]), "['A', 'B']");
        assert_eq!(repr_list::<&str>(&[]), "[]");
    }

    #[test]
    fn dumps_matches_python() {
        // python3 -c 'import json; print(json.dumps({"b": [1, None, True], "a": "T\u00e9l\u00e9 \U0001f4fa \"q\"\\n\x01"}))'
        let v = serde_json::json!({"b": [1, null, true], "a": "T\u{e9}l\u{e9} \u{1f4fa} \"q\"\n\u{1}"});
        assert_eq!(dumps(&v), r#"{"b": [1, null, true], "a": "T\u00e9l\u00e9 \ud83d\udcfa \"q\"\n\u0001"}"#);
        assert_eq!(dumps(&serde_json::json!([])), "[]");
    }
}

//! A result bounded as a whole (handoff 009i, audit A-26; REQ-MCP-006,
//! REQ-CTX-006, REQ-CTX-009).
//!
//! A result is cleaned first (every string and key sanitized and redacted,
//! nothing cut), then checked against its output schema by the caller, and only
//! then bounded: a cut can never make a result `output_invalid`. The bound is
//! on the serialized payload in total. The longest strings are cut first, to
//! one common length, so that the JSON stays valid and the short strings, which
//! often hold the facts, stay whole. Each kind of change is counted once per
//! result, and the caller says it once.
//!
//! Nothing here reads a clock or takes a lock; the functions are pure.

use mimlys_core::context::Redactor;
use mimlys_core::limits::LIM_I5_MESSAGE_CHARS;
use mimlys_core::observation::{excerpt, sanitize};
use serde_json::{Map, Value, json};
use std::io::Write;

/// How a serialized payload is measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// In characters: what the model's context budget counts (LIM-C1).
    Chars,
    /// In bytes: what the stored copy is bounded by (LIM-S4).
    Bytes,
}

/// The size of the JSON text of `value`, in `unit`.
pub fn serialized_size(value: &Value, unit: Unit) -> usize {
    struct Count {
        unit: Unit,
        n: usize,
    }
    impl Write for Count {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.n += match self.unit {
                Unit::Bytes => buf.len(),
                // A byte that is not a continuation byte starts a character.
                Unit::Chars => buf.iter().filter(|b| (**b & 0xC0) != 0x80).count(),
            };
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut count = Count { unit, n: 0 };
    // Serializing a `Value` into a counter cannot fail.
    let _ = serde_json::to_writer(&mut count, value);
    count.n
}

/// The size of one string as a JSON value (quotes and escapes included).
fn string_size(text: &str, unit: Unit) -> usize {
    serialized_size(&Value::String(text.to_owned()), unit)
}

/// What cleaning a result removed.
#[derive(Debug, Default)]
pub struct Cleaned {
    /// Characters removed from strings and keys (REQ-CTX-008), in total.
    pub removed: u64,
    /// The names of the secrets that were replaced (REQ-CTX-007).
    pub found: Vec<String>,
}

/// Sanitizes and redacts every string and every key of a payload, cutting
/// nothing: the check against the output schema comes next, and a cut must
/// never be the reason for `output_invalid`. Each text is redacted whole.
pub fn clean(value: &mut Value, redactor: &Redactor, cleaned: &mut Cleaned) {
    match value {
        Value::String(s) => {
            if let Some(text) = clean_text(s, redactor, cleaned) {
                *s = text;
            }
        }
        Value::Array(items) => {
            for item in items {
                clean(item, redactor, cleaned);
            }
        }
        Value::Object(map) => {
            rekey(map, |key, item| {
                clean(item, redactor, cleaned);
                clean_text(key, redactor, cleaned)
            });
        }
        _ => {}
    }
}

/// The clean text of `text`, when cleaning changed it.
fn clean_text(text: &str, redactor: &Redactor, cleaned: &mut Cleaned) -> Option<String> {
    let (clean, removed) = sanitize(text);
    cleaned.removed += removed as u64;
    let (clean, names) = redactor.redact(&clean);
    let redacted = !names.is_empty();
    cleaned.found.extend(names);
    (removed > 0 || redacted).then_some(clean)
}

/// Rebuilds an object with new keys: `f` gets each key and its value, and
/// returns the new key when it changes. Two keys that come to one text keep both
/// values (`key#2`).
fn rekey(map: &mut Map<String, Value>, mut f: impl FnMut(&str, &mut Value) -> Option<String>) {
    let entries = std::mem::take(map);
    for (key, mut item) in entries {
        let mut key = f(&key, &mut item).unwrap_or(key);
        let mut n = 1;
        while map.contains_key(&key) {
            n += 1;
            key = format!("{key}#{n}");
        }
        map.insert(key, item);
    }
}

/// A result cut to its bound.
#[derive(Debug)]
pub struct Cut {
    /// The bounded result.
    pub value: Value,
    /// The size in characters of everything cut: the full length of each string
    /// and key cut, and the whole serialized result when it was replaced.
    pub total_chars: u64,
}

/// Bounds `value` to `limit` in `unit`: `None` when it is within the limit
/// already (and, with `keys`, holds no key over LIM-I5). Otherwise the keys over
/// LIM-I5 are cut (with `keys`), then the longest strings, all to one common
/// length found by search, so that the result fits; when the structure alone is
/// over the limit (more keys, numbers and brackets than it can hold) the result
/// is replaced by a short object that says its size. The result is valid JSON
/// in every case.
pub fn bound(value: &Value, limit: usize, unit: Unit, keys: bool) -> Option<Cut> {
    let size = serialized_size(value, unit);
    if size <= limit && !(keys && has_long_key(value)) {
        return None;
    }
    let mut cut = value.clone();
    let mut total_chars = 0u64;
    if keys {
        cut_keys(&mut cut, &mut total_chars);
    }
    let size = serialized_size(&cut, unit);
    if size > limit {
        // The strings, longest first: the largest common length that fits.
        let mut strings = Vec::new();
        collect(&cut, &mut strings);
        let longest = strings.iter().map(|s| s.chars().count()).max().unwrap_or(0);
        let fits = |cap: usize| size_with_cap(size, &strings, cap, unit) <= limit;
        // `fits(longest)` is the size above, over the limit.
        let cap = (longest > 0 && fits(0)).then(|| {
            let (mut lo, mut hi) = (0, longest);
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                if fits(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo
        });
        if let Some(cap) = cap {
            cap_strings(&mut cut, cap, &mut total_chars);
        }
    }
    if serialized_size(&cut, unit) > limit {
        // What is left is structure: say what was there and no more.
        let whole = serialized_size(value, Unit::Chars) as u64;
        return Some(Cut {
            value: json!({
                "truncated": "the result is too large to show",
                "total_chars": whole,
            }),
            total_chars: whole,
        });
    }
    Some(Cut {
        value: cut,
        total_chars,
    })
}

/// Whether an object of the payload has a key over LIM-I5.
fn has_long_key(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(has_long_key),
        Value::Object(map) => map
            .iter()
            .any(|(k, v)| k.chars().nth(LIM_I5_MESSAGE_CHARS).is_some() || has_long_key(v)),
        _ => false,
    }
}

/// A string cut to `cap` characters: its first three quarters and its last
/// quarter with a marker between, as every excerpt is (LIM-C2); `None` when it
/// is within the cap, or when the cut would not make it shorter.
fn cut_string(text: &str, cap: usize) -> Option<String> {
    let n = text.chars().count();
    if n <= cap {
        return None;
    }
    let head = cap * 3 / 4;
    let (cut, _) = excerpt(text, head, cap - head);
    (cut.chars().count() < n).then_some(cut)
}

/// Every string value of a payload.
fn collect<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(s) => out.push(s),
        Value::Array(items) => items.iter().for_each(|i| collect(i, out)),
        Value::Object(map) => map.values().for_each(|i| collect(i, out)),
        _ => {}
    }
}

/// The size of the payload when every string is cut to `cap`, from its size
/// now: only the strings that change are measured.
fn size_with_cap(size: usize, strings: &[&str], cap: usize, unit: Unit) -> usize {
    let mut size = size;
    for s in strings {
        if let Some(cut) = cut_string(s, cap) {
            // A short cut can be longer serialized than what it replaced (the
            // marker's newlines are escaped, its `…` is three bytes), so the cut
            // is added before the string is taken away: `size` still holds `s`.
            size = size + string_size(&cut, unit) - string_size(s, unit);
        }
    }
    size
}

/// Cuts every string of a payload to `cap` characters, adding the full length
/// of each one cut to `total`.
fn cap_strings(value: &mut Value, cap: usize, total: &mut u64) {
    match value {
        Value::String(s) => {
            if let Some(cut) = cut_string(s, cap) {
                *total += s.chars().count() as u64;
                *s = cut;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| cap_strings(i, cap, total)),
        Value::Object(map) => map.values_mut().for_each(|i| cap_strings(i, cap, total)),
        _ => {}
    }
}

/// Cuts every key longer than LIM-I5 to it (the bound of a message), adding its
/// full length to `total`. A key is a name the model reads, not data to be
/// excerpted: it is cut at the head.
fn cut_keys(value: &mut Value, total: &mut u64) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(|i| cut_keys(i, total)),
        Value::Object(map) => rekey(map, |key, item| {
            cut_keys(item, total);
            let n = key.chars().count();
            (n > LIM_I5_MESSAGE_CHARS).then(|| {
                *total += n as u64;
                key.chars().take(LIM_I5_MESSAGE_CHARS).collect()
            })
        }),
        _ => {}
    }
}

#[cfg(test)]
mod tests;

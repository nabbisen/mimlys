//! Fitting the arguments of a request into what LIM-I6 leaves (REQ-APR-002).
//!
//! The arguments line names every argument. The characters that remain after
//! the names, the ` = ` and the separators are shared out among the values by
//! the same rule for each: a value that fits in full is shown in full, and the
//! others are cut to equal shares (the shares of the shorter ones, which need
//! less, go to the longer ones). A cut value ends with a marker that says how
//! many characters are left out, so the user knows what the line does not show.
//!
//! **Whether every name fits.** An action has at most LIM-I3 (8) arguments, each
//! named by a parameter of its tool. What a name takes is its length, three
//! characters for ` = `, three for the separator, and at least 29 for a cut
//! value (`""… (262 144 more characters)`, a value being at most LIM-D1 long).
//! With a 64-character tool name, the longest server and tool names together,
//! and a reason of 500 quotes (the longest once quoted), LIM-I6 leaves 2 650
//! characters or more for the line, which holds eight names of up to 290
//! characters. The limits bound the number of parameters and the length of a
//! tool's name, but not the length of a parameter's name, so a tool can declare
//! names that do not fit; the request is then not made ([`ArgumentsDoNotFit`])
//! and the runtime refuses the action. A bound on the length of a parameter's
//! name, below 290 characters, makes that case unreachable, and this check
//! stays as the guard.

use super::{ArgumentView, ArgumentsDoNotFit};

/// Between two arguments on the line.
const SEPARATOR: &str = " · ";
/// Between an argument's name and its value.
const EQUALS: &str = " = ";
/// Before and after the resolved path of a path parameter.
const RESOLVED_OPEN: &str = " (resolved: ";
const RESOLVED_CLOSE: &str = ")";

/// The marker after a cut value: how many characters are left out.
pub(super) fn marker(omitted: usize) -> String {
    let digits = omitted.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            grouped.push(' ');
        }
        grouped.push(c);
    }
    // The marker is longer than the one character that would be left out, so a
    // value is never cut for one character: the plural is always the right form.
    format!("… ({grouped} more characters)")
}

fn marker_len(omitted: usize) -> usize {
    marker(omitted).chars().count()
}

/// One value as it is shown: a quoted string, which is cut inside its quotes
/// and keeps them, or the JSON text of any other value.
struct Piece<'a> {
    text: &'a str,
}

impl<'a> Piece<'a> {
    fn quoted(&self) -> bool {
        self.text.starts_with('"')
    }

    /// The text that is cut, and the characters around it that stay.
    fn body(&self) -> (&'a str, usize) {
        if self.quoted() && self.text.len() >= 2 {
            (&self.text[1..self.text.len() - 1], 2)
        } else {
            (self.text, 0)
        }
    }

    /// The pieces of the body that are cut whole: a character, or a backslash
    /// sequence (which is one character of the string).
    fn units(&self) -> impl Iterator<Item = &'a str> {
        let (body, _) = self.body();
        let quoted = self.quoted();
        let mut rest = body;
        std::iter::from_fn(move || {
            let first = rest.chars().next()?;
            let len = if quoted && first == '\\' {
                match rest[1..].chars().next() {
                    Some('u') => 6.min(rest.len()),
                    Some(c) => 1 + c.len_utf8(),
                    None => 1,
                }
            } else {
                first.len_utf8()
            };
            let (unit, tail) = rest.split_at(len);
            rest = tail;
            Some(unit)
        })
    }

    fn full_len(&self) -> usize {
        self.text.chars().count()
    }

    fn unit_count(&self) -> usize {
        self.units().count()
    }

    /// The least this value takes when it is cut: nothing of it kept.
    fn floor(&self) -> usize {
        let (_, around) = self.body();
        around + marker_len(self.unit_count())
    }

    /// The value within `share` characters: in full when it fits, else as
    /// many of its first characters as fit, then the marker.
    fn fit(&self, share: usize) -> String {
        if self.full_len() <= share {
            return self.text.to_owned();
        }
        let (body, around) = self.body();
        let total = self.unit_count();
        let mut used = around;
        let mut kept_bytes = 0;
        let mut kept_units = 0;
        for (i, unit) in self.units().enumerate() {
            used += unit.chars().count();
            // The line grows with every unit kept and the marker shrinks by
            // a character only when its number loses a digit: the first unit
            // that does not fit ends the search.
            if used + marker_len(total - (i + 1)) > share {
                break;
            }
            kept_bytes += unit.len();
            kept_units = i + 1;
        }
        let omitted = total - kept_units;
        if self.quoted() {
            format!("\"{}\"{}", &body[..kept_bytes], marker(omitted))
        } else {
            format!("{}{}", &body[..kept_bytes], marker(omitted))
        }
    }
}

/// How an argument's name is shown: as it is when it is a plain identifier,
/// else JSON-quoted, so that a name cannot pass for a value or a separator.
pub(super) fn shown_name(name: &str) -> String {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        name.to_owned()
    } else {
        serde_json::to_string(name).expect("a string serializes")
    }
}

/// The arguments line for `views`, at most `available` characters, every
/// argument named. Fails when even the names and the markers do not fit.
pub(super) fn fit(views: &[ArgumentView], available: usize) -> Result<String, ArgumentsDoNotFit> {
    if views.is_empty() {
        let none = "none";
        return if none.len() <= available {
            Ok(none.to_owned())
        } else {
            Err(ArgumentsDoNotFit {
                needed: none.len(),
                available,
            })
        };
    }
    // The values to share out: each argument's value, and its resolved path.
    let mut pieces: Vec<Piece<'_>> = Vec::new();
    let mut overhead = SEPARATOR.chars().count() * (views.len() - 1);
    for v in views {
        overhead += shown_name(&v.name).chars().count() + EQUALS.chars().count();
        pieces.push(Piece { text: &v.value });
        if let Some(resolved) = &v.resolved {
            overhead += RESOLVED_OPEN.chars().count() + RESOLVED_CLOSE.chars().count();
            pieces.push(Piece { text: resolved });
        }
    }
    let fulls: Vec<usize> = pieces.iter().map(Piece::full_len).collect();
    // A value that is no longer than the least its cut takes is never cut.
    let floors: Vec<usize> = pieces
        .iter()
        .zip(&fulls)
        .map(|(p, full)| p.floor().min(*full))
        .collect();
    let needed = overhead + floors.iter().sum::<usize>();
    if needed > available {
        return Err(ArgumentsDoNotFit { needed, available });
    }
    let pool = available - overhead;
    let shares: Vec<usize> = if fulls.iter().sum::<usize>() <= pool {
        fulls.clone()
    } else {
        shares(&fulls, &floors, pool)
    };
    let mut shown = shares
        .iter()
        .zip(&pieces)
        .map(|(share, piece)| piece.fit(*share));
    let mut parts = Vec::new();
    for v in views {
        let value = shown.next().expect("a share per value");
        let mut part = format!("{}{EQUALS}{value}", shown_name(&v.name));
        if v.resolved.is_some() {
            let resolved = shown.next().expect("a share per resolved path");
            part = format!("{part}{RESOLVED_OPEN}{resolved}{RESOLVED_CLOSE}");
        }
        parts.push(part);
    }
    Ok(parts.join(SEPARATOR))
}

/// The share of each value: the largest level at which every value takes at
/// most the level (and at least its floor) within `pool`; what that leaves,
/// less than one character per value that is still cut, goes to the first ones.
fn shares(fulls: &[usize], floors: &[usize], pool: usize) -> Vec<usize> {
    let take = |level: usize| -> Vec<usize> {
        fulls
            .iter()
            .zip(floors)
            .map(|(full, floor)| level.max(*floor).min(*full))
            .collect()
    };
    let (mut low, mut high) = (0, fulls.iter().copied().max().unwrap_or(0));
    while low < high {
        let mid = (low + high).div_ceil(2);
        if take(mid).iter().sum::<usize>() <= pool {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    let mut shares = take(low);
    let mut left = pool - shares.iter().sum::<usize>();
    for (share, full) in shares.iter_mut().zip(fulls) {
        if left == 0 {
            break;
        }
        if *share < *full {
            *share += 1;
            left -= 1;
        }
    }
    shares
}

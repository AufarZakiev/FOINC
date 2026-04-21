//! Canonical stdout normalization used when computing a `result_hash`.
//!
//! The rules are intentionally narrow: we only strip *trailing* whitespace
//! from each line and drop *trailing* blank lines. Leading whitespace and
//! in-line spacing are preserved because they may be semantically
//! meaningful (indented JSON, fixed-width numeric output, etc.). The goal
//! is to make two honest workers who disagree only about trailing editor
//! artefacts hash to the same bytes, while still catching genuine
//! disagreement.

/// Canonicalize stdout for hashing.
///
/// - Every line has its trailing whitespace removed (covers `\r\n` -> `\n`,
///   trailing spaces/tabs, etc.).
/// - Trailing blank lines are dropped so that `"a\n"` and `"a\n\n"` hash
///   the same.
/// - Leading blank lines and internal blank lines are preserved.
/// - The output never has a trailing `\n`. Callers that want one must add
///   it explicitly (the aggregation endpoint does NOT call this function;
///   see the spec).
///
/// Pure, deterministic, zero-allocation for the happy path would be nice
/// but we keep the implementation obvious: split on `\n`, rstrip, rejoin.
pub fn normalize_stdout(s: &str) -> String {
    // `split('\n')` preserves an empty trailing element when the input
    // ends with `\n`; we want that so the rstrip/drop-trailing-blank pass
    // below sees every logical line exactly once.
    let mut lines: Vec<&str> = s
        .split('\n')
        .map(|line| line.trim_end_matches(|c: char| c.is_whitespace()))
        .collect();

    // Drop trailing blank lines.
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rstrips_each_line() {
        assert_eq!(normalize_stdout("a   \nb\t\t\n"), "a\nb");
    }

    #[test]
    fn drops_trailing_blank_lines() {
        assert_eq!(normalize_stdout("a\n\n\n"), "a");
    }

    #[test]
    fn normalizes_crlf_to_lf() {
        // `\r\n` -> rstrip drops the `\r`, split on `\n` joins with `\n`.
        assert_eq!(normalize_stdout("a\r\nb\r\n"), "a\nb");
    }

    #[test]
    fn preserves_leading_and_internal_blanks() {
        assert_eq!(normalize_stdout("\n\na\n\nb\n"), "\n\na\n\nb");
    }

    #[test]
    fn empty_input_is_empty() {
        assert_eq!(normalize_stdout(""), "");
    }

    #[test]
    fn only_whitespace_becomes_empty() {
        assert_eq!(normalize_stdout("   \n\t\n\n"), "");
    }
}

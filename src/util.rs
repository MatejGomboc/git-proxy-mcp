//! Cross-cutting utility helpers.
//!
//! Small functions that don't belong to any specific module but are
//! shared across multiple ones. Currently:
//!
//! - [`sanitize_for_log`] — escape control characters and cap length
//!   on a string before logging it. Used for any value that crosses a
//!   trust boundary (client-controlled JSON-RPC fields, subprocess
//!   stderr, etc.) so a hostile or buggy producer can't disrupt the
//!   operator's terminal log reader.

/// Maximum length (in bytes) for a sanitised string before truncation.
///
/// 200 bytes is enough to be useful for debugging and small enough that
/// a hostile or buggy producer can't flood the log file with a huge
/// single value.
pub const MAX_LOG_STRING_LEN: usize = 200;

/// Sanitises a string for safe logging.
///
/// Two protections, both targeting buggy or hostile producers (e.g. a
/// client sending `clientInfo.name`, or a subprocess emitting stderr):
///
/// - **Control characters** (newlines, ANSI escape sequences, NULs,
///   etc.) are escaped via `char::escape_debug`, so a value like
///   `"foo\x1b[31mEVIL\x1b[0m"` renders as the literal characters
///   rather than disrupting the operator's terminal log reader. Each
///   `char::escape_debug` output is ASCII-printable.
/// - **Length** is capped at [`MAX_LOG_STRING_LEN`] bytes with a `…`
///   truncation marker, at a UTF-8 char boundary so we never panic
///   mid-codepoint.
#[must_use]
pub fn sanitize_for_log(s: &str) -> String {
    let mut escaped: String = s.chars().flat_map(char::escape_debug).collect();
    if escaped.len() > MAX_LOG_STRING_LEN {
        // Truncate at a char boundary so we don't slice mid-codepoint.
        let mut cut = MAX_LOG_STRING_LEN;
        while !escaped.is_char_boundary(cut) {
            cut -= 1;
        }
        escaped.truncate(cut);
        escaped.push('…');
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_clean_strings_unchanged() {
        assert_eq!(sanitize_for_log("Claude AI"), "Claude AI");
        assert_eq!(sanitize_for_log(""), "");
        assert_eq!(sanitize_for_log("v1.2.3-rc.1"), "v1.2.3-rc.1");
    }

    #[test]
    fn escapes_ansi_escape_sequences() {
        // Raw ESC must not pass through, or it could repaint the
        // operator's terminal (and fake "harmless" log lines around
        // an injected message).
        let evil = "foo\x1b[31mEVIL\x1b[0m";
        let sanitised = sanitize_for_log(evil);
        assert!(!sanitised.contains('\x1b'), "raw ESC must be escaped");
        // `char::escape_debug` renders `\x1b` as `\u{1b}`.
        assert!(sanitised.contains("\\u{1b}"));
    }

    #[test]
    fn escapes_newline_tab_and_carriage_return() {
        // Newlines are the highest-value escape — without them, a
        // hostile producer can fake log-line boundaries.
        assert_eq!(sanitize_for_log("a\nb"), "a\\nb");
        assert_eq!(sanitize_for_log("a\tb\rc"), "a\\tb\\rc");
    }

    #[test]
    fn escapes_nul_using_short_form() {
        // `escape_debug` emits the short `\0` form for NUL rather than
        // the verbose `\u{0}`.
        assert_eq!(sanitize_for_log("a\0b"), "a\\0b");
    }

    #[test]
    fn caps_length_to_prevent_log_flood() {
        // 1 KiB of `a` — must be truncated.
        let huge = "a".repeat(1024);
        let sanitised = sanitize_for_log(&huge);
        assert!(sanitised.len() <= MAX_LOG_STRING_LEN + "…".len());
        assert!(
            sanitised.ends_with('…'),
            "truncation marker must be appended"
        );
    }

    #[test]
    fn truncates_at_char_boundary_for_multibyte_input() {
        // 2-byte chars: `é` is 2 bytes in UTF-8, and 200 (= `MAX_LOG_STRING_LEN`)
        // is divisible by 2, so byte 200 lands cleanly on a char boundary
        // and the boundary-search loop body doesn't actually fire here.
        // This test is the no-panic invariant; the loop-body coverage
        // test below uses a 3-byte char that forces the back-up.
        let s = "é".repeat(150); // 300 bytes
        let sanitised = sanitize_for_log(&s);
        assert!(sanitised.ends_with('…'));
        let prefix = &sanitised[..sanitised.len() - "…".len()];
        assert!(prefix.is_char_boundary(prefix.len()));
    }

    #[test]
    fn boundary_search_loop_backs_up_when_cut_lands_mid_codepoint() {
        // Coverage gap fix: the boundary-search `while` loop body
        // (`cut -= 1`) only fires when the initial cut at byte
        // `MAX_LOG_STRING_LEN` is NOT on a char boundary. The "a" and
        // "é" tests don't exercise this — both line up cleanly on byte
        // 200. We need a codepoint whose byte-width doesn't divide 200.
        //
        // `€` (Euro sign) is 3 bytes in UTF-8. With `"€".repeat(70)`
        // = 210 bytes, byte 200 falls mid-codepoint:
        //   - 66 × 3 = 198 (start of the 67th `€`)
        //   - bytes 199, 200 are continuation bytes of that char
        //   - 67 × 3 = 201 (start of the 68th `€`)
        // The loop must back up: cut=200 → not boundary → cut=199 →
        // not boundary → cut=198 → boundary → truncate at 198.
        // Without the back-up, `String::truncate(200)` would panic
        // with "byte index 200 is not a char boundary".
        let s = "€".repeat(70); // 210 bytes
        let sanitised = sanitize_for_log(&s);
        assert!(sanitised.ends_with('…'));
        // The truncated prefix must be 198 bytes (66 full `€` chars).
        let prefix_len = sanitised.len() - "…".len();
        assert_eq!(prefix_len, 198, "expected back-up to byte 198");
        // And it must be valid UTF-8 (we'd have panicked otherwise,
        // but assert explicitly for documentation).
        assert!(sanitised[..prefix_len].is_char_boundary(prefix_len));
    }

    #[test]
    fn preserves_unicode_in_well_formed_short_strings() {
        // A short string with non-ASCII characters should pass through
        // (sans escaping for non-printable chars). Note that
        // `char::escape_debug` does NOT escape printable Unicode.
        assert_eq!(sanitize_for_log("héllo wörld"), "héllo wörld");
        assert_eq!(sanitize_for_log("日本語"), "日本語");
    }
}

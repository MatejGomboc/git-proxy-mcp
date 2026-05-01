//! Property-based tests for parsers and validators.
//!
//! These tests use [`proptest`] to generate random inputs and verify
//! invariants hold across the entire input space — not just the cases
//! we thought to write down. They are particularly valuable for:
//!
//! - URL sanitisation (must never leak credentials regardless of input shape)
//! - URL validation (rejects/accepts must be consistent)
//! - LFS pointer detection and parsing (must not panic on adversarial bytes,
//!   must round-trip well-formed inputs)

use git_proxy_mcp::git2_ops::auth::{sanitize_url_for_logging, validate_url};
use git_proxy_mcp::git2_ops::lfs::{is_lfs_pointer, parse_lfs_pointer};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// URL sanitisation
// ---------------------------------------------------------------------------

proptest! {
    /// Sanitisation must never panic regardless of input.
    #[test]
    fn sanitise_url_does_not_panic(url in ".*") {
        let _ = sanitize_url_for_logging(&url);
    }

    /// Sanitisation must never produce a longer string than the input plus a
    /// fixed suffix overhead (`***@` is 4 chars, prepended to the host part).
    #[test]
    fn sanitise_url_bounded_growth(url in ".*") {
        let out = sanitize_url_for_logging(&url);
        // Worst case: `https://***@host` from `https://user:pass@host` —
        // the substitution can grow the URL by at most 4 chars (the literal
        // `***@`). For any input, output length stays within input + 8.
        prop_assert!(out.len() <= url.len() + 8);
    }

    /// If the input contains an `@` after a scheme, the output must not
    /// contain the original userinfo segment (`user:pass`) verbatim.
    /// We test this with deliberately-crafted credential-shaped inputs.
    #[test]
    fn sanitise_url_strips_inline_credentials(
        scheme in "(https|http|ssh)",
        user in "[a-zA-Z0-9]{1,16}",
        pass in "[a-zA-Z0-9]{8,32}",
        host in "[a-z0-9]{1,16}\\.(com|org|io)",
        path in "/[a-z0-9_/-]{1,32}\\.git",
    ) {
        let url = format!("{scheme}://{user}:{pass}@{host}{path}");
        let out = sanitize_url_for_logging(&url);
        prop_assert!(!out.contains(&pass), "password leaked in {out}");
        prop_assert!(out.contains("***@"), "no redaction marker in {out}");
        prop_assert!(out.contains(&host), "host missing from {out}");
    }
}

// ---------------------------------------------------------------------------
// URL validation
// ---------------------------------------------------------------------------

proptest! {
    /// Validation must never panic.
    #[test]
    fn validate_url_does_not_panic(url in ".*") {
        let _ = validate_url(&url);
    }

    /// Any URL with `file://` (case-insensitive) must be rejected.
    #[test]
    fn validate_url_rejects_file_scheme(suffix in "[a-zA-Z0-9/.-]{0,32}") {
        let url = format!("file://{suffix}");
        prop_assert!(validate_url(&url).is_err());
    }

    /// Any URL with `ext::` (case-insensitive) must be rejected.
    #[test]
    fn validate_url_rejects_ext_scheme(suffix in "[a-zA-Z0-9 ]{0,32}") {
        let url = format!("ext::{suffix}");
        prop_assert!(validate_url(&url).is_err());
    }

    /// Inputs without `://` and without a `git@` prefix must be rejected.
    /// The regex [a-zA-Z0-9/.-]+ cannot produce `:` or `@`, so by
    /// construction any generated input has no scheme and validation
    /// must fail.
    #[test]
    fn validate_url_rejects_no_scheme(s in "[a-zA-Z0-9/.-]+") {
        prop_assert!(validate_url(&s).is_err());
    }
}

// ---------------------------------------------------------------------------
// LFS pointer detection
// ---------------------------------------------------------------------------

proptest! {
    /// `is_lfs_pointer` must not panic on any byte sequence.
    #[test]
    fn is_lfs_pointer_does_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = is_lfs_pointer(&bytes);
    }

    /// `parse_lfs_pointer` must not panic on any byte sequence.
    #[test]
    fn parse_lfs_pointer_does_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let _ = parse_lfs_pointer(&bytes);
    }

    /// Any input that doesn't start with the LFS version line is not a
    /// pointer (the quick-check should reject it).
    #[test]
    fn is_lfs_pointer_rejects_inputs_not_starting_with_version(
        prefix in "[a-zA-Z]{1,16}",
        rest in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        // Ensure the prefix doesn't accidentally match the LFS version line.
        if prefix.starts_with("version") {
            return Ok(());
        }
        let mut content = prefix.into_bytes();
        content.extend(&rest);
        prop_assert!(!is_lfs_pointer(&content));
    }

    /// A well-formed pointer with valid OID and size must parse successfully.
    /// Cover the full u64 range — `format!("size {N}")` and the `parse::<u64>()`
    /// round-trip work correctly for all values up to `u64::MAX`.
    #[test]
    fn parse_lfs_pointer_accepts_well_formed_input(
        oid in "[0-9a-f]{64}",
        size in any::<u64>(),
    ) {
        let content = format!(
            "version https://git-lfs.github.com/spec/v1\n\
             oid sha256:{oid}\n\
             size {size}\n"
        );
        let parsed = parse_lfs_pointer(content.as_bytes());
        prop_assert!(parsed.is_some());
        let p = parsed.unwrap();
        prop_assert_eq!(p.oid, oid);
        prop_assert_eq!(p.size, size);
    }
}

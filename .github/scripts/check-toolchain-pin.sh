#!/usr/bin/env bash
# Verifies that every `dtolnay/rust-toolchain@<sha> # vX.Y.Z` pin in
# .github/workflows/ uses the same version comment, and that the version
# matches the channel field in rust-toolchain.toml. Exits non-zero on mismatch.
#
# Run from the repository root.

set -euo pipefail

toml_channel=$(awk -F'"' '/^[[:space:]]*channel[[:space:]]*=/ {print $2; exit}' rust-toolchain.toml)

if [[ -z "$toml_channel" ]]; then
    echo "ERROR: could not extract channel from rust-toolchain.toml" >&2
    exit 1
fi

mapfile -t pin_versions < <(
    grep -h 'uses:[[:space:]]*dtolnay/rust-toolchain@' .github/workflows/*.yml \
        | sed -nE 's|.*#[[:space:]]+v?([0-9]+\.[0-9]+\.[0-9]+).*|\1|p' \
        | sort -u
)

if [[ ${#pin_versions[@]} -eq 0 ]]; then
    echo "ERROR: no dtolnay/rust-toolchain pins found in .github/workflows/" >&2
    exit 1
fi

if [[ ${#pin_versions[@]} -ne 1 ]]; then
    echo "ERROR: dtolnay/rust-toolchain pins disagree across workflows: ${pin_versions[*]}" >&2
    exit 1
fi

pin_version=${pin_versions[0]}

if [[ "$pin_version" != "$toml_channel" ]]; then
    echo "ERROR: rust-toolchain.toml channel ($toml_channel) does not match action pin (v$pin_version)" >&2
    echo "Bump both together when updating Rust." >&2
    exit 1
fi

echo "OK: Rust pinned to $pin_version (rust-toolchain.toml + dtolnay/rust-toolchain action)"

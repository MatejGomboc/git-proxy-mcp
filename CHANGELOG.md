# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Configurable output size limits to prevent protocol buffer overflow
    - New `limits.max_output_bytes` config option (default: 10 MiB)
    - Output truncation with warning when limits are exceeded
    - Truncation occurs at UTF-8 character boundaries to avoid invalid sequences

## Pre-release

This project is in early development. No releases yet.

See `TODO.md` for development progress.

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

### Added

- Configurable request timeout for git command execution (prevents hung processes)
    - New `timeouts.request_timeout_secs` configuration option
    - Default: 300 seconds (5 minutes)
    - Commands exceeding the timeout are terminated with a clear error message

---

## Pre-release

This project is in early development. No releases yet.

See `TODO.md` for development progress.

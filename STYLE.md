# Style Guide

Code style conventions for git-proxy-mcp.

---

## General Rules

| Rule | Setting |
|------|--------|
| Indentation | 4 spaces (no tabs) |
| Max line length | 170 characters |
| Charset | UTF-8 |
| Final newline | Always |
| Trailing whitespace | Trim (except Markdown) |

These rules are enforced by `.editorconfig`. Install the EditorConfig plugin for your editor:

- **VS Code:** [EditorConfig for VS Code](https://marketplace.visualstudio.com/items?itemName=EditorConfig.EditorConfig)

VS Code also displays a ruler at 170 characters (configured in `.vscode/settings.json`).

---

## Rust

### Formatting

Use `rustfmt` with default settings. CI enforces this.

```bash
cargo fmt --all        # Format all code
cargo fmt --all --check # Check without modifying
```

### Linting

Use `clippy` with warnings as errors. CI enforces this.

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

### Naming Conventions

| Item | Convention | Example |
|------|------------|--------|
| Crates | snake_case | `git_proxy_mcp` |
| Modules | snake_case | `credential_store` |
| Types | PascalCase | `GitCredential` |
| Functions | snake_case | `load_config` |
| Constants | SCREAMING_SNAKE_CASE | `MAX_RETRIES` |
| Variables | snake_case | `repo_path` |

### Documentation

- All public items must have doc comments (`///`)
- Use British spelling in documentation 🇬🇧
- CI checks documentation builds without warnings

---

## YAML (GitHub Actions)

### Indentation

**4 spaces** — aligned with project-wide convention.

```yaml
jobs:
    build:
        name: Build
        runs-on: ubuntu-latest

        steps:
            - name: Checkout
              uses: actions/checkout@v4

            - name: Build
              run: cargo build
```

### Structure

- Blank line between top-level keys (`on`, `env`, `jobs`)
- Blank line between jobs
- Blank line before `steps:` in complex jobs
- Comments on their own line, not inline

---

## JSON

### Indentation

**4 spaces**.

```json
{
    "key": "value",
    "nested": {
        "item": 123
    }
}
```

---

## Markdown

### Headings

Use ATX-style headings with blank lines before and after:

```markdown
## Section Title

Content here.
```

### Lists

Use `-` for unordered lists, `1.` for ordered lists.

### Code Blocks

Always specify the language:

````markdown
```rust
fn main() {
    println!("Hello!");
}
```
````

### Trailing Whitespace

Markdown files are exempt from trailing whitespace trimming (needed for line breaks).

---

## Commit Messages

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>: <description>

[optional body]

[optional footer]
```

### Types

| Type | Description |
|------|-------------|
| `feat` | New feature |
| `fix` | Bug fix |
| `docs` | Documentation only |
| `style` | Code style (formatting, no logic change) |
| `refactor` | Code change (no feature or fix) |
| `perf` | Performance improvement |
| `test` | Adding/updating tests |
| `chore` | Maintenance (deps, CI, etc.) |

### Examples

```
feat: add clone progress streaming
fix: prevent credential leak in error messages
docs: update README with installation instructions
style: fix YAML indentation to 4 spaces
chore: update dependencies
```

---

## British Spelling 🇬🇧

Use British spelling in all documentation and user-facing text.

| ❌ American | ✅ British |
|-------------|------------|
| color | colour |
| behavior | behaviour |
| organization | organisation |
| center | centre |
| license (noun) | licence |
| analyze | analyse |
| initialize | initialise |
| customize | customise |
| serialized | serialised |

**Note:** Code identifiers may use American spelling where it matches Rust/library conventions (e.g., `Color` if a library uses it).

---

*Last updated: 2025-12-28*

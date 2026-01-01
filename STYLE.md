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

## Single Source of Truth

Avoid duplicating information across files. Each piece of information should have one canonical location.

| Information | Canonical Source |
|-------------|------------------|
| Build commands | `CONTRIBUTING.md` § Development Setup |
| Coding standards | `CONTRIBUTING.md` § Coding Standards |
| Commit conventions | `CONTRIBUTING.md` § Commit Messages |
| British spelling | `CONTRIBUTING.md` § British Spelling |
| PR requirements | `CONTRIBUTING.md` § Pull Requests |
| Security policy | `SECURITY.md` |
| Development roadmap | `TODO.md` |
| Formatting rules | `.editorconfig` |

**Guidelines:**

- Reference the canonical source instead of duplicating content
- If information must appear in multiple places (e.g., PR template checklists), keep it minimal
- When updating information, update the canonical source first
- Cross-reference using `filename` § Section Name format

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

### Multi-line Scripts (`run: |`)

Use **4 spaces from the `-` column** for shell script content inside `run: |` blocks. This provides clear visual separation between YAML structure and shell commands.

```yaml
            - name: Example step
              shell: bash
              run: |
                if [[ -n "$VAR" ]]; then
                    echo "Variable is set"
                else
                    echo "Variable is not set"
                fi
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

See `CONTRIBUTING.md` § Commit Messages for conventions and allowed types.

---

## British Spelling 🇬🇧

See `CONTRIBUTING.md` § British Spelling for the full reference table.

**Quick rule:** Use British spelling in documentation (colour, behaviour, organisation). Code identifiers may use American spelling where it matches Rust/library conventions.

---

*Last updated: 2025-12-31*

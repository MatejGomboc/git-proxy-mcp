# Security Policy

> **Scope of this document:** vulnerability reporting, supported versions, and what counts as a security issue.
> For the *technical* design — threat model, architecture-level controls, and how each principle is enforced in the
> code — see [`docs/SECURITY.md`](docs/SECURITY.md).

## Our Commitment

**git-proxy-mcp** is a security-focused project designed to let AI assistants work with private
Git repositories while keeping your credentials safe on your machine. We take security vulnerabilities extremely seriously.

**Key security property:** The MCP server does NOT store credentials. It uses the git2 library
with credential callbacks that delegate to your existing Git configuration (credential helpers, SSH agent).

## Supported Versions

| Version | Supported                   |
| ------- | --------------------------- |
| 1.x.x   | ✅ Yes (current)            |
| 0.x.x   | ❌ No (pre-v1, unsupported) |

Security updates are maintained for the current major version. Once we reach v2.0, we will also maintain
updates for the most recent v1.x release.

## Reporting a Vulnerability

**Please do NOT report security vulnerabilities through public GitHub issues.**

### How to Report

1. **Preferred:** Use [GitHub Security Advisories](https://github.com/MatejGomboc/git-proxy-mcp/security/advisories/new) to report vulnerabilities privately.

2. **Alternative:** Email the repository owner directly at <matejg03@gmail.com>.

### What to Include

When reporting a vulnerability, please include:

- A clear description of the vulnerability
- Steps to reproduce the issue
- Potential impact assessment
- Any suggested fixes (optional but appreciated)

### What Qualifies as a Security Issue

Given our focus on credential security, we consider these especially critical:

| Severity | Examples |
|----------|----------|
| **Critical** | Credential leakage in logs, errors, or MCP responses |
| **Critical** | Authentication bypass or credential exposure |
| **High** | Unauthorised access to repositories |
| **High** | Path traversal or arbitrary file access |
| **Medium** | Denial of service vulnerabilities |
| **Medium** | Information disclosure (non-credential) |
| **Low** | Issues requiring local access or unlikely scenarios |

### Response Timeline

| Action | Timeframe |
|--------|----------|
| Initial acknowledgement | Within 48 hours |
| Preliminary assessment | Within 1 week |
| Fix development | Depends on severity and complexity |
| Security advisory publication | After fix is available |

### What to Expect

1. **Acknowledgement:** We will acknowledge receipt of your report within 48 hours.

2. **Communication:** We will keep you informed of our progress and may ask for additional information.

3. **Credit:** Unless you prefer to remain anonymous, we will credit you in our security advisory and release notes.

4. **Disclosure:** We follow responsible disclosure practices. We ask that you give us reasonable time to address the issue before any public disclosure.

## Security Best Practices for Users

### Git Configuration

The MCP server uses your existing Git setup, so your credentials must be stored
securely — with an OS credential helper for HTTPS (PATs) or ssh-agent for SSH.
The README's [Git authentication](README.md#git-authentication) section has the
exact per-platform setup commands.

### Credential Recommendations

- **Use fine-grained tokens:** Create tokens with minimal required permissions
- **Rotate regularly:** Rotate your Personal Access Tokens periodically
- **Use SSH keys:** Where possible, prefer SSH keys over PATs
- **Enable 2FA:** Always enable two-factor authentication on your Git hosting accounts
- **Use credential helpers:** Never store tokens in plain text files

### MCP Server Configuration

The configuration file contains security settings, logging, timeouts, limits, and rate limits — never credentials.

**Config file location:**

- **Linux/macOS:** `~/.git-proxy-mcp/config.json`
- **Windows:** `%USERPROFILE%\.git-proxy-mcp\config.json`

See `config/example-config.json` for the full structure.

### Audit Logging

When enabled, audit logs record all git operations. Configure in your `config.json`:

```json
{
    "logging": {
        "level": "info",
        "audit_log_path": "/path/to/audit.log"
    }
}
```

Review these logs periodically for unexpected activity.

## Security Design Principles

This project follows these security principles:

1. **No credential storage:** The MCP server never stores credentials — it uses git's native credential system
2. **Credential isolation:** Credentials never leave the user's machine and are never included in MCP responses
3. **URL sanitisation:** All URLs are sanitised before logging to remove embedded credentials
4. **Defence in depth:** Multiple layers of protection (security guards, repo filters, audit logging)
5. **Secure defaults:** Force push disabled, protected branches enforced by default
6. **Transparency:** Open source code for community review
7. **Industry standard:** Uses the same credential approach as VS Code, TortoiseGit, and other Git tools

## Acknowledgements

We thank the security researchers and community members who help keep this project secure.

---

*This security policy was last updated on 2026-01-10.*

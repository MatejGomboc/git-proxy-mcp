# Security Policy

## Our Commitment

**git-proxy-mcp** is a security-focused project designed to let AI assistants work with private
Git repositories while keeping your credentials safe on your machine. We take security vulnerabilities extremely seriously.

**Key security property:** The MCP server does NOT store credentials. It spawns git as a subprocess
and relies on your existing Git configuration (credential helpers, SSH agent).

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.x.x   | :white_check_mark: (development) |

Once we reach v1.0, we will maintain security updates for the current major version and one previous major version.

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

The MCP server uses your existing Git setup. Ensure your credentials are stored securely:

**For HTTPS (PATs):**

```bash
# macOS - use Keychain
git config --global credential.helper osxkeychain

# Windows - use Credential Manager
git config --global credential.helper manager

# Linux - use libsecret or cache
git config --global credential.helper libsecret
```

**For SSH:**

```bash
# Add key to ssh-agent (prompted for passphrase once)
ssh-add ~/.ssh/id_ed25519

# macOS: store passphrase in Keychain
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
```

### Credential Recommendations

- **Use fine-grained tokens:** Create tokens with minimal required permissions
- **Rotate regularly:** Rotate your Personal Access Tokens periodically
- **Use SSH keys:** Where possible, prefer SSH keys over PATs
- **Enable 2FA:** Always enable two-factor authentication on your Git hosting accounts
- **Use credential helpers:** Never store tokens in plain text files

### MCP Server Configuration

The `config.json` file only contains security settings (protected branches, repo filters) — no credentials.
It can safely be committed to version control if desired.

### Audit Logging

When enabled, audit logs record all git operations. Configure via:

```json
{
    "logging": {
        "audit_log_path": "/path/to/audit.log"
    }
}
```

Review these logs periodically for unexpected activity.

## Security Design Principles

This project follows these security principles:

1. **No credential storage:** The MCP server never stores credentials — it uses git's native credential system
2. **Credential isolation:** Credentials never leave the user's machine and are never included in MCP responses
3. **Output sanitisation:** All git output is sanitised to remove accidentally leaked credentials
4. **Defence in depth:** Multiple layers of protection (command validation, security guards, audit logging)
5. **Secure defaults:** Force push disabled, protected branches enforced by default
6. **Transparency:** Open source code for community review
7. **Industry standard:** Uses the same credential approach as VS Code, TortoiseGit, and other Git tools

## Acknowledgements

We thank the security researchers and community members who help keep this project secure.

---

*This security policy was last updated on 2026-01-01.*

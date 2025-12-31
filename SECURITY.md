# Security Policy

## Our Commitment

**git-proxy-mcp** is a security-focused project designed to keep your Git credentials safe
while enabling AI assistants to work with private repositories. We take security vulnerabilities extremely seriously.

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.x.x   | :white_check_mark: (development) |

Once we reach v1.0, we will maintain security updates for the current major version and one previous major version.

## Reporting a Vulnerability

**Please do NOT report security vulnerabilities through public GitHub issues.**

### How to Report

1. **Preferred:** Use [GitHub Security Advisories](https://github.com/MatejGomboc/git-proxy-mcp/security/advisories/new) to report vulnerabilities privately.

2. **Alternative:** Email the maintainer directly at the email address listed in the [CODEOWNERS](.github/CODEOWNERS) file or on the maintainer's GitHub profile.

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

### Configuration File Security

Your `config.json` contains sensitive credentials. Please ensure:

- **File permissions:** Restrict access to your config file (e.g., `chmod 600 config.json` on Unix)
- **Never commit:** Add your config file to `.gitignore`
- **Use environment-specific configs:** Don't share config files between environments

### Credential Recommendations

- **Use fine-grained tokens:** Create tokens with minimal required permissions
- **Rotate regularly:** Rotate your Personal Access Tokens periodically
- **Use SSH keys:** Where possible, prefer SSH keys over PATs
- **Enable 2FA:** Always enable two-factor authentication on your Git hosting accounts

### Audit Logging

When enabled, audit logs are written to `~/.config/git-proxy-mcp/audit.log`. Review these logs periodically for unexpected activity.

## Security Design Principles

This project follows these security principles:

1. **Credential isolation:** Credentials never leave the user's machine and are never included in MCP responses
2. **Minimal permissions:** Request only the permissions needed for Git operations
3. **Defence in depth:** Multiple layers of protection (config validation, policy enforcement, audit logging)
4. **Secure defaults:** Safe defaults for all security-related settings
5. **Transparency:** Open source code for community review

## Acknowledgements

We thank the security researchers and community members who help keep this project secure.

---

*This security policy was last updated on 2025-12-28.*

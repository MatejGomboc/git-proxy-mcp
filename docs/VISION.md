# Vision: The Three Tiers of git-proxy-mcp

This document describes the full vision for git-proxy-mcp, from the initial implementation to the ultimate goal.

## The Problem

Cloud-based AI assistants (Claude.ai, ChatGPT, Gemini) have:
- ✅ Full Linux VMs with compute capability
- ✅ Ability to run git, build code, run tests
- ❌ No access to user's credentials for private repos

## The Three Tiers

```
┌─────────────────────────────────────────────────────────────────────────┐
│  TIER 1: Memory Buffer (First Draft)                                    │
│                                                                         │
│  GitHub ──► MCP (buffer in RAM) ──► AI                                  │
│                                                                         │
│  • No files on user's DISK                                             │
│  • File bytes pass through user's RAM                                  │
│  • Simple to implement                                                 │
│  • Memory pressure for large repos                                     │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  TIER 2: Chunked Streaming (Improvement)                                │
│                                                                         │
│  GitHub ──► MCP (small chunks) ──► AI                                   │
│                                                                         │
│  • No files on disk                                                    │
│  • Constant memory usage (chunked)                                     │
│  • Handles large repos                                                 │
│  • Still passes through user's PC                                      │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│  TIER 3: Token Delegation (THE GOLDEN GOAL) ⭐                          │
│                                                                         │
│  GitHub ◄──────────── DIRECT ────────────► AI                             │
│            ▲                                                            │
│            │                                                            │
│       MCP provides short-lived token                                   │
│                                                                         │
│  • ZERO bytes through user's PC                                        │
│  • AI connects directly to GitHub                                      │
│  • MCP only brokers authentication                                     │
│  • Ultimate security and performance                                   │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## Tier 3: The Golden Architecture

### How It Works

```
┌─────────────────────────────────────────────────────────────────────────┐
│  TOKEN DELEGATION FLOW                                                 │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  1. ONE-TIME SETUP (User does once)                                    │
│     ──────────────────────────                                          │
│     User installs "git-proxy-mcp" GitHub App on their repos            │
│     App gets permission to: read code, write code, create branches     │
│     MCP server stores App's private key (NOT user's PAT)               │
│                                                                         │
│  2. AI REQUESTS ACCESS                                                 │
│     ─────────────────────                                               │
│     AI's VM                        User's PC                           │
│        │                              │                                │
│        │  MCP: repo/get_token         │                                │
│        │  { url: "github.com/x/y" }   │                                │
│        ├─────────────────────────────►│                                │
│        │                              │                                │
│                                                                         │
│  3. MCP GENERATES TOKEN                                                │
│     ──────────────────────                                              │
│                              User's PC                    GitHub       │
│                                 │                            │         │
│                                 │  App auth + request        │         │
│                                 │  installation token        │         │
│                                 ├───────────────────────────►│         │
│                                 │                            │         │
│                                 │  Token: ghs_xxxx           │         │
│                                 │  (1 hour, repo-scoped)     │         │
│                                 │◄───────────────────────────┤         │
│                                 │                            │         │
│                                                                         │
│  4. TOKEN SENT TO AI                                                   │
│     ───────────────────                                                  │
│     AI's VM                        User's PC                           │
│        │                              │                                │
│        │  { token: "ghs_xxxx",        │                                │
│        │    expires: "1 hour",        │                                │
│        │    clone_url: "https://..." }│                                │
│        │◄─────────────────────────────┤                                │
│        │                              │                                │
│                                                                         │
│  5. AI CLONES DIRECTLY (!!!)                                           │
│     ─────────────────────────                                            │
│     AI's VM                                               GitHub       │
│        │                                                     │         │
│        │  git clone https://x-access-token:ghs_xxx@github... │         │
│        ├────────────────────────────────────────────────────►│         │
│        │                                                     │         │
│        │  Full repository contents                           │         │
│        │◄────────────────────────────────────────────────────┤         │
│        │                                                     │         │
│        │         ZERO BYTES through user's PC!               │         │
│        │                                                     │         │
└─────────────────────────────────────────────────────────────────────────┘
```

### Why Token Delegation Is Safe

| Concern | Answer |
|---------|--------|
| "You're giving AI a credential!" | Yes, but it's ephemeral, scoped, and revocable |
| "What if AI's VM is compromised?" | Attacker gets 1-hour token for one repo, not user's PAT |
| "What about SSH keys?" | Can't delegate SSH signing; HTTPS tokens only |
| "Audit trail?" | GitHub App has complete audit log |

### Token Properties

| Property | Value |
|----------|-------|
| Lifetime | 1 hour (configurable) |
| Scope | Single repository |
| Permissions | Configured when App installed |
| Revocable | Instantly via GitHub UI |
| Renewable | AI can request new token before expiry |

### Comparison: User's PAT vs App Token

| Aspect | User's PAT | App Installation Token |
|--------|-----------|----------------------|
| Lifetime | Months/years | 1 hour |
| Scope | All repos user can access | Single repo |
| If leaked | Full account access | Limited damage |
| Revocation | User must notice & revoke | Auto-expires |
| Audit | Mixed with user's activity | Separate App audit log |

---

## Implementation Phases

### Phase 1-2: Tier 1 (Memory Buffer)

```rust
// MCP streams data through memory
pub async fn handle_clone(url: &str) -> TarArchive {
    let repo = fetch_bare(url)?;        // git2 fetch
    let tar = stream_tree_to_tar(&repo); // In-memory
    tar
}
```

**Pros:** Simple, works today  
**Cons:** Data passes through user's PC

### Phase 3: Tier 2 (Chunked Streaming)

```rust
// Stream in chunks to handle large repos
pub async fn handle_clone(url: &str) -> impl Stream<Item = Chunk> {
    let repo = fetch_bare(url)?;
    stream_tree_chunked(&repo, CHUNK_SIZE)
}
```

**Pros:** Handles large repos, constant memory  
**Cons:** Still passes through user's PC

### Phase 4+: Tier 3 (Token Delegation) ⭐

```rust
// Generate token, AI clones directly
pub async fn handle_get_token(url: &str) -> TokenResponse {
    let app = GitHubApp::load()?;
    let installation = app.find_installation(url)?;
    let token = installation.create_access_token(
        repos: [url],
        permissions: { contents: "write" },
        expires: Duration::hours(1),
    )?;
    
    TokenResponse {
        token: token.value,
        expires_at: token.expires_at,
        clone_url: format!("https://x-access-token:{token}@github.com/..."),
    }
}
```

**Pros:** Zero data through user's PC!  
**Cons:** Requires GitHub App setup

---

## MCP Tools by Tier

### Tier 1-2 Tools

| Tool | Description |
|------|-------------|
| `repo/clone` | Stream repo contents as tar.gz |
| `repo/push` | Receive bundle, push with auth |
| `repo/pull` | Stream delta of changes |

### Tier 3 Tools (Additional)

| Tool | Description |
|------|-------------|
| `auth/get_token` | Generate short-lived token for repo |
| `auth/refresh_token` | Refresh token before expiry |
| `auth/revoke_token` | Revoke token early |
| `auth/status` | Check App installation status |

---

## Provider Support for Tier 3

| Provider | Token Mechanism | Status |
|----------|-----------------|--------|
| **GitHub** | GitHub App installation tokens | Primary target |
| **GitLab** | Project/Group Access Tokens | Planned |
| **Bitbucket** | Repository Access Tokens | If needed |
| **Azure DevOps** | PAT delegation | Research needed |

### GitHub App Setup (One-Time)

1. User goes to: `https://github.com/apps/git-proxy-mcp`
2. Clicks "Install"
3. Selects repositories to grant access
4. Done!

The MCP server (published by us) is the App. User just installs it.

---

## Security Model by Tier

| Aspect | Tier 1-2 | Tier 3 |
|--------|----------|--------|
| User's PAT/SSH key | Used by MCP | NOT used |
| Data through user's PC | Yes (memory) | No |
| Token lifetime | N/A | 1 hour |
| Token scope | N/A | Single repo |
| Credential storage | Never | App key only |
| If MCP compromised | User's creds at risk | Only App key |
| If AI compromised | N/A | 1-hour token only |

---

## The Ultimate Vision

```
┌─────────────────────────────────────────────────────────────────────────┐
│                                                                         │
│    User's PC is ONLY an authentication broker.                         │
│                                                                         │
│    It never sees the code.                                              │
│    It never stores the code.                                            │
│    It never transmits the code.                                         │
│                                                                         │
│    The code flows directly: GitHub ↔ AI                                │
│                                                                         │
│    The PC just says: "Yes, this AI is allowed to access this repo."    │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

This is the golden goal. Tier 3. True authentication brokering.

---

## Roadmap

| Phase | Tier | Focus |
|-------|------|-------|
| 1-2 | Tier 1 | Bare repo + memory streaming (first working version) |
| 3 | Tier 2 | Chunked streaming (large repo support) |
| 4 | Tier 3 | GitHub App + token delegation (the goal!) |
| 5 | Tier 3 | GitLab, Bitbucket support |

---

*This is where we're going. Tier 1 gets us working. Tier 3 gets us perfect.*

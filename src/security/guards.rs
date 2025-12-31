//! Security guards for Git operations.
//!
//! This module provides security controls that can block operations:
//!
//! - **Branch guards**: Prevent operations on protected branches
//! - **Push guards**: Block force pushes
//! - **Repository filters**: Allow/block specific repositories

use std::collections::HashSet;

/// Result of a security check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityCheckResult {
    /// Operation is allowed.
    Allowed,

    /// Operation is blocked.
    Blocked {
        /// Reason for blocking.
        reason: String,
    },
}

impl SecurityCheckResult {
    /// Returns `true` if the operation is allowed.
    #[must_use]
    pub const fn is_allowed(&self) -> bool {
        matches!(self, Self::Allowed)
    }

    /// Returns `true` if the operation is blocked.
    #[must_use]
    pub const fn is_blocked(&self) -> bool {
        matches!(self, Self::Blocked { .. })
    }

    /// Returns the blocking reason, if blocked.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Allowed => None,
            Self::Blocked { reason } => Some(reason),
        }
    }
}

/// A security guard that can check operations.
pub trait SecurityGuard {
    /// Checks if a command should be allowed.
    ///
    /// # Arguments
    ///
    /// * `command` — The Git subcommand (e.g., "push", "checkout")
    /// * `args` — Command arguments
    ///
    /// # Returns
    ///
    /// `SecurityCheckResult::Allowed` if the operation should proceed,
    /// `SecurityCheckResult::Blocked` with a reason otherwise.
    fn check(&self, command: &str, args: &[String]) -> SecurityCheckResult;
}

/// Guard that protects specific branches from modifications.
#[derive(Debug, Clone)]
pub struct BranchGuard {
    /// Set of protected branch names.
    protected_branches: HashSet<String>,
}

impl BranchGuard {
    /// Creates a new branch guard with the given protected branches.
    #[must_use]
    pub fn new(protected_branches: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            protected_branches: protected_branches.into_iter().map(Into::into).collect(),
        }
    }

    /// Creates a branch guard with common default protections.
    ///
    /// Protected by default: `main`, `master`, `develop`, `release/*`
    #[must_use]
    pub fn with_defaults() -> Self {
        Self::new(["main", "master", "develop"])
    }

    /// Adds a branch to the protected set.
    pub fn protect(&mut self, branch: impl Into<String>) {
        self.protected_branches.insert(branch.into());
    }

    /// Removes a branch from the protected set.
    pub fn unprotect(&mut self, branch: &str) {
        self.protected_branches.remove(branch);
    }

    /// Checks if a branch is protected.
    #[must_use]
    pub fn is_protected(&self, branch: &str) -> bool {
        // Direct match
        if self.protected_branches.contains(branch) {
            return true;
        }

        // Check for wildcard patterns (e.g., "release/*")
        for pattern in &self.protected_branches {
            if pattern.ends_with('*') {
                let prefix = &pattern[..pattern.len() - 1];
                if branch.starts_with(prefix) {
                    return true;
                }
            }
        }

        false
    }

    /// Extracts branch name from command arguments.
    fn extract_branch_from_args(command: &str, args: &[String]) -> Option<String> {
        match command {
            "push" => {
                // push [remote] [refspec]
                // refspec can be: branch, local:remote, +local:remote
                for arg in args {
                    if arg.starts_with('-') {
                        continue;
                    }
                    // Check if this looks like a refspec
                    if arg.contains(':') {
                        // local:remote format
                        let parts: Vec<&str> = arg.split(':').collect();
                        if parts.len() == 2 {
                            let remote_ref = parts[1].trim_start_matches('+');
                            return Some(remote_ref.to_string());
                        }
                    }
                    // else: Might be a branch name (not a URL or remote)
                    // Skip the remote name (first non-flag arg)
                }
                // Check the last non-flag argument
                args.iter().filter(|a| !a.starts_with('-')).nth(1).cloned()
            }
            "checkout" | "branch" => {
                // Get the first non-flag argument
                args.iter().find(|a| !a.starts_with('-')).cloned()
            }
            "merge" | "rebase" => {
                // Target branch is typically first non-flag arg
                args.iter().find(|a| !a.starts_with('-')).cloned()
            }
            _ => None,
        }
    }
}

impl Default for BranchGuard {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl SecurityGuard for BranchGuard {
    fn check(&self, command: &str, args: &[String]) -> SecurityCheckResult {
        // Only check commands that can modify branches
        let modifying_commands = ["push", "branch", "checkout", "merge", "rebase", "reset"];

        if !modifying_commands.contains(&command) {
            return SecurityCheckResult::Allowed;
        }

        // For branch command, check for deletion (-d, -D, --delete)
        if command == "branch" {
            let is_delete = args
                .iter()
                .any(|a| a == "-d" || a == "-D" || a == "--delete" || a.starts_with("--delete"));

            if is_delete {
                if let Some(branch) = Self::extract_branch_from_args(command, args) {
                    if self.is_protected(&branch) {
                        return SecurityCheckResult::Blocked {
                            reason: format!("Cannot delete protected branch '{branch}'"),
                        };
                    }
                }
            }
            return SecurityCheckResult::Allowed;
        }

        // For push, check the target branch
        if command == "push" {
            if let Some(branch) = Self::extract_branch_from_args(command, args) {
                // Check for force push to protected branch
                let is_force = args
                    .iter()
                    .any(|a| a == "-f" || a == "--force" || a == "--force-with-lease");

                if is_force && self.is_protected(&branch) {
                    return SecurityCheckResult::Blocked {
                        reason: format!("Cannot force push to protected branch '{branch}'"),
                    };
                }
            }
        }

        SecurityCheckResult::Allowed
    }
}

/// Guard that blocks force push operations.
#[derive(Debug, Clone)]
pub struct PushGuard {
    /// Whether force push is allowed.
    allow_force_push: bool,

    /// Branches where force push is explicitly allowed (overrides global setting).
    force_push_allowed_branches: HashSet<String>,
}

impl PushGuard {
    /// Creates a new push guard.
    ///
    /// # Arguments
    ///
    /// * `allow_force_push` — Whether force push is allowed globally
    #[must_use]
    pub fn new(allow_force_push: bool) -> Self {
        Self {
            allow_force_push,
            force_push_allowed_branches: HashSet::new(),
        }
    }

    /// Creates a push guard that blocks all force pushes.
    #[must_use]
    pub fn block_force_push() -> Self {
        Self::new(false)
    }

    /// Creates a push guard that allows force pushes.
    #[must_use]
    pub fn allow_force_push() -> Self {
        Self::new(true)
    }

    /// Allows force push to a specific branch.
    pub fn allow_force_push_to(&mut self, branch: impl Into<String>) {
        self.force_push_allowed_branches.insert(branch.into());
    }
}

impl Default for PushGuard {
    fn default() -> Self {
        Self::block_force_push()
    }
}

impl SecurityGuard for PushGuard {
    fn check(&self, command: &str, args: &[String]) -> SecurityCheckResult {
        if command != "push" {
            return SecurityCheckResult::Allowed;
        }

        // Check for force push flags
        let is_force = args.iter().any(|a| {
            a == "-f"
                || a == "--force"
                || a == "--force-with-lease"
                || a.starts_with("--force-with-lease=")
        });

        if !is_force {
            return SecurityCheckResult::Allowed;
        }

        // Force push detected
        if self.allow_force_push {
            return SecurityCheckResult::Allowed;
        }

        // Check if force push is allowed for this specific branch
        let branch = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .nth(1) // Skip remote name
            .map(String::as_str);

        if let Some(branch) = branch {
            if self.force_push_allowed_branches.contains(branch) {
                return SecurityCheckResult::Allowed;
            }
        }

        SecurityCheckResult::Blocked {
            reason: "Force push is not allowed. Use --force-with-lease for safer updates, \
                     or contact your administrator to enable force push."
                .to_string(),
        }
    }
}

/// Filter that controls which repositories can be accessed.
#[derive(Debug, Clone)]
pub struct RepoFilter {
    /// Allowlist of repository patterns.
    allowlist: HashSet<String>,

    /// Blocklist of repository patterns.
    blocklist: HashSet<String>,

    /// Whether to use allowlist mode (only allow listed repos).
    allowlist_mode: bool,
}

impl RepoFilter {
    /// Creates a new repository filter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            allowlist: HashSet::new(),
            blocklist: HashSet::new(),
            allowlist_mode: false,
        }
    }

    /// Creates a filter that allows all repositories except those in the blocklist.
    #[must_use]
    pub fn blocklist_mode() -> Self {
        Self {
            allowlist: HashSet::new(),
            blocklist: HashSet::new(),
            allowlist_mode: false,
        }
    }

    /// Creates a filter that only allows repositories in the allowlist.
    #[must_use]
    pub fn allowlist_mode() -> Self {
        Self {
            allowlist: HashSet::new(),
            blocklist: HashSet::new(),
            allowlist_mode: true,
        }
    }

    /// Adds a repository pattern to the allowlist.
    pub fn allow(&mut self, pattern: impl Into<String>) {
        self.allowlist.insert(pattern.into());
    }

    /// Adds a repository pattern to the blocklist.
    pub fn block(&mut self, pattern: impl Into<String>) {
        self.blocklist.insert(pattern.into());
    }

    /// Checks if a repository URL is allowed.
    #[must_use]
    pub fn is_allowed(&self, repo_url: &str) -> bool {
        // Normalise the URL for comparison
        let normalised = Self::normalise_url(repo_url);

        // Check blocklist first (always applies)
        for pattern in &self.blocklist {
            if Self::matches_pattern(&normalised, pattern) {
                return false;
            }
        }

        // In allowlist mode, repo must be in allowlist
        if self.allowlist_mode {
            for pattern in &self.allowlist {
                if Self::matches_pattern(&normalised, pattern) {
                    return true;
                }
            }
            return false;
        }

        // In blocklist mode, anything not blocked is allowed
        true
    }

    /// Normalises a repository URL for comparison.
    fn normalise_url(url: &str) -> String {
        let mut normalised = url.to_lowercase();

        // Remove protocol
        if let Some(pos) = normalised.find("://") {
            normalised = normalised[pos + 3..].to_string();
        }

        // Handle SSH URLs in git@host:path format
        // Convert git@github.com:user/repo to github.com/user/repo
        if normalised.starts_with("git@") {
            normalised = normalised[4..].to_string();
            // Replace first : with / (separates host from path in SSH URLs)
            if let Some(colon_pos) = normalised.find(':') {
                normalised.replace_range(colon_pos..=colon_pos, "/");
            }
        } else if let Some(at_pos) = normalised.find('@') {
            // Remove credentials if present (user:pass@host format)
            normalised = normalised[at_pos + 1..].to_string();
        }

        // Remove .git suffix (case-insensitive)
        if normalised.len() >= 4 && normalised[normalised.len() - 4..].eq_ignore_ascii_case(".git")
        {
            normalised.truncate(normalised.len() - 4);
        }

        // Remove trailing slash
        if normalised.ends_with('/') {
            normalised.pop();
        }

        normalised
    }

    /// Checks if a URL matches a pattern.
    fn matches_pattern(url: &str, pattern: &str) -> bool {
        let normalised_pattern = Self::normalise_url(pattern);

        // Exact match
        if url == normalised_pattern {
            return true;
        }

        // Wildcard matching
        if normalised_pattern.contains('*') {
            let parts: Vec<&str> = normalised_pattern.split('*').collect();

            if parts.len() == 2 {
                let prefix = parts[0];
                let suffix = parts[1];

                return url.starts_with(prefix) && url.ends_with(suffix);
            }
        }

        // Prefix matching (e.g., "github.com/org" matches "github.com/org/repo")
        if url.starts_with(&normalised_pattern) {
            let remainder = &url[normalised_pattern.len()..];
            return remainder.is_empty() || remainder.starts_with('/');
        }

        false
    }

    /// Extracts repository URL from command arguments.
    fn extract_repo_url(command: &str, args: &[String]) -> Option<String> {
        match command {
            "clone" => {
                // clone <url> [directory]
                args.first().cloned()
            }
            "push" | "pull" | "fetch" | "ls-remote" => {
                // First non-flag argument might be the remote
                args.iter().find(|a| !a.starts_with('-')).cloned()
            }
            "remote" => {
                // remote add <name> <url>
                if args.first().map(String::as_str) == Some("add") {
                    args.get(2).cloned()
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

impl Default for RepoFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl SecurityGuard for RepoFilter {
    fn check(&self, command: &str, args: &[String]) -> SecurityCheckResult {
        // Only check commands that access remote repositories
        let remote_commands = ["clone", "push", "pull", "fetch", "ls-remote", "remote"];

        if !remote_commands.contains(&command) {
            return SecurityCheckResult::Allowed;
        }

        if let Some(repo_url) = Self::extract_repo_url(command, args) {
            // Skip checking for remote names like "origin"
            if !repo_url.contains('/') && !repo_url.contains('.') {
                return SecurityCheckResult::Allowed;
            }

            if !self.is_allowed(&repo_url) {
                return SecurityCheckResult::Blocked {
                    reason: format!("Repository '{repo_url}' is not allowed by policy"),
                };
            }
        }

        SecurityCheckResult::Allowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // BranchGuard tests

    #[test]
    fn branch_guard_protects_default_branches() {
        let guard = BranchGuard::with_defaults();

        assert!(guard.is_protected("main"));
        assert!(guard.is_protected("master"));
        assert!(guard.is_protected("develop"));
        assert!(!guard.is_protected("feature/test"));
    }

    #[test]
    fn branch_guard_wildcard_pattern() {
        let mut guard = BranchGuard::new(std::iter::empty::<String>());
        guard.protect("release/*");

        assert!(guard.is_protected("release/1.0"));
        assert!(guard.is_protected("release/2.0.0"));
        assert!(!guard.is_protected("releases/1.0"));
    }

    #[test]
    fn branch_guard_blocks_delete() {
        let guard = BranchGuard::with_defaults();

        let result = guard.check("branch", &["-d".to_string(), "main".to_string()]);
        assert!(result.is_blocked());

        let result = guard.check("branch", &["-D".to_string(), "feature".to_string()]);
        assert!(result.is_allowed());
    }

    #[test]
    fn branch_guard_blocks_force_push() {
        let guard = BranchGuard::with_defaults();

        let result = guard.check(
            "push",
            &[
                "--force".to_string(),
                "origin".to_string(),
                "main".to_string(),
            ],
        );
        assert!(result.is_blocked());
    }

    // PushGuard tests

    #[test]
    fn push_guard_blocks_force_push_by_default() {
        let guard = PushGuard::default();

        let result = guard.check("push", &["--force".to_string()]);
        assert!(result.is_blocked());

        let result = guard.check("push", &["-f".to_string()]);
        assert!(result.is_blocked());
    }

    #[test]
    fn push_guard_allows_normal_push() {
        let guard = PushGuard::default();

        let result = guard.check("push", &["origin".to_string(), "main".to_string()]);
        assert!(result.is_allowed());
    }

    #[test]
    fn push_guard_allows_force_push_when_configured() {
        let guard = PushGuard::allow_force_push();

        let result = guard.check("push", &["--force".to_string()]);
        assert!(result.is_allowed());
    }

    #[test]
    fn push_guard_allows_force_push_to_specific_branch() {
        let mut guard = PushGuard::block_force_push();
        guard.allow_force_push_to("feature-branch");

        let result = guard.check(
            "push",
            &[
                "--force".to_string(),
                "origin".to_string(),
                "feature-branch".to_string(),
            ],
        );
        assert!(result.is_allowed());

        let result = guard.check(
            "push",
            &[
                "--force".to_string(),
                "origin".to_string(),
                "main".to_string(),
            ],
        );
        assert!(result.is_blocked());
    }

    // RepoFilter tests

    #[test]
    fn repo_filter_blocklist_mode() {
        let mut filter = RepoFilter::blocklist_mode();
        filter.block("github.com/evil/repo");

        assert!(filter.is_allowed("https://github.com/good/repo.git"));
        assert!(!filter.is_allowed("https://github.com/evil/repo.git"));
    }

    #[test]
    fn repo_filter_allowlist_mode() {
        let mut filter = RepoFilter::allowlist_mode();
        filter.allow("github.com/myorg/*");

        assert!(filter.is_allowed("https://github.com/myorg/repo1.git"));
        assert!(filter.is_allowed("https://github.com/myorg/repo2.git"));
        assert!(!filter.is_allowed("https://github.com/other/repo.git"));
    }

    #[test]
    fn repo_filter_normalises_urls() {
        let mut filter = RepoFilter::blocklist_mode();
        filter.block("github.com/blocked/repo");

        // All these should be blocked
        assert!(!filter.is_allowed("https://github.com/blocked/repo.git"));
        assert!(!filter.is_allowed("http://github.com/blocked/repo"));
        assert!(!filter.is_allowed("git@github.com:blocked/repo.git"));
        assert!(!filter.is_allowed("HTTPS://GITHUB.COM/BLOCKED/REPO.GIT"));
    }

    #[test]
    fn repo_filter_wildcard_matching() {
        let mut filter = RepoFilter::blocklist_mode();
        filter.block("github.com/org/*");

        assert!(!filter.is_allowed("https://github.com/org/repo1.git"));
        assert!(!filter.is_allowed("https://github.com/org/repo2.git"));
        assert!(filter.is_allowed("https://github.com/other/repo.git"));
    }

    #[test]
    fn repo_filter_blocks_clone() {
        let mut filter = RepoFilter::blocklist_mode();
        filter.block("github.com/blocked/repo");

        let result = filter.check(
            "clone",
            &["https://github.com/blocked/repo.git".to_string()],
        );
        assert!(result.is_blocked());

        let result = filter.check(
            "clone",
            &["https://github.com/allowed/repo.git".to_string()],
        );
        assert!(result.is_allowed());
    }

    #[test]
    fn repo_filter_allows_origin() {
        let filter = RepoFilter::allowlist_mode();

        // "origin" is a remote name, not a URL - should be allowed
        let result = filter.check("push", &["origin".to_string(), "main".to_string()]);
        assert!(result.is_allowed());
    }

    // SecurityCheckResult tests

    #[test]
    fn security_check_result_methods() {
        let allowed = SecurityCheckResult::Allowed;
        assert!(allowed.is_allowed());
        assert!(!allowed.is_blocked());
        assert!(allowed.reason().is_none());

        let blocked = SecurityCheckResult::Blocked {
            reason: "test".to_string(),
        };
        assert!(!blocked.is_allowed());
        assert!(blocked.is_blocked());
        assert_eq!(blocked.reason(), Some("test"));
    }
}

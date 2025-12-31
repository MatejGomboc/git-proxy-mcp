//! git-proxy-mcp: Secure Git proxy MCP server for AI assistants
//!
//! This tool allows AI assistants to clone, pull, and push to private Git
//! repositories while keeping credentials secure on the user's machine.
//! Credentials are never transmitted through MCP responses.

use clap::Parser;

pub mod auth;
pub mod config;
pub mod error;
pub mod mcp;

/// Secure Git proxy MCP server for AI assistants.
///
/// Allows AI assistants to work with private Git repositories while keeping
/// credentials secure on your machine. Credentials are never transmitted
/// through MCP responses.
#[derive(Parser, Debug)]
#[command(name = "git-proxy-mcp")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "FILE")]
    config: Option<std::path::PathBuf>,

    /// Increase logging verbosity
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Decrease logging verbosity
    #[arg(short, long)]
    quiet: bool,
}

/// Entry point for the git-proxy-mcp server.
fn main() {
    let args = Args::parse();

    // TODO: Initialise tracing/logging based on verbosity

    println!("git-proxy-mcp v{}", env!("CARGO_PKG_VERSION"));

    // Load configuration
    let config_path = args.config.as_deref();
    match config::load_config(config_path) {
        Ok(cfg) => {
            println!("Configuration loaded successfully!");
            println!("  Remotes configured: {}", cfg.remotes.len());
            if let Some(ref identity) = cfg.ai_identity {
                println!("  AI Identity: {} <{}>", identity.name, identity.email);
            }
            println!("  Force push allowed: {}", cfg.security.allow_force_push);
            println!(
                "  Protected branches: {:?}",
                cfg.security.protected_branches
            );
            println!("  Log level: {}", cfg.logging.level);

            // Convert to credentials (demonstrates the pipeline works)
            let credentials = cfg.into_credentials();
            println!("\nCredentials loaded: {}", credentials.len());
            for cred in &credentials {
                println!("  - {} ({})", cred.name(), cred.url_pattern());
            }
        }
        Err(e) => {
            println!("Configuration error: {e}");
            if config_path.is_none() {
                if let Some(default_path) = config::default_config_path() {
                    println!("\nExpected config at: {}", default_path.display());
                    println!("Create one based on config/example-config.json");
                }
            }
        }
    }

    println!("\nMCP server not yet implemented. Coming soon!");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }
}

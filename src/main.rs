//! git-proxy-mcp: Secure Git proxy MCP server for AI assistants
//!
//! This tool allows AI assistants to clone, pull, and push to private Git
//! repositories while keeping credentials secure on the user's machine.
//! Credentials are never transmitted through MCP responses.

use anyhow::Result;
use clap::Parser;

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

fn main() -> Result<()> {
    let args = Args::parse();

    // TODO: Initialise tracing/logging based on verbosity
    // TODO: Load configuration
    // TODO: Start MCP server

    println!("git-proxy-mcp v{}", env!("CARGO_PKG_VERSION"));
    println!("Config: {:?}", args.config);
    println!("Verbose level: {}", args.verbose);
    println!("Quiet mode: {}", args.quiet);

    println!("\nMCP server not yet implemented. Coming soon!");

    Ok(())
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

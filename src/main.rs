//! git-proxy-mcp: Secure Git proxy MCP server for AI assistants
//!
//! This tool allows AI assistants to clone, pull, and push to private Git
//! repositories using the user's existing Git credential configuration.
//! The MCP server does not store credentials — it relies on credential
//! helpers and SSH agent already configured on the user's machine.

use std::process::ExitCode;

use clap::Parser;
use tracing::{error, info, Level};
use tracing_subscriber::EnvFilter;

use git_proxy_mcp::config;
use git_proxy_mcp::git::executor::GitExecutor;
use git_proxy_mcp::mcp::server::{McpServer, SecurityConfig};
use git_proxy_mcp::security::{AuditEvent, AuditLogger};

/// Secure Git proxy MCP server for AI assistants.
///
/// Allows AI assistants to work with private Git repositories using your
/// existing Git credential configuration. Credentials are managed by your
/// system's credential helpers and SSH agent, not by this server.
#[derive(Parser, Debug)]
#[command(name = "git-proxy-mcp")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, value_name = "FILE")]
    config: Option<std::path::PathBuf>,

    /// Increase logging verbosity (-v for info, -vv for debug, -vvv for trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Decrease logging verbosity (only show errors)
    #[arg(short, long)]
    quiet: bool,
}

/// Determines the log level from CLI arguments.
fn get_log_level(verbose: u8, quiet: bool, config_level: &str) -> Level {
    if quiet {
        return Level::ERROR;
    }

    match verbose {
        0 => match config_level.to_lowercase().as_str() {
            "trace" => Level::TRACE,
            "debug" => Level::DEBUG,
            "info" => Level::INFO,
            "error" => Level::ERROR,
            _ => Level::WARN,
        },
        1 => Level::INFO,
        2 => Level::DEBUG,
        _ => Level::TRACE,
    }
}

/// Initialises the tracing subscriber for logging.
fn init_tracing(level: Level) {
    let filter = EnvFilter::from_default_env().add_directive(level.into());

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}

/// Entry point for the git-proxy-mcp server.
fn main() -> ExitCode {
    let args = Args::parse();

    // Load configuration first to get log level
    let config_path = args.config.as_deref();
    let cfg = match config::load_config(config_path) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            if config_path.is_none() {
                if let Some(default_path) = config::default_config_path() {
                    eprintln!("\nExpected config at: {}", default_path.display());
                    eprintln!("Create one based on config/example-config.json");
                }
            }
            return ExitCode::FAILURE;
        }
    };

    // Initialise logging
    let log_level = get_log_level(args.verbose, args.quiet, &cfg.logging.level);
    init_tracing(log_level);

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting git-proxy-mcp server"
    );

    // Create audit logger
    let audit_logger = if let Some(path) = &cfg.logging.audit_log_path {
        match AuditLogger::new(path) {
            Ok(logger) => {
                info!(path = %path.display(), "Audit logging enabled");
                logger
            }
            Err(e) => {
                error!(error = %e, "Failed to create audit logger");
                return ExitCode::FAILURE;
            }
        }
    } else {
        info!("Audit logging disabled");
        AuditLogger::disabled()
    };

    // Log server start
    audit_logger.log_silent(&AuditEvent::server_started());

    // Build security config
    let security_config = SecurityConfig {
        allow_force_push: cfg.security.allow_force_push,
        protected_branches: cfg.security.protected_branches,
        repo_allowlist: cfg.security.repo_allowlist,
        repo_blocklist: cfg.security.repo_blocklist,
    };

    info!(
        force_push = security_config.allow_force_push,
        protected_branches = ?security_config.protected_branches,
        "Configuration loaded"
    );

    // Create git executor (no credentials stored — uses system git config)
    let executor = GitExecutor::new();

    // Create MCP server
    let mut server = McpServer::new(executor, security_config, audit_logger);

    info!("MCP server ready, waiting for client connection...");
    info!("Note: Authentication uses your existing Git credential configuration");

    // Run the server
    // Using current-thread runtime since MCP uses single-connection stdio transport
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");
    let result = runtime.block_on(server.run());

    match result {
        Ok(()) => {
            info!("Server shut down gracefully");
            ExitCode::SUCCESS
        }
        Err(e) => {
            error!(error = %e, "Server error");
            ExitCode::FAILURE
        }
    }
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

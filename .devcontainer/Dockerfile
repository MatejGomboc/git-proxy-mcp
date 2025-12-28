# Devcontainer for git-proxy-mcp
# Based on the official Rust image with additional dependencies for git2

FROM mcr.microsoft.com/devcontainers/rust:1-bookworm

# Install dependencies required for git2-rs (libgit2 bindings)
# - libssl-dev: OpenSSL development libraries
# - pkg-config: Helper tool for compiling applications
# - libgit2-dev: Optional, can use bundled libgit2 instead
# - cmake: Required if building libgit2 from source
RUN apt-get update && export DEBIAN_FRONTEND=noninteractive \
    && apt-get -y install --no-install-recommends \
        libssl-dev \
        pkg-config \
        cmake \
    && apt-get clean \
    && rm -rf /var/lib/apt/lists/*

# Install additional Rust components
RUN rustup component add clippy rustfmt rust-src

# Set up cargo environment
ENV CARGO_HOME=/usr/local/cargo
ENV PATH="${CARGO_HOME}/bin:${PATH}"

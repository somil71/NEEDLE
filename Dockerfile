# ── Build stage ───────────────────────────────────────────────────────────────
FROM rust:1.88-bookworm AS builder

WORKDIR /app

# Install build deps for rusqlite bundled (needs cc + build tools)
RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev build-essential \
    && rm -rf /var/lib/apt/lists/*

# Cache dependencies — copy manifests first, dummy src to trigger dep compile.
# src-tauri is a workspace member (desktop app); its manifest must be present
# for Cargo to resolve the workspace, but we never build it here — the server
# image doesn't need a webview/GUI toolchain (gtk3, webkit2gtk, etc.).
COPY Cargo.toml Cargo.lock ./
COPY src-tauri ./src-tauri
RUN mkdir src && echo "fn main(){}" > src/main.rs && \
    cargo build --release -p needle 2>/dev/null || true && \
    rm -rf src

# Copy real source and compile (server binary only)
COPY src ./src
COPY benches ./benches
RUN cargo build --release -p needle

# ── Runtime stage ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim

WORKDIR /app

# Runtime deps: openssl (reqwest TLS), ca-certs (GitHub API HTTPS), git (repo cloning)
RUN apt-get update && apt-get install -y \
    ca-certificates libssl3 git \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/needle /usr/local/bin/needle

# Persistent volume for SQLite user DB (mounted by Railway at /data)
RUN mkdir -p /data
ENV DATA_DIR=/data

EXPOSE 8080

CMD ["needle", "serve"]

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Alchemy is an npm-compatible package manager written in Rust. It implements a pnpm-style `node_modules/` layout using hard links from a global content-addressable store (`~/.alchemy-store/`).

## Build Commands

```bash
cargo check --workspace        # Type-check without building
cargo build --release          # Build release binary
cargo test --workspace         # Run all tests
cargo clippy --workspace -- -D warnings  # Lint (CI treats warnings as errors)
cargo fmt --all                # Format code
cargo fmt --all -- --check     # Check formatting without modifying
```

The binary is named `alchemy` (defined in `crates/alchemy_cli/Cargo.toml`).

## Architecture

Five workspace crates with a clear dependency flow:

```
alchemy_cli → alchemy_core, alchemy_registry, alchemy_store, alchemy_linker
alchemy_registry → alchemy_core
alchemy_linker → alchemy_core
alchemy_store (standalone)
alchemy_core (standalone)
```

### Crate Responsibilities

- **alchemy_cli** — Binary entry point. Clap-based CLI with subcommands: `install`, `add`, `remove`, `init`. Each command is a separate module.
- **alchemy_core** — Core types and logic: `resolver` (greedy DFS semver resolution with cycle detection), `dependency` (PackageId/ResolvedPackage), `manifest` (package.json parsing), `lockfile` (alchemy-lock.yaml), `graph` (petgraph DiGraph).
- **alchemy_registry** — HTTP/2 client for the npm registry. Uses reqwest with rustls-tls. Fetches metadata and tarballs with a 32-concurrent semaphore.
- **alchemy_store** — Content-addressable store at `~/.alchemy-store/packages/{name}/{version}/`. SHA-256 integrity verification.
- **alchemy_linker** — Creates pnpm-style node_modules layout: hard links from store → `.pnpm/` virtual store → root symlinks. Handles `.bin/` creation and lifecycle scripts (preinstall/install/postinstall).

### Install Pipeline (3 phases)

1. **Resolve** — Read package.json, recursively fetch registry metadata, build dependency graph
2. **Download** — Parallel tarball downloads, extract to global store with integrity check
3. **Link** — Hard link from store into `.pnpm/`, create dependency symlinks (relative), create root symlinks for direct deps, wire up `.bin/`

### node_modules Layout

```
node_modules/
├── .pnpm/
│   └── pkg@version/node_modules/
│       ├── pkg/           (hard-linked from ~/.alchemy-store/)
│       └── dep → ../../dep@ver/node_modules/dep  (relative symlink)
├── direct-dep → .pnpm/direct-dep@ver/node_modules/direct-dep
└── .bin/
```

Scoped packages (`@scope/name`) use `+` as separator in `.pnpm/` directory names (e.g., `@scope+name@version`).

## Key Dependencies

- **tokio** — Async runtime (full features)
- **petgraph** — Dependency graph (DiGraph)
- **node-semver** — npm-compatible semver matching
- **clap** — CLI argument parsing (derive macros)
- **tracing** — Structured logging (env filter, default INFO)

## Lockfile

Format: `alchemy-lock.yaml` (YAML, pnpm-inspired). Contains `lockfileVersion`, `importers` (root project specifiers), and `packages` (resolved versions with dependencies).

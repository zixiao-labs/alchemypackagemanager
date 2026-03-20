# Alchemy

A fast, disk-efficient npm package manager written in Rust.

## Features

- **Fast** — HTTP/2 multiplexing, parallel downloads (32 concurrent), async I/O
- **No phantom dependencies** — pnpm-style strict `node_modules` structure; only declared dependencies are `require()`-able
- **Disk efficient** — Global content-addressable store (`~/.alchemy-store/`) with hard links; multiple projects share the same physical files

## Installation

### From source (requires Rust toolchain)

```bash
cargo install --git https://github.com/zixiao-labs/alchemypackagemanager.git alchemy_cli
```

### Install script (macOS / Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/zixiao-labs/alchemypackagemanager/main/install.sh | bash
```

### From release binaries

Download the latest binary from [Releases](https://github.com/zixiao-labs/alchemypackagemanager/releases) and add it to your `PATH`.

## Usage

```bash
# Initialize a new project
alchemy init

# Install all dependencies from package.json
alchemy install

# Add a package
alchemy add express
alchemy add typescript -D   # as devDependency

# Remove a package
alchemy remove express
```

## How It Works

### Three-phase install pipeline

```
Resolve → Download → Link
```

1. **Resolve** — Reads `package.json`, recursively fetches registry metadata, picks the highest satisfying version for each package (greedy DFS), and builds a dependency graph.
2. **Download** — Parallel downloads of missing tarballs (semaphore-limited to 32), extracts into global content-addressable store (`~/.alchemy-store/`), verifies SHA-256 integrity.
3. **Link** — Creates pnpm-style `node_modules/` layout with hard links from the global store and symlinks for dependency relationships.

### pnpm-style `node_modules` (no phantom dependencies)

```
node_modules/
├── express → .pnpm/express@4.18.2/node_modules/express      # direct dep: symlink
├── .pnpm/
│   ├── express@4.18.2/
│   │   └── node_modules/
│   │       ├── express/          # hard-linked from global store
│   │       ├── accepts → ../../accepts@1.3.8/node_modules/accepts
│   │       └── body-parser → ../../body-parser@1.20.1/node_modules/body-parser
│   ├── accepts@1.3.8/
│   │   └── node_modules/
│   │       ├── accepts/          # hard-linked from global store
│   │       └── mime-types → ../../mime-types@2.1.35/node_modules/mime-types
│   └── ...
```

- Root `node_modules/` only contains symlinks to direct dependencies — you can't `require()` undeclared packages.
- Transitive dependencies are isolated inside their `.pnpm` directories.

### Global content-addressable store

```
~/.alchemy-store/
└── packages/
    └── express/
        └── 4.18.2/              # files hard-linked into projects
```

- Files are stored once globally and hard-linked into each project.
- Saves ~70% disk space compared to npm.

### Lockfile

Alchemy generates `alchemy-lock.yaml`, a YAML lockfile similar to pnpm's format:

```yaml
lockfileVersion: "1.0"
importers:
  .:
    dependencies:
      express:
        specifier: "^4.17.0"
        version: "4.18.2"
packages:
  /express@4.18.2:
    resolution:
      integrity: "sha512-..."
    dependencies:
      accepts: "1.3.8"
```

## Project Structure

```
crates/
├── alchemy_cli/         # Binary — CLI entry point (clap)
├── alchemy_core/        # Core types, resolver, dependency graph, lockfile
├── alchemy_registry/    # npm registry HTTP/2 client
├── alchemy_store/       # Content-addressable store (~/.alchemy-store/)
└── alchemy_linker/      # pnpm-style node_modules linker
```

## Requirements

- Rust 1.75+ (for building from source)
- Node.js (for running installed packages)

## License

MIT

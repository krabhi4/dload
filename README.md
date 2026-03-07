# DLoad — Memory-Efficient Download Manager

[![CI](https://github.com/krabhi4/dload/actions/workflows/ci.yml/badge.svg)](https://github.com/krabhi4/dload/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Contributions Welcome](https://img.shields.io/badge/contributions-welcome-brightgreen.svg)](CONTRIBUTING.md)

![Language](https://img.shields.io/badge/language-Rust-orange?logo=rust)
![Framework](https://img.shields.io/badge/framework-Axum-blue)
![Database](https://img.shields.io/badge/database-SQLite-lightblue?logo=sqlite)
![Platform](https://img.shields.io/badge/platform-amd64%20%7C%20arm64-lightgrey?logo=docker)
![Memory](https://img.shields.io/badge/memory-~20--50MB-success)
![Docker Image](https://img.shields.io/badge/docker-ghcr.io%2Fkrabhi4%2Fdload-blue?logo=github)

A fast, lightweight download manager built in Rust that streams directly to disk without buffering in RAM. Solves aria2's high memory usage problem.

## Features

- **Low Memory Usage**: Streams chunks directly to disk (~20-50MB vs aria2's 100-500MB+)
- **HTTP/HTTPS Support**: Fast async downloading with progress tracking
- **BitTorrent / Magnet Links**: Torrent support via `librqbit`
- **Web UI**: Modern browser-based interface
- **REST API**: Programmatic access
- **SQLite Database**: Persistent download history
- **Docker Ready**: Multi-platform builds (amd64, arm64)

## Quick Start

### Docker Compose (Recommended)

```bash
# Clone and run
docker compose up -d

# Access at http://localhost:8080
```

### Docker Run

```bash
docker run -d \
  --name dload \
  -p 8080:8080 \
  -v ./downloads:/data \
  ghcr.io/krabhi4/dload:latest
```

### Build from Source

```bash
cargo build --release
./target/release/dload
```

## Configuration

Default settings:
- **Port**: 8080
- **Download Directory**: `/data`
- **Max Concurrent**: 3 downloads
- **Username**: admin
- **Password**: admin

## API Endpoints

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/auth/login` | Login |
| GET | `/api/downloads` | List downloads |
| POST | `/api/downloads` | Add download |
| DELETE | `/api/downloads/:id` | Remove download |
| GET | `/api/settings` | Get settings |
| PUT | `/api/settings` | Update settings |

## Development

```bash
# Build
cargo build

# Run
cargo run

# Run tests
cargo test

# Lint
cargo clippy

# Format
cargo fmt

# Docker build
docker build -t dload .
```

## Architecture

```
┌─────────────────────────────────────┐
│         Web UI (Vanilla JS)          │
└─────────────────┬───────────────────┘
                  │ HTTP
┌─────────────────▼───────────────────┐
│         REST API (Axum)              │
├─────────────────────────────────────┤
│      Download Manager               │
├──────────────┬──────────────────────┤
│  HTTP Worker │  BitTorrent Worker  │
│  (reqwest)   │  (librqbit)         │
├──────────────┴──────────────────────┤
│    Direct to Disk (streaming)       │
└─────────────────────────────────────┘
```

## Memory Comparison

| Tool | Memory Usage |
|------|--------------|
| aria2 | 100-500MB+ |
| **dload** | **~20-50MB** |

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) to get started. Make sure to follow our [Code of Conduct](CODE_OF_CONDUCT.md).

## Security

Found a security issue? Please report it responsibly via [SECURITY.md](SECURITY.md).

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for a history of notable changes.

## License

MIT — see [LICENSE](LICENSE) for details.

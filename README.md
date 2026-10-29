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
- **HTTP/HTTPS Support**: Multi-connection downloads that resume after a pause or restart
- **BitTorrent / Magnet Links**: Torrent support via `librqbit`
- **Multiple Download Folders**: Configure multiple folders with labels, set a default, and map qBittorrent categories to specific folders for Sonarr/Radarr integration
- **Web UI**: Modern browser-based interface
- **REST API**: Programmatic access
- **qBittorrent API Compatible**: Works as a drop-in replacement for qBittorrent in Sonarr, Radarr, and other arr apps
- **SQLite Database**: Persistent download history
- **Docker Ready**: Multi-platform builds (amd64, arm64)
- **Browser Extension**: [`send-to-dload`](extensions/send-to-dload/) for Chrome / Firefox — detects any browser download, magnet link, or `.torrent` URL and forwards it to dload (alpha)

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
  -e PUID=$(id -u) \
  -e PGID=$(id -g) \
  -v ./data:/data \
  -v ./downloads:/downloads \
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
- **Download Directory**: `/downloads` (configurable, supports multiple folders)
- **Max Concurrent**: 3 downloads
- **Username**: admin
- **Password**: admin

### File Permissions (PUID / PGID)

dload drops root inside the container and runs as the `PUID`/`PGID` you provide
(default `1000:1000`) — the same convention as the LinuxServer.io and *arr
images. This keeps the SQLite database and downloaded files owned by your host
user instead of `root:root`. Set them to your own ids (`id -u` / `id -g`):

```yaml
environment:
  - PUID=1000
  - PGID=1000
```

On startup the entrypoint also re-owns existing files under `/data` and
`/downloads` to match, so volumes created by an older root-only build are
migrated automatically.

### Multiple Download Folders

Configure multiple download folders in Settings, each with a label and path. One folder is the default for manual downloads. When adding a download via the UI, a folder dropdown appears (hidden if only one folder is configured).

For Sonarr/Radarr integration, give a download folder the same label as the client's category (for example a folder labelled `tv-sonarr` at `/downloads/tv`, and one labelled `radarr` at `/downloads/movies`). Downloads with that category go to that folder. Mapping a category with the `createCategory` API and a matching `savePath` also works.

### Sonarr and Radarr

Add dload in Sonarr/Radarr as a **qBittorrent** download client: host and port of dload, and a dload admin username and password.

dload works with them the way qBittorrent does:
- **After a download finishes**, the torrent keeps seeding. Sonarr/Radarr import it into your library, by **hardlink** when the download folder and the library are on the same filesystem and by copy otherwise. A hardlink is a second name for the same file, so it uses no extra space.
- **When a seed limit is reached**, the torrent stops and Sonarr/Radarr remove it if "Remove Completed" is on.
- **Where limits come from**: set global limits in dload's Settings (**Stop seeding at ratio**, **Stop seeding after**). An indexer's Seed Ratio or Seed Time in Sonarr/Radarr overrides them for torrents grabbed from that indexer.

For hardlinks, mount one shared folder at the same path in dload, Sonarr and Radarr, and keep downloads and the library inside it:

```yaml
# dload:          ./media:/media   (download folders /media/torrents/tv and /media/torrents/movies)
# sonarr, radarr: ./media:/media   (root folders /media/library/tv and /media/library/movies)
```

Label the dload folders `tv-sonarr` and `radarr` (the default categories) so each app's downloads land in its own folder.

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

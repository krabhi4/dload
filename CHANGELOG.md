# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-03-10

### Added
- BitTorrent/magnet link download support via `librqbit`
- Download history tab in the Web UI
- SQLite-backed persistent download history
- HTTP mirror for torrent downloads — accelerate slow torrents by pasting a seedbox HTTP link
  - Supports single-file torrents (direct download) and multi-file torrents (zip extraction)
  - Preserves arr stack integration data (info_hash, category, save_path) throughout
  - Per-download "keep seeding" option after mirror completes
  - Multi-connection HTTP downloads with range requests
  - Zip extraction with path traversal protection
  - SSRF protection on mirror URL input
  - Automatic startup recovery from interrupted mirrors

### Changed
- Improved memory efficiency for large file downloads

## [0.1.0] - 2026-03-07

### Added
- Initial release
- HTTP/HTTPS download support with streaming to disk
- Web UI with real-time progress tracking
- REST API for programmatic control
- JWT-based authentication
- SQLite database for download state persistence
- Docker support with multi-platform builds (amd64, arm64)
- Docker Compose configuration

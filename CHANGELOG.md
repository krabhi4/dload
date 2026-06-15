# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-06-15

### Security
- Authenticated API requests now confirm the token's user still exists in the database on every request. Because JWTs are signed with a stable secret, a leftover browser token previously kept working after its account was deleted or the database was wiped/recreated (e.g. it could still add downloads with no users present). The database role is now authoritative, so a demoted admin's existing token no longer grants admin access.

### Fixed
- Downloaded files and the SQLite database are no longer owned by `root`. The container drops privileges to a configurable `PUID`/`PGID` via `gosu` and re-owns `/data` and `/downloads` on startup.
- Torrent sessions failed to initialize when running as a non-root user because librqbit's DHT state directory under `$HOME` was not writable; `$HOME` now points at the writable `/data` volume.
- The Web UI drops a stale/expired token on load and shows the login/registration screen instead of silently using a dead session.

### Added
- `PUID`/`PGID` environment variables (LinuxServer.io / *arr convention) for host-friendly file ownership.
- Tag-driven container releases: pushing a `vX.Y.Z` git tag publishes versioned `ghcr.io` image tags (`X.Y.Z`, `X.Y`) alongside the rolling `latest` built from `main`.

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

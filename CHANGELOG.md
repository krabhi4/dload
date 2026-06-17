# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-06-18

### Added
- **API keys.** Users can generate long-lived, named, revocable API keys under **Profile → API keys** in the web UI. A key is shown once at creation (only its SHA-256 hash is stored), can be revoked at any time, and authenticates the download/service API via `Authorization: Bearer <key>` (the key prefix is `dload_`). Endpoints: `GET/POST /api/auth/api-keys`, `DELETE /api/auth/api-keys/{id}`. Account-security and admin-management endpoints (change password, user management, key management) remain **session-only** (JWT) so a leaked key cannot escalate; a deleted user's keys are purged automatically.
- `send-to-dload` browser extension — see `extensions/send-to-dload/`. Detects any browser download, magnet link, or `.torrent` URL and forwards it to a self-hosted dload server. Supports Chrome / Edge / Brave / Opera and Firefox from a single source tree, ships with a filter pipeline (host whitelist/blacklist, extension include/exclude, size bounds), a durable retry queue with exponential backoff, and an activity log. Authenticates with an API key. Distributed as `.zip` (Chrome) and `.xpi` (Firefox) on GitHub Releases — no store listing in this release.
- CI: new `extension` job in `.github/workflows/ci.yml` runs web-ext lint + vitest + build, uploads the artifacts as a workflow run artifact.

## [0.3.1] - 2026-06-16

### Security
- Bumped `rand` 0.9.2 → 0.9.3 to pick up the fix for GHSA-cq8v-f236-94qc (a soundness issue in `ThreadRng` that could cause undefined behaviour when a custom `log` logger calls `rand::rng()` during reseeding). dload does not meet the trigger conditions, but the vulnerable version is no longer in the dependency tree.

### Changed
- Dropped OpenSSL from the dependency tree: `librqbit` now uses its `rust-tls` feature (rustls via `ring`), so the build and runtime images no longer need `pkg-config`/`libssl-dev` or `libssl3`. The application's own `reqwest` client already used rustls.
- Container build now produces a true multi-architecture image (`linux/amd64` + `linux/arm64`) built natively per-arch on GitHub-hosted runners, replacing the amd64-only build.
- Rebuilt the GHCR build pipeline for speed: the dependency layer is cached with `cargo-chef`, the release profile uses thin LTO with parallel codegen, and the builder image is pinned to Debian bookworm so its glibc matches the runtime image.

## [0.3.0] - 2026-06-16

### Changed
- Redesigned the Web UI from the editorial (serif) theme to a terminal/TUI aesthetic: Nord palette, monospace grid, ASCII-style progress bars, glyph + label status badges, and box-drawing panels. Dark-first, with a light variant that follows the OS preference. The stylesheet is hand-rolled vanilla CSS with no build step and no external font/CDN dependency — the strict Content-Security-Policy blocks web fonts, so the system monospace stack is used.
- Renamed the "Archive" tab back to "History" (canonical route `#history`, with `#archive`/`#completed` kept as aliases) and replaced its promotional copy with factual descriptions.

### Added
- Downloads page: status filter chips (all / active / queued / paused / completed) and a live-refresh indicator.
- History page: a search box, completed/failed filters, and a select-all toggle.
- Settings: minimum split size and listen port are now editable (previously persisted but not surfaced in the UI).
- Responsive mobile layout — download rows collapse into cards and navigation moves into a drawer.

## [0.2.2] - 2026-06-16

### Changed
- Dependency upgrades: axum 0.8, axum-extra 0.12, rusqlite 0.40, reqwest 0.13, tower-http 0.6, thiserror 2, bcrypt 0.19, zip 8, jsonwebtoken 10 (with the rust_crypto provider), openssl 0.10.80, plus a batch of minor/patch updates.
- Docker build image bumped to rust 1.96-slim.

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

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.6] - 2026-08-19

### Changed
- Lowered minimum password length requirement from 8 to 4 characters for user registration, user creation, and password changes.

### Dependencies
- Bumped `base64` (0.23.0 → 0.23.1), `rusqlite` (0.40.1 → 0.40.2), `thiserror` (2.0.19 → 2.0.20), `futures` (0.3.33 → 0.3.34), and `openssl-probe` (0.38.1 → 0.38.2) in backend dependencies.
- Bumped `fast-uri` 3.1.4 → 3.1.5 and `js-yaml` 4.3.0 → 4.3.1 in `extensions/send-to-dload`.

## [0.4.5] - 2026-08-05

### Added
- **Tags / labels.** Downloads can now carry multiple tags (free-form strings), matching the qBittorrent Web API. Tags are exposed in the UI as small chips on each download row and in the detail panel. Endpoints: `GET /api/v2/torrents/tags`, `POST /api/v2/torrents/createTags`, `POST /api/v2/torrents/deleteTags`, `POST /api/v2/torrents/addTags`, `POST /api/v2/torrents/removeTags`, `POST /api/v2/torrents/setTags`. The `info` endpoint now supports `?tag=` filtering (empty = untagged only).
- **Tag-to-folder routing.** When a torrent is added with tags, the server automatically routes it to the download folder whose **label** matches a tag (case-insensitive). Priority: category mapping > tag label match > savepath > default. This lets users tag content as `movies`, `tv`, etc., and have it land in the right folder without per-client configuration.
- qBittorrent `addTags`, `removeTags`, `setTags`, `createTags`, `deleteTags` endpoints now persist instead of returning no-op stubs.

### Changed
- Downloads list now shows tag chips inline next to the protocol badge.
- Download detail panel includes a Tags row.
- The `tags` column is now persisted in the `downloads` table (one-time migration adds `tags TEXT DEFAULT ''`).

### Security
- SSRF: URL validation now rejects decimal-IP hosts (e.g. `2130706433`) in both the native API and the qBit-compat add handlers, and the redirect policy now blocks decimal-IP redirect targets.
- IPv6: `is_private_ip` now rejects link-local (`fe80::/10`) and unique-local (`fc00::/7`) addresses, closing an IPv6 SSRF bypass.
- JWT: encode and decode paths now explicitly restrict to `HS256`; `alg:none` is no longer accepted.
- Headers: `Strict-Transport-Security` and `Permissions-Policy` are now set on every response.
- Input limits: `max_concurrent` and `max_connections_per_file` are now capped at 100; URL-based add endpoints cap at 100 URLs per request.
- Path traversal: torrent metadata paths are sanitized through `sanitize_rel_path` before joining with the download folder, preventing `..`-based escapes.

### Dependencies
- Bumped `undici` 7.28.0 → 7.29.0 in `extensions/send-to-dload` to pick up security fixes for Cache-Control parsing (GHSA-4cwx-7wf7-3272), header value CRLF injection (GHSA-m8rv-5g2x-5cg5), cache bypass via qualified directives (GHSA-jr45-8vmc-qm54), stale Content-Length on retry (GHSA-8xcm-r25x-g524), and cookie attribute injection (GHSA-v3r7-h72x-cjcm).

## [0.4.1] - 2026-07-24

### Fixed
- **Torrents failed to start with `error initializing persistent DHT`.** The container drops privileges to `PUID:PGID` via `gosu`, which resets `HOME` to the target user's home directory (`/` for a UID with no `/etc/passwd` entry). That silently overrode the `export HOME=/data` in the entrypoint, so `librqbit` tried to persist its DHT routing table at `/.cache/com.rqbit.dht/dht.json` — a path the unprivileged user cannot create — and every torrent/magnet add failed at `Session::new()`. HTTP downloads were unaffected. The entrypoint now sets `HOME=/data` on `gosu`'s exec'd command (`gosu … env HOME=/data …`) so it survives into the application process and DHT state persists to the writable `/data` volume.

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

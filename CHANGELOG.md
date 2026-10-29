# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-10-29

A redesign of the web UI and the browser extension's pages, a rewritten HTTP download engine, qBittorrent-style seeding for Sonarr/Radarr, and a round of torrent, queue and security fixes. Upgrading is an image swap: the database gains three columns automatically and no configuration changes are needed. Before upgrading, know that:

- **Torrents now keep seeding after Sonarr/Radarr import them**, as with qBittorrent. Before, dload told them seeding was already done, so they moved the files out and deleted the torrent straight away. To keep that old behaviour, set **Stop seeding at ratio** to `0` in Settings.
- Download actions on the native API (`pause`, `resume`, `cancel`, `delete`, `resume-all`, `http-mirror`) now answer errors with 401/403/404 instead of HTTP 200 with `success: false`.
- A `DLOAD_JWT_SECRET` shorter than 32 characters or starting with `changeme` is ignored in favour of a generated secret saved in the data folder. Users sign in again once.
- HTTP downloads that were half-finished when you upgrade restart from the beginning as `name (1).ext`. Older versions kept no resume data. You can delete the old partial file.

### Changed
- **New web UI design.** A graphite theme, with a matching light theme that follows the OS setting, replaces the Nord terminal look. Host Grotesk sets the text and Atkinson Hyperlegible Mono sets only measured values (sizes, speeds, ETAs, paths, times). Amber marks the primary action and the active tab; blue marks speeds and progress.
- **Downloads.** The add bar leads, and the queue is one panel with the filters and bulk actions in its top strip. Statuses read as words (Downloading, Seeding, Paused, Done, Failed) instead of glyph codes.
- **History** groups entries in one panel per day, with native checkboxes and a Select all / Deselect all toggle.
- **Settings** sets each option as a row with its description beside the field; user management sits in its own panel.
- **Profile** shows the account details as text, with Change password and API keys side by side.
- **Sign-in** shows the logo and plain Username and Password labels.
- **Scroll position.** The page scrollbar is hidden and a thin line at the top of the header shows scroll progress. Browsers without CSS scroll timelines (Firefox for now) keep a thin themed scrollbar. Page width no longer shifts when switching tabs.
- **Phones.** Queue rows keep size, speed and progress readable, and long names wrap to two lines.
- **Accessibility.** Visible focus rings on every control, field edges at 3:1 contrast, reduced-motion support, and the closed mobile drawer no longer takes keyboard focus.
- Labels and buttons use sentence case without terminal glyphs.
- **Smaller container image.** The image is now Alpine with a static musl binary (using the mimalloc allocator) instead of Debian, down from 180 MB to 38 MB (12 MB compressed). `su-exec` replaces `gosu`; `PUID`/`PGID` work as before.
- **`latest` now tracks releases.** Images are pushed to GHCR only from `vX.Y.Z` tags (as `X.Y.Z`, `X.Y`, `X` and `latest`), and the tag is created automatically when the version in `Cargo.toml` changes on `main`, with signed SLSA provenance and an SBOM. `main` and pull requests build the image without publishing it, so the per-commit `sha-*` tags are gone.

### Added
- **Seeding that works with Sonarr and Radarr the way qBittorrent does.**
  - While a torrent seeds, dload reports it as `uploading`/`stalledUP`, so Sonarr and Radarr import it by hardlink (or copy) and leave it seeding.
  - When a seed limit is reached, the torrent stops and shows as `pausedUP`, so they can then remove it. Hardlinked library files are unaffected.
  - Two new settings: **Stop seeding at ratio** and **Stop seeding after (minutes)**. Leave them blank to seed forever.
  - The per-indexer **Seed Ratio** and **Seed Time** that Sonarr/Radarr send with each grab override them for that torrent, as does `setShareLimits`.
  - Upload totals and ratios are tracked and survive restarts.
- **Category folders.** Give a download folder the same label as a Sonarr/Radarr category (for example `tv-sonarr` or `radarr`) and that category's downloads land there, like a qBittorrent category save path.
- Self-hosted fonts served from `/ui/fonts/` (about 37 KB, SIL Open Font License), so the UI makes no third-party requests and works under the existing Content Security Policy.
- **HTTP pause and resume.** Paused, stopped, failed and interrupted HTTP downloads now continue where they left off instead of starting over. Progress is kept in a small `<file>.dload` file next to the download and removed when it completes. HTTP downloads that were running when the container stopped resume on startup, like torrents.

### Fixed
- **Queued downloads never started.** When a slot freed up, the next queued download switched to Downloading but stayed at 0 bytes.
- **Multi-connection HTTP downloads were often no faster than one connection.** On HTTP/2 servers (most CDNs, GitHub releases) every "connection" shared a single TCP connection. Each range now gets its own HTTP/1.1 connection.
- **Speed dropped off near the end and fluctuated.** Idle connections now take over half of the largest unfinished range instead of waiting for the slowest one. Disk writes no longer block the network reads, and the reported speed is averaged over 5 seconds instead of one.
- **Servers that limit connections (HTTP 429/503) no longer fail the download.** Refused connections give up their range to the ones that work.
- **Downloads failed on servers that reject `HEAD` requests.** The size and range check now uses a `GET`, as browsers do.
- **Error pages were saved as completed downloads.** A 404 or 403 now fails the download with the HTTP status.
- **Existing files were overwritten.** A new download whose name is taken is saved as `name (1).ext`.
- **Long magnet links were rejected as too long.** The limit is now 512 KiB for magnets and 16 KiB for other URLs, and magnets are checked for a valid info hash when added.
- **Very long filenames failed with "File name too long".** Names are shortened to 240 bytes, keeping the extension.
- The real filename from `Content-Disposition` now shows while downloading, not only after completion.
- Failed downloads report the underlying cause (for example "connection refused") instead of only "error sending request".
- Finishing or failing an HTTP download no longer resets its queue position, tags or category to the values from when it started.
- FTP links are rejected when added instead of failing after they start.
- **Torrent speed jumped around (10, then 34 MB/s on a 25 MB/s line).** librqbit's figure covers only about half a second; dload now averages the bytes received over 5 seconds, so the speed and ETA hold steady. The ETA uses the same format as HTTP downloads.
- **Seeding torrents blocked the queue.** They counted toward the concurrent-download limit, so a few seeding torrents left every new download Queued. Only downloading items use a slot now.
- **Torrents added by link or magnet stopped seeding after a restart.** Their protocol was never saved, so they came back as finished HTTP downloads. Existing databases are repaired on startup.
- **Deleting with files could remove data that wasn't the download's.** A torrent with `./`-prefixed paths could make it empty the whole download folder, adding a multi-file torrent could delete same-named files already in the folder, and deleting a failed entry could delete another download's file. Torrent metadata is now read without touching the disk, and deletion only removes paths strictly inside a download folder that the entry itself wrote and no other entry uses.
- **Adding a torrent that was already in the list shared it between two entries**, so deleting either one deleted both. Duplicates are refused: 409 on the native API, `Fails.` on the qBittorrent API (as qBittorrent does), and a clear error for `.torrent` links.
- **Pausing, stopping or reordering a torrent while it was fetching metadata or checking files was undone a moment later**, and pausing a magnet that was still fetching left it stuck. A quick pause then resume could also leave a torrent paused and removed from the session.
- **Changing the default download folder broke all later torrents until a restart.** One torrent session now serves every folder.
- Torrents that hit a disk or I/O error are marked Failed with the reason instead of showing Downloading forever, and failed torrents no longer auto-resume on every restart.
- Queued `.torrent` uploads to a non-default folder failed when their turn came, and were invisible to Sonarr/Radarr while queued. The `.torrent` files now live in the data folder (`/data/torrents`) and the info hash is known straight away.
- A `.torrent` fetched by an HTTP download now goes through the queue, uses that download's folder, and resumes after a restart.
- Resuming when every slot is busy queues the download instead of going over the limit, and queued downloads keep their place in line after a restart.
- Dragging to reorder no longer starts paused, stopped or failed downloads, and reordering inside a filter no longer pushes running downloads back to Queued.
- HTTP mirrors of multi-file torrents without a top folder land in the right place, and "keep seeding" off now stops seeding once the recheck finishes.
- Torrents use each tracker's own announce interval instead of a forced 60 seconds, which private trackers can treat as abuse.
- `docker stop` shuts down cleanly: HTTP downloads save their resume state before exit.
- Web UI:
  - Enter and Space work on the buttons inside a download row.
  - The list refreshes after a menu is closed.
  - "Add HTTP mirror" works for every torrent.
  - Pasting several links adds them all.
  - `.torrent` files over 2 MB upload, and invalid or duplicate files are reported.
  - Failed actions show the server's error.
  - Changing your password no longer signs you out.
  - Login shows the rate-limit message.
  - History shows up to 1,000 entries.
  - The new API key dialog can't be closed by accident.
  - The folder picker keeps keyboard focus.
  - The unused "Listen port" setting is gone.
- Startup no longer logs "migration … failed: duplicate column name" for migrations that already ran.
- A torrent whose file check after a restart took longer than two minutes got stuck, neither downloading nor seeding. A torrent that librqbit leaves paused without being asked to is now restarted.
- Resuming a torrent while an older task for it was still shutting down could remove it from the session.
- Deleting a torrent with its files no longer deletes data that another entry also uses, for example a cross-seed.
- Adding several links at once could add a torrent that was already in the list.
- Two quick starts of the same queued download could launch it twice.
- Links containing spaces, and magnet names with spaces, are no longer split into separate links when pasted.
- HTTP mirrors of a paused or stopped multi-file torrent land in the right folder, and the empty `.tmp` folder is removed afterwards.
- Queued `.torrent` uploads saved by older versions in a non-default folder are found after upgrading.
- Seed limits sent by Sonarr/Radarr with a `.torrent` upload were lost when the torrent started.
- Stopped HTTP downloads no longer restart after a reboot.
- A torrent that hit a disk error can be added again.
- The folder picker keeps keyboard focus when opening a folder.
- "Clear completed" no longer reports an error when another app already removed an item.

### Security
- `.torrent` URLs are fetched through the SSRF-safe client, limited to 10 MiB.
- SSRF checks now apply to every redirect hop, not only the first host. Proxies set in `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` may still use private addresses.
- **The example `docker-compose.yml` shipped a public JWT secret**, which let anyone forge an admin token on installs that kept it. The line is now commented out; without `DLOAD_JWT_SECRET` (or with a placeholder or short value) dload generates a random secret and keeps it in `data/jwt_secret`, so sessions survive restarts.
- Unknown usernames are checked against a real bcrypt hash, so login timing no longer reveals which accounts exist.
- qBittorrent Basic Auth checks the rate limit before the password, so a locked-out client can't keep guessing. Successful logins don't count toward the limit, so several Sonarr/Radarr instances can't lock out the web login after a restart.
- With `DLOAD_TRUST_PROXY_HEADERS`, the client IP comes from the last `X-Forwarded-For` hop (the one your proxy added) instead of the first, which the client controls.
- Deleting history requires an admin.
- A deleted and re-created username no longer accepts the old account's tokens.
- The qBittorrent API only signs in admins (other users got a session that failed on every call).
- Download-folder and browse paths are normalised before the system-folder check, so `//etc` or `/./proc` are refused.
- HTTP mirror re-adds fetch `.torrent` files through the SSRF-safe client.

### Extension
- send-to-dload 0.2.0 restyles the popup and options pages to match, and 0.2.1 fixes captured downloads being saved twice, ignores clicks that pages fake, and makes failures visible (see `extensions/send-to-dload/CHANGELOG.md`).

## [0.4.7] - 2026-08-19

### Security
- **SSRF: DNS rebinding protection.** The HTTP and mirror downloaders now resolve the target hostname and reject the request if the resolved IP is private/internal (loopback, RFC 1918, link-local, cloud metadata `169.254.169.254`, etc.). The resolved IP is pinned to the reqwest client via `.resolve()` so a DNS rebinding attack cannot redirect the server to an internal address between validation and connection. Previously only the string hostname was checked — a domain resolving to `169.254.169.254` passed validation.
- **JWT revocation on password change.** Added a `token_version` column to the `users` table (default 0). JWT claims now carry a `ver` field; `authenticate_jwt` rejects tokens whose `ver` no longer matches the DB. Bumped atomically with the password hash on every password change, so a stolen token minted before the change is immediately invalid (previously it remained valid for up to 24h).
- **Register endpoint DoS mitigation.** The `/api/auth/register` endpoint now rate-limits (shared with login) and checks whether registration is open (no users exist) **before** hashing the password with bcrypt. Previously every request — even after registration was closed — spent ~250ms of bcrypt CPU with no rate limit.
- **Proxy header spoofing.** `X-Forwarded-For` and `X-Real-IP` headers are now only trusted when `DLOAD_TRUST_PROXY_HEADERS=1` (or `true`) is set. Default is off, so direct deployments without a reverse proxy use the socket peer address for rate limiting — preventing IP-spoofing bypass of the login throttle.
- **Last-admin deletion race.** The admin-count check and the delete now run under a single `Mutex<Connection>` acquisition (`delete_user_guard_last_admin`), closing a TOCTOU window where two concurrent admin deletions could both pass the guard and delete both admins.
- **Username uniqueness race.** `insert_user` now detects the SQLite UNIQUE constraint violation and returns a typed `InsertUserError::UsernameConflict`, so a concurrent create-user race surfaces "Username already exists" instead of a generic "Failed to create user".

### Changed
- HTTP mirror cancellation no longer uses a fixed 100ms sleep; replaced with `yield_now()` since the subsequent handle removal and librqbit's idempotent `session.delete()` make a timed wait unnecessary.

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

# HTTP Mirror for Torrent Downloads

## Problem

Torrent downloads with few seeds max out at 0-400 KBps. Users upload the same torrent to a seedbox provider which downloads it fast, then want to use the seedbox's HTTP download link to accelerate the torrent in DLoad. This must preserve arr stack (Sonarr/Radarr) integration data (info_hash, category, etc.).

## Constraints

- Seedbox provides a direct file link for single-file torrents, but a **zip** for multi-file torrents (folders)
- Arr stack identifies torrents by `info_hash` via the qBittorrent compat API — this must never change or disappear
- Download category, save_path, content_path, and all metadata must survive the mirror process
- librqbit v8 piece-level APIs are `pub(crate)` — not accessible from DLoad as an external consumer

## Approach: Pause, HTTP Download, Re-add

Instead of piece-level coordination (which requires librqbit internals), leverage librqbit's `initial_check()` which runs automatically when a torrent is added and verifies existing files on disk:

1. Pause the torrent in librqbit
2. Delete from librqbit session (keep files on disk)
3. Download via HTTP to the same file location
4. Re-add the torrent — `initial_check()` hash-verifies existing data
5. Only corrupted/missing pieces are downloaded via torrent peers

### Why This Preserves Arr Stack Data

Two separate layers exist:
- **DLoad tracking** — `downloads` HashMap + SQLite DB. This is what `/api/v2/torrents/info` reads. **Never modified** during mirror.
- **librqbit session** — internal torrent engine. Deleted and re-created. Arr stack has zero visibility into this.

The `Download` record (id, info_hash, category, save_path, content_path, filename, created_at) stays in both HashMap and SQLite throughout. The only thing that changes is the `torrent_handles` mapping (DLoad UUID → librqbit usize ID).

## Single-File Torrent Flow

1. Set `http_mirror_status = "downloading"`, `http_mirror_url = url`
2. Snapshot `save_path`, `content_path`, `filename` from the Download record
3. Pause torrent in librqbit
4. Delete from librqbit session with `keep_files=true`
5. Remove `torrent_handles` entry
6. Register a new cancel token for the HTTP phase
7. Start a **dedicated mirror HTTP download** (NOT `HttpDownloader` — see below) to the torrent's `save_path`/`content_path`
8. Monitor progress — update Download.speed, downloaded_size, progress from HTTP atomics
9. When HTTP completes (or fails partway):
   - Set `http_mirror_status = "rechecking"`
   - Re-add torrent using `mirror_readd_torrent()` (see below), NOT `session_add_and_wait()`
   - Restore snapshotted `save_path`, `content_path`, `filename` after re-add
   - librqbit runs `initial_check()`, hash-verifies every piece on disk
   - New librqbit ID stored in `torrent_handles`
   - Register a new cancel token for the torrent monitoring phase
10. Resume monitoring with `monitor_torrent()`
11. Clear `http_mirror_status = null`
12. Seeding behavior based on user's per-download choice

**Resilience:** If HTTP fails at 60%, re-add still happens. librqbit finds 60% valid pieces and only downloads the remaining 40%.

## Multi-File Torrent Flow (Zip)

1. Set `http_mirror_status = "downloading"`, `http_mirror_url = url`
2. Snapshot `save_path`, `content_path`, `filename`
3. Pause + delete from librqbit session (keep files) — same as single-file
4. Detect zip — check the GET response `Content-Type` for `application/zip` or URL/filename ending in `.zip` (seedboxes may send `application/octet-stream` on HEAD, so always check GET)
5. Download zip to temp location: `{download_dir}/.tmp/{download_id}.zip`
6. Set `http_mirror_status = "extracting"`
7. Extract zip to the torrent's `content_path` parent directory, **sanitizing every entry path** (reject entries containing `..` or absolute paths — same defense as `sanitize_filename`)
8. Handle wrapper folder: if zip root is a single folder matching torrent name, extract accordingly
9. Delete temp zip file
10. Set `http_mirror_status = "rechecking"`
11. Re-add torrent, restore snapshotted fields, monitor — same as single-file steps 9-12

## API

`POST /api/downloads/{id}/http-mirror`

```json
{
  "url": "https://seedbox.example.com/download/file.mkv",
  "keep_seeding": true
}
```

**Validations:**
- Download exists and is a torrent (`protocol == Torrent`)
- Download is in `Downloading`, `Paused`, or `Seeding` status
- `http_mirror_status` is `null` (prevents concurrent mirrors on same download)
- URL is valid HTTP/HTTPS

**Response:** `200 OK` — mirror process starts asynchronously

## UI

### Ellipsis Menu

Add "Add HTTP Mirror" as a third option in the torrent ellipsis menu (after "Copy Magnet" and "Download .torrent"). Only visible when torrent status is `Downloading`, `Paused`, or `Seeding`.

### Inline Form

Clicking "Add HTTP Mirror" shows an inline dropdown with:
- Text input for the URL
- Checkbox: "Keep seeding after completion" (default: checked)
- "Start" button

### Progress Display

- Status badge stays "Downloading" (arr stack compatible)
- Speed shows HTTP download speed during mirror phase
- Small label shows mirror status ("via HTTP mirror", "extracting...", "rechecking...")
- No new DownloadStatus enum variant needed

## Domain Changes

Add to `Download` struct:
- `http_mirror_status: Option<String>` — values: `"downloading"`, `"extracting"`, `"rechecking"`, `null`
- `http_mirror_url: Option<String>` — persisted mirror URL for display/retry

These are display-only and NOT exposed to the qBittorrent compat API.

## DB Changes

Two nullable columns added to `downloads` table:
- `http_mirror_status TEXT`
- `http_mirror_url TEXT`

Migration via `ALTER TABLE` on startup.

## Critical Implementation Details

### 1. Do NOT reuse `HttpDownloader` or `session_add_and_wait`

**`HttpDownloader`** mutates `download.filename` and `download.save_path` from the HTTP response's `Content-Disposition` header. The mirror must use a dedicated download function that writes to a fixed path without mutating any Download fields.

**`session_add_and_wait`** unconditionally overwrites `filename`, `save_path`, and `content_path` from torrent metadata (lines 1023-1030). The mirror must use a dedicated `mirror_readd_torrent()` helper that:
1. Calls `session.add_torrent()` with `output_folder` set to the **snapshotted save_path parent** (not the current global `download_dir()` — prevents breakage if user changes download dir mid-mirror)
2. Stores the new handle in `torrent_handles`
3. Does NOT overwrite `filename`, `save_path`, or `content_path`

### 2. Cancel Token Lifecycle

Three distinct phases, each with its own cancel token:
1. **HTTP download phase** — register token, used to cancel HTTP download
2. **Re-add phase** — brief, no cancellation needed
3. **Torrent monitoring phase** — register a NEW token for `monitor_torrent()`

When user pauses/cancels during HTTP phase: the HTTP cancel token is cancelled, HTTP download stops, but we still re-add the torrent (so partial data is preserved). The torrent is then paused in librqbit.

### 3. Re-add URL Dispatch

The mirror must replicate `handle_torrent_download`'s URL-type dispatch:
- URL starts with `magnet:` → `AddTorrent::from_url(&url)`
- URL starts with `torrent://` → load from `{download_dir}/.torrents/{download_id}.torrent`, use `AddTorrent::from_bytes()`
- Otherwise → `AddTorrent::from_url(&url)` (HTTP torrent file URL)

### 4. Startup Recovery

On app startup, if a download has `http_mirror_status != null`:
- Clean up temp zip file at `{download_dir}/.tmp/{download_id}.zip` if it exists
- Reset `http_mirror_status = null`
- The download will be in `Paused` status (existing startup logic sets all Downloading → Paused)
- User can resume (re-starts torrent from peers) or retry the HTTP mirror

### 5. Zip Path Traversal Protection

Every zip entry path must be validated before extraction:
- Reject entries containing `..` components
- Reject absolute paths
- Reject paths that resolve outside the target extraction directory
- Use the same defensive approach as `sanitize_filename()` in `domain/download.rs`

## New Dependency

- `zip = "2"` — for zip extraction in multi-file torrent flow

## Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` | Add `zip = "2"` |
| `src/domain/download.rs` | Add `http_mirror_status`, `http_mirror_url` fields |
| `src/db/repository.rs` | Add columns, update insert/update/get queries |
| `src/manager/mod.rs` | Add `start_http_mirror()`, `mirror_readd_torrent()`, `mirror_download_file()` methods; add startup cleanup for stale mirrors |
| `src/api/` | Add `POST /api/downloads/:id/http-mirror` route |
| `src/ui/app.js` | Add menu item, inline form, mirror status label |

## Not Changed

- qBittorrent compat API — no modifications, arr stack sees no difference
- `Download.info_hash`, `category`, `id` — never touched
- `DownloadStatus` enum — no new variants
- `HttpDownloader` — not reused; dedicated mirror download function instead
- `session_add_and_wait` — not reused; dedicated re-add helper instead

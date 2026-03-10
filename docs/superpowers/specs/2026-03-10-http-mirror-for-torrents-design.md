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

1. Pause torrent in librqbit
2. Delete from librqbit session with `keep_files=true`
3. Remove `torrent_handles` entry
4. Start HTTP multi-connection download to the torrent's `save_path`/`content_path`
5. Monitor progress — update Download.speed, downloaded_size, progress from HTTP atomics
6. When HTTP completes (or fails partway):
   - Re-add torrent to librqbit using original magnet URL or persisted `.torrent` file
   - librqbit runs `initial_check()`, hash-verifies every piece on disk
   - Good pieces marked as "have", corrupted/missing pieces re-downloaded via peers
   - New librqbit ID stored in `torrent_handles`
7. Resume monitoring with `monitor_torrent()`
8. Seeding behavior based on user's per-download choice

**Resilience:** If HTTP fails at 60%, re-add still happens. librqbit finds 60% valid pieces and only downloads the remaining 40%.

## Multi-File Torrent Flow (Zip)

1. Pause + delete from librqbit session (keep files) — same as single-file
2. Detect zip via HTTP `Content-Type` header or URL/filename ending in `.zip`
3. Download zip to temp location: `{download_dir}/.tmp/{download_id}.zip`
4. Extract zip to the torrent's `content_path` parent directory
5. Handle wrapper folder: if zip root is a single folder matching torrent name, extract accordingly
6. Delete temp zip file
7. Re-add torrent — `initial_check()` verifies extracted files
8. Resume monitoring

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

## New Dependency

- `zip = "2"` — for zip extraction in multi-file torrent flow

## Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` | Add `zip = "2"` |
| `src/domain/download.rs` | Add `http_mirror_status`, `http_mirror_url` fields |
| `src/db/repository.rs` | Add columns, update insert/update/get queries |
| `src/manager/mod.rs` | Add `start_http_mirror()` method |
| `src/api/` | Add `POST /api/downloads/:id/http-mirror` route |
| `src/ui/app.js` | Add menu item, inline form, mirror status label |

## Not Changed

- qBittorrent compat API — no modifications, arr stack sees no difference
- `Download.info_hash`, `category`, `id` — never touched
- `DownloadStatus` enum — no new variants

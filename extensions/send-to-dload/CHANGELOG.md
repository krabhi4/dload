# Changelog

All notable changes to **send-to-dload** (the browser extension).

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> The extension is versioned independently of the dload server.

## [0.2.1] — Unreleased

### Fixed
- Captured browser downloads are now actually cancelled. Chrome ignored `suggest({ cancel: true })` and Firefox never cancelled at all, so every captured file downloaded twice (three times after a 4xx). The filters now decide first; only captured downloads are cancelled and erased, and skipped ones continue in the browser untouched.
- Firefox: size filters use `totalBytes` (`fileSize` is -1 until the download completes), only the file's base name is sent, and downloads the extension re-issued itself are no longer re-captured.
- A cancelled download that dload marks `Failed` within about 20 seconds (for example, one that needs browser cookies) is handed back to the browser and logged.
- Firefox: a download that finishes before it can be cancelled (small, fast files) is no longer also sent to dload. The browser keeps it and the log shows `skipped (browser-kept)`, instead of the file being downloaded twice.
- The content script ignores synthetic (untrusted) clicks, so pages can't push magnets or `.torrent` links without a real user gesture.
- Missing server URL, API key or host permission is treated as permanent: such sends are not queued, and the retry queue drops items with permanent errors (400/401/403/409) immediately. A notification is shown when the queue gives up on an item.
- Firefox host permission: the options page requests access straight from the click (no lost user gesture), origin patterns no longer include the port, and "Saved." is no longer shown when access was denied (the settings are not saved).
- Firefox for Android: missing `contextMenus` no longer stops the background script from registering its other listeners.
- A skipped or failed item no longer blocks the next attempt for 60 seconds; only sent or queued items are deduped.
- Requests to dload time out after 15 seconds and are retried like other network errors.
- URLs in the activity log are capped at 2,000 characters so large magnets can't overflow storage; the retry queue keeps full URLs.
- `.torrent` names in the query string classify as torrents, and `magnet:` links are matched case-insensitively.
- FTP URLs are no longer captured or advertised (dload rejects them).
- The retry alarm is restored at background startup when the queue has items and cleared when the queue empties.
- Popup and options messages (status, resend, connection test) are accepted only from extension pages.
- A queued send that the retry queue later delivers is logged as sent, instead of staying "queued-retry" in the activity log.

## [0.2.0] — Unreleased

### Changed
- Popup and options pages restyled to match the dload 1.0 web UI: shared tokens in `src/shared/tokens.css`, bundled Host Grotesk and Atkinson Hyperlegible Mono fonts, light and dark themes that follow the OS setting, and visible focus rings.
- Options page: "Save settings" is the one primary button; "Test connection" is secondary.

## [0.1.0] — 2026-06-17

### Added
- Initial release.
- Manifest V3 source tree that builds for Chromium and Firefox from one manifest.
- Three detection channels: browser download interception, right-click context menu, content-script magnet scanner.
- POST to `dload /api/downloads` (JSON `{url, folder_id?}`) authenticated with a dload **API key** (generated under Profile → API keys; stored as `apiKey`).
- Filter pipeline (whitelist/blacklist, extension include/exclude, size floor/ceiling, per-kind toggles).
- Persistent retry queue with exponential backoff (30s → 6h, max 10 attempts) backed by `chrome.alarms`.
- Options page with "Test connection" + activity log (last 200 events).
- Popup with "Send current page" button + recent activity.
- Cross-browser polyfill so the codebase uses one `browser.*` Promise API on both Chrome and Firefox.
- Vitest unit tests for `classify`, `filters`, `retry`, `config`.
- GitHub Actions CI job: lint + test + build artifacts uploaded as workflow artifact.

### Known limitations
- No Options field to pick a default folder yet — `folder_id` is honored by the server but not user-selectable, so downloads land in dload's default folder. The suggested filename is sent but the server currently ignores it.
- No Chrome Web Store or AMO listing yet.
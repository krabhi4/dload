# Changelog

All notable changes to **send-to-dload** (the browser extension).

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> The extension is versioned independently of the dload server.

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
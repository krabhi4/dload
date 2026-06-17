// shared/config.js — chrome.storage.local accessors.
// The activity log lives under its own "log" key (not inside "cfg") so frequent
// background log writes can't read-modify-write the whole config and revert a
// concurrent options-page save. All writes are serialized via runExclusive.

import { runExclusive } from "./lock.js";

const STORAGE_KEY = "cfg";
const LOG_KEY = "log";
const LOG_MAX = 200;

export const DEFAULT_CONFIG = Object.freeze({
  serverUrl: "",
  apiKey: "",
  defaultFolderId: "",
  rules: {
    enabled: true,
    mode: "blacklist", // "blacklist" | "whitelist"
    hostnameList: [],
    extensionList: {
      include: [
        ".torrent", ".iso", ".zip", ".rar", ".7z", ".tar", ".gz", ".bz2",
        ".xz", ".dmg", ".apk", ".exe", ".msi", ".deb", ".rpm", ".pkg",
        ".mp4", ".mkv", ".avi", ".mov", ".webm", ".m4v",
        ".mp3", ".flac", ".ogg", ".m4a", ".opus", ".wav",
        ".pdf", ".epub", ".mobi",
      ],
      exclude: [
        ".html", ".htm", ".js", ".mjs", ".css",
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".avif", ".ico",
        ".txt", ".json", ".xml",
      ],
    },
    minSizeBytes: 0,
    maxSizeBytes: 0,
    allowMagnets: true,
    allowHttpDownloads: true,
    notifyOnSend: true,
    notifyOnSkip: false,
  },
  log: [],
});

/** Merge a partial config onto the defaults (shallow merge, rules deep-merged). */
export function withDefaults(partial) {
  const out = {
    ...DEFAULT_CONFIG,
    ...(partial || {}),
    rules: {
      ...DEFAULT_CONFIG.rules,
      ...((partial && partial.rules) || {}),
      extensionList: {
        ...DEFAULT_CONFIG.rules.extensionList,
        ...((partial && partial.rules && partial.rules.extensionList) || {}),
      },
    },
    log: Array.isArray(partial && partial.log) ? partial.log : [],
  };
  return out;
}

/** Load full config (+ activity log) from chrome.storage.local. */
export async function loadConfig() {
  try {
    const got = await browser.storage.local.get([STORAGE_KEY, LOG_KEY]);
    const cfg = withDefaults(got[STORAGE_KEY]);
    if (Array.isArray(got[LOG_KEY])) cfg.log = got[LOG_KEY];
    return cfg;
  } catch (err) {
    console.warn("[send-to-dload] loadConfig failed:", err);
    return withDefaults({});
  }
}

/** Save the config (or a partial — partial gets merged with existing first). */
export async function saveConfig(partial) {
  return runExclusive(async () => {
    const current = await loadConfig();
    const merged = withDefaults({ ...current, ...partial });
    const cfgOnly = { ...merged };
    delete cfgOnly.log; // the log has its own storage key
    await browser.storage.local.set({ [STORAGE_KEY]: cfgOnly });
    return merged;
  });
}

/** Append one activity log entry; drops oldest beyond LOG_MAX. */
export async function logActivity(entry) {
  return runExclusive(async () => {
    const got = await browser.storage.local.get(LOG_KEY);
    const log = Array.isArray(got[LOG_KEY]) ? got[LOG_KEY].slice() : [];
    log.push({
      ts: Date.now(),
      ...entry,
    });
    while (log.length > LOG_MAX) log.shift();
    await browser.storage.local.set({ [LOG_KEY]: log });
    return log;
  });
}

/** Wipe the activity log. */
export async function clearLog() {
  return runExclusive(async () => {
    await browser.storage.local.set({ [LOG_KEY]: [] });
  });
}

/** Strip trailing slash + ensure no whitespace. */
export function normalizeServerUrl(url) {
  return (url || "").trim().replace(/\/+$/, "");
}
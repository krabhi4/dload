// shared/logger.js
// Lightweight console wrapper that prefixes everything and lets the popup
// surface the last 20 entries via the rolling chrome.storage.local log.

const PREFIX = "[send-to-dload]";

export const logger = {
  info(...args) {
    console.info(PREFIX, ...args);
  },
  warn(...args) {
    console.warn(PREFIX, ...args);
  },
  error(...args) {
    console.error(PREFIX, ...args);
  },
  debug(...args) {
    if (globalThis.__send_to_dload_debug) {
      console.debug(PREFIX, ...args);
    }
  },
};
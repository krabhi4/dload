// shared/polyfill.js — loaded first in the background and content scripts.
// Firefox already has a Promise-based `browser` namespace (no-op there); on
// Chromium we install a minimal Promise shim over the chrome.* APIs we use, so
// the rest of the code can `await browser.*` on both engines without a bundler.

(function () {
  if (typeof globalThis.browser !== "undefined" && globalThis.browser.runtime) {
    if (typeof globalThis.chrome === "undefined") {
      globalThis.chrome = globalThis.browser;
    }
    return;
  }

  // Minimal Promise-based shim for the chrome.* APIs we touch.
  const wrapCallback = (fn) => (...args) => {
    return new Promise((resolve, reject) => {
      try {
        fn(...args, (result) => {
          if (chrome.runtime?.lastError) {
            reject(new Error(chrome.runtime.lastError.message));
          } else {
            resolve(result);
          }
        });
      } catch (err) {
        reject(err);
      }
    });
  };

  const promisifyNamespace = (ns, methods) => {
    if (!ns) return;
    for (const m of methods) {
      if (typeof ns[m] === "function" && !m.endsWith("Listener")) {
        try {
          ns[m] = wrapCallback(ns[m].bind(ns));
        } catch {
          // Some methods aren't wrappable (e.g. already return Promises); ignore.
        }
      }
    }
  };

  promisifyNamespace(chrome.storage?.local, ["get", "set", "remove"]);
  promisifyNamespace(chrome.storage?.session, ["get", "set", "remove"]);
  promisifyNamespace(chrome.storage?.sync, ["get", "set", "remove"]);
  promisifyNamespace(chrome.permissions, ["contains", "request", "remove"]);
  promisifyNamespace(chrome.notifications, ["create", "clear", "getAll"]);
  promisifyNamespace(chrome.downloads, ["download", "search", "cancel"]);
  promisifyNamespace(chrome.tabs, ["query", "create", "sendMessage"]);
  promisifyNamespace(chrome.scripting, ["executeScript", "registerContentScripts"]);

  // runtime.sendMessage / connect get a Promise wrapper too.
  if (chrome.runtime?.sendMessage) {
    chrome.runtime.sendMessage = wrapCallback(chrome.runtime.sendMessage.bind(chrome.runtime));
  }

  globalThis.browser = chrome;
})();
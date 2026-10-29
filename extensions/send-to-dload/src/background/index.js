// background/index.js — service-worker (Chrome) / event-page (Firefox) entry.

import "../shared/polyfill.js"; // defines `browser` on the Chromium SW — do not remove
import { onCandidate, notify } from "./detector.js";
import { verifyToken, sendToDload } from "./api.js";
import { loadConfig, logActivity } from "../shared/config.js";
import { downloadSize } from "./filters.js";
import * as retry from "./retry.js";
import { MSG, REPLY } from "./messaging.js";
import { logger } from "../shared/logger.js";

const MENU_LINK_ID = "send-to-dload-link";
const MENU_PAGE_ID = "send-to-dload-page";

retry.ensureAlarm();

browser.runtime.onInstalled.addListener(async () => {
  await loadConfig(); // seeds defaults
  await registerContextMenus();
  logger.info("installed");
});

// onStartup exists on Firefox MV3 event pages but not Chromium SWs — guard it.
const onStartup = browser.runtime.onStartup;
if (onStartup && typeof onStartup.addListener === "function") {
  onStartup.addListener(async () => {
    await registerContextMenus();
    logger.info("started");
  });
}

async function registerContextMenus() {
  if (!browser.contextMenus) return;
  try {
    await browser.contextMenus.removeAll();
  } catch {
    /* ignore */
  }
  try {
    browser.contextMenus.create({
      id: MENU_LINK_ID,
      title: "Send to dload",
      contexts: ["link", "video", "audio"],
    });
    browser.contextMenus.create({
      id: MENU_PAGE_ID,
      title: "Send this page URL to dload",
      contexts: ["page"],
    });
  } catch (err) {
    logger.warn("contextMenus.create failed:", err);
  }
}

browser.contextMenus?.onClicked.addListener(async (info) => {
  let url = "";
  if (info.menuItemId === MENU_LINK_ID) {
    url = info.linkUrl || info.srcUrl || "";
  } else if (info.menuItemId === MENU_PAGE_ID) {
    url = info.pageUrl || "";
  }
  if (!url) return;

  await onCandidate({
    url,
    source: "context-menu",
    referer: info.pageUrl || undefined,
  });
});

// Channel 1: intercept browser downloads. Register exactly ONE listener.
// Chrome uses onDeterminingFilename, Firefox (which lacks it) uses onCreated.
// Either way the download is only cancelled once the filters decide to capture it.
if (browser.downloads?.onDeterminingFilename) {
  browser.downloads.onDeterminingFilename.addListener((item, suggest) => {
    handleInterceptedDownload(item, suggest);
    return true; // tells Chrome to await the async suggest()
  });
} else if (browser.downloads?.onCreated) {
  browser.downloads.onCreated.addListener((item) => {
    handleInterceptedDownload(item, () => {});
  });
}

async function cancelDownload(id) {
  try {
    await browser.downloads.cancel(id);
  } catch (err) {
    logger.warn("downloads.cancel failed:", err);
    return false;
  }
  try {
    await browser.downloads.erase({ id });
  } catch (err) {
    logger.warn("downloads.erase failed:", err);
  }
  return true;
}

async function handleInterceptedDownload(item, suggest) {
  let suggested = false;
  const release = () => {
    if (suggested) return;
    suggested = true;
    try {
      suggest();
    } catch {
      /* ignore */
    }
  };
  try {
    // A download we re-issued after a failed POST: let the browser save it,
    // else we'd re-intercept and cancel our own fallback in a loop.
    if (item.byExtensionId && item.byExtensionId === browser.runtime.id) return;

    const cfg = await loadConfig();
    if (!cfg.serverUrl || !cfg.apiKey || !cfg.rules.enabled) return;

    const url = item.finalUrl || item.url || "";
    if (!url || url.startsWith("blob:") || url.startsWith("data:")) return;

    await onCandidate(
      {
        url,
        source: "download",
        filename: String(item.filename || "").split(/[\\/]/).pop() || undefined,
        mime: item.mime || undefined,
        size: downloadSize(item),
        referer: item.referrer || undefined,
      },
      {
        cancelBrowserDownload: () => {
          release();
          return cancelDownload(item.id);
        },
      },
    );
  } catch (err) {
    logger.error("handleInterceptedDownload threw:", err);
  } finally {
    release();
  }
}

function fromExtensionPage(sender) {
  return typeof sender.url === "string" && sender.url.startsWith(browser.runtime.getURL(""));
}

browser.runtime.onMessage.addListener((msg, sender, sendResponse) => {
  if (!msg || typeof msg !== "object") return false;

  if (msg.type === MSG.FROM_CONTENT_MAGNET) {
    onCandidate({
      url: msg.href,
      source: "content-script",
      filename: msg.filename || undefined,
      referer: sender.url || undefined,
    })
      .then((result) => sendResponse({ ok: result.ok, reason: result.reason }))
      .catch((err) => sendResponse({ ok: false, error: err.message }));
    return true; // keep the async message channel open
  }

  if (
    (msg.type === MSG.FROM_POPUP_TEST || msg.type === MSG.FROM_POPUP_STATUS || msg.type === MSG.FROM_POPUP_RESEND) &&
    !fromExtensionPage(sender)
  ) {
    return false;
  }

  if (msg.type === MSG.FROM_POPUP_TEST) {
    verifyToken(msg.serverUrl, msg.apiKey)
      .then((r) => sendResponse({ type: REPLY.TEST_OK, result: r }))
      .catch((err) => sendResponse({ type: REPLY.TEST_FAIL, error: err.message }));
    return true;
  }

  if (msg.type === MSG.FROM_POPUP_STATUS) {
    loadConfig()
      .then((cfg) => {
        const last = (cfg.log || []).slice(-10).reverse();
        sendResponse({ type: REPLY.STATUS, configured: Boolean(cfg.serverUrl && cfg.apiKey), last });
      })
      .catch((err) => sendResponse({ type: REPLY.STATUS, error: err.message }));
    return true;
  }

  if (msg.type === MSG.FROM_POPUP_RESEND) {
    onCandidate({
      url: msg.url,
      source: "context-menu",
      filename: msg.filename || undefined,
    })
      .then((result) =>
        sendResponse({
          type: result.ok ? REPLY.RESEND_OK : REPLY.RESEND_FAIL,
          reason: result.reason,
          error: result.error,
        }),
      )
      .catch((err) => sendResponse({ type: REPLY.RESEND_FAIL, error: err.message }));
    return true;
  }

  return false;
});

browser.alarms.onAlarm.addListener(async (alarm) => {
  if (alarm.name !== "send-to-dload-retry") return;
  try {
    const deliver = async (payload) => {
      const result = await sendToDload(payload);
      await logActivity({ kind: "retry", url: payload.url, decision: "sent", reason: "retry-delivered" });
      return result;
    };
    await retry.tick(deliver, (payload, reason) =>
      notify("dload: gave up on a queued send", `${String(payload.url || "").slice(0, 80)} (${reason})`),
    );
  } catch (err) {
    logger.error("retry tick threw:", err);
  }
});
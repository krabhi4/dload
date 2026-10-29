// background/detector.js — funnels all three detection channels through
// onCandidate(): classify → filter → dedupe → POST (or queue/retry).

import { classify } from "../shared/classify.js";
import { shouldCapture, applyHostFilter } from "./filters.js";
import { sendToDload, fetchDownload } from "./api.js";
import * as retry from "./retry.js";
import { loadConfig, logActivity } from "../shared/config.js";
import { logger } from "../shared/logger.js";

const RECENT = new Map(); // dedupeKey -> ts; one send per (url, filename) within the TTL
const RECENT_TTL_MS = 60 * 1000;
const RECENT_MAX_ENTRIES = 500; // safety valve against unbounded growth
const WATCH_INTERVAL_MS = 2000;
const WATCH_WINDOW_MS = 20 * 1000;

function dedupeKey({ url, filename }) {
  return `${url}::${filename || ""}`;
}

/** Schemes dload's /api/downloads can actually act on. */
function isForwardableUrl(url) {
  if (typeof url !== "string" || !url) return false;
  if (/^magnet:/i.test(url)) return true;
  try {
    const p = new URL(url);
    return p.protocol === "http:" || p.protocol === "https:";
  } catch {
    return false;
  }
}

function isDuplicate(key) {
  const now = Date.now();
  const seen = RECENT.get(key);
  if (seen && now - seen < RECENT_TTL_MS) return true;
  if (RECENT.size >= RECENT_MAX_ENTRIES) {
    RECENT.clear();
  } else {
    for (const [k, t] of RECENT.entries()) {
      if (now - t > RECENT_TTL_MS) RECENT.delete(k);
    }
  }
  RECENT.set(key, now);
  return false;
}

/** Re-issue a download we cancelled, so a failed POST doesn't lose the file. */
async function restoreBrowserDownload(url, filename) {
  try {
    if (!url) return;
    await browser.downloads.download({ url, filename: filename || undefined });
  } catch (err) {
    logger.warn("restoreBrowserDownload failed:", err);
  }
}

async function watchServerDownload(id, input, kind) {
  const deadline = Date.now() + WATCH_WINDOW_MS;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, WATCH_INTERVAL_MS));
    let dl;
    try {
      dl = await fetchDownload(id);
    } catch (err) {
      logger.warn("watchServerDownload: status check failed:", err);
      continue;
    }
    if (!dl) return;
    if (dl.status === "Failed") {
      await restoreBrowserDownload(input.url, input.filename);
      await logActivity({
        kind,
        url: input.url,
        decision: "error",
        reason: `dload failed (${dl.error_message || "no reason given"}); restored browser download`,
      });
      return;
    }
    if (dl.status === "Completed" || (dl.status === "Downloading" && dl.downloaded_size > 0)) return;
  }
}

/**
 * @param {{ url: string, source: 'download'|'context-menu'|'content-script'|'retry',
 *   filename?: string, mime?: string, size?: number, referer?: string }} input
 * @param {{ cancelBrowserDownload?: () => Promise<boolean> }} [hooks]
 */
export async function onCandidate(input, { cancelBrowserDownload } = {}) {
  const key = dedupeKey({ url: input.url, filename: input.filename });
  let cancelled = false;
  try {
    const cfg = await loadConfig();
    if (!cfg.serverUrl || !cfg.apiKey) {
      await logActivity({
        kind: input.source,
        url: input.url,
        decision: "skipped",
        reason: "not-configured",
      });
      return { ok: false, reason: "not-configured" };
    }

    const item = { ...input, ...classify({ url: input.url, mime: input.mime || "", filename: input.filename || "" }) };
    // Explicit actions (context menu, popup) bypass the auto-capture filters but
    // must still respect the kill-switch and be a URL dload can actually fetch.
    const explicit = input.source === "context-menu";
    const enabled = cfg.rules?.enabled !== false;
    const decision = !enabled
      ? { capture: false, reason: "disabled" }
      : explicit
        ? isForwardableUrl(input.url)
          ? { capture: true, reason: "explicit" }
          : { capture: false, reason: "unsupported-scheme" }
        : applyHostFilter(item, cfg.rules, shouldCapture(item, cfg.rules));

    if (!decision.capture) {
      await logActivity({
        kind: item.kind,
        url: input.url,
        decision: "skipped",
        reason: decision.reason,
      });
      if (cfg.rules.notifyOnSkip) {
        notify(`dload: skipped (${decision.reason})`, truncate(input.url, 80));
      }
      return { ok: false, reason: decision.reason };
    }

    if (input.source !== "retry" && isDuplicate(key)) {
      await logActivity({
        kind: input.source,
        url: input.url,
        decision: "skipped",
        reason: "duplicate",
      });
      return { ok: false, reason: "duplicate" };
    }

    if (cancelBrowserDownload) {
      cancelled = await cancelBrowserDownload();
      if (!cancelled) {
        RECENT.delete(key);
        await logActivity({ kind: item.kind, url: input.url, decision: "skipped", reason: "browser-kept" });
        return { ok: false, reason: "browser-kept" };
      }
    }

    let result;
    try {
      result = await sendToDload({
        url: input.url,
        filename: input.filename,
        folderId: cfg.defaultFolderId || undefined,
      });
    } catch (err) {
      const msg = err && err.message ? err.message : String(err);

      if (err && err.permanent) {
        // Re-sending unchanged can't succeed (bad token, rejected URL). Skip the
        // retry queue and hand any cancelled download back to the browser.
        RECENT.delete(key);
        await logActivity({ kind: item.kind, url: input.url, decision: "error", reason: msg });
        if (cancelled) {
          await restoreBrowserDownload(input.url, input.filename);
        }
        notify("dload: send failed", truncate(input.url, 80));
        return { ok: false, reason: "error", error: msg };
      }

      // Transient (server down/network): queue for retry. No browser restore —
      // the queue delivers to dload later; restoring would dup on every blip.
      await logActivity({ kind: item.kind, url: input.url, decision: "queued-retry", reason: msg });
      await retry.enqueue(
        { url: input.url, filename: input.filename, folderId: cfg.defaultFolderId || undefined },
        msg,
      );
      notify("dload: queued for retry", truncate(input.url, 80));
      return { ok: false, reason: "queued", error: msg };
    }

    await logActivity({
      kind: item.kind,
      url: input.url,
      decision: "sent",
      reason: decision.reason,
    });

    if (cancelled && result && result.id) {
      watchServerDownload(result.id, input, item.kind).catch((err) =>
        logger.warn("watchServerDownload threw:", err),
      );
    }

    if (cfg.rules.notifyOnSend) {
      notify("dload: sent", truncate(input.url, 80));
    }

    return { ok: true, result };
  } catch (err) {
    logger.error("onCandidate threw:", err);
    RECENT.delete(key);
    if (cancelled) await restoreBrowserDownload(input.url, input.filename);
    await logActivity({
      kind: "unknown",
      url: input.url,
      decision: "error",
      reason: err && err.message ? err.message : String(err),
    });
    return { ok: false, error: err.message };
  }
}

export function notify(title, body) {
  try {
    browser.notifications.create({
      type: "basic",
      iconUrl: browser.runtime.getURL("src/icons/icon-48.png"),
      title: title || "Send to dload",
      message: body || "",
    });
  } catch {
    /* notifications permission might not be granted in dev */
  }
}

function truncate(s, n) {
  s = String(s || "");
  return s.length > n ? s.slice(0, n) + "…" : s;
}
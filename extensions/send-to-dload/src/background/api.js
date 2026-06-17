// background/api.js — dload HTTP client. Tokens go in Authorization: Bearer …;
// fetches use credentials:"omit" so dload never sees browser cookies.

import { loadConfig, normalizeServerUrl } from "../shared/config.js";
import { logger } from "../shared/logger.js";

// Only checks the grant; requesting it must happen from a user gesture (the
// options page), which Firefox requires and the background can't provide.
export async function ensureHostPermission(serverUrl) {
  try {
    const u = new URL(serverUrl);
    const origin = `${u.protocol}//${u.host}/*`;
    return await browser.permissions.contains({ origins: [origin] });
  } catch (err) {
    logger.warn("ensureHostPermission check failed:", err);
    return false;
  }
}

/** POST to /api/auth/verify (the API key as a bare JSON string). */
export async function verifyToken(serverUrl, apiKey) {
  const base = normalizeServerUrl(serverUrl);
  if (!base) throw new Error("Server URL not set");
  if (!apiKey) throw new Error("API key not set");
  if (!(await ensureHostPermission(base))) {
    throw new Error("Site-access permission for the dload server was not granted");
  }

  const resp = await fetch(`${base}/api/auth/verify`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "Accept": "application/json" },
    body: JSON.stringify(apiKey),
    credentials: "omit",
    mode: "cors",
  });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  return resp.json();
}

/** Forward a URL to dload. Returns the download object, or throws (err.permanent
 *  marks failures the retry queue should not retry). */
export async function sendToDload({ url, filename, folderId }) {
  if (!url || typeof url !== "string") {
    throw new Error("No URL to send");
  }

  const cfg = await loadConfig();
  const serverUrl = normalizeServerUrl(cfg.serverUrl);
  if (!serverUrl) throw new Error("Server URL not configured");
  if (!cfg.apiKey) throw new Error("API key not configured");

  const ok = await ensureHostPermission(serverUrl);
  if (!ok) throw new Error("Host permission denied for dload server");

  // JSON body — dload's JSON branch honors {url, folder_id} (the server ignores
  // filename for now; it's sent for forward-compat).
  let resp;
  try {
    resp = await fetch(`${serverUrl}/api/downloads`, {
      method: "POST",
      headers: {
        "Authorization": `Bearer ${cfg.apiKey}`,
        "Content-Type": "application/json",
        "Accept": "application/json",
      },
      body: JSON.stringify({
        url,
        ...(filename ? { filename } : {}),
        ...(folderId ? { folder_id: folderId } : {}),
      }),
      credentials: "omit",
      mode: "cors",
    });
  } catch (err) {
    // Connection-level failure (server down, DNS, TLS): transient, leave to retry.
    throw new Error(`Network error: ${err && err.message ? err.message : err}`);
  }

  if (resp.status === 401) {
    throw permanentError("Unauthorized — token rejected by dload");
  }
  if (!resp.ok) {
    const text = await resp.text().catch(() => "");
    const err = new Error(`dload ${resp.status}: ${truncate(text, 200)}`);
    err.permanent = resp.status >= 400 && resp.status < 500; // 4xx won't change on retry
    throw err;
  }

  try {
    return await resp.json();
  } catch {
    return { ok: true };
  }
}

function permanentError(message) {
  const err = new Error(message);
  err.permanent = true;
  return err;
}

function truncate(s, n) {
  s = String(s || "");
  return s.length > n ? s.slice(0, n) + "…" : s;
}
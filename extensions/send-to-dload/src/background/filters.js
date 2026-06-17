// background/filters.js — pure filter pipeline → { capture, reason }.

import { classify, hostMatches } from "../shared/classify.js";

// Match the extension only against the pathname (query/hash stripped) and the
// suggested filename. Scanning the whole URL would let a query token like
// "?thumb=preview.jpg" wrongly match ".jpg" and mis-filter the download.
function matchExt(extList, item) {
  const candidates = [];
  try {
    candidates.push(new URL(item.url).pathname);
  } catch {
    candidates.push(String(item.url || "").split(/[?#]/)[0]);
  }
  if (item.filename) candidates.push(item.filename);

  for (const candidate of candidates) {
    const lower = candidate.toLowerCase();
    for (const ext of extList) {
      const e = ext.toLowerCase();
      if (e && lower.endsWith(e)) {
        return true;
      }
    }
  }
  return false;
}

export function shouldCapture(item, rules) {
  if (!rules || rules.enabled === false) {
    return { capture: false, reason: "disabled" };
  }

  if (item.kind === "magnet") {
    if (!rules.allowMagnets) return { capture: false, reason: "magnets-disabled" };
    return { capture: true, reason: "magnet-allowed" };
  }

  if (item.kind === "torrent") {
    if (!rules.allowHttpDownloads) return { capture: false, reason: "http-disabled" };
    return { capture: true, reason: "torrent-allowed" };
  }

  if (item.kind === "http-download") {
    if (!rules.allowHttpDownloads) return { capture: false, reason: "http-disabled" };

    const inc = rules.extensionList?.include || [];
    const exc = rules.extensionList?.exclude || [];
    if (exc.length > 0 && matchExt(exc, item)) {
      return { capture: false, reason: "extension-excluded" };
    }
    if (inc.length > 0 && !matchExt(inc, item)) {
      return { capture: false, reason: "extension-not-included" };
    }

    if (typeof item.size === "number" && item.size > 0) {
      if (rules.minSizeBytes > 0 && item.size < rules.minSizeBytes) {
        return { capture: false, reason: "size-too-small" };
      }
      if (rules.maxSizeBytes > 0 && item.size > rules.maxSizeBytes) {
        return { capture: false, reason: "size-too-large" };
      }
    }

    return { capture: true, reason: "http-allowed" };
  }

  return { capture: false, reason: "kind-unknown" };
}

// Layer the host whitelist/blacklist over the host-agnostic basic result.
// Hostname-less items (magnets) carry no host, so host rules don't apply to them.
export function applyHostFilter(item, rules, basicResult) {
  if (!basicResult.capture) return basicResult;
  if (!item.hostname || rules.mode === "blacklist") {
    if (rules.mode !== "blacklist") return basicResult;
    if (!Array.isArray(rules.hostnameList) || rules.hostnameList.length === 0) {
      return basicResult;
    }
    if (rules.hostnameList.some((p) => hostMatches(item.hostname, p))) {
      return { capture: false, reason: "hostname-blacklisted" };
    }
    return basicResult;
  }

  if (!Array.isArray(rules.hostnameList) || rules.hostnameList.length === 0) {
    return { capture: false, reason: "whitelist-empty" };
  }
  if (!rules.hostnameList.some((p) => hostMatches(item.hostname, p))) {
    return { capture: false, reason: "hostname-not-whitelisted" };
  }
  return basicResult;
}

/** classify + shouldCapture + applyHostFilter in one call. */
export function evaluate(input, rules) {
  const item = { ...input, ...classify(input) };
  const basic = shouldCapture(item, rules);
  return applyHostFilter(item, rules, basic);
}
// shared/classify.js — classify({ url, mime, filename }) → { kind, reason,
// hostname, pathname }, kind ∈ magnet | torrent | http-download | unknown.
// URL + Content-Type + suggested filename are all used so extensionless
// download URLs (e.g. /download?id=123) are still caught.

const TORRENT_MIME = "application/x-bittorrent";
const MAGNET_PREFIX = "magnet:";

const TORRENT_FILENAME_RE = /\.torrent(\?|#|$)/i;
const ARCHIVE_RE = /\.(zip|rar|7z|tar|gz|bz2|xz|iso|dmg|apk|exe|msi|deb|rpm|pkg|appx|msix|cab)(\?|#|$)/i;
const VIDEO_RE = /\.(mp4|mkv|avi|mov|wmv|flv|webm|m4v|mpg|mpeg|m2ts|ts)(\?|#|$)/i;
const AUDIO_RE = /\.(mp3|flac|ogg|wav|m4a|aac|opus|wma|alac|aiff)(\?|#|$)/i;
const DOC_RE = /\.(pdf|epub|mobi|azw3|fb2|djvu)(\?|#|$)/i;

const ARCHIVE_MIMES = new Set([
  "application/zip", "application/x-zip-compressed", "application/x-rar-compressed",
  "application/vnd.rar", "application/x-7z-compressed", "application/x-tar",
  "application/gzip", "application/x-gzip", "application/x-bzip2", "application/x-xz",
  "application/x-iso9660-image", "application/x-apple-diskimage",
  "application/vnd.android.package-archive", "application/x-msdownload",
  "application/x-msi", "application/x-debian-package",
  "application/vnd.debian.binary-package", "application/x-redhat-package-manager",
  "application/x-rpm",
]);
const DOC_MIMES = new Set([
  "application/pdf", "application/epub+zip",
  "application/x-mobipocket-ebook", "application/vnd.amazon.ebook",
]);

/** Match an extension family against a path or filename. Returns the family or null. */
function extFamily(s) {
  if (!s) return null;
  if (ARCHIVE_RE.test(s)) return "archive";
  if (VIDEO_RE.test(s)) return "video";
  if (AUDIO_RE.test(s)) return "audio";
  if (DOC_RE.test(s)) return "doc";
  return null;
}

// Map a Content-Type to a download family, or null for non-downloads (html, css,
// images, …). octet-stream → "generic" since the browser already chose to download.
function mimeFamily(mime) {
  const m = (mime || "").split(";")[0].trim().toLowerCase();
  if (!m) return null;
  if (m === TORRENT_MIME) return "torrent";
  if (m.startsWith("video/")) return "video";
  if (m.startsWith("audio/")) return "audio";
  if (DOC_MIMES.has(m)) return "doc";
  if (ARCHIVE_MIMES.has(m)) return "archive";
  if (m === "application/octet-stream" || m === "application/binary") return "generic";
  return null;
}

export function classify({ url, mime = "", filename = "" }) {
  if (!url || typeof url !== "string") {
    return { kind: "unknown", reason: "empty-url", hostname: "", pathname: "" };
  }

  if (url.startsWith(MAGNET_PREFIX)) {
    return { kind: "magnet", reason: "magnet-scheme", hostname: "", pathname: url };
  }

  let parsed;
  try {
    parsed = new URL(url);
  } catch {
    return { kind: "unknown", reason: "bad-url", hostname: "", pathname: url };
  }

  if (parsed.protocol !== "http:" && parsed.protocol !== "https:" && parsed.protocol !== "ftp:") {
    return {
      kind: "unknown",
      reason: `scheme:${parsed.protocol}`,
      hostname: parsed.hostname,
      pathname: parsed.pathname,
    };
  }

  const pathname = parsed.pathname || "";
  const mlc = (mime || "").toLowerCase();

  // Torrent: by MIME, by .torrent in the URL, or by the suggested filename.
  if (
    mlc.startsWith(TORRENT_MIME) ||
    TORRENT_FILENAME_RE.test(parsed.search + pathname) ||
    TORRENT_FILENAME_RE.test(filename)
  ) {
    return {
      kind: "torrent",
      reason: mlc.startsWith(TORRENT_MIME) ? "torrent-mime" : "torrent-extension",
      hostname: parsed.hostname,
      pathname,
      matchedRule: TORRENT_FILENAME_RE.source,
    };
  }

  // Extension family from path/filename, falling back to Content-Type.
  const byExt = extFamily(pathname) || extFamily(filename);
  const fam = byExt || mimeFamily(mlc);
  if (fam && fam !== "torrent") {
    return {
      kind: "http-download",
      reason: byExt ? `${byExt}-extension` : `mime-${fam}`,
      hostname: parsed.hostname,
      pathname,
      matchedRule: fam,
    };
  }

  return {
    kind: "unknown",
    reason: "no-extension-match",
    hostname: parsed.hostname,
    pathname,
  };
}

/** Cheap hostname match supporting `*.example.com` and exact `example.com`. */
export function hostMatches(hostname, pattern) {
  if (!hostname || !pattern) return false;
  const h = hostname.toLowerCase();
  const p = pattern.toLowerCase().trim();
  if (!p) return false;
  if (p.startsWith("*.")) {
    const suffix = p.slice(1); // ".example.com"
    return h.endsWith(suffix) && h.length > suffix.length;
  }
  return h === p;
}

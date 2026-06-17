// tests/unit/filters.test.mjs
import { describe, it, expect } from "vitest";
import { classify } from "../../src/shared/classify.js";
import { shouldCapture, applyHostFilter } from "../../src/background/filters.js";

const RULES = {
  enabled: true,
  mode: "blacklist",
  hostnameList: [],
  extensionList: {
    include: [".torrent", ".iso", ".zip", ".mp4", ".pdf"],
    exclude: [".html", ".css"],
  },
  minSizeBytes: 0,
  maxSizeBytes: 0,
  allowMagnets: true,
  allowHttpDownloads: true,
  notifyOnSend: true,
  notifyOnSkip: false,
};

function item(url, extra = {}) {
  return { url, ...classify({ url }), ...extra };
}

describe("shouldCapture()", () => {
  it("captures magnets by default", () => {
    const r = shouldCapture(item("magnet:?xt=urn:btih:abc"), RULES);
    expect(r).toEqual({ capture: true, reason: "magnet-allowed" });
  });

  it("skips magnets when allowMagnets=false", () => {
    const r = shouldCapture(item("magnet:?xt=urn:btih:abc"), { ...RULES, allowMagnets: false });
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("magnets-disabled");
  });

  it("captures .torrent files", () => {
    const r = shouldCapture(item("https://t.example/foo.torrent"), RULES);
    expect(r.capture).toBe(true);
  });

  it("captures allowed extensions", () => {
    const r = shouldCapture(item("https://example.com/file.zip"), RULES);
    expect(r.capture).toBe(true);
  });

  it("rejects extensions not in include list", () => {
    const r = shouldCapture(item("https://example.com/song.mp3"), RULES);
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("extension-not-included");
  });

  it("rejects excluded extensions even if also in include", () => {
    // Construct a case where the URL has a kind that's http-download, the
    // extension is in both include AND exclude lists — exclude wins.
    const rules = {
      ...RULES,
      extensionList: {
        include: [".zip", ".iso"],
        exclude: [".iso"],
      },
    };
    const r = shouldCapture(item("https://example.com/disk.iso"), rules);
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("extension-excluded");
  });

  it("applies size floor", () => {
    const r = shouldCapture(
      item("https://example.com/big.zip", { size: 1000 }),
      { ...RULES, minSizeBytes: 5000 },
    );
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("size-too-small");
  });

  it("applies size ceiling", () => {
    const r = shouldCapture(
      item("https://example.com/big.zip", { size: 10_000_000 }),
      { ...RULES, maxSizeBytes: 1_000_000 },
    );
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("size-too-large");
  });

  it("ignores size when size is unknown", () => {
    const r = shouldCapture(item("https://example.com/big.zip"), { ...RULES, minSizeBytes: 5000 });
    expect(r.capture).toBe(true);
  });

  it("returns disabled when enabled=false", () => {
    const r = shouldCapture(item("https://example.com/big.zip"), { ...RULES, enabled: false });
    expect(r).toEqual({ capture: false, reason: "disabled" });
  });

  it("returns http-disabled when allowHttpDownloads=false", () => {
    const r = shouldCapture(
      item("https://example.com/file.zip"),
      { ...RULES, allowHttpDownloads: false },
    );
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("http-disabled");
  });

  it("returns kind-unknown when classify returns unknown", () => {
    // Force a kind=unknown item by classifying a URL that doesn't match any
    // extension family in classify().
    const r = shouldCapture(
      { url: "https://example.com/x.tar.gz.bin", kind: "unknown", mime: "" },
      RULES,
    );
    expect(r).toEqual({ capture: false, reason: "kind-unknown" });
  });
});

describe("applyHostFilter()", () => {
  const base = shouldCapture(item("https://example.com/file.zip"), RULES);

  it("blacklist skips matched hosts", () => {
    const rules = { ...RULES, mode: "blacklist", hostnameList: ["example.com"] };
    const r = applyHostFilter(item("https://example.com/file.zip"), rules, base);
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("hostname-blacklisted");
  });

  it("blacklist with empty list lets everything through", () => {
    const rules = { ...RULES, mode: "blacklist", hostnameList: [] };
    const r = applyHostFilter(item("https://example.com/file.zip"), rules, base);
    expect(r).toEqual(base);
  });

  it("whitelist captures only matched hosts", () => {
    const rules = { ...RULES, mode: "whitelist", hostnameList: ["other.com"] };
    const r = applyHostFilter(item("https://example.com/file.zip"), rules, base);
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("hostname-not-whitelisted");
  });

  it("whitelist with empty list captures nothing", () => {
    const rules = { ...RULES, mode: "whitelist", hostnameList: [] };
    const r = applyHostFilter(item("https://example.com/file.zip"), rules, base);
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("whitelist-empty");
  });

  it("whitelist with *.example.com matches subdomains", () => {
    const rules = { ...RULES, mode: "whitelist", hostnameList: ["*.example.com"] };
    const r = applyHostFilter(item("https://cdn.example.com/file.zip"), rules, base);
    expect(r.capture).toBe(true);
  });

  it("does not override an earlier rejection", () => {
    const rules = { ...RULES, mode: "whitelist", hostnameList: ["example.com"] };
    const skipped = { capture: false, reason: "extension-not-included" };
    const r = applyHostFilter(item("https://example.com/song.mp3"), rules, skipped);
    expect(r).toEqual(skipped);
  });
});

describe("shouldCapture() — filename-aware extension matching", () => {
  const RULES2 = {
    enabled: true,
    mode: "blacklist",
    hostnameList: [],
    extensionList: { include: [".iso", ".zip"], exclude: [".html"] },
    minSizeBytes: 0,
    maxSizeBytes: 0,
    allowMagnets: true,
    allowHttpDownloads: true,
  };

  it("captures an extensionless URL whose suggested filename matches the include list", () => {
    const url = "https://host.example/download?id=9";
    const it_ = { url, filename: "ubuntu-24.04.iso", ...classify({ url, filename: "ubuntu-24.04.iso" }) };
    expect(it_.kind).toBe("http-download");
    expect(shouldCapture(it_, RULES2).capture).toBe(true);
  });

  it("excludes by suggested filename too", () => {
    const it_ = { url: "https://host.example/view?id=9", filename: "page.html", kind: "http-download" };
    const r = shouldCapture(it_, RULES2);
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("extension-excluded");
  });
});

describe("shouldCapture() — query string is not treated as the extension", () => {
  const RULES3 = {
    enabled: true,
    mode: "blacklist",
    hostnameList: [],
    extensionList: { include: [".zip"], exclude: [".jpg"] },
    minSizeBytes: 0,
    maxSizeBytes: 0,
    allowMagnets: true,
    allowHttpDownloads: true,
  };

  it("does not exclude a .zip download whose query string contains .jpg", () => {
    const it_ = { url: "https://site.example/get/file.zip?thumb=preview.jpg", kind: "http-download" };
    expect(shouldCapture(it_, RULES3).capture).toBe(true);
  });

  it("does not include a download just because its query string contains .zip", () => {
    const it_ = { url: "https://site.example/view?file=archive.zip", kind: "http-download" };
    const r = shouldCapture(it_, RULES3);
    expect(r.capture).toBe(false);
    expect(r.reason).toBe("extension-not-included");
  });

  it("still matches a real extension before the query string", () => {
    const it_ = { url: "https://site.example/movie.mp4?token=abc#t=1", kind: "http-download" };
    expect(shouldCapture(it_, { ...RULES3, extensionList: { include: [".mp4"], exclude: [] } }).capture).toBe(true);
  });
});
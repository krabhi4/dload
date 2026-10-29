// tests/unit/detector.test.mjs
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let data;
let events;
let restored;
let fetchImpl;

async function loadDetector() {
  globalThis.browser = {
    runtime: { getURL: (p) => p },
    storage: {
      local: {
        get: async (keys) => {
          const out = {};
          for (const k of [].concat(keys)) out[k] = data[k];
          return out;
        },
        set: async (obj) => {
          Object.assign(data, obj);
        },
      },
    },
    alarms: { get: async () => null, create: async () => {}, clear: async () => {} },
    notifications: { create: () => {} },
    permissions: { contains: async () => true },
    downloads: {
      download: async (opts) => {
        restored.push(opts);
      },
    },
  };
  globalThis.fetch = async (url, opts = {}) => {
    events.push(`${opts.method || "GET"} ${url}`);
    return fetchImpl(url, opts);
  };
  vi.resetModules();
  return import("../../src/background/detector.js");
}

const ok = (body) => ({ ok: true, status: 200, json: async () => body });

describe("onCandidate()", () => {
  let onCandidate;
  const cancelBrowserDownload = async () => {
    events.push("cancel");
    return true;
  };

  beforeEach(async () => {
    data = { cfg: { serverUrl: "http://dload:8080", apiKey: "tok" } };
    events = [];
    restored = [];
    fetchImpl = async () => ok({});
    ({ onCandidate } = await loadDetector());
  });

  afterEach(() => {
    vi.useRealTimers();
    delete globalThis.fetch;
    delete globalThis.browser;
  });

  it("leaves a download the filters skip alone", async () => {
    const r = await onCandidate({ url: "https://h.example/page.html", source: "download" }, { cancelBrowserDownload });
    expect(r.ok).toBe(false);
    expect(events).toEqual([]);
  });

  it("cancels a captured browser download before posting it", async () => {
    const r = await onCandidate({ url: "https://h.example/file.iso", source: "download" }, { cancelBrowserDownload });
    expect(r.ok).toBe(true);
    expect(events).toEqual(["cancel", "POST http://dload:8080/api/downloads"]);
  });

  it("leaves a download alone when the browser finished it before it could be cancelled", async () => {
    const tooLate = async () => {
      events.push("cancel");
      return false;
    };
    const r = await onCandidate({ url: "https://h.example/file.iso", source: "download" }, { cancelBrowserDownload: tooLate });
    expect(r).toMatchObject({ ok: false, reason: "browser-kept" });
    expect(events).toEqual(["cancel"]);
    expect(data.log.at(-1)).toMatchObject({ decision: "skipped", reason: "browser-kept" });

    expect((await onCandidate({ url: "https://h.example/file.iso", source: "download" }, { cancelBrowserDownload })).ok).toBe(true);
  });

  it("does not let a failed send block the next attempt", async () => {
    fetchImpl = async () => ({ ok: false, status: 400, text: async () => "bad" });
    expect((await onCandidate({ url: "https://h.example/file.iso", source: "content-script" })).reason).toBe("error");

    fetchImpl = async () => ok({});
    expect((await onCandidate({ url: "https://h.example/file.iso", source: "content-script" })).ok).toBe(true);
  });

  it("does not let a filter-skipped item block the next attempt", async () => {
    data.cfg.rules = { allowHttpDownloads: false };
    expect((await onCandidate({ url: "https://h.example/file.iso", source: "content-script" })).reason).toBe("http-disabled");

    data.cfg.rules = { allowHttpDownloads: true };
    expect((await onCandidate({ url: "https://h.example/file.iso", source: "content-script" })).ok).toBe(true);
  });

  it("still dedupes a repeat of a successful send", async () => {
    await onCandidate({ url: "https://h.example/file.iso", source: "content-script" });
    expect((await onCandidate({ url: "https://h.example/file.iso", source: "content-script" })).reason).toBe("duplicate");
  });

  it("restores the browser download when dload marks it Failed", async () => {
    vi.useFakeTimers();
    fetchImpl = async (_url, opts) =>
      opts.method === "POST"
        ? ok({ id: "d1", status: "Queued" })
        : ok([{ id: "d1", status: "Failed", error_message: "HTTP 403" }]);

    await onCandidate({ url: "https://h.example/file.iso", source: "download", filename: "file.iso" }, { cancelBrowserDownload });
    expect(restored).toEqual([]);

    await vi.advanceTimersByTimeAsync(2000);
    expect(restored).toEqual([{ url: "https://h.example/file.iso", filename: "file.iso" }]);
    expect(data.log.at(-1)).toMatchObject({ decision: "error", reason: expect.stringMatching(/HTTP 403.*restored/) });
  });

  it("stops watching once dload is downloading", async () => {
    vi.useFakeTimers();
    fetchImpl = async (_url, opts) =>
      opts.method === "POST"
        ? ok({ id: "d1", status: "Queued" })
        : ok([{ id: "d1", status: "Downloading", downloaded_size: 1024 }]);

    await onCandidate({ url: "https://h.example/file.iso", source: "download" }, { cancelBrowserDownload });
    await vi.advanceTimersByTimeAsync(30000);
    expect(events.filter((e) => e.startsWith("GET"))).toHaveLength(1);
    expect(restored).toEqual([]);
  });

  it("gives up watching after about 20s", async () => {
    vi.useFakeTimers();
    fetchImpl = async (_url, opts) =>
      opts.method === "POST" ? ok({ id: "d1", status: "Queued" }) : ok([{ id: "d1", status: "Queued" }]);

    await onCandidate({ url: "https://h.example/file.iso", source: "download" }, { cancelBrowserDownload });
    await vi.advanceTimersByTimeAsync(60000);
    expect(events.filter((e) => e.startsWith("GET"))).toHaveLength(10);
  });

  it("does not watch sends that did not cancel a browser download", async () => {
    vi.useFakeTimers();
    fetchImpl = async () => ok({ id: "d1", status: "Queued" });

    await onCandidate({ url: "magnet:?xt=urn:btih:abc", source: "content-script" });
    await vi.advanceTimersByTimeAsync(30000);
    expect(events.filter((e) => e.startsWith("GET"))).toHaveLength(0);
  });
});

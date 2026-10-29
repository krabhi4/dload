// tests/unit/background.test.mjs
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

let data;
let events;
let listeners;

function makeBrowser({ chromeDownloads = true, contextMenus = true } = {}) {
  const on = (name) => ({
    addListener: (fn) => {
      listeners[name] = fn;
    },
  });
  const browser = {
    runtime: {
      id: "ext-id",
      getURL: (p) => `chrome-extension://ext-id/${p}`,
      onInstalled: on("installed"),
      onMessage: on("message"),
    },
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
    alarms: {
      get: async () => null,
      create: async () => events.push("alarm-create"),
      clear: async () => events.push("alarm-clear"),
      onAlarm: on("alarm"),
    },
    notifications: { create: () => {} },
    permissions: { contains: async () => true },
    downloads: {
      cancel: async (id) => events.push(`cancel ${id}`),
      erase: async (q) => events.push(`erase ${q.id}`),
      download: async () => events.push("download"),
      ...(chromeDownloads ? { onDeterminingFilename: on("determine") } : { onCreated: on("created") }),
    },
  };
  if (contextMenus) {
    browser.contextMenus = { removeAll: async () => {}, create: () => {}, onClicked: on("menu") };
  }
  return browser;
}

async function loadBackground(opts) {
  globalThis.browser = makeBrowser(opts);
  vi.resetModules();
  await import("../../src/background/index.js");
}

describe("background/index.js", () => {
  let posts;

  beforeEach(() => {
    data = { cfg: { serverUrl: "http://dload:8080", apiKey: "tok" } };
    events = [];
    listeners = {};
    posts = [];
    globalThis.fetch = async (url, opts) => {
      events.push("post");
      posts.push(JSON.parse(opts.body));
      return { ok: true, status: 200, json: async () => ({}) };
    };
  });

  afterEach(() => {
    delete globalThis.fetch;
    delete globalThis.browser;
    delete globalThis.chrome;
  });

  describe("Chrome onDeterminingFilename", () => {
    it("lets a download the filters skip continue untouched", async () => {
      await loadBackground();
      const suggest = vi.fn(() => events.push("suggest"));
      listeners.determine({ id: 1, url: "https://h.example/page.html", filename: "page.html" }, suggest);

      await vi.waitFor(() => expect(suggest).toHaveBeenCalled());
      expect(suggest).toHaveBeenCalledWith();
      expect(events.filter((e) => e !== "alarm-clear")).toEqual(["suggest"]);
    });

    it("calls suggest() then cancels and erases a captured download", async () => {
      await loadBackground();
      const suggest = vi.fn(() => events.push("suggest"));
      listeners.determine({ id: 5, url: "https://h.example/file.iso", filename: "file.iso" }, suggest);

      await vi.waitFor(() => expect(events).toContain("post"));
      expect(suggest).toHaveBeenCalledWith();
      expect(events.filter((e) => e !== "alarm-clear")).toEqual(["suggest", "cancel 5", "erase 5", "post"]);
    });
  });

  describe("Firefox onCreated", () => {
    it("cancels a captured download and sends only the file's base name", async () => {
      await loadBackground({ chromeDownloads: false });
      listeners.created({
        id: 7,
        url: "https://h.example/file.iso",
        filename: "/home/u/Downloads/file.iso",
        fileSize: -1,
        totalBytes: 4096,
      });

      await vi.waitFor(() => expect(events).toContain("post"));
      expect(events).toContain("cancel 7");
      expect(posts[0].filename).toBe("file.iso");
    });

    it("applies size filters from totalBytes and leaves skipped downloads alone", async () => {
      data.cfg.rules = { minSizeBytes: 10000 };
      await loadBackground({ chromeDownloads: false });
      listeners.created({ id: 8, url: "https://h.example/file.iso", fileSize: -1, totalBytes: 4096 });

      await vi.waitFor(() => expect(data.log).toHaveLength(1));
      expect(data.log[0]).toMatchObject({ decision: "skipped", reason: "size-too-small" });
      expect(events.some((e) => e.startsWith("cancel"))).toBe(false);
    });

    it("ignores downloads this extension re-issued", async () => {
      await loadBackground({ chromeDownloads: false });
      listeners.created({ id: 9, url: "https://h.example/file.iso", byExtensionId: "ext-id" });

      await new Promise((r) => setTimeout(r, 20));
      expect(data.log).toBeUndefined();
      expect(events.some((e) => e.startsWith("cancel"))).toBe(false);
    });
  });

  it("still registers its other listeners when contextMenus is missing", async () => {
    await loadBackground({ contextMenus: false });
    expect(listeners.menu).toBeUndefined();
    expect(typeof listeners.message).toBe("function");
    expect(typeof listeners.determine).toBe("function");
  });

  it("creates the retry alarm at startup when the queue is not empty", async () => {
    data.retryQueue = [{ id: "a", payload: { url: "https://x" }, attempts: 1, nextAttemptAt: 0 }];
    await loadBackground();
    await vi.waitFor(() => expect(events).toContain("alarm-create"));
  });

  describe("popup-only messages", () => {
    it("are ignored when they come from a content script", async () => {
      await loadBackground();
      const sendResponse = vi.fn();
      const sender = { url: "https://evil.example/", tab: { id: 1 } };
      expect(listeners.message({ type: "from-popup-status" }, sender, sendResponse)).toBe(false);
      expect(listeners.message({ type: "from-popup-resend", url: "https://x/a.iso" }, sender, sendResponse)).toBe(false);
      expect(sendResponse).not.toHaveBeenCalled();
      expect(posts).toHaveLength(0);
    });

    it("are answered when they come from an extension page", async () => {
      await loadBackground();
      const sendResponse = vi.fn();
      const sender = { url: "chrome-extension://ext-id/src/popup/popup.html" };
      expect(listeners.message({ type: "from-popup-status" }, sender, sendResponse)).toBe(true);
      await vi.waitFor(() => expect(sendResponse).toHaveBeenCalledWith(expect.objectContaining({ type: "status" })));
    });
  });
});

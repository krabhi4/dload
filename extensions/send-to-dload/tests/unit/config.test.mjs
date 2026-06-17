// tests/unit/config.test.mjs
import { describe, it, expect, beforeEach, afterEach } from "vitest";
import { withDefaults, normalizeServerUrl } from "../../src/shared/config.js";

describe("withDefaults()", () => {
  it("returns defaults for empty input", () => {
    const d = withDefaults({});
    expect(d.serverUrl).toBe("");
    expect(d.rules.enabled).toBe(true);
    expect(d.rules.mode).toBe("blacklist");
    expect(d.rules.extensionList.include).toContain(".torrent");
    expect(d.rules.extensionList.exclude).toContain(".html");
  });

  it("preserves partial overrides", () => {
    const d = withDefaults({
      serverUrl: "http://localhost:8080",
      rules: { mode: "whitelist", hostnameList: ["example.com"] },
    });
    expect(d.serverUrl).toBe("http://localhost:8080");
    expect(d.rules.mode).toBe("whitelist");
    expect(d.rules.hostnameList).toEqual(["example.com"]);
    // defaults still present
    expect(d.rules.enabled).toBe(true);
  });

  it("deep-merges extensionList", () => {
    const d = withDefaults({
      rules: { extensionList: { include: [".foo"] } },
    });
    expect(d.rules.extensionList.include).toEqual([".foo"]);
    expect(d.rules.extensionList.exclude).toContain(".html");
  });
});

describe("normalizeServerUrl()", () => {
  it("trims and strips trailing slashes", () => {
    expect(normalizeServerUrl("  http://x:8080/  ")).toBe("http://x:8080");
    expect(normalizeServerUrl("http://x:8080///")).toBe("http://x:8080");
  });

  it("handles empty", () => {
    expect(normalizeServerUrl("")).toBe("");
    expect(normalizeServerUrl(null)).toBe("");
  });
});

describe("config / log storage separation", () => {
  let data;

  beforeEach(() => {
    data = {};
    globalThis.browser = {
      storage: {
        local: {
          get: async (keys) => {
            if (Array.isArray(keys)) {
              const out = {};
              for (const k of keys) out[k] = data[k];
              return out;
            }
            if (typeof keys === "string") return { [keys]: data[keys] };
            return { ...data };
          },
          set: async (obj) => {
            Object.assign(data, obj);
          },
        },
      },
    };
  });

  afterEach(() => {
    delete globalThis.browser;
  });

  it("logActivity writes the 'log' key only, never the 'cfg' object", async () => {
    const { saveConfig, logActivity } = await import("../../src/shared/config.js");
    await saveConfig({ serverUrl: "http://x:8080", apiKey: "tok" });
    await logActivity({ url: "u", decision: "sent" });

    expect(data.cfg.serverUrl).toBe("http://x:8080");
    expect(data.cfg).not.toHaveProperty("log");
    expect(data.log).toHaveLength(1);
  });

  it("interleaved log writes do not revert a concurrent settings save", async () => {
    const { saveConfig, logActivity, loadConfig } = await import("../../src/shared/config.js");
    await saveConfig({ serverUrl: "http://a", apiKey: "t1" });

    await Promise.all([
      logActivity({ url: "1", decision: "sent" }),
      saveConfig({ serverUrl: "http://b", apiKey: "t2" }),
      logActivity({ url: "2", decision: "sent" }),
    ]);

    const cfg = await loadConfig();
    expect(cfg.serverUrl).toBe("http://b");
    expect(cfg.apiKey).toBe("t2");
    expect(cfg.log).toHaveLength(2);
  });
});
// tests/unit/api.test.mjs
// Guards the dload server contract: /api/auth/verify deserializes Json<String>,
// so the token must be posted as a bare JSON string, NOT { token }.

import { describe, it, expect, beforeEach, afterEach } from "vitest";

describe("verifyToken()", () => {
  let calls;

  beforeEach(() => {
    calls = [];
    globalThis.browser = {
      permissions: { contains: async () => true, request: async () => true },
      storage: { local: { get: async () => ({}), set: async () => {} } },
    };
    globalThis.fetch = async (url, opts) => {
      calls.push({ url, opts });
      return {
        ok: true,
        status: 200,
        json: async () => ({ valid: true, username: "u", role: "ADMIN" }),
      };
    };
  });

  afterEach(() => {
    delete globalThis.fetch;
    delete globalThis.browser;
  });

  it("posts the token as a bare JSON string and hits /api/auth/verify", async () => {
    const { verifyToken } = await import("../../src/background/api.js");
    const r = await verifyToken("http://x:8080/", "tok123");

    expect(calls).toHaveLength(1);
    expect(calls[0].url).toBe("http://x:8080/api/auth/verify");
    expect(calls[0].opts.body).toBe(JSON.stringify("tok123"));
    expect(JSON.parse(calls[0].opts.body)).toBe("tok123");
    expect(r.valid).toBe(true);
  });

  it("throws a clear error (not a raw fetch) when host permission is not granted", async () => {
    globalThis.browser.permissions.contains = async () => false;
    globalThis.browser.permissions.request = async () => false;
    const { verifyToken } = await import("../../src/background/api.js");
    await expect(verifyToken("http://x:8080/", "tok")).rejects.toThrow(/permission/i);
    expect(calls).toHaveLength(0); // never reached fetch
  });
});

describe("sendToDload() error classification", () => {
  let fetchImpl;

  beforeEach(() => {
    globalThis.browser = {
      permissions: { contains: async () => true },
      storage: {
        local: {
          get: async () => ({ cfg: { serverUrl: "http://x:8080", apiKey: "tok" } }),
          set: async () => {},
        },
      },
    };
    globalThis.fetch = async (...args) => fetchImpl(...args);
  });

  afterEach(() => {
    delete globalThis.fetch;
    delete globalThis.browser;
  });

  async function send() {
    const { sendToDload } = await import("../../src/background/api.js");
    return sendToDload({ url: "https://example.com/x.iso" });
  }

  it("marks 401 as permanent (don't retry a bad token)", async () => {
    fetchImpl = async () => ({ ok: false, status: 401 });
    const err = await send().catch((e) => e);
    expect(err.permanent).toBe(true);
  });

  it("marks other 4xx as permanent", async () => {
    fetchImpl = async () => ({ ok: false, status: 400, text: async () => "Invalid URL" });
    const err = await send().catch((e) => e);
    expect(err.permanent).toBe(true);
  });

  it("treats 5xx as transient", async () => {
    fetchImpl = async () => ({ ok: false, status: 503, text: async () => "down" });
    const err = await send().catch((e) => e);
    expect(err.permanent).toBeFalsy();
  });

  it("treats a network error as transient", async () => {
    fetchImpl = async () => {
      throw new Error("connection refused");
    };
    const err = await send().catch((e) => e);
    expect(err.permanent).toBeFalsy();
    expect(err.message).toMatch(/Network error/);
  });
});

// tests/unit/retry.test.mjs
// Exercises the pure backoff math + the tick() orchestration against a fake
// chrome.storage.local + browser.alarms (we stub globalThis.browser before
// importing the module).

import { describe, it, expect, beforeEach, vi } from "vitest";

function makeStorage(initial = {}) {
  const data = { ...initial };
  return {
    get: async (key) => (typeof key === "string" ? { [key]: data[key] } : data),
    set: async (obj) => {
      Object.assign(data, obj);
    },
    _data: data,
  };
}

function makeAlarms() {
  return {
    get: async () => null,
    create: async () => {},
  };
}

/**
 * Install a fresh browser stub into globalThis and reset the retry module's
 * cached `browser` reference. The module evaluates `browser` at the top, so
 * resetting modules + re-importing is the cleanest way to point it at the new
 * stub.
 */
async function loadRetryModuleWith({ storage, alarms }) {
  globalThis.browser = {
    storage: { local: storage },
    alarms,
  };
  vi.resetModules();
  return await import("../../src/background/retry.js");
}

describe("retry queue", () => {
  let retry;

  beforeEach(async () => {
    const storage = makeStorage();
    const alarms = makeAlarms();
    retry = await loadRetryModuleWith({ storage, alarms });
    // Stash the storage/alarms on the module for tests that need them.
    retry.__test_storage = storage;
    retry.__test_alarms = alarms;
  });

  it("enqueue persists one item and schedules alarm", async () => {
    await retry.enqueue({ url: "https://x" }, "boom");
    const queue = await retry.__test_storage.get("retryQueue");
    expect(queue.retryQueue).toHaveLength(1);
    expect(queue.retryQueue[0].attempts).toBe(1);
    expect(queue.retryQueue[0].lastError).toBe("boom");
  });

  it("peek returns items whose nextAttemptAt has passed", async () => {
    await retry.enqueue({ url: "https://x" }, null);
    const data = retry.__test_storage._data;
    data.retryQueue[0].nextAttemptAt = Date.now() - 1;
    const due = await retry.peek();
    expect(due).toHaveLength(1);
  });

  it("tick removes items that succeed", async () => {
    await retry.enqueue({ url: "https://x" }, null);
    const data = retry.__test_storage._data;
    data.retryQueue[0].nextAttemptAt = 0;

    const stats = await retry.tick(async () => "ok");
    expect(stats).toEqual({ tried: 1, succeeded: 1, failed: 0 });
    expect(await retry.size()).toBe(0);
  });

  it("tick bumps attempts on failure and schedules a later retry", async () => {
    await retry.enqueue({ url: "https://x" }, "first");
    const data = retry.__test_storage._data;
    data.retryQueue[0].nextAttemptAt = 0;

    const stats = await retry.tick(async () => {
      throw new Error("nope");
    });
    expect(stats.failed).toBe(1);
    const queue = await retry.__test_storage.get("retryQueue");
    expect(queue.retryQueue[0].attempts).toBe(2);
    expect(queue.retryQueue[0].lastError).toMatch(/nope/);
    expect(queue.retryQueue[0].nextAttemptAt).toBeGreaterThan(Date.now());
  });

  it("tick drops items after MAX_ATTEMPTS", async () => {
    await retry.enqueue({ url: "https://x" }, null);
    const data = retry.__test_storage._data;
    data.retryQueue[0].attempts = 10;
    data.retryQueue[0].nextAttemptAt = 0;

    const stats = await retry.tick(async () => {
      throw new Error("nope");
    });
    expect(stats.failed).toBe(1);
    expect(await retry.size()).toBe(0);
  });

  it("remove() drops by id", async () => {
    await retry.enqueue({ url: "https://a" }, null);
    await retry.enqueue({ url: "https://b" }, null);
    const data = retry.__test_storage._data;
    await retry.remove(data.retryQueue[0].id);
    expect(await retry.size()).toBe(1);
  });

  it("skips an overlapping tick while one is still in flight", async () => {
    await retry.enqueue({ url: "https://x" }, null);
    retry.__test_storage._data.retryQueue[0].nextAttemptAt = 0;

    let resolveSend;
    let markSendCalled;
    const sendCalled = new Promise((r) => { markSendCalled = r; });
    const sendFn = () => {
      markSendCalled();
      return new Promise((res) => { resolveSend = res; });
    };

    const inflight = retry.tick(sendFn); // grabs the in-flight lock
    await sendCalled; // first tick is now parked inside sendFn, lock held

    const second = await retry.tick(sendFn); // must be refused, not double-send
    expect(second.skipped).toBe(true);
    expect(second.tried).toBe(0);

    resolveSend();
    const first = await inflight;
    expect(first.succeeded).toBe(1);
    expect(await retry.size()).toBe(0);
  });
});
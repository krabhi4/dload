// tests/unit/content.test.mjs
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

describe("content script click handler", () => {
  let onClick;
  let sendMessage;

  beforeEach(async () => {
    const anchor = {
      href: "magnet:?xt=urn:btih:abc",
      textContent: "abc",
      addEventListener: (_type, fn) => {
        onClick = fn;
      },
    };
    globalThis.document = { querySelectorAll: () => [anchor], documentElement: {} };
    globalThis.MutationObserver = class {
      observe() {}
    };
    sendMessage = vi.fn(async () => ({ ok: true }));
    globalThis.browser = { runtime: { sendMessage } };
    vi.resetModules();
    await import("../../src/content/content.js");
  });

  afterEach(() => {
    delete globalThis.document;
    delete globalThis.MutationObserver;
    delete globalThis.browser;
  });

  function click(isTrusted) {
    const e = { isTrusted, button: 0, preventDefault: vi.fn(), stopImmediatePropagation: vi.fn() };
    onClick(e);
    return e;
  }

  it("ignores synthetic clicks", () => {
    const e = click(false);
    expect(sendMessage).not.toHaveBeenCalled();
    expect(e.preventDefault).not.toHaveBeenCalled();
  });

  it("forwards real user clicks", () => {
    const e = click(true);
    expect(sendMessage).toHaveBeenCalledWith(expect.objectContaining({ href: "magnet:?xt=urn:btih:abc" }));
    expect(e.preventDefault).toHaveBeenCalled();
  });
});

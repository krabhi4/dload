// shared/lock.js — serializes async read-modify-write sections in one JS context.
// chrome.storage get/set across awaits isn't atomic, so concurrent writers can
// clobber each other; chaining them on one promise makes them run one at a time.

let chain = Promise.resolve();

// Runs fn after all previously scheduled sections settle. Returns fn's result;
// a rejection doesn't poison the chain for later callers.
export function runExclusive(fn) {
  const result = chain.then(fn, fn);
  chain = result.then(
    () => {},
    () => {},
  );
  return result;
}

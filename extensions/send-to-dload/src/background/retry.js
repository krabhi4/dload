// background/retry.js — durable retry queue on chrome.storage.local + chrome.alarms.
// Items { id, payload, attempts, nextAttemptAt, lastError } are re-POSTed via a
// supplied sendFn with exponential backoff (30s → 6h), capped at MAX_ATTEMPTS.

import { logger } from "../shared/logger.js";
import { runExclusive } from "../shared/lock.js";

const QUEUE_KEY = "retryQueue";
const ALARM_NAME = "send-to-dload-retry";
const ALARM_PERIOD_MIN = 1; // Chrome's minimum alarm period
const MAX_ATTEMPTS = 10;
const BATCH_SIZE = 10;

const BACKOFF_MS = [
  30 * 1000,
  60 * 1000,
  5 * 60 * 1000,
  15 * 60 * 1000,
  60 * 60 * 1000,
  6 * 60 * 60 * 1000,
];

function backoffMs(attemptIndex) {
  if (attemptIndex < 0) return BACKOFF_MS[0];
  if (attemptIndex >= BACKOFF_MS.length) return BACKOFF_MS[BACKOFF_MS.length - 1];
  return BACKOFF_MS[attemptIndex];
}

async function readQueue() {
  try {
    const got = await browser.storage.local.get(QUEUE_KEY);
    return Array.isArray(got[QUEUE_KEY]) ? got[QUEUE_KEY] : [];
  } catch (err) {
    logger.warn("readQueue failed:", err);
    return [];
  }
}

async function writeQueue(queue) {
  await browser.storage.local.set({ [QUEUE_KEY]: queue });
}

// Serialized read-modify-write so concurrent enqueue/remove/tick can't clobber
// each other. `mutator(queue)` returns the next queue (may mutate in place).
async function mutateQueue(mutator) {
  return runExclusive(async () => {
    const queue = await readQueue();
    const next = mutator(queue) || queue;
    await writeQueue(next);
    return next;
  });
}

export async function ensureAlarm() {
  try {
    const existing = await browser.alarms.get(ALARM_NAME);
    if (!existing) {
      await browser.alarms.create(ALARM_NAME, { periodInMinutes: ALARM_PERIOD_MIN });
    }
  } catch (err) {
    logger.warn("ensureAlarm failed:", err);
  }
}

export async function enqueue(payload, lastError) {
  await mutateQueue((queue) => {
    queue.push({
      id: cryptoRandomId(),
      payload,
      attempts: 1,
      nextAttemptAt: Date.now() + backoffMs(0),
      lastError: lastError ? String(lastError).slice(0, 200) : null,
    });
    return queue;
  });
  await ensureAlarm();
}

export async function peek() {
  const queue = await readQueue();
  const now = Date.now();
  return queue.filter((it) => it.nextAttemptAt <= now).slice(0, BATCH_SIZE);
}

export async function remove(id) {
  await mutateQueue((queue) => queue.filter((it) => it.id !== id));
}

export async function size() {
  return (await readQueue()).length;
}

// Prevents overlapping ticks (a slow sendFn outlasting the 1-min alarm) from
// peeking the same due items and double-sending. In-memory is enough: the SW
// stays alive while the tick promise is pending, and resets the flag if killed.
let ticking = false;

/** Run one tick: POST every due item, removing on success and backing off on
 *  failure. Returns { tried, succeeded, failed }. */
export async function tick(sendFn) {
  if (ticking) return { tried: 0, succeeded: 0, failed: 0, skipped: true };
  ticking = true;
  try {
    const due = await peek();
    if (due.length === 0) return { tried: 0, succeeded: 0, failed: 0 };

    let succeeded = 0;
    let failed = 0;

    for (const item of due) {
      try {
        await sendFn(item.payload);
        await mutateQueue((queue) => queue.filter((it) => it.id !== item.id));
        succeeded++;
      } catch (err) {
        failed++;
        const msg = String(err && err.message ? err.message : err).slice(0, 200);
        await mutateQueue((queue) => {
          const idx = queue.findIndex((it) => it.id === item.id);
          if (idx < 0) return queue;
          if (queue[idx].attempts >= MAX_ATTEMPTS) {
            logger.error("retry: giving up on", item.id, "after", queue[idx].attempts, "attempts");
            return queue.filter((it) => it.id !== item.id);
          }
          queue[idx].attempts++;
          queue[idx].lastError = msg;
          queue[idx].nextAttemptAt = Date.now() + backoffMs(queue[idx].attempts - 1);
          return queue;
        });
      }
    }

    return { tried: due.length, succeeded, failed };
  } finally {
    ticking = false;
  }
}

function cryptoRandomId() {
  if (globalThis.crypto && globalThis.crypto.randomUUID) {
    return globalThis.crypto.randomUUID();
  }
  return Math.random().toString(36).slice(2);
}
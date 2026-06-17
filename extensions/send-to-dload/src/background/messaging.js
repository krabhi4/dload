// background/messaging.js
// Typed wrappers around runtime.sendMessage / onMessage used by content scripts
// and the popup. Centralizes the message names so a typo doesn't cost a
// silent failure.

export const MSG = Object.freeze({
  FROM_CONTENT_MAGNET: "from-content-magnet",
  FROM_POPUP_TEST: "from-popup-test",
  FROM_POPUP_STATUS: "from-popup-status",
  FROM_POPUP_RESEND: "from-popup-resend",
});

export const REPLY = Object.freeze({
  TEST_OK: "test-ok",
  TEST_FAIL: "test-fail",
  STATUS: "status",
  RESEND_OK: "resend-ok",
  RESEND_FAIL: "resend-fail",
});
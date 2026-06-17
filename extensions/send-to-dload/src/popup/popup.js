// popup/popup.js
// Reads config + last 10 log entries, exposes a "Send this page" button,
// and links to the Options page.

import { MSG } from "../background/messaging.js";

const $ = (sel) => document.querySelector(sel);

function setStatus(kind, text) {
  const el = $("#status");
  el.className = `pill pill-${kind}`;
  el.textContent = text;
}

function renderLog(entries) {
  const list = $("#recent-list");
  list.replaceChildren();
  if (!entries || entries.length === 0) {
    const li = document.createElement("li");
    li.className = "muted";
    li.textContent = "(no activity yet)";
    list.appendChild(li);
    return;
  }
  for (const e of entries) {
    const li = document.createElement("li");
    const when = new Date(e.ts).toLocaleTimeString();
    const decisionClass = ({
      sent: "decision-sent",
      skipped: "decision-skip",
      error: "decision-err",
      "queued-retry": "decision-queue",
    })[e.decision] || "decision-skip";

    const url = String(e.url || "").slice(0, 60);
    const whenSpan = document.createElement("span");
    whenSpan.className = "when";
    whenSpan.textContent = when;
    const decisionSpan = document.createElement("span");
    decisionSpan.className = decisionClass;
    decisionSpan.textContent = e.decision || "";
    const urlText = document.createTextNode(" " + url);
    li.append(whenSpan, decisionSpan, urlText);
    list.appendChild(li);
  }
}

async function refresh() {
  try {
    const reply = await browser.runtime.sendMessage({ type: MSG.FROM_POPUP_STATUS });
    if (!reply || reply.error) {
      setStatus("err", "error");
      return;
    }
    if (!reply.configured) {
      setStatus("err", "not configured");
      $("#unconfigured").hidden = false;
      $("#form").hidden = true;
    } else {
      setStatus("ok", "ready");
      $("#unconfigured").hidden = true;
      $("#form").hidden = false;
    }
    renderLog(reply.last || []);
  } catch (err) {
    setStatus("err", "background unreachable");
    console.warn("[send-to-dload] popup refresh failed:", err);
  }
}

async function sendCurrentPage() {
  const btn = $("#send-page");
  btn.disabled = true;
  const result = $("#send-result");
  result.hidden = true;

  let tabs;
  try {
    tabs = await browser.tabs.query({ active: true, currentWindow: true });
  } catch {
    result.hidden = false;
    result.className = "result err";
    result.textContent = "Could not read the current tab.";
    btn.disabled = false;
    return;
  }

  const tab = tabs && tabs[0];
  if (!tab || !tab.url) {
    result.hidden = false;
    result.className = "result err";
    result.textContent = "No URL on the current tab.";
    btn.disabled = false;
    return;
  }

  try {
    const r = await browser.runtime.sendMessage({
      type: MSG.FROM_POPUP_RESEND,
      url: tab.url,
      filename: tab.title || undefined,
    });
    if (r && r.type === "resend-ok") {
      result.hidden = false;
      result.className = "result ok";
      result.textContent = "Sent to dload.";
    } else {
      result.hidden = false;
      result.className = "result err";
      result.textContent = describeFailure(r);
    }
  } catch (err) {
    result.hidden = false;
    result.className = "result err";
    result.textContent = err && err.message ? err.message : String(err);
  } finally {
    btn.disabled = false;
    await refresh();
  }
}

function describeFailure(r) {
  const reason = r && r.reason;
  const friendly = {
    "not-configured": "Not configured — open Settings.",
    queued: "Server unreachable — queued for retry.",
    duplicate: "Already sent moments ago.",
    disabled: "Extension is disabled.",
    "unsupported-scheme": "This page URL can't be sent to dload.",
  };
  if (reason && friendly[reason]) return friendly[reason];
  return (r && (r.error || (reason ? `Skipped (${reason}).` : null))) || "Failed.";
}

function openOptions() {
  const url = browser.runtime.getURL("src/options/options.html");
  if (browser.tabs && browser.tabs.create) {
    browser.tabs.create({ url });
  } else {
    window.open(url, "_blank");
  }
}

document.addEventListener("DOMContentLoaded", () => {
  $("#open-options").addEventListener("click", (e) => {
    e.preventDefault();
    openOptions();
  });
  $("#open-options-2").addEventListener("click", (e) => {
    e.preventDefault();
    openOptions();
  });
  $("#send-page").addEventListener("click", sendCurrentPage);
  refresh();
});
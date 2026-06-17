// options/options.js — reads/writes config and renders the activity log.

import { loadConfig, saveConfig, clearLog } from "../shared/config.js";
import { MSG } from "../background/messaging.js";

const $ = (sel) => document.querySelector(sel);

// Must run from a user gesture — Firefox rejects permissions.request() otherwise.
async function ensureServerPermission(serverUrl) {
  try {
    const origin = new URL(serverUrl).origin + "/*";
    if (await browser.permissions.contains({ origins: [origin] })) return true;
    return await browser.permissions.request({ origins: [origin] });
  } catch (err) {
    console.warn("[send-to-dload] permission request failed:", err);
    return false;
  }
}

function parseList(text) {
  return String(text || "")
    .split(/[\n,]+/g)
    .map((s) => s.trim())
    .filter(Boolean);
}

function renderList(arr) {
  return (arr || []).join("\n");
}

async function load() {
  const cfg = await loadConfig();

  $("#serverUrl").value = cfg.serverUrl || "";
  $("#apiKey").value = cfg.apiKey || "";
  $("#enabled").checked = cfg.rules.enabled !== false;
  $("#mode").value = cfg.rules.mode || "blacklist";
  $("#hostnameList").value = renderList(cfg.rules.hostnameList);
  $("#extensionInclude").value = renderList(cfg.rules.extensionList.include);
  $("#extensionExclude").value = renderList(cfg.rules.extensionList.exclude);
  $("#minSize").value = cfg.rules.minSizeBytes || 0;
  $("#maxSize").value = cfg.rules.maxSizeBytes || 0;
  $("#allowMagnets").checked = cfg.rules.allowMagnets !== false;
  $("#allowHttpDownloads").checked = cfg.rules.allowHttpDownloads !== false;
  $("#notifyOnSend").checked = cfg.rules.notifyOnSend !== false;
  $("#notifyOnSkip").checked = cfg.rules.notifyOnSkip === true;

  renderLog(cfg.log || []);
}

function renderLog(entries) {
  const tbody = $("#log-body");
  tbody.replaceChildren();
  if (!entries.length) {
    const tr = document.createElement("tr");
    const td = document.createElement("td");
    td.colSpan = 5;
    td.style.color = "var(--muted)";
    td.textContent = "No activity yet.";
    tr.appendChild(td);
    tbody.appendChild(tr);
    return;
  }
  for (const e of entries.slice().reverse()) {
    const tr = document.createElement("tr");
    const decisionClass = ({
      sent: "decision-sent",
      skipped: "decision-skip",
      error: "decision-err",
      "queued-retry": "decision-queue",
    })[e.decision] || "decision-skip";

    const cells = [
      new Date(e.ts).toLocaleString(),
      e.kind || "",
      truncate(e.url, 200),
    ];
    for (const txt of cells) {
      const td = document.createElement("td");
      if (txt === cells[2]) td.className = "url-cell";
      td.textContent = txt;
      tr.appendChild(td);
    }
    const tdDecision = document.createElement("td");
    tdDecision.className = decisionClass;
    tdDecision.textContent = e.decision || "";
    tr.appendChild(tdDecision);
    const tdReason = document.createElement("td");
    tdReason.textContent = e.reason || "";
    tr.appendChild(tdReason);

    tbody.appendChild(tr);
  }
}

async function save() {
  const partial = {
    serverUrl: $("#serverUrl").value.trim(),
    apiKey: $("#apiKey").value.trim(),
    rules: {
      enabled: $("#enabled").checked,
      mode: $("#mode").value,
      hostnameList: parseList($("#hostnameList").value),
      extensionList: {
        include: parseList($("#extensionInclude").value),
        exclude: parseList($("#extensionExclude").value),
      },
      minSizeBytes: Number($("#minSize").value) || 0,
      maxSizeBytes: Number($("#maxSize").value) || 0,
      allowMagnets: $("#allowMagnets").checked,
      allowHttpDownloads: $("#allowHttpDownloads").checked,
      notifyOnSend: $("#notifyOnSend").checked,
      notifyOnSkip: $("#notifyOnSkip").checked,
    },
  };
  if (partial.serverUrl) {
    await ensureServerPermission(partial.serverUrl);
  }
  await saveConfig(partial);
  const result = $("#save-result");
  result.textContent = "Saved.";
  result.className = "result ok";
  setTimeout(() => {
    result.textContent = "";
    result.className = "result";
  }, 1800);
}

async function test() {
  const result = $("#test-result");
  const url = $("#serverUrl").value.trim();
  const apiKey = $("#apiKey").value.trim();
  if (!url || !apiKey) {
    result.textContent = "Server URL and API key are both required.";
    result.className = "result err";
    return;
  }
  result.textContent = "Testing…";
  result.className = "result";
  if (!(await ensureServerPermission(url))) {
    result.textContent = "Site-access permission was denied. Allow access to the server, then retry.";
    result.className = "result err";
    return;
  }
  try {
    const reply = await browser.runtime.sendMessage({
      type: MSG.FROM_POPUP_TEST,
      serverUrl: url,
      apiKey,
    });
    if (reply && reply.type === "test-ok" && reply.result && reply.result.valid) {
      const v = reply.result;
      result.textContent = `Connected as ${v.username || "?"} (${v.role || "?"}).`;
      result.className = "result ok";
    } else if (reply && reply.type === "test-ok") {
      result.textContent = "Token rejected by dload.";
      result.className = "result err";
    } else {
      result.textContent = (reply && reply.error) || "Connection failed.";
      result.className = "result err";
    }
  } catch (err) {
    result.textContent = err && err.message ? err.message : String(err);
    result.className = "result err";
  }
}

async function onClearLog() {
  await clearLog();
  await load();
}

function truncate(s, n) {
  s = String(s || "");
  return s.length > n ? s.slice(0, n) + "…" : s;
}

document.addEventListener("DOMContentLoaded", () => {
  $("#save").addEventListener("click", save);
  $("#test").addEventListener("click", test);
  $("#clear-log").addEventListener("click", onClearLog);
  load();
});
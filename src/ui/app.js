var API_BASE = "/api";
var token = localStorage.getItem("dload_token") || "";
var refreshInterval = null;
var lastDownloads = [];
var openMenuId = null;
var openMoreMenuId = null;
var expandedId = null;

// ─── Toast Notifications ────────────────────────────

var toastCounter = 0;

function showToast(type, title, message, duration) {
  duration = duration || 5000;
  var container = document.getElementById("toast-container");
  if (!container) return;

  var id = "toast-" + ++toastCounter;
  var icons = {
    error:
      '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>',
    success:
      '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 11-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/></svg>',
    info: '<svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>',
  };

  var toast = document.createElement("div");
  toast.className = "toast " + type;
  toast.id = id;
  toast.innerHTML =
    '<div class="toast-icon">' +
    (icons[type] || icons.info) +
    "</div>" +
    '<div class="toast-body">' +
    '<div class="toast-title">' +
    escapeHtml(title) +
    "</div>" +
    (message
      ? '<div class="toast-message">' + escapeHtml(message) + "</div>"
      : "") +
    "</div>" +
    '<button class="toast-close" onclick="dismissToast(\'' +
    id +
    "')\">" +
    '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>' +
    "</button>" +
    '<div class="toast-progress" style="animation-duration: ' +
    duration +
    'ms"></div>';

  container.appendChild(toast);

  setTimeout(function () {
    dismissToast(id);
  }, duration);
}

function dismissToast(id) {
  var toast = document.getElementById(id);
  if (!toast || toast.classList.contains("removing")) return;
  toast.classList.add("removing");
  setTimeout(function () {
    if (toast.parentNode) toast.parentNode.removeChild(toast);
  }, 250);
}

// ─── API ────────────────────────────────────────────

async function apiRequest(endpoint, options) {
  options = options || {};
  var headers = {
    "Content-Type": "application/json",
  };
  if (token) {
    headers["Authorization"] = "Bearer " + token;
  }
  if (options.headers) {
    var k;
    for (k in options.headers) {
      if (options.headers.hasOwnProperty(k)) {
        headers[k] = options.headers[k];
      }
    }
  }

  var response = await fetch(API_BASE + endpoint, {
    method: options.method || "GET",
    headers: headers,
    body: options.body || undefined,
  });

  if (response.status === 401) {
    logout();
    throw new Error("Unauthorized");
  }

  if (!response.ok) {
    var errText = await response.text().catch(function () {
      return "Unknown error";
    });
    throw new Error(errText || "Request failed (" + response.status + ")");
  }

  return response.json();
}

// ─── Safe DOM helpers ───────────────────────────────

function escapeHtml(str) {
  var div = document.createElement("div");
  div.textContent = str;
  return div.innerHTML;
}

function safeRender(container, html) {
  container.innerHTML = html;
}

// ─── Formatting ─────────────────────────────────────

function formatSize(bytes) {
  if (!bytes || bytes === 0) return "0 B";
  var k = 1024;
  var sizes = ["B", "KB", "MB", "GB", "TB"];
  var i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatSpeed(bytesPerSec) {
  if (!bytesPerSec || bytesPerSec === 0) return "0 B/s";
  return formatSize(bytesPerSec) + "/s";
}

// ─── Views ──────────────────────────────────────────

var currentPage = "downloads";

function showDownloads() {
  return (
    '<div class="stats-bar">' +
    '<span class="stat-val" id="active-count">0</span> Active' +
    ' <span class="stat-sep">&middot;</span> ' +
    '&darr; <span class="stat-val speed-val" id="total-speed">0 B/s</span>' +
    '<button id="resume-all-btn" class="btn-ghost" style="display:none;margin-left:auto;font-size:0.8em;padding:4px 10px" onclick="resumeAllDownloads()">Resume All</button>' +
    "</div>" +
    '<div class="add-section">' +
    '<form id="add-download-form" class="input-group">' +
    '<input type="text" id="download-url" placeholder="Paste URL — http, ftp, sftp, magnet, or .torrent" required>' +
    '<button type="submit" class="btn-primary">Download</button>' +
    "</form>" +
    "</div>" +
    '<div id="downloads-list"></div>'
  );
}

function showCompleted() {
  return (
    '<div class="page-header">' +
    "<h1>Completed</h1>" +
    "<p>Finished downloads</p>" +
    "</div>" +
    '<div id="downloads-list"></div>'
  );
}

var selectedHistoryIds = new Set();

function showHistory() {
  return (
    '<div class="page-header history-header">' +
    "<div>" +
    "<h1>History</h1>" +
    "<p>All downloads</p>" +
    "</div>" +
    '<div class="history-actions" id="history-actions">' +
    '<button class="btn-danger-outline btn-small" id="delete-selected-btn" style="display:none" onclick="deleteSelectedHistory()">Delete Selected</button>' +
    '<button class="btn-ghost btn-small" onclick="clearAllHistory()">Clear All</button>' +
    "</div>" +
    "</div>" +
    '<div id="history-list"></div>'
  );
}

function showSettings() {
  return (
    '<div class="page-header">' +
    "<h1>Settings</h1>" +
    "<p>Configure your download manager</p>" +
    "</div>" +
    '<form id="settings-form" class="settings-grid">' +
    '<div class="form-field">' +
    '<label for="settings-dir">Download Directory</label>' +
    '<div class="input-group">' +
    '<input type="text" id="settings-dir" value="/downloads">' +
    '<button type="button" class="btn-ghost" onclick="openFolderBrowser()">Browse</button>' +
    "</div>" +
    '<span class="hint">Path inside the container where files are saved</span>' +
    "</div>" +
    '<div class="form-field">' +
    '<label for="settings-max-concurrent">Max Concurrent Downloads</label>' +
    '<input type="number" id="settings-max-concurrent" value="3" min="1" max="10">' +
    "</div>" +
    '<div class="form-field">' +
    '<label for="settings-connections">Max Connections Per File</label>' +
    '<input type="number" id="settings-connections" value="4" min="1" max="16">' +
    "</div>" +
    '<div class="settings-actions">' +
    '<button type="submit" class="btn-primary">Save Settings</button>' +
    "</div>" +
    "</form>" +
    '<div id="user-management-section"></div>'
  );
}

function showProfile() {
  return (
    '<div class="page-header">' +
    "<h1>Profile</h1>" +
    "<p>Manage your account settings</p>" +
    "</div>" +
    '<div class="profile-section">' +
    '<div class="form-field">' +
    "<label>Username</label>" +
    '<input type="text" id="profile-username" readonly>' +
    "</div>" +
    '<div class="form-field">' +
    "<label>Role</label>" +
    '<input type="text" id="profile-role" readonly>' +
    "</div>" +
    '<div class="form-field">' +
    "<label>Member Since</label>" +
    '<input type="text" id="profile-created" readonly>' +
    "</div>" +
    "<hr>" +
    "<h2>Change Password</h2>" +
    '<form id="change-password-form">' +
    '<div class="form-field">' +
    '<label for="current-password">Current Password</label>' +
    '<input type="password" id="current-password" required>' +
    "</div>" +
    '<div class="form-field">' +
    '<label for="new-password">New Password</label>' +
    '<input type="password" id="new-password" required>' +
    "</div>" +
    '<div class="form-field">' +
    '<label for="confirm-password">Confirm New Password</label>' +
    '<input type="password" id="confirm-password" required>' +
    "</div>" +
    '<button type="submit" class="btn-primary">Update Password</button>' +
    "</form>" +
    "</div>"
  );
}

// ─── Filter & Detail ────────────────────────────────

function sortDownloads(downloads) {
  var statusOrder = {
    Downloading: 0,
    Seeding: 1,
    Queued: 2,
    Paused: 3,
    Stopped: 4,
    Failed: 5,
    Completed: 6,
  };
  return downloads.slice().sort(function (a, b) {
    var oa = statusOrder[a.status] !== undefined ? statusOrder[a.status] : 99;
    var ob = statusOrder[b.status] !== undefined ? statusOrder[b.status] : 99;
    if (oa !== ob) return oa - ob;
    // Within same status, newest first
    return new Date(b.created_at) - new Date(a.created_at);
  });
}

function getPageDownloads(downloads) {
  var sorted = sortDownloads(downloads);
  if (currentPage === "completed") {
    return sorted.filter(function (d) {
      return d.status === "Completed";
    });
  }
  // Downloads page: everything except completed
  return sorted.filter(function (d) {
    return d.status !== "Completed";
  });
}

function toggleDetail(id, event) {
  if (event.target.closest('button') || event.target.closest('.actions') || event.target.closest('.delete-dropdown') || event.target.closest('.more-dropdown')) {
    return;
  }
  if (expandedId === id) {
    expandedId = null;
  } else {
    expandedId = id;
  }
  document.querySelectorAll('.download-detail').forEach(function (el) {
    el.classList.remove('open');
  });
  if (expandedId !== null) {
    var detail = document.getElementById('detail-' + expandedId);
    if (detail) detail.classList.add('open');
  }
}

// ─── Rendering ──────────────────────────────────────

function buildDownloadItem(d) {
  var statusClass = escapeHtml(d.status.toLowerCase());
  var progressClass =
    statusClass === "completed"
      ? "completed"
      : statusClass === "failed"
        ? "failed"
        : "";
  var safeId = escapeHtml(d.id);
  var safeName = escapeHtml(d.filename);
  var safeUrl = escapeHtml(d.url);
  var safeProtocol = escapeHtml(d.protocol);
  var safeStatus = escapeHtml(d.status);
  var progress = Math.min(d.progress, 100);
  var isActive = d.status === "Downloading";
  var isTorrent = d.protocol === "Torrent";
  var canMirror = isTorrent && !d.http_mirror_status
      && (safeStatus === 'Downloading' || safeStatus === 'Paused' || safeStatus === 'Seeding');

  if (d.status === "Completed" && d.total_size === 0 && d.downloaded_size > 0) {
    progress = 100;
  }

  // Protocol icon
  var protocolIcon;
  if (isTorrent) {
    protocolIcon =
      '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">' +
      '<path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2z"/>' +
      '<path d="M8 12l2 2 4-4"/>' +
      "</svg>";
  } else {
    protocolIcon =
      '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">' +
      '<circle cx="12" cy="12" r="10"/>' +
      '<path d="M2 12h20M12 2a15.3 15.3 0 014 10 15.3 15.3 0 01-4 10 15.3 15.3 0 01-4-10 15.3 15.3 0 014-10z"/>' +
      "</svg>";
  }

  // Size display
  var sizeDisplay;
  if (d.status === "Completed") {
    sizeDisplay = formatSize(d.total_size || d.downloaded_size);
  } else if (isActive && d.total_size > 0) {
    sizeDisplay =
      formatSize(d.downloaded_size) + " / " + formatSize(d.total_size);
  } else {
    sizeDisplay = formatSize(d.downloaded_size);
  }

  // Speed display
  var isSeeding = d.status === "Seeding";
  var displaySpeed = isActive
    ? formatSpeed(d.speed)
    : isSeeding
      ? "\u2191 " + formatSpeed(d.upload_speed)
      : "--";

  // Connections display
  var connDisplay = "";
  if (isActive || isSeeding) {
    if (isTorrent) {
      connDisplay = isSeeding
        ? (d.seeds || 0) + " seed" + ((d.seeds || 0) !== 1 ? "s" : "")
        : (d.peers || 0) + " peer" + ((d.peers || 0) !== 1 ? "s" : "");
    } else {
      connDisplay = (d.connections || 1) + " conn";
    }
  }

  // ETA display
  var etaDisplay = isActive && d.eta ? escapeHtml(d.eta) : "";

  // Mirror status label
  var mirrorLabel = d.http_mirror_status
      ? '<span class="mirror-status">' + ({
          'downloading': 'HTTP Mirror',
          'extracting': 'Extracting...',
          'rechecking': 'Rechecking...'
      }[d.http_mirror_status] || escapeHtml(d.http_mirror_status)) + '</span>'
      : '';

  // Action buttons (admin only)
  var actions = "";
  var isAdmin = window.currentUserRole === "ADMIN";
  if (isAdmin) {
    if (isActive) {
      actions =
        '<button class="action-btn pause-btn" onclick="event.stopPropagation(); pauseDownload(\'' +
        safeId +
        '\')" title="Pause">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>' +
        "</button>" +
        '<button class="action-btn cancel-btn" onclick="event.stopPropagation(); cancelDownload(\'' +
        safeId +
        '\')" title="Cancel">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>' +
        "</button>";
    } else if (d.status === "Seeding") {
      actions =
        '<button class="action-btn cancel-btn" onclick="event.stopPropagation(); cancelDownload(\'' +
        safeId +
        '\')" title="Stop Seeding">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="4" y="4" width="16" height="16" rx="2"/></svg>' +
        "</button>";
    } else if (
      d.status === "Paused" ||
      d.status === "Failed" ||
      d.status === "Stopped"
    ) {
      actions =
        '<button class="action-btn resume-btn" onclick="event.stopPropagation(); resumeDownload(\'' +
        safeId +
        '\')" title="Resume">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><polygon points="5 3 19 12 5 21 5 3"/></svg>' +
        "</button>";
    }
  }

  // Detail panel
  var detailRows =
    '<span class="detail-label">URL</span>' +
    '<span class="detail-value">' +
    safeUrl +
    "</span>" +
    '<span class="detail-label">Save Path</span>' +
    '<span class="detail-value">' +
    escapeHtml(d.save_path || "") +
    "</span>" +
    '<span class="detail-label">Protocol</span>' +
    '<span class="detail-value">' +
    safeProtocol +
    "</span>" +
    '<span class="detail-label">Created</span>' +
    '<span class="detail-value">' +
    (d.created_at ? escapeHtml(new Date(d.created_at).toLocaleString()) : "") +
    "</span>";

  if (d.completed_at) {
    detailRows +=
      '<span class="detail-label">Completed</span>' +
      '<span class="detail-value">' +
      escapeHtml(new Date(d.completed_at).toLocaleString()) +
      "</span>";
  }

  if (isTorrent) {
    detailRows +=
      '<span class="detail-label">Peers</span>' +
      '<span class="detail-value detail-peers">' +
      (d.peers || 0) +
      "</span>" +
      '<span class="detail-label">Seeds</span>' +
      '<span class="detail-value detail-seeds">' +
      (d.seeds || 0) +
      "</span>" +
      '<span class="detail-label">Upload Speed</span>' +
      '<span class="detail-value detail-upload-speed">' +
      formatSpeed(d.upload_speed) +
      "</span>";
  }

  if (d.error_message) {
    detailRows +=
      '<span class="detail-label">Error</span>' +
      '<span class="detail-value error">' +
      escapeHtml(d.error_message) +
      "</span>";
  }

  var isExpanded = expandedId === d.id;

  if (d.completed_at) {
    detailRows += '<span class="detail-label">Completed</span>'
      + '<span class="detail-value">' + escapeHtml(new Date(d.completed_at).toLocaleString()) + '</span>';
  }

  if (isTorrent) {
    detailRows += '<span class="detail-label">Peers</span>'
      + '<span class="detail-value detail-peers">' + (d.peers || 0) + '</span>'
      + '<span class="detail-label">Seeds</span>'
      + '<span class="detail-value detail-seeds">' + (d.seeds || 0) + '</span>'
      + '<span class="detail-label">Upload Speed</span>'
      + '<span class="detail-value detail-upload-speed">' + formatSpeed(d.upload_speed) + '</span>';
  }

  if (d.error_message) {
    detailRows += '<span class="detail-label">Error</span>'
      + '<span class="detail-value error">' + escapeHtml(d.error_message) + '</span>';
  }

  var isExpanded = (expandedId === d.id);

  return '<div class="download-item ' + statusClass + '" data-id="' + safeId + '" onclick="toggleDetail(\'' + safeId + '\', event)">'
    + '<div class="download-row">'
    + '<div class="protocol-icon">' + protocolIcon + '</div>'
    + '<div class="download-info">'
    + '<div class="download-name">' + safeName + '</div>'
    + '<div class="download-url" title="' + safeUrl + '">' + safeUrl + '</div>'
    + '</div>'
    + '<div class="download-metrics">'
    + '<span>' + sizeDisplay + '</span>'
    + '<span class="speed">' + displaySpeed + '</span>'
    + '<span class="conn">' + connDisplay + '</span>'
    + '<span class="eta">' + etaDisplay + '</span>'
    + mirrorLabel
    + '</div>'
    + '<span class="status-badge ' + statusClass + '">' + safeStatus + '</span>'
    + '<div class="actions">'
    + actions
    + (isTorrent ? '<div class="more-dropdown">'
      + '<button class="more-btn" onclick="toggleMoreMenu(event, \'' + safeId + '\')" title="More options">'
      + '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor">'
      + '<circle cx="5" cy="12" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="19" cy="12" r="2"/>'
      + '</svg>'
      + '</button>'
      + '<div class="more-menu" id="more-menu-' + safeId + '">'
      + '<button onclick="copyMagnet(event, \'' + safeId + '\')">'
      + 'Copy Magnet'
      + '</button>'
      + '<button onclick="downloadTorrent(event, \'' + safeId + '\')">'
      + 'Download .torrent'
      + '</button>'
      + (canMirror ? '<button onclick="showMirrorForm(event, \'' + safeId + '\')">'
      + 'Add HTTP Mirror'
      + '</button>' : '')
      + '</div>'
      + '</div>'
      + (isTorrent ? '<div class="mirror-form" id="mirror-form-' + safeId + '" style="display:none">'
      + '<input type="text" id="mirror-url-' + safeId + '" placeholder="HTTP/HTTPS mirror URL" class="mirror-input">'
      + '<label class="mirror-checkbox"><input type="checkbox" id="mirror-seed-' + safeId + '" checked> Keep seeding</label>'
      + '<div class="mirror-actions">'
      + '<button class="mirror-start-btn" onclick="startMirror(event, \'' + safeId + '\')">Start</button>'
      + '<button class="mirror-cancel-btn" onclick="hideMirrorForm(\'' + safeId + '\')">Cancel</button>'
      + '</div>'
      + '</div>' : '')
      : '')
    + (isAdmin ? '<div class="delete-dropdown">'
      + '<button class="delete-btn" onclick="toggleDeleteMenu(event, \'' + safeId + '\')" title="Remove">'
      + '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M3 6h18"/><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/>'
      + '</svg>'
      + '</button>'
      + '<div class="delete-menu" id="delete-menu-' + safeId + '">'
      + '<button onclick="event.stopPropagation(); deleteDownload(\'' + safeId + '\', false)">Remove from list</button>'
      + '<button class="danger" onclick="event.stopPropagation(); deleteDownload(\'' + safeId + '\', true)">Delete from disk</button>'
      + '</div>'
      + '</div>' : '')
    + '</div>'
    + '</div>'
    + '<div class="download-progress">'
    + '<div class="progress-bar">'
    + '<div class="progress-fill ' + progressClass + '" style="width: ' + progress + '%"></div>'
    + '</div>'
    + '</div>'
    + '<div class="download-detail' + (isExpanded ? ' open' : '') + '" id="detail-' + safeId + '">'
    + '<div class="detail-content">'
    + '<div class="detail-grid">'
    + detailRows
    + '</div>'
    + '</div>'
    + '</div>'
    + '</div>';
}

function renderDownloads(downloads) {
  var container = document.getElementById("downloads-list");
  if (!container) return;

  var filtered = getPageDownloads(downloads);

  var emptyMsg = currentPage === 'completed'
    ? '<p>No completed downloads</p><span>Finished downloads will appear here</span>'
    : '<p>No active downloads</p><span>Add a URL above to start downloading</span>';

  if (filtered.length === 0) {
    safeRender(container, '<div class="empty-state">'
      + '<svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">'
      + '<path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4"/>'
      + '<polyline points="7 10 12 15 17 10"/>'
      + '<line x1="12" y1="15" x2="12" y2="3"/>'
      + '</svg>'
      + emptyMsg
      + '</div>');
    lastDownloads = filtered;
    return;
  }

  // Check if the set of download IDs or their statuses changed — if so, full rebuild
  var currentIds = filtered.map(function (d) { return d.id + ':' + d.status; }).join(',');
  var prevIds = lastDownloads.map(function (d) { return d.id + ':' + d.status; }).join(',');

  if (currentIds !== prevIds) {
    // Skip full rebuild if a menu is open (user is interacting)
    if (openMenuId || openMoreMenuId) {
      lastDownloads = filtered;
      return;
    }
    // Full rebuild
    var html = filtered.map(buildDownloadItem).join('');
    safeRender(container, html);
    // Restore expanded detail
    if (expandedId !== null) {
      var detail = document.getElementById('detail-' + expandedId);
      if (detail) detail.classList.add('open');
    }
  } else {
    // Incremental update — only update changing values in-place
    filtered.forEach(function (d) {
      var el = container.querySelector('[data-id="' + CSS.escape(d.id) + '"]');
      if (!el) return;

      var progress = Math.min(d.progress, 100);
      if (d.status === 'Completed' && d.total_size === 0 && d.downloaded_size > 0) {
        progress = 100;
      }
      var isActive = d.status === 'Downloading';

      // Update filename and URL (e.g. magnet resolved to real name)
      var nameEl = el.querySelector('.download-name');
      if (nameEl && nameEl.textContent !== d.filename) {
        nameEl.textContent = d.filename;
      }
      var urlEl = el.querySelector('.download-url');
      if (urlEl && urlEl.textContent !== d.url) {
        urlEl.textContent = d.url;
        urlEl.title = d.url;
      }

      // Update progress bar width
      var fill = el.querySelector('.progress-fill');
      if (fill) fill.style.width = progress + '%';

      // Update metrics
      var metricsSpans = el.querySelectorAll('.download-metrics > span');
      if (metricsSpans[0]) {
        var sizeDisplay;
        if (d.status === 'Completed') {
          sizeDisplay = formatSize(d.total_size || d.downloaded_size);
        } else if (isActive && d.total_size > 0) {
          sizeDisplay = formatSize(d.downloaded_size) + ' / ' + formatSize(d.total_size);
        } else {
          sizeDisplay = formatSize(d.downloaded_size);
        }
        metricsSpans[0].textContent = sizeDisplay;
      }

      // Update speed
      var speedEl = el.querySelector('.speed');
      if (speedEl) {
        var isSeeding = d.status === 'Seeding';
        speedEl.textContent = isActive ? formatSpeed(d.speed) : isSeeding ? ('\u2191 ' + formatSpeed(d.upload_speed)) : '--';
      }

      // Update connections
      var connEl = el.querySelector('.conn');
      if (connEl) {
        var isTorrent = d.protocol === 'Torrent';
        var isSeeding = d.status === 'Seeding';
        var connDisplay = '';
        if (isActive || isSeeding) {
          if (isTorrent) {
            connDisplay = isSeeding
              ? (d.seeds || 0) + ' seed' + ((d.seeds || 0) !== 1 ? 's' : '')
              : (d.peers || 0) + ' peer' + ((d.peers || 0) !== 1 ? 's' : '');
          } else {
            connDisplay = (d.connections || 1) + ' conn';
          }
        }
        connEl.textContent = connDisplay;
      }

      // Update ETA
      var etaEl = el.querySelector('.eta');
      if (etaEl) {
        etaEl.textContent = (isActive && d.eta) ? d.eta : '';
      }

      // Update torrent detail fields
      var peersEl = el.querySelector('.detail-peers');
      if (peersEl) peersEl.textContent = d.peers || 0;
      var seedsEl = el.querySelector('.detail-seeds');
      if (seedsEl) seedsEl.textContent = d.seeds || 0;
      var uploadSpeedEl = el.querySelector('.detail-upload-speed');
      if (uploadSpeedEl) uploadSpeedEl.textContent = formatSpeed(d.upload_speed);
    });
  }

  lastDownloads = filtered;
}

function updateStats(downloads) {
  var active = downloads.filter(function (d) {
    return d.status === "Downloading" || d.status === "Seeding";
  }).length;
  var totalSpeed = downloads.reduce(function (sum, d) {
    return sum + (d.status === "Downloading" ? d.speed || 0 : 0);
  }, 0);

  var el;
  el = document.getElementById("active-count");
  if (el) el.textContent = active;
  el = document.getElementById("total-speed");
  if (el) el.textContent = formatSpeed(totalSpeed);

  // Show/hide Resume All button
  var pausedCount = downloads.filter(function (d) {
    return d.status === "Paused" || d.status === "Failed" || d.status === "Stopped";
  }).length;
  var resumeBtn = document.getElementById("resume-all-btn");
  if (resumeBtn) {
    resumeBtn.style.display = pausedCount > 0 ? "" : "none";
  }
}

// ─── Data Loading ───────────────────────────────────

async function loadDownloads() {
  try {
    var downloads = await apiRequest("/downloads");
    // Detect newly failed downloads
    if (lastDownloads.length > 0) {
      downloads.forEach(function (d) {
        if (d.status === "Failed") {
          var prev = lastDownloads.find(function (p) {
            return p.id === d.id;
          });
          if (prev && prev.status !== "Failed") {
            showToast(
              "error",
              "Download failed",
              d.filename + (d.error_message ? ": " + d.error_message : ""),
            );
          }
        }
      });
    }
    renderDownloads(downloads);
    updateStats(downloads);
  } catch (e) {
    // Silently ignore polling errors to avoid toast spam
  }
}

// ─── Profile ────────────────────────────────────────

async function loadProfile() {
  try {
    var result = await apiRequest("/auth/me", {
      method: "POST",
      body: JSON.stringify(token),
    });

    if (result.success) {
      document.getElementById("profile-username").value = result.user.username;
      document.getElementById("profile-role").value = result.user.role;
      document.getElementById("profile-created").value = new Date(
        result.user.created_at,
      ).toLocaleDateString();
    }
  } catch (e) {
    console.error("Failed to load profile:", e);
  }
}

async function changePassword() {
  var currentPassword = document.getElementById("current-password").value;
  var newPassword = document.getElementById("new-password").value;
  var confirmPassword = document.getElementById("confirm-password").value;

  if (newPassword !== confirmPassword) {
    showToast("error", "Password mismatch", "New passwords do not match");
    return;
  }

  if (newPassword.length < 6) {
    showToast(
      "error",
      "Weak password",
      "Password must be at least 6 characters",
    );
    return;
  }

  try {
    var result = await apiRequest("/auth/profile", {
      method: "POST",
      body: JSON.stringify({
        token: token,
        current_password: currentPassword,
        new_password: newPassword,
      }),
    });

    if (result.success) {
      showToast(
        "success",
        "Password updated",
        "Your password has been changed successfully",
      );
      document.getElementById("change-password-form").reset();
    } else {
      showToast("error", "Failed to update password", result.error);
    }
  } catch (e) {
    showToast("error", "Failed to update password", e.message);
  }
}

async function loadSettings() {
  try {
    var settings = await apiRequest("/settings");
    var el;
    el = document.getElementById("settings-dir");
    if (el) el.value = settings.download_dir;
    el = document.getElementById("settings-max-concurrent");
    if (el) el.value = settings.max_concurrent;
    el = document.getElementById("settings-connections");
    if (el) el.value = settings.max_connections_per_file;

    // Load user management for admin users
    if (window.currentUserRole === "ADMIN") {
      loadUserManagement();
    }
  } catch (e) {
    console.error("Failed to load settings:", e);
  }
}

async function loadUserManagement() {
  try {
    var result = await apiRequest("/auth/users");
    var userSection = document.getElementById("user-management-section");
    if (!userSection) return;

    var userTable =
      '<div class="user-management">' +
      "<h2>User Management</h2>" +
      '<div class="form-field">' +
      "<h3>Create New User</h3>" +
      '<form id="create-user-form">' +
      '<div class="form-row">' +
      '<div class="form-field-half">' +
      '<label for="new-username">Username</label>' +
      '<input type="text" id="new-username" required>' +
      "</div>" +
      '<div class="form-field-half">' +
      '<label for="create-user-password">Password</label>' +
      '<input type="password" id="create-user-password" required>' +
      "</div>" +
      "</div>" +
      '<div class="form-field">' +
      '<label for="new-role">Role</label>' +
      '<select id="new-role">' +
      '<option value="USER">User</option>' +
      '<option value="ADMIN">Admin</option>' +
      "</select>" +
      "</div>" +
      '<button type="submit" class="btn-primary">Create User</button>' +
      "</form>" +
      "</div>" +
      '<div class="form-field">' +
      "<h3>Existing Users</h3>" +
      '<table class="user-table">' +
      "<thead>" +
      "<tr>" +
      "<th>Username</th>" +
      "<th>Role</th>" +
      "<th>Created</th>" +
      "<th>Actions</th>" +
      "</tr>" +
      "</thead>" +
      '<tbody id="users-table-body">';

    if (result.users && result.users.length > 0) {
      result.users.forEach(function (user) {
        userTable +=
          "<tr>" +
          "<td>" +
          escapeHtml(user.username) +
          "</td>" +
          '<td><span class="role-badge ' +
          user.role.toLowerCase() +
          '">' +
          user.role +
          "</span></td>" +
          "<td>" +
          new Date(user.created_at).toLocaleDateString() +
          "</td>" +
          "<td>" +
          (user.id !== window.currentUserId
            ? '<button class="btn-danger btn-small" onclick="deleteUser(\'' +
            escapeHtml(user.id) +
            "')\">Delete</button>"
            : "Current User") +
          "</td>" +
          "</tr>";
      });
    }

    userTable += "</tbody>" + "</table>" + "</div>" + "</div>";

    userSection.innerHTML = userTable;

    // Bind the create user form
    var createUserForm = document.getElementById("create-user-form");
    if (createUserForm) {
      createUserForm.addEventListener("submit", function (e) {
        e.preventDefault();
        createUser();
      });
    }
  } catch (e) {
    console.error("Failed to load user management:", e);
  }
}

async function createUser() {
  var username = document.getElementById("new-username").value.trim();
  var password = document.getElementById("create-user-password").value;
  var role = document.getElementById("new-role").value;

  if (!username || !password) {
    showToast("error", "Missing fields", "Username and password are required");
    return;
  }

  if (password.length < 6) {
    showToast(
      "error",
      "Weak password",
      "Password must be at least 6 characters",
    );
    return;
  }

  try {
    var result = await apiRequest("/auth/users", {
      method: "POST",
      body: JSON.stringify({
        token: token,
        username: username,
        password: password,
        role: role,
      }),
    });
    if (result.success) {
      showToast("success", "User created", username + " has been created");
      loadUserManagement();
    } else {
      showToast("error", "Failed to create user", result.error);
    }
  } catch (e) {
    showToast("error", "Failed to create user", e.message);
  }
}

async function deleteUser(id) {
  if (!confirm("Are you sure you want to delete this user?")) return;
  try {
    var result = await apiRequest("/auth/users/" + encodeURIComponent(id), {
      method: "DELETE",
    });
    if (result.success) {
      showToast("success", "User deleted");
      loadUserManagement();
    } else {
      showToast("error", "Failed to delete user", result.error);
    }
  } catch (e) {
    showToast("error", "Failed to delete user", e.message);
  }
}

// ─── History ────────────────────────────────────────

async function loadHistory() {
  try {
    var history = await apiRequest("/history");
    renderHistory(history);
  } catch (e) {
    console.error("Failed to load history:", e);
  }
}

function renderHistory(items) {
  var container = document.getElementById("history-list");
  if (!container) return;

  if (!items || items.length === 0) {
    safeRender(
      container,
      '<div class="empty-state">' +
      '<svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">' +
      '<circle cx="12" cy="12" r="10"/>' +
      '<polyline points="12 6 12 12 16 14"/>' +
      "</svg>" +
      "<p>No history yet</p>" +
      "<span>Downloads will appear here</span>" +
      "</div>",
    );
    var actionsDiv = document.getElementById("history-actions");
    if (actionsDiv) actionsDiv.style.display = "none";
    return;
  }

  var actionsDiv = document.getElementById("history-actions");
  if (actionsDiv) actionsDiv.style.display = "flex";

  var html = items
    .map(function (h) {
      var isSelected = selectedHistoryIds.has(h.id);
      var sizeDisplay = formatSize(h.total_size || 0);
      var createdDate = new Date(h.created_at).toLocaleDateString(undefined, {
        year: "numeric",
        month: "short",
        day: "numeric",
        hour: "2-digit",
        minute: "2-digit",
      });
      var completedDate = h.completed_at
        ? new Date(h.completed_at).toLocaleDateString(undefined, {
          year: "numeric",
          month: "short",
          day: "numeric",
          hour: "2-digit",
          minute: "2-digit",
        })
        : null;
      var statusClass = h.status.toLowerCase();

      return (
        '<div class="history-item' +
        (isSelected ? " selected" : "") +
        '" data-id="' +
        escapeHtml(h.id) +
        '">' +
        '<label class="history-checkbox">' +
        '<input type="checkbox" ' +
        (isSelected ? "checked" : "") +
        " onchange=\"toggleHistorySelect('" +
        escapeHtml(h.id) +
        "', this.checked)\" />" +
        '<span class="checkmark"></span>' +
        "</label>" +
        '<div class="history-info">' +
        '<div class="history-name">' +
        escapeHtml(h.filename) +
        "</div>" +
        '<div class="history-url" title="' +
        escapeHtml(h.url) +
        '">' +
        escapeHtml(h.url) +
        "</div>" +
        '<div class="history-meta">' +
        '<span class="status-badge ' +
        statusClass +
        '">' +
        escapeHtml(h.status) +
        "</span>" +
        "<span>" +
        sizeDisplay +
        "</span>" +
        "<span>" +
        createdDate +
        "</span>" +
        (completedDate ? "<span>Completed " + completedDate + "</span>" : "") +
        "</div>" +
        "</div>" +
        '<button class="delete-btn" onclick="deleteHistoryItem(\'' +
        escapeHtml(h.id) +
        '\')" title="Remove from history">' +
        '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/></svg>' +
        "</button>" +
        "</div>"
      );
    })
    .join("");

  safeRender(container, html);
  updateSelectedBtn();
}

function toggleHistorySelect(id, checked) {
  if (checked) {
    selectedHistoryIds.add(id);
  } else {
    selectedHistoryIds.delete(id);
  }
  // Update visual state without full re-render
  var item = document.querySelector(
    '.history-item[data-id="' + CSS.escape(id) + '"]',
  );
  if (item) item.classList.toggle("selected", checked);
  updateSelectedBtn();
}

function updateSelectedBtn() {
  var btn = document.getElementById("delete-selected-btn");
  if (btn) {
    if (selectedHistoryIds.size > 0) {
      btn.style.display = "inline-flex";
      btn.textContent = "Delete Selected (" + selectedHistoryIds.size + ")";
    } else {
      btn.style.display = "none";
    }
  }
}

async function deleteHistoryItem(id) {
  try {
    await apiRequest("/history/" + encodeURIComponent(id), {
      method: "DELETE",
    });
    selectedHistoryIds.delete(id);
    loadHistory();
  } catch (e) {
    showToast("error", "Failed to delete history item", e.message);
  }
}

async function deleteSelectedHistory() {
  var ids = Array.from(selectedHistoryIds);
  if (ids.length === 0) return;
  try {
    for (var i = 0; i < ids.length; i++) {
      await apiRequest("/history/" + encodeURIComponent(ids[i]), {
        method: "DELETE",
      });
    }
    selectedHistoryIds.clear();
    showToast(
      "success",
      "History items deleted",
      ids.length + " item(s) removed",
    );
    loadHistory();
  } catch (e) {
    showToast("error", "Failed to delete history items", e.message);
    loadHistory();
  }
}

async function clearAllHistory() {
  if (!confirm("Clear all download history?")) return;
  try {
    await apiRequest("/history", { method: "DELETE" });
    selectedHistoryIds.clear();
    showToast("success", "History cleared");
    loadHistory();
  } catch (e) {
    showToast("error", "Failed to clear history", e.message);
  }
}

// ─── Actions ────────────────────────────────────────

async function addDownload(url) {
  try {
    await apiRequest("/downloads", {
      method: "POST",
      body: JSON.stringify({ url: url }),
    });
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Failed to add download", e.message);
  }
}

function toggleDeleteMenu(event, id) {
  event.stopPropagation();
  document.querySelectorAll('.more-menu.show').forEach(function (m) { m.classList.remove('show'); });
  openMoreMenuId = null;
  // Close all other menus
  document.querySelectorAll('.delete-menu.show').forEach(function (m) { m.classList.remove('show'); });

  if (openMenuId === id) {
    openMenuId = null;
    return;
  }

  var menu = document.getElementById("delete-menu-" + id);
  if (menu) {
    menu.classList.add("show");
    openMenuId = id;
  }
}

function toggleMoreMenu(event, id) {
  event.stopPropagation();
  // Close delete menus
  document.querySelectorAll('.delete-menu.show').forEach(function (m) { m.classList.remove('show'); });
  openMenuId = null;
  // Toggle this more menu
  document.querySelectorAll('.more-menu.show').forEach(function (m) { m.classList.remove('show'); });
  if (openMoreMenuId === id) {
    openMoreMenuId = null;
    return;
  }
  var menu = document.getElementById('more-menu-' + id);
  if (menu) {
    menu.classList.add('show');
    openMoreMenuId = id;
  }
}

async function copyMagnet(event, id) {
  event.stopPropagation();
  openMoreMenuId = null;
  document.querySelectorAll('.more-menu.show').forEach(function (m) { m.classList.remove('show'); });

  var d = lastDownloads.find(function (x) { return x.id === id; });
  var magnetUri = '';
  if (d && d.url && d.url.startsWith('magnet:')) {
    magnetUri = d.url;
  } else if (d && d.info_hash) {
    magnetUri = 'magnet:?xt=urn:btih:' + d.info_hash + '&dn=' + encodeURIComponent(d.filename || '');
  } else {
    showToast('error', 'No magnet link', 'Magnet URI not yet available \u2014 torrent metadata still resolving');
    return;
  }

  try {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(magnetUri);
    } else {
      // Fallback for non-HTTPS
      var ta = document.createElement('textarea');
      ta.value = magnetUri;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
    }
    showToast('success', 'Copied', 'Magnet link copied to clipboard');
  } catch (e) {
    showToast('error', 'Copy failed', e.message || 'Could not copy to clipboard');
  }
}

async function downloadTorrent(event, id) {
  event.preventDefault();
  event.stopPropagation();
  openMoreMenuId = null;
  document.querySelectorAll('.more-menu.show').forEach(function (m) { m.classList.remove('show'); });
  try {
    var d = lastDownloads.find(function (x) { return x.id === id; });
    var suggestedName = (d && d.filename ? d.filename : 'torrent') + '.torrent';
    var resp = await fetch(API_BASE + '/downloads/' + encodeURIComponent(id) + '/torrent', {
      headers: { 'Authorization': 'Bearer ' + token }
    });
    if (resp.status === 401) {
      logout();
      return;
    }
    if (!resp.ok) {
      var text = await resp.text();
      throw new Error(text || 'HTTP ' + resp.status);
    }
    var blob = await resp.blob();
    var url = URL.createObjectURL(blob);
    var a = document.createElement('a');
    a.href = url;
    a.download = suggestedName;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  } catch (e) {
    showToast('error', 'Download failed', e.message || 'Could not download .torrent file');
  }
}

function showMirrorForm(event, id) {
  event.stopPropagation();
  // Close all menus
  document.querySelectorAll('.more-menu').forEach(function(m) { m.classList.remove('open'); });
  document.querySelectorAll('.delete-menu').forEach(function(m) { m.classList.remove('open'); });
  openMenuId = null;
  openMoreMenuId = null;
  var form = document.getElementById('mirror-form-' + CSS.escape(id));
  if (form) {
    form.style.display = form.style.display === 'none' ? 'block' : 'none';
  }
}

function hideMirrorForm(id) {
  var form = document.getElementById('mirror-form-' + CSS.escape(id));
  if (form) form.style.display = 'none';
}

function startMirror(event, id) {
  event.stopPropagation();
  var urlInput = document.getElementById('mirror-url-' + CSS.escape(id));
  var seedCheckbox = document.getElementById('mirror-seed-' + CSS.escape(id));
  var url = urlInput ? urlInput.value.trim() : '';
  var keepSeeding = seedCheckbox ? seedCheckbox.checked : true;

  if (!url) {
    alert('Please enter a mirror URL');
    return;
  }

  var token = localStorage.getItem('dload_token');
  fetch('/api/downloads/' + encodeURIComponent(id) + '/http-mirror', {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': 'Bearer ' + token
    },
    body: JSON.stringify({ url: url, keep_seeding: keepSeeding })
  })
  .then(function(resp) {
    if (!resp.ok) {
      return resp.text().then(function(t) { throw new Error(t); });
    }
    return resp.json();
  })
  .then(function() {
    hideMirrorForm(id);
    refreshDownloads();
  })
  .catch(function(err) {
    alert('Failed to start mirror: ' + err.message);
  });
}

async function deleteDownload(id, deleteFiles) {
  // Check if user has admin permissions
  if (window.currentUserRole !== "ADMIN") {
    showToast(
      "error",
      "Permission denied",
      "Only administrators can delete downloads",
    );
    openMenuId = null;
    return;
  }

  openMenuId = null;
  try {
    var qs = deleteFiles ? "?delete_files=true" : "";
    await apiRequest("/downloads/" + encodeURIComponent(id) + qs, {
      method: "DELETE",
    });
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Failed to delete", e.message);
  }
}

async function pauseDownload(id) {
  try {
    await apiRequest("/downloads/" + encodeURIComponent(id) + "/pause", {
      method: "POST",
    });
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Failed to pause", e.message);
  }
}

async function cancelDownload(id) {
  try {
    await apiRequest("/downloads/" + encodeURIComponent(id) + "/cancel", {
      method: "POST",
    });
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Failed to cancel", e.message);
  }
}

async function resumeDownload(id) {
  try {
    await apiRequest("/downloads/" + encodeURIComponent(id) + "/resume", {
      method: "POST",
    });
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Failed to resume", e.message);
  }
}

var resumeAllInProgress = false;

async function resumeAllDownloads() {
  if (resumeAllInProgress) return;
  resumeAllInProgress = true;
  var btn = document.getElementById("resume-all-btn");
  if (btn) {
    btn.disabled = true;
    btn.textContent = "Resuming\u2026";
  }
  try {
    var result = await apiRequest("/downloads/resume-all", { method: "POST" });
    showToast("success", "Resume All", "Resuming " + (result.count || 0) + " downloads");
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Resume All Failed", e.message);
  } finally {
    resumeAllInProgress = false;
    if (btn) {
      btn.disabled = false;
      btn.textContent = "Resume All";
    }
  }
}

// ─── Folder Browser ─────────────────────────────────

var folderBrowserModal = null;

function openFolderBrowser() {
  var startPath = document.getElementById("settings-dir").value || "/";
  showFolderBrowser(startPath);
}

function showFolderBrowser(path) {
  if (folderBrowserModal) {
    folderBrowserModal.remove();
  }

  var modal = document.createElement("div");
  modal.className = "modal";
  modal.id = "folder-browser-modal";
  modal.style.display = "flex";

  var backdrop = document.createElement("div");
  backdrop.className = "modal-backdrop";
  backdrop.onclick = closeFolderBrowser;
  modal.appendChild(backdrop);

  var content = document.createElement("div");
  content.className = "modal-content folder-browser";

  var header = document.createElement("div");
  header.className = "folder-browser-header";
  var h2 = document.createElement("h2");
  h2.textContent = "Select Folder";
  var pathDiv = document.createElement("div");
  pathDiv.className = "folder-browser-path";
  pathDiv.id = "browser-path";
  header.appendChild(h2);
  header.appendChild(pathDiv);

  var list = document.createElement("div");
  list.className = "folder-browser-list";
  list.id = "browser-list";

  var actions = document.createElement("div");
  actions.className = "folder-browser-actions";
  var newBtn = document.createElement("button");
  newBtn.className = "btn-ghost btn-small";
  newBtn.textContent = "New Folder";
  newBtn.onclick = browserCreateFolder;
  var spacer = document.createElement("div");
  spacer.style.flex = "1";
  var cancelBtn = document.createElement("button");
  cancelBtn.className = "btn-ghost";
  cancelBtn.textContent = "Cancel";
  cancelBtn.onclick = closeFolderBrowser;
  var selectBtn = document.createElement("button");
  selectBtn.className = "btn-primary";
  selectBtn.textContent = "Select";
  selectBtn.id = "browser-select-btn";
  selectBtn.onclick = function () {
    var sel = pathDiv.getAttribute("data-path");
    if (sel) document.getElementById("settings-dir").value = sel;
    closeFolderBrowser();
  };
  actions.appendChild(newBtn);
  actions.appendChild(spacer);
  actions.appendChild(cancelBtn);
  actions.appendChild(selectBtn);

  content.appendChild(header);
  content.appendChild(list);
  content.appendChild(actions);
  modal.appendChild(content);
  document.body.appendChild(modal);
  folderBrowserModal = modal;

  loadBrowserDir(path);
}

function closeFolderBrowser() {
  if (folderBrowserModal) {
    folderBrowserModal.remove();
    folderBrowserModal = null;
  }
}

function createFolderItem(label, iconSvg, onDblClick) {
  var div = document.createElement("div");
  div.className = "folder-item" + (label === ".." ? " folder-parent" : "");
  div.ondblclick = onDblClick;
  var iconSpan = document.createElement("span");
  iconSpan.className = "folder-icon";
  iconSpan.textContent = label === ".." ? "\u2190" : "\uD83D\uDCC1";
  var nameSpan = document.createElement("span");
  nameSpan.textContent = label;
  div.appendChild(iconSpan);
  div.appendChild(nameSpan);
  return div;
}

async function loadBrowserDir(path) {
  var listEl = document.getElementById("browser-list");
  var pathEl = document.getElementById("browser-path");
  if (!listEl || !pathEl) return;

  listEl.textContent = "";
  var loading = document.createElement("div");
  loading.className = "folder-browser-loading";
  loading.textContent = "Loading...";
  listEl.appendChild(loading);

  try {
    var result = await apiRequest("/browse?path=" + encodeURIComponent(path));
    pathEl.textContent = result.current;
    pathEl.setAttribute("data-path", result.current);

    listEl.textContent = "";

    if (result.parent !== null && result.parent !== undefined) {
      var parentPath = result.parent;
      listEl.appendChild(
        createFolderItem("..", null, function () {
          loadBrowserDir(parentPath);
        }),
      );
    }

    for (var i = 0; i < result.dirs.length; i++) {
      (function (d) {
        listEl.appendChild(
          createFolderItem(d.name, null, function () {
            loadBrowserDir(d.path);
          }),
        );
      })(result.dirs[i]);
    }

    if (result.dirs.length === 0) {
      var empty = document.createElement("div");
      empty.className = "folder-empty";
      empty.textContent = result.parent
        ? "Empty directory"
        : "No subdirectories";
      listEl.appendChild(empty);
    }
  } catch (e) {
    listEl.textContent = "";
    var errDiv = document.createElement("div");
    errDiv.className = "folder-empty";
    errDiv.textContent = "Error: " + e.message;
    listEl.appendChild(errDiv);
  }
}

async function browserCreateFolder() {
  var pathEl = document.getElementById("browser-path");
  var currentPath = pathEl ? pathEl.getAttribute("data-path") : "/";

  var name = prompt("New folder name:");
  if (!name || !name.trim()) return;
  name = name.trim();

  if (name.indexOf("/") !== -1 || name.indexOf("..") !== -1) {
    showToast("error", "Invalid folder name");
    return;
  }

  var newPath = currentPath.replace(/\/$/, "") + "/" + name;
  try {
    await apiRequest("/browse/mkdir", {
      method: "POST",
      body: JSON.stringify({ path: newPath }),
    });
    loadBrowserDir(currentPath);
  } catch (e) {
    showToast("error", "Failed to create folder", e.message);
  }
}

async function saveSettings() {
  try {
    // Fetch current settings to preserve non-UI fields
    var current = await apiRequest("/settings");
    var settings = {
      download_dir: document.getElementById("settings-dir").value,
      max_concurrent: parseInt(
        document.getElementById("settings-max-concurrent").value,
      ),
      max_connections_per_file: parseInt(
        document.getElementById("settings-connections").value,
      ),
      min_split_size: current.min_split_size || 20971520,
      username: current.username || "",
      port: current.port || 8080,
    };

    await apiRequest("/settings", {
      method: "PUT",
      body: JSON.stringify(settings),
    });
    showToast("success", "Settings saved");
  } catch (e) {
    showToast("error", "Failed to save settings", e.message);
  }
}

// ─── Navigation ─────────────────────────────────────

function navigate(hash) {
  var routes = {
    "#downloads": showDownloads,
    "#completed": showCompleted,
    "#history": showHistory,
    "#settings": showSettings,
    "#profile": showProfile,
  };

  var render = routes[hash] || showDownloads;
  currentPage =
    hash === "#completed"
      ? "completed"
      : hash === "#history"
        ? "history"
        : hash === "#profile"
          ? "profile"
          : "downloads";
  lastDownloads = [];
  expandedId = null;
  selectedHistoryIds.clear();
  safeRender(document.getElementById("main-content"), render());

  // Update active nav
  document.querySelectorAll(".nav-item").forEach(function (item) {
    item.classList.toggle("active", item.getAttribute("href") === hash);
  });

  // Bind forms after render
  bindForms();

  // Load data
  if (hash === "#settings") {
    loadSettings();
  } else if (hash === "#profile") {
    loadProfile();
  } else if (hash === "#history") {
    loadHistory();
  } else {
    loadDownloads();
  }
}

function bindForms() {
  var addForm = document.getElementById("add-download-form");
  if (addForm) {
    addForm.addEventListener("submit", function (e) {
      e.preventDefault();
      var input = document.getElementById("download-url");
      if (input.value.trim()) {
        addDownload(input.value.trim());
        input.value = "";
      }
    });
  }

  var settingsForm = document.getElementById("settings-form");
  if (settingsForm) {
    settingsForm.addEventListener("submit", function (e) {
      e.preventDefault();
      saveSettings();
    });
  }

  var changePasswordForm = document.getElementById("change-password-form");
  if (changePasswordForm) {
    changePasswordForm.addEventListener("submit", function (e) {
      e.preventDefault();
      changePassword();
    });
  }
}

// ─── Auth ───────────────────────────────────────────

function logout() {
  if (!token) return; // already logged out
  token = "";
  localStorage.removeItem("dload_token");
  window.currentUserRole = null;
  window.currentUserId = null;
  if (refreshInterval) {
    clearInterval(refreshInterval);
    refreshInterval = null;
  }

  // Reset login form to Sign In state
  var loginForm = document.getElementById("login-form");
  if (loginForm) {
    loginForm.reset();
    var btn = loginForm.querySelector('button[type="submit"]');
    if (btn) {
      btn.textContent = "Sign In";
      delete btn.dataset.mode;
    }
  }
  var hint = document.getElementById("login-hint");
  if (hint) hint.textContent = "Sign in to continue";
  var subtitle = document.querySelector(".login-subtitle");
  if (subtitle) subtitle.textContent = "Sign in to your download manager";

  document.getElementById("login-modal").style.display = "flex";
}

// ─── Init ───────────────────────────────────────────

async function checkFirstUser() {
  try {
    var result = await apiRequest("/auth/status");
    return result.needs_setup === true;
  } catch (e) {
    return false;
  }
}

async function init() {
  var loginForm = document.getElementById("login-form");
  if (loginForm) {
    loginForm.addEventListener("submit", async function (e) {
      e.preventDefault();
      var username = document.getElementById("login-username").value;
      var password = document.getElementById("login-password").value;
      var btn = loginForm.querySelector('button[type="submit"]');

      if (btn.dataset.mode === "register") {
        // First user registration
        if (password.length < 8) {
          showToast(
            "error",
            "Weak password",
            "Password must be at least 8 characters",
          );
          return;
        }
        try {
          var result = await apiRequest("/auth/register", {
            method: "POST",
            body: JSON.stringify({ username: username, password: password }),
          });

          if (result.success) {
            token = result.token;
            localStorage.setItem("dload_token", token);
            document.getElementById("login-modal").style.display = "none";
            await updateUserInformation();
            startApp();
          } else {
            showToast("error", "Registration failed", result.error);
          }
        } catch (e) {
          showToast("error", "Registration failed", e.message);
        }
      } else {
        // Normal login
        try {
          var result = await apiRequest("/auth/login", {
            method: "POST",
            body: JSON.stringify({ username: username, password: password }),
          });

          if (result.success) {
            token = result.token;
            localStorage.setItem("dload_token", token);
            document.getElementById("login-modal").style.display = "none";
            await updateUserInformation();
            startApp();
          } else {
            showToast("error", "Login failed", "Invalid username or password");
            document.getElementById("login-password").value = "";
            document.getElementById("login-password").focus();
          }
        } catch (e) {
          showToast("error", "Login failed", e.message);
        }
      }
    });
  }

  if (!token) {
    document.getElementById("login-modal").style.display = "flex";
    // Check if this is the first user
    checkFirstUser().then(function (isFirst) {
      if (isFirst) {
        var hint = document.getElementById("login-hint");
        if (hint) hint.textContent = "Create your admin account to get started";
        var subtitle = document.querySelector(".login-subtitle");
        if (subtitle) subtitle.textContent = "Set up your download manager";
        var btn = loginForm.querySelector('button[type="submit"]');
        if (btn) {
          btn.textContent = "Create Account";
          btn.dataset.mode = "register";
        }
      }
    });
    return;
  }

  await startApp();
}

async function startApp() {
  // Must resolve user role BEFORE rendering anything, otherwise
  // admin buttons won't appear until the next refresh cycle
  if (!window.currentUserRole) {
    await updateUserInformation();
  }

  navigate(window.location.hash || "#downloads");

  // Only add hashchange listener once
  if (!window._hashListenerAdded) {
    window.addEventListener("hashchange", function () {
      navigate(window.location.hash);
    });
    window._hashListenerAdded = true;
  }

  if (refreshInterval) clearInterval(refreshInterval);
  refreshInterval = setInterval(loadDownloads, 1000);
}

async function updateUserInformation() {
  try {
    var result = await apiRequest("/auth/me", {
      method: "POST",
      body: JSON.stringify(token),
    });

    if (result.success) {
      // Update username
      document.getElementById("username").textContent = result.user.username;

      // Update avatar - use first letter of username
      var firstLetter = result.user.username.charAt(0).toUpperCase();
      document.getElementById("user-avatar").textContent = firstLetter;

      // Update role display
      document.getElementById("user-role").textContent = result.user.role;
      document.getElementById("user-role").className =
        "user-role " + result.user.role.toLowerCase();

      // Store user role and ID for permission checks
      window.currentUserRole = result.user.role;
      window.currentUserId = result.user.id;
    }
  } catch (e) {
    console.error("Failed to update user information:", e);
  }
}

// Close delete menus when clicking outside
document.addEventListener('click', function (e) {
  if (!e.target.closest('.delete-dropdown')) {
    document.querySelectorAll('.delete-menu.show').forEach(function (m) { m.classList.remove('show'); });
    openMenuId = null;
  }
  if (!e.target.closest('.more-dropdown')) {
    document.querySelectorAll('.more-menu.show').forEach(function (m) { m.classList.remove('show'); });
    openMoreMenuId = null;
  }
});

document.addEventListener("DOMContentLoaded", init);

var API_BASE = "/api";
var token = localStorage.getItem("dload_token") || "";
var refreshInterval = null;
var lastDownloads = [];
var openMenuId = null;
var openMoreMenuId = null;
var expandedId = null;
var dragSrcId = null;
var isDragging = false;

// ─── App Loading ─────────────────────────────────────

function hideAppLoading() {
  var el = document.getElementById('app-loading');
  if (el) el.style.display = 'none';
}

// ─── Toast Notifications ────────────────────────────

var toastCounter = 0;
var MAX_TOASTS = 3;

function showToast(type, title, message, duration) {
  duration = duration || 5000;
  var container = document.getElementById("toast-container");
  if (!container) return;

  // Deduplicate: skip if an identical toast is already visible
  var existing = container.querySelectorAll('.toast:not(.removing)');
  for (var i = 0; i < existing.length; i++) {
    var t = existing[i];
    var tTitle = t.querySelector('.toast-title');
    var tMsg = t.querySelector('.toast-message');
    if (tTitle && tTitle.textContent === title &&
        (!message && !tMsg || tMsg && tMsg.textContent === message)) {
      return;
    }
  }

  // Limit visible toasts — dismiss oldest if at max
  var visible = container.querySelectorAll('.toast:not(.removing)');
  while (visible.length >= MAX_TOASTS) {
    dismissToast(visible[0].id);
    visible = container.querySelectorAll('.toast:not(.removing)');
  }

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

function renderHtml(container, html) {
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
    '<div class="add-section">' +
    '<form id="add-download-form" class="input-group">' +
    '<label for="download-url" class="sr-only">Download URL</label>' +
    '<input type="text" id="download-url" placeholder="Paste URL — http, ftp, sftp, magnet, or .torrent" required>' +
    '<button type="submit" class="btn-primary">Download</button>' +
    "</form>" +
    "</div>" +
    '<div class="stats-bar">' +
    '<span class="stat-val" id="active-count">0</span> Active' +
    '<span class="stat-sep">&middot;</span>' +
    '&darr; <span class="stat-val speed-val" id="total-speed">0 B/s</span>' +
    '<span class="stat-sep" id="upload-sep" style="display:none">&middot;</span>' +
    '<span id="upload-stat" style="display:none">&uarr; <span class="stat-val" id="total-upload">0 B/s</span></span>' +
    '<span class="stat-sep" id="queue-sep" style="display:none">&middot;</span>' +
    '<span id="queue-stat" style="display:none"><span class="stat-val" id="queued-count">0</span> Queued</span>' +
    '<button id="resume-all-btn" class="btn-ghost" style="display:none;margin-left:auto;font-size:0.8em;padding:4px 10px" onclick="resumeAllDownloads()">Resume All</button>' +
    "</div>" +
    '<div id="downloads-list">' + renderSkeletons() + '</div>'
  );
}

function renderSkeletons() {
  var html = '';
  for (var i = 0; i < 4; i++) {
    html += '<div class="skeleton-item">'
      + '<div class="skeleton-row">'
      + '<div class="skeleton skeleton-icon"></div>'
      + '<div style="flex:1">'
      + '<div class="skeleton skeleton-text skeleton-text-lg"></div>'
      + '<div class="skeleton skeleton-text skeleton-text-sm"></div>'
      + '</div>'
      + '</div>'
      + '<div class="skeleton skeleton-bar"></div>'
      + '</div>';
  }
  return html;
}

function showCompleted() {
  return (
    '<div class="page-header">' +
    "<h1>Completed</h1>" +
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
    '<span class="hint">Additional downloads are queued and start automatically</span>' +
    "</div>" +
    '<div class="form-field">' +
    '<label for="settings-connections">Max Connections Per File</label>' +
    '<input type="number" id="settings-connections" value="4" min="1" max="16">' +
    '<span class="hint">More connections can improve speed for HTTP downloads</span>' +
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
    '<input type="password" id="current-password" required autocomplete="current-password">' +
    "</div>" +
    '<div class="form-field">' +
    '<label for="new-password">New Password</label>' +
    '<input type="password" id="new-password" required autocomplete="new-password">' +
    "</div>" +
    '<div class="form-field">' +
    '<label for="confirm-password">Confirm New Password</label>' +
    '<input type="password" id="confirm-password" required autocomplete="new-password">' +
    "</div>" +
    '<button type="submit" class="btn-primary">Update Password</button>' +
    "</form>" +
    "</div>"
  );
}

// ─── Filter & Detail ────────────────────────────────

function sortDownloads(downloads) {
  return downloads.slice().sort(function (a, b) {
    // Completed page: sort by completed_at (newest first), not position
    if (currentPage === 'completed') {
      return new Date(b.completed_at || b.created_at) - new Date(a.completed_at || a.created_at);
    }
    return (a.position || 0) - (b.position || 0);
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
  if (event.target.closest('button') || event.target.closest('.actions') || event.target.closest('.delete-dropdown') || event.target.closest('.more-dropdown') || event.target.closest('.drag-handle')) {
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
        '\')" title="Pause" aria-label="Pause download">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>' +
        "</button>" +
        '<button class="action-btn cancel-btn" onclick="event.stopPropagation(); cancelDownload(\'' +
        safeId +
        '\')" title="Cancel" aria-label="Cancel download">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>' +
        "</button>";
    } else if (d.status === "Seeding") {
      actions =
        '<button class="action-btn cancel-btn" onclick="event.stopPropagation(); cancelDownload(\'' +
        safeId +
        '\')" title="Stop Seeding" aria-label="Stop seeding">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="4" y="4" width="16" height="16" rx="2"/></svg>' +
        "</button>";
    } else if (
      d.status === "Paused" ||
      d.status === "Failed" ||
      d.status === "Stopped" ||
      d.status === "Queued"
    ) {
      actions =
        '<button class="action-btn resume-btn" onclick="event.stopPropagation(); resumeDownload(\'' +
        safeId +
        '\')" title="Resume" aria-label="Resume download">' +
        '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><polygon points="5 3 19 12 5 21 5 3"/></svg>' +
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

  var isCompleted = d.status === 'Completed';
  return '<div class="download-item ' + statusClass + '" data-id="' + safeId + '"'
    + (isCompleted ? '' : ' draggable="true"')
    + ' tabindex="0" role="button" aria-expanded="' + (isExpanded ? 'true' : 'false') + '"'
    + ' aria-label="' + safeName + ' \u2014 ' + safeStatus + '"'
    + ' onclick="toggleDetail(\'' + safeId + '\', event)"'
    + ' onkeydown="if(event.key===\'Enter\'||event.key===\' \'){event.preventDefault();toggleDetail(\'' + safeId + '\', event)}">'
    + '<div class="download-row">'
    + (isCompleted ? '' : '<div class="drag-handle" title="Drag to reorder">'
    + '<svg width="12" height="16" viewBox="0 0 12 16" fill="currentColor">'
    + '<circle cx="4" cy="3" r="1.5"/><circle cx="4" cy="8" r="1.5"/><circle cx="4" cy="13" r="1.5"/>'
    + '<circle cx="8" cy="3" r="1.5"/><circle cx="8" cy="8" r="1.5"/><circle cx="8" cy="13" r="1.5"/>'
    + '</svg></div>')
    + '<div class="protocol-icon ' + (isTorrent ? 'torrent' : 'http') + '">' + protocolIcon + '</div>'
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
      + '<button class="more-btn" id="more-btn-' + safeId + '" onclick="toggleMoreMenu(event, \'' + safeId + '\')" title="More options" aria-label="More options" aria-haspopup="true" aria-expanded="false">'
      + '<svg width="16" height="16" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">'
      + '<circle cx="5" cy="12" r="2"/><circle cx="12" cy="12" r="2"/><circle cx="19" cy="12" r="2"/>'
      + '</svg>'
      + '</button>'
      + '<div class="more-menu" id="more-menu-' + safeId + '" role="menu">'
      + '<button role="menuitem" onclick="copyMagnet(event, \'' + safeId + '\')">'
      + 'Copy Magnet'
      + '</button>'
      + '<button role="menuitem" onclick="downloadTorrent(event, \'' + safeId + '\')">'
      + 'Download .torrent'
      + '</button>'
      + (canMirror ? '<button role="menuitem" onclick="showMirrorForm(event, \'' + safeId + '\')">'
      + 'Add HTTP Mirror'
      + '</button>' : '')
      + '</div>'
      + '</div>'
      + (isTorrent ? '<div class="mirror-form" id="mirror-form-' + safeId + '" style="display:none">'
      + '<input type="text" id="mirror-url-' + safeId + '" placeholder="HTTP/HTTPS mirror URL" class="mirror-input">'
      + '<label class="mirror-checkbox"><input type="checkbox" id="mirror-seed-' + safeId + '" checked> Keep seeding</label>'
      + '<div class="mirror-actions">'
      + '<button class="btn-primary btn-small" onclick="startMirror(event, \'' + safeId + '\')">Start</button>'
      + '<button class="btn-ghost btn-small" onclick="hideMirrorForm(\'' + safeId + '\')">Cancel</button>'
      + '</div>'
      + '</div>' : '')
      : '')
    + (isAdmin ? '<div class="delete-dropdown">'
      + '<button class="delete-btn" id="delete-btn-' + safeId + '" onclick="toggleDeleteMenu(event, \'' + safeId + '\')" title="Remove" aria-label="Remove download" aria-haspopup="true" aria-expanded="false">'
      + '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">'
      + '<path d="M3 6h18"/><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/>'
      + '</svg>'
      + '</button>'
      + '<div class="delete-menu" id="delete-menu-' + safeId + '" role="menu">'
      + '<button role="menuitem" onclick="event.stopPropagation(); deleteDownload(\'' + safeId + '\', false)">Remove from list</button>'
      + '<button role="menuitem" class="danger" onclick="event.stopPropagation(); deleteDownload(\'' + safeId + '\', true)">Delete from disk</button>'
      + '</div>'
      + '</div>' : '')
    + '</div>'
    + '</div>'
    + '<div class="download-progress">'
    + '<div class="progress-bar" role="progressbar" aria-valuenow="' + progress + '" aria-valuemin="0" aria-valuemax="100" aria-label="Download progress">'
    + '<div class="progress-fill ' + progressClass + '" style="transform: scaleX(' + (progress / 100) + ')"></div>'
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

// ─── Drag & Drop Reorder (Desktop + Mobile Touch) ────

function bindDragEvents() {
  if (currentPage !== 'downloads') return;
  var items = document.querySelectorAll('#downloads-list .download-item[draggable="true"]');
  items.forEach(function (el) {
    // Desktop HTML5 drag events
    el.addEventListener('dragstart', onDragStart);
    el.addEventListener('dragend', onDragEnd);
    el.addEventListener('dragover', onDragOver);
    el.addEventListener('dragleave', onDragLeave);
    el.addEventListener('drop', onDrop);
    // Mobile touch events — only on the drag handle to avoid blocking scroll
    var handle = el.querySelector('.drag-handle');
    if (handle) {
      handle.addEventListener('touchstart', onTouchStart, { passive: false });
    }
  });
}

// ─── Desktop Drag ────────────────────────────────────

function onDragStart(e) {
  if (e.target.closest('button') || e.target.closest('input') || e.target.closest('select') || e.target.closest('.actions')) {
    e.preventDefault();
    return;
  }
  dragSrcId = this.getAttribute('data-id');
  isDragging = true;
  e.dataTransfer.effectAllowed = 'move';
  e.dataTransfer.setData('text/plain', dragSrcId);
  var self = this;
  setTimeout(function () { self.classList.add('dragging'); }, 0);
}

function onDragEnd() {
  isDragging = false;
  dragSrcId = null;
  clearDragClasses();
}

function onDragOver(e) {
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  var targetId = this.getAttribute('data-id');
  if (targetId === dragSrcId) return;
  showDropIndicator(this, e.clientY);
}

function onDragLeave(e) {
  if (!this.contains(e.relatedTarget)) {
    this.classList.remove('drag-over-top', 'drag-over-bottom');
  }
}

function onDrop(e) {
  e.preventDefault();
  e.stopPropagation();
  finishDrop(this, e.clientY);
}

// ─── Mobile Touch Drag ──────────────────────────────

var touchDragEl = null;
var touchClone = null;
var touchScrollTimer = null;

function onTouchStart(e) {
  var item = this.closest('.download-item');
  if (!item) return;

  // Require a brief hold to differentiate from scroll
  var startY = e.touches[0].clientY;
  var startX = e.touches[0].clientX;
  var moved = false;
  var holdTimer = setTimeout(function () {
    beginTouchDrag(item, startY);
  }, 150);

  function onEarlyMove(ev) {
    var dx = ev.touches[0].clientX - startX;
    var dy = ev.touches[0].clientY - startY;
    if (Math.abs(dx) > 10 || Math.abs(dy) > 10) {
      // Moved too early — this is a scroll, not a drag
      clearTimeout(holdTimer);
      cleanup();
    }
  }
  function cleanup() {
    document.removeEventListener('touchmove', onEarlyMove);
    document.removeEventListener('touchend', onEarlyCancel);
  }
  function onEarlyCancel() {
    clearTimeout(holdTimer);
    cleanup();
  }

  document.addEventListener('touchmove', onEarlyMove, { passive: true });
  document.addEventListener('touchend', onEarlyCancel, { once: true });
}

function beginTouchDrag(item, startY) {
  dragSrcId = item.getAttribute('data-id');
  isDragging = true;
  touchDragEl = item;
  item.classList.add('dragging');

  // Create a floating clone for visual feedback
  touchClone = item.cloneNode(true);
  touchClone.classList.add('touch-drag-clone');
  touchClone.style.width = item.offsetWidth + 'px';
  touchClone.style.top = (item.getBoundingClientRect().top) + 'px';
  touchClone.style.left = item.getBoundingClientRect().left + 'px';
  document.body.appendChild(touchClone);

  document.addEventListener('touchmove', onTouchMove, { passive: false });
  document.addEventListener('touchend', onTouchEnd, { once: true });
}

function onTouchMove(e) {
  e.preventDefault();
  var y = e.touches[0].clientY;

  // Move the floating clone
  if (touchClone) {
    touchClone.style.top = (y - touchClone.offsetHeight / 2) + 'px';
  }

  // Auto-scroll near edges
  clearTimeout(touchScrollTimer);
  var edgeZone = 60;
  if (y < edgeZone) {
    window.scrollBy(0, -8);
    touchScrollTimer = setTimeout(function () { onTouchMove(e); }, 16);
  } else if (y > window.innerHeight - edgeZone) {
    window.scrollBy(0, 8);
    touchScrollTimer = setTimeout(function () { onTouchMove(e); }, 16);
  }

  // Find the element under the touch point (exclude the clone)
  if (touchClone) touchClone.style.pointerEvents = 'none';
  var target = document.elementFromPoint(e.touches[0].clientX, y);
  if (touchClone) touchClone.style.pointerEvents = '';

  if (target) {
    var targetItem = target.closest('.download-item[draggable="true"]');
    if (targetItem && targetItem.getAttribute('data-id') !== dragSrcId) {
      showDropIndicator(targetItem, y);
    }
  }
}

function onTouchEnd(e) {
  document.removeEventListener('touchmove', onTouchMove);
  clearTimeout(touchScrollTimer);

  // Find target under last touch position
  var y = e.changedTouches[0].clientY;
  if (touchClone) touchClone.style.pointerEvents = 'none';
  var target = document.elementFromPoint(e.changedTouches[0].clientX, y);
  if (touchClone) touchClone.style.pointerEvents = '';

  // Remove clone
  if (touchClone && touchClone.parentNode) {
    touchClone.parentNode.removeChild(touchClone);
  }
  touchClone = null;
  touchDragEl = null;

  if (target) {
    var targetItem = target.closest('.download-item[draggable="true"]');
    if (targetItem && targetItem.getAttribute('data-id') !== dragSrcId) {
      finishDrop(targetItem, y);
      clearDragClasses();
      return;
    }
  }

  // No valid drop target — cancel
  isDragging = false;
  dragSrcId = null;
  clearDragClasses();
}

// ─── Shared Helpers ─────────────────────────────────

function clearDragClasses() {
  document.querySelectorAll('.download-item').forEach(function (el) {
    el.classList.remove('drag-over-top', 'drag-over-bottom', 'dragging');
  });
}

function showDropIndicator(targetEl, clientY) {
  var targetId = targetEl.getAttribute('data-id');
  var rect = targetEl.getBoundingClientRect();
  var isAbove = clientY < rect.top + rect.height / 2;

  document.querySelectorAll('.download-item.drag-over-top, .download-item.drag-over-bottom').forEach(function (el) {
    if (el.getAttribute('data-id') !== targetId) {
      el.classList.remove('drag-over-top', 'drag-over-bottom');
    }
  });
  targetEl.classList.remove('drag-over-top', 'drag-over-bottom');
  targetEl.classList.add(isAbove ? 'drag-over-top' : 'drag-over-bottom');
}

function finishDrop(targetEl, clientY) {
  var targetId = targetEl.getAttribute('data-id');
  if (!dragSrcId || dragSrcId === targetId) return;

  var container = document.getElementById('downloads-list');
  var items = Array.from(container.querySelectorAll('.download-item'));
  var ids = items.map(function (el) { return el.getAttribute('data-id'); });

  var srcIdx = ids.indexOf(dragSrcId);
  var dstIdx = ids.indexOf(targetId);
  if (srcIdx === -1 || dstIdx === -1) return;

  var rect = targetEl.getBoundingClientRect();
  var isAbove = clientY < rect.top + rect.height / 2;

  ids.splice(srcIdx, 1);
  var insertIdx = ids.indexOf(targetId);
  if (!isAbove) insertIdx++;
  ids.splice(insertIdx, 0, dragSrcId);

  // Optimistically reorder the DOM
  var srcEl = container.querySelector('[data-id="' + CSS.escape(dragSrcId) + '"]');
  var refEl = container.querySelector('[data-id="' + CSS.escape(targetId) + '"]');
  if (srcEl && refEl) {
    if (isAbove) {
      container.insertBefore(srcEl, refEl);
    } else {
      container.insertBefore(srcEl, refEl.nextSibling);
    }
  }

  isDragging = false;
  dragSrcId = null;
  sendReorder(ids);
}

var reorderTimer = null;
function sendReorder(orderedIds) {
  clearTimeout(reorderTimer);
  reorderTimer = setTimeout(async function () {
    try {
      await apiRequest('/downloads/reorder', {
        method: 'POST',
        body: JSON.stringify({ ids: orderedIds }),
      });
    } catch (e) {
      showToast('error', 'Reorder failed', e.message || 'Could not reorder downloads');
    }
  }, 200);
}

function renderDownloads(downloads) {
  var container = document.getElementById("downloads-list");
  if (!container) return;

  var filtered = getPageDownloads(downloads);

  if (filtered.length === 0) {
    if (currentPage === 'completed') {
      renderHtml(container, '<div class="empty-state">'
        + '<p>No completed downloads</p>'
        + '<span>Active downloads will appear here once they finish</span>'
        + '</div>');
    } else {
      renderHtml(container, '<div class="empty-state">'
        + '<p>Ready when you are</p>'
        + '<span>Paste a URL, magnet link, or .torrent path into the input above</span>'
        + '<div class="empty-protocols">'
        + '<span class="empty-proto">HTTP / HTTPS</span>'
        + '<span class="empty-proto">FTP / SFTP</span>'
        + '<span class="empty-proto">Magnet</span>'
        + '<span class="empty-proto">.torrent</span>'
        + '</div>'
        + '</div>');
    }
    lastDownloads = filtered;
    return;
  }

  // Check if the set of download IDs, statuses, or positions changed — if so, full rebuild
  var currentIds = filtered.map(function (d) { return d.id + ':' + d.status + ':' + (d.position || 0); }).join(',');
  var prevIds = lastDownloads.map(function (d) { return d.id + ':' + d.status + ':' + (d.position || 0); }).join(',');

  if (currentIds !== prevIds) {
    // Skip full rebuild if a menu is open or user is dragging
    if (openMenuId || openMoreMenuId || isDragging) {
      lastDownloads = filtered;
      return;
    }

    // Detect newly completed downloads for flash animation
    var newlyCompleted = [];
    if (lastDownloads.length > 0) {
      filtered.forEach(function (d) {
        if (d.status === 'Completed') {
          var prev = lastDownloads.find(function (p) { return p.id === d.id; });
          if (prev && prev.status !== 'Completed') {
            newlyCompleted.push(d.id);
          }
        }
      });
    }

    // Full rebuild
    var html = filtered.map(buildDownloadItem).join('');
    renderHtml(container, html);

    // Apply completion flash
    newlyCompleted.forEach(function (id) {
      var el = container.querySelector('[data-id="' + CSS.escape(id) + '"]');
      if (el) el.classList.add('just-completed');
    });

    // Restore expanded detail
    if (expandedId !== null) {
      var detail = document.getElementById('detail-' + expandedId);
      if (detail) detail.classList.add('open');
    }
    bindDragEvents();
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
      if (fill) fill.style.transform = 'scaleX(' + (progress / 100) + ')';

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
  var totalUpload = downloads.reduce(function (sum, d) {
    return sum + ((d.status === "Seeding" || d.status === "Downloading") ? d.upload_speed || 0 : 0);
  }, 0);
  var queued = downloads.filter(function (d) {
    return d.status === "Queued";
  }).length;

  var el;
  el = document.getElementById("active-count");
  if (el) el.textContent = active;
  el = document.getElementById("total-speed");
  if (el) el.textContent = formatSpeed(totalSpeed);

  // Upload stats
  var showUpload = totalUpload > 0;
  el = document.getElementById("upload-sep");
  if (el) el.style.display = showUpload ? "" : "none";
  el = document.getElementById("upload-stat");
  if (el) el.style.display = showUpload ? "" : "none";
  el = document.getElementById("total-upload");
  if (el) el.textContent = formatSpeed(totalUpload);

  // Queue stats
  var showQueue = queued > 0;
  el = document.getElementById("queue-sep");
  if (el) el.style.display = showQueue ? "" : "none";
  el = document.getElementById("queue-stat");
  if (el) el.style.display = showQueue ? "" : "none";
  el = document.getElementById("queued-count");
  if (el) el.textContent = queued;

  // Show/hide Resume All button with entrance animation
  var pausedCount = downloads.filter(function (d) {
    return d.status === "Paused" || d.status === "Failed" || d.status === "Stopped" || d.status === "Queued";
  }).length;
  var resumeBtn = document.getElementById("resume-all-btn");
  if (resumeBtn) {
    var wasHidden = resumeBtn.style.display === "none";
    resumeBtn.style.display = pausedCount > 0 ? "" : "none";
    if (pausedCount > 0 && wasHidden) {
      resumeBtn.classList.remove("resume-all-enter");
      void resumeBtn.offsetHeight;
      resumeBtn.classList.add("resume-all-enter");
    }
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
    showToast("error", "Failed to load profile", e.message);
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

  if (newPassword.length < 8) {
    showToast(
      "error",
      "Weak password",
      "Password must be at least 8 characters",
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
    showToast("error", "Failed to load settings", e.message);
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
      '<div class="table-scroll-wrapper">' +
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

    userTable += "</tbody>" + "</table>" + "</div>" + "</div>" + "</div>";

    renderHtml(userSection, userTable);

    // Hide scroll fade when table is scrolled to the end
    var scrollWrapper = userSection.querySelector('.table-scroll-wrapper');
    var scrollTable = scrollWrapper ? scrollWrapper.querySelector('.user-table') : null;
    if (scrollTable) {
      scrollTable.addEventListener('scroll', function () {
        var atEnd = scrollTable.scrollLeft + scrollTable.clientWidth >= scrollTable.scrollWidth - 2;
        scrollWrapper.classList.toggle('scrolled-end', atEnd);
        scrollWrapper.classList.toggle('scrolled-start', scrollTable.scrollLeft <= 2);
      });
      // If table fits without scrolling, hide the fade
      if (scrollTable.scrollWidth <= scrollTable.clientWidth) {
        scrollWrapper.classList.add('scrolled-end');
      }
    }

    // Bind the create user form
    var createUserForm = document.getElementById("create-user-form");
    if (createUserForm) {
      createUserForm.addEventListener("submit", function (e) {
        e.preventDefault();
        createUser();
      });
    }
  } catch (e) {
    showToast("error", "Failed to load users", e.message);
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

  if (password.length < 8) {
    showToast(
      "error",
      "Weak password",
      "Password must be at least 8 characters",
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
    showToast("error", "Failed to load history", e.message);
  }
}

function renderHistory(items) {
  var container = document.getElementById("history-list");
  if (!container) return;

  if (!items || items.length === 0) {
    renderHtml(
      container,
      '<div class="empty-state">' +
      "<p>No history yet</p>" +
      "<span>Completed and removed downloads are logged here for reference</span>" +
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
        'aria-label="Select ' + escapeHtml(h.filename) + '" ' +
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
        '\')" title="Remove from history" aria-label="Remove from history">' +
        '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/></svg>' +
        "</button>" +
        "</div>"
      );
    })
    .join("");

  renderHtml(container, html);
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
    await Promise.all(ids.map(function (id) {
      return apiRequest("/history/" + encodeURIComponent(id), {
        method: "DELETE",
      });
    }));
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
  var btn = document.querySelector('#add-download-form .btn-primary');
  if (btn) { btn.disabled = true; btn.textContent = "Adding\u2026"; }
  try {
    await apiRequest("/downloads", {
      method: "POST",
      body: JSON.stringify({ url: url }),
    });
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Failed to add download", e.message);
  } finally {
    if (btn) { btn.disabled = false; btn.textContent = "Download"; }
  }
}

function closeAllMenus() {
  document.querySelectorAll('.delete-menu.show').forEach(function (m) { m.classList.remove('show'); });
  document.querySelectorAll('.delete-btn[aria-expanded="true"]').forEach(function (b) { b.setAttribute('aria-expanded', 'false'); });
  document.querySelectorAll('.more-menu.show').forEach(function (m) { m.classList.remove('show'); });
  document.querySelectorAll('.more-btn[aria-expanded="true"]').forEach(function (b) { b.setAttribute('aria-expanded', 'false'); });
  openMenuId = null;
  openMoreMenuId = null;
}

function focusFirstMenuItem(menu) {
  var first = menu.querySelector('button, a, [role="menuitem"]');
  if (first) first.focus();
}

function handleMenuKeydown(e, triggerBtnId) {
  var menu = e.currentTarget;
  var items = Array.prototype.slice.call(menu.querySelectorAll('button, a, [role="menuitem"]'));
  if (items.length === 0) return;
  var idx = items.indexOf(document.activeElement);

  if (e.key === 'ArrowDown') {
    e.preventDefault();
    items[(idx + 1) % items.length].focus();
  } else if (e.key === 'ArrowUp') {
    e.preventDefault();
    items[(idx - 1 + items.length) % items.length].focus();
  } else if (e.key === 'Escape') {
    e.preventDefault();
    closeAllMenus();
    var trigger = document.getElementById(triggerBtnId);
    if (trigger) trigger.focus();
  }
}

function toggleDeleteMenu(event, id) {
  event.stopPropagation();
  var wasOpen = openMenuId === id;
  closeAllMenus();

  if (wasOpen) {
    return;
  }

  var menu = document.getElementById("delete-menu-" + id);
  var btn = document.getElementById("delete-btn-" + id);
  if (menu) {
    menu.classList.add("show");
    if (btn) btn.setAttribute('aria-expanded', 'true');
    openMenuId = id;
    menu.onkeydown = function (e) { handleMenuKeydown(e, 'delete-btn-' + id); };
    focusFirstMenuItem(menu);
  }
}

function toggleMoreMenu(event, id) {
  event.stopPropagation();
  var wasOpen = openMoreMenuId === id;
  closeAllMenus();

  if (wasOpen) {
    return;
  }
  var menu = document.getElementById('more-menu-' + id);
  var btn = document.getElementById('more-btn-' + id);
  if (menu) {
    menu.classList.add('show');
    if (btn) btn.setAttribute('aria-expanded', 'true');
    openMoreMenuId = id;
    menu.onkeydown = function (e) { handleMenuKeydown(e, 'more-btn-' + id); };
    focusFirstMenuItem(menu);
  }
}

async function copyMagnet(event, id) {
  event.stopPropagation();
  closeAllMenus();

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
  closeAllMenus();
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
  closeAllMenus();
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
    showToast('error', 'Missing URL', 'Please enter a mirror URL');
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
    loadDownloads();
  })
  .catch(function(err) {
    showToast('error', 'Mirror failed', err.message || 'Could not start HTTP mirror');
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

async function downloadAction(id, action) {
  try {
    await apiRequest("/downloads/" + encodeURIComponent(id) + "/" + action, {
      method: "POST",
    });
    lastDownloads = [];
    loadDownloads();
  } catch (e) {
    showToast("error", "Failed to " + action, e.message);
  }
}

function pauseDownload(id) { downloadAction(id, "pause"); }
function cancelDownload(id) { downloadAction(id, "cancel"); }
function resumeDownload(id) { downloadAction(id, "resume"); }

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
  modal.setAttribute("role", "dialog");
  modal.setAttribute("aria-modal", "true");
  modal.setAttribute("aria-label", "Select folder");

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

function createFolderItem(label, iconSvg, onActivate) {
  var div = document.createElement("div");
  div.className = "folder-item" + (label === ".." ? " folder-parent" : "");
  div.tabIndex = 0;
  div.setAttribute("role", "button");
  div.onclick = onActivate;
  div.onkeydown = function (e) {
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      onActivate();
    }
  };
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
        : hash === "#settings"
          ? "settings"
          : hash === "#profile"
            ? "profile"
            : "downloads";
  lastDownloads = [];
  expandedId = null;
  selectedHistoryIds.clear();
  var main = document.getElementById("main-content");
  main.classList.remove("page-enter");
  void main.offsetHeight;
  renderHtml(main, render());
  main.classList.add("page-enter");

  // Update active nav
  document.querySelectorAll(".nav-tab").forEach(function (item) {
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
  if (hint) { hint.textContent = ""; hint.style.color = ""; }
  var subtitle = document.querySelector(".login-subtitle");
  if (subtitle) subtitle.textContent = "";

  document.getElementById("login-modal").style.display = "flex";
  document.getElementById("login-username").focus();
}

// ─── Init ───────────────────────────────────────────

async function checkFirstUser() {
  try {
    var result = await apiRequest("/auth/status");
    return result.needs_setup === true;
  } catch (e) {
    return null; // null = connection error, false = no setup needed
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
      var btnLabel = btn.textContent;
      btn.disabled = true;
      btn.textContent = btn.dataset.mode === "register" ? "Creating\u2026" : "Signing in\u2026";

      if (btn.dataset.mode === "register") {
        // First user registration
        if (password.length < 8) {
          showToast(
            "error",
            "Weak password",
            "Password must be at least 8 characters",
          );
          btn.disabled = false;
          btn.textContent = btnLabel;
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
            showToast("success", "Welcome to DLoad", "Paste any URL, magnet link, or .torrent to start downloading");
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
      btn.disabled = false;
      btn.textContent = btnLabel;
    });
  }

  if (!token) {
    hideAppLoading();
    document.getElementById("login-modal").style.display = "flex";
    document.getElementById("login-username").focus();
    // Check if this is the first user
    checkFirstUser().then(function (isFirst) {
      if (isFirst === null) {
        // Connection failed — show error in login hint
        var hint = document.getElementById("login-hint");
        if (hint) hint.textContent = "Cannot reach server. Check your connection and refresh.";
        if (hint) hint.style.color = "var(--danger)";
      } else if (isFirst) {
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
  hideAppLoading();

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

  // Pause polling when tab is hidden to save battery/bandwidth
  if (!window._visibilityListenerAdded) {
    document.addEventListener('visibilitychange', function () {
      if (document.hidden) {
        if (refreshInterval) { clearInterval(refreshInterval); refreshInterval = null; }
      } else {
        loadDownloads();
        if (!refreshInterval) { refreshInterval = setInterval(loadDownloads, 1000); }
      }
    });
    window._visibilityListenerAdded = true;
  }
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
    // Silent failure — user info will refresh on next navigation
  }
}

// Close menus when clicking outside
document.addEventListener('click', function (e) {
  if (!e.target.closest('.delete-dropdown') && !e.target.closest('.more-dropdown')) {
    closeAllMenus();
  } else if (!e.target.closest('.delete-dropdown')) {
    document.querySelectorAll('.delete-menu.show').forEach(function (m) { m.classList.remove('show'); });
    document.querySelectorAll('.delete-btn[aria-expanded="true"]').forEach(function (b) { b.setAttribute('aria-expanded', 'false'); });
    openMenuId = null;
  } else if (!e.target.closest('.more-dropdown')) {
    document.querySelectorAll('.more-menu.show').forEach(function (m) { m.classList.remove('show'); });
    document.querySelectorAll('.more-btn[aria-expanded="true"]').forEach(function (b) { b.setAttribute('aria-expanded', 'false'); });
    openMoreMenuId = null;
  }
});

// Close menus and modals on Escape, trap focus in login modal
document.addEventListener('keydown', function (e) {
  if (e.key === 'Escape') {
    if (openMenuId || openMoreMenuId) {
      var focusBtn = openMenuId
        ? document.getElementById('delete-btn-' + openMenuId)
        : document.getElementById('more-btn-' + openMoreMenuId);
      closeAllMenus();
      if (focusBtn) focusBtn.focus();
      return;
    }
    if (folderBrowserModal) {
      closeFolderBrowser();
    }
  }
  // Focus trap for login modal
  var loginModal = document.getElementById('login-modal');
  if (loginModal && loginModal.style.display === 'flex' && e.key === 'Tab') {
    var focusable = loginModal.querySelectorAll('input:not([type="hidden"]), button, [tabindex]:not([tabindex="-1"])');
    if (focusable.length === 0) return;
    var first = focusable[0];
    var last = focusable[focusable.length - 1];
    if (e.shiftKey) {
      if (document.activeElement === first) { e.preventDefault(); last.focus(); }
    } else {
      if (document.activeElement === last) { e.preventDefault(); first.focus(); }
    }
  }
});

document.addEventListener("DOMContentLoaded", init);

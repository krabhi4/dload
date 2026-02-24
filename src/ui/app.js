var API_BASE = '/api';
var token = localStorage.getItem('dload_token') || '';
var refreshInterval = null;
var lastDownloads = [];
var openMenuId = null;
var currentFilter = 'all';
var expandedId = null;

// ─── API ────────────────────────────────────────────

async function apiRequest(endpoint, options) {
    options = options || {};
    var headers = {
        'Content-Type': 'application/json'
    };
    if (token) {
        headers['Authorization'] = 'Bearer ' + token;
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
        method: options.method || 'GET',
        headers: headers,
        body: options.body || undefined
    });

    if (response.status === 401) {
        logout();
        throw new Error('Unauthorized');
    }

    return response.json();
}

// ─── Safe DOM helpers ───────────────────────────────

function escapeHtml(str) {
    var div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}

function safeRender(container, html) {
    container.innerHTML = html;
}

// ─── Formatting ─────────────────────────────────────

function formatSize(bytes) {
    if (!bytes || bytes === 0) return '0 B';
    var k = 1024;
    var sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
    var i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
}

function formatSpeed(bytesPerSec) {
    if (!bytesPerSec || bytesPerSec === 0) return '0 B/s';
    return formatSize(bytesPerSec) + '/s';
}

// ─── Views ──────────────────────────────────────────

function showDownloads() {
    return '<div class="stats-bar">'
        + '<span class="stat-val" id="active-count">0</span> Active'
        + ' <span class="stat-sep">&middot;</span> '
        + '<span class="stat-val" id="completed-count">0</span> Completed'
        + ' <span class="stat-sep">&middot;</span> '
        + '&darr; <span class="stat-val speed-val" id="total-speed">0 B/s</span>'
        + '</div>'
        + '<div class="add-section">'
        + '<form id="add-download-form" class="input-group">'
        + '<input type="text" id="download-url" placeholder="Paste URL — http, ftp, sftp, magnet, or .torrent" required>'
        + '<button type="submit" class="btn-primary">Download</button>'
        + '</form>'
        + '</div>'
        + '<div class="filter-tabs">'
        + '<button class="filter-tab active" data-filter="all" onclick="setFilter(\'all\')">All <span class="count">0</span></button>'
        + '<button class="filter-tab" data-filter="downloading" onclick="setFilter(\'downloading\')">Downloading <span class="count">0</span></button>'
        + '<button class="filter-tab" data-filter="completed" onclick="setFilter(\'completed\')">Completed <span class="count">0</span></button>'
        + '<button class="filter-tab" data-filter="failed" onclick="setFilter(\'failed\')">Failed <span class="count">0</span></button>'
        + '</div>'
        + '<div id="downloads-list"></div>';
}

function showSettings() {
    return '<div class="page-header">'
        + '<h1>Settings</h1>'
        + '<p>Configure your download manager</p>'
        + '</div>'
        + '<form id="settings-form" class="settings-grid">'
        + '<div class="form-field">'
        + '<label for="settings-dir">Download Directory</label>'
        + '<input type="text" id="settings-dir" value="/downloads">'
        + '<span class="hint">Path inside the container where files are saved</span>'
        + '</div>'
        + '<div class="form-field">'
        + '<label for="settings-max-concurrent">Max Concurrent Downloads</label>'
        + '<input type="number" id="settings-max-concurrent" value="3" min="1" max="10">'
        + '</div>'
        + '<div class="form-field">'
        + '<label for="settings-connections">Max Connections Per File</label>'
        + '<input type="number" id="settings-connections" value="4" min="1" max="16">'
        + '</div>'
        + '<div class="settings-actions">'
        + '<button type="submit" class="btn-primary">Save Settings</button>'
        + '</div>'
        + '</form>';
}

// ─── Filter & Detail ────────────────────────────────

function setFilter(filter) {
    currentFilter = filter;
    lastDownloads = [];
    loadDownloads();
}

function sortDownloads(downloads) {
    var statusOrder = {
        'Downloading': 0,
        'Queued': 1,
        'Paused': 2,
        'Stopped': 3,
        'Failed': 4,
        'Completed': 5
    };
    return downloads.slice().sort(function(a, b) {
        var oa = statusOrder[a.status] !== undefined ? statusOrder[a.status] : 99;
        var ob = statusOrder[b.status] !== undefined ? statusOrder[b.status] : 99;
        if (oa !== ob) return oa - ob;
        // Within same status, newest first
        return new Date(b.created_at) - new Date(a.created_at);
    });
}

function filterDownloads(downloads, filter) {
    var sorted = sortDownloads(downloads);
    if (filter === 'all') return sorted;
    if (filter === 'downloading') return sorted.filter(function(d) { return d.status === 'Downloading' || d.status === 'Queued'; });
    if (filter === 'completed') return sorted.filter(function(d) { return d.status === 'Completed'; });
    if (filter === 'failed') return sorted.filter(function(d) { return d.status === 'Failed' || d.status === 'Stopped' || d.status === 'Paused'; });
    return sorted;
}

function toggleDetail(id, event) {
    if (event.target.closest('button') || event.target.closest('.actions') || event.target.closest('.delete-dropdown')) {
        return;
    }
    if (expandedId === id) {
        expandedId = null;
    } else {
        expandedId = id;
    }
    document.querySelectorAll('.download-detail').forEach(function(el) {
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
    var progressClass = statusClass === 'completed' ? 'completed' : statusClass === 'failed' ? 'failed' : '';
    var safeId = escapeHtml(d.id);
    var safeName = escapeHtml(d.filename);
    var safeUrl = escapeHtml(d.url);
    var safeProtocol = escapeHtml(d.protocol);
    var safeStatus = escapeHtml(d.status);
    var progress = Math.min(d.progress, 100);
    var isActive = d.status === 'Downloading';
    var isTorrent = d.protocol === 'Torrent';

    if (d.status === 'Completed' && d.total_size === 0 && d.downloaded_size > 0) {
        progress = 100;
    }

    // Protocol icon
    var protocolIcon;
    if (isTorrent) {
        protocolIcon = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">'
            + '<path d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2z"/>'
            + '<path d="M8 12l2 2 4-4"/>'
            + '</svg>';
    } else {
        protocolIcon = '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">'
            + '<circle cx="12" cy="12" r="10"/>'
            + '<path d="M2 12h20M12 2a15.3 15.3 0 014 10 15.3 15.3 0 01-4 10 15.3 15.3 0 01-4-10 15.3 15.3 0 014-10z"/>'
            + '</svg>';
    }

    // Size display
    var sizeDisplay;
    if (d.status === 'Completed') {
        sizeDisplay = formatSize(d.total_size || d.downloaded_size);
    } else if (isActive && d.total_size > 0) {
        sizeDisplay = formatSize(d.downloaded_size) + ' / ' + formatSize(d.total_size);
    } else {
        sizeDisplay = formatSize(d.downloaded_size);
    }

    // Speed display
    var displaySpeed = isActive ? formatSpeed(d.speed) : '--';

    // ETA display
    var etaDisplay = (isActive && d.eta) ? escapeHtml(d.eta) : '';

    // Action buttons
    var actions = '';
    if (isActive) {
        actions = '<button class="action-btn pause-btn" onclick="event.stopPropagation(); pauseDownload(\'' + safeId + '\')" title="Pause">'
            + '<svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>'
            + '</button>'
            + '<button class="action-btn cancel-btn" onclick="event.stopPropagation(); cancelDownload(\'' + safeId + '\')" title="Cancel">'
            + '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>'
            + '</button>';
    }

    // Detail panel
    var detailRows = '<span class="detail-label">URL</span>'
        + '<span class="detail-value">' + safeUrl + '</span>'
        + '<span class="detail-label">Save Path</span>'
        + '<span class="detail-value">' + escapeHtml(d.save_path || '') + '</span>'
        + '<span class="detail-label">Protocol</span>'
        + '<span class="detail-value">' + safeProtocol + '</span>'
        + '<span class="detail-label">Created</span>'
        + '<span class="detail-value">' + (d.created_at ? escapeHtml(new Date(d.created_at).toLocaleString()) : '') + '</span>';

    if (d.completed_at) {
        detailRows += '<span class="detail-label">Completed</span>'
            + '<span class="detail-value">' + escapeHtml(new Date(d.completed_at).toLocaleString()) + '</span>';
    }

    if (isTorrent) {
        detailRows += '<span class="detail-label">Peers</span>'
            + '<span class="detail-value">' + (d.peers || 0) + '</span>'
            + '<span class="detail-label">Seeds</span>'
            + '<span class="detail-value">' + (d.seeds || 0) + '</span>'
            + '<span class="detail-label">Upload Speed</span>'
            + '<span class="detail-value">' + formatSpeed(d.upload_speed) + '</span>';
    }

    if (d.error_message) {
        detailRows += '<span class="detail-label">Error</span>'
            + '<span class="detail-value error">' + escapeHtml(d.error_message) + '</span>';
    }

    var isExpanded = (expandedId === d.id);

    return '<div class="download-item ' + statusClass + '" data-id="' + safeId + '" onclick="toggleDetail(\'' + safeId + '\', event)">'
        + '<div class="download-row">'
        +   '<div class="protocol-icon">' + protocolIcon + '</div>'
        +   '<div class="download-info">'
        +     '<div class="download-name">' + safeName + '</div>'
        +     '<div class="download-url" title="' + safeUrl + '">' + safeUrl + '</div>'
        +   '</div>'
        +   '<div class="download-metrics">'
        +     '<span>' + sizeDisplay + '</span>'
        +     '<span class="speed">' + displaySpeed + '</span>'
        +     '<span class="eta">' + etaDisplay + '</span>'
        +   '</div>'
        +   '<span class="status-badge ' + statusClass + '">' + safeStatus + '</span>'
        +   '<div class="actions">'
        +     actions
        +     '<div class="delete-dropdown">'
        +       '<button class="delete-btn" onclick="toggleDeleteMenu(event, \'' + safeId + '\')" title="Remove">'
        +         '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">'
        +           '<path d="M3 6h18"/><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/>'
        +         '</svg>'
        +       '</button>'
        +       '<div class="delete-menu" id="delete-menu-' + safeId + '">'
        +         '<button onclick="event.stopPropagation(); deleteDownload(\'' + safeId + '\', false)">Remove from list</button>'
        +         '<button class="danger" onclick="event.stopPropagation(); deleteDownload(\'' + safeId + '\', true)">Delete from disk</button>'
        +       '</div>'
        +     '</div>'
        +   '</div>'
        + '</div>'
        + '<div class="download-progress">'
        +   '<div class="progress-bar">'
        +     '<div class="progress-fill ' + progressClass + '" style="width: ' + progress + '%"></div>'
        +   '</div>'
        + '</div>'
        + '<div class="download-detail' + (isExpanded ? ' open' : '') + '" id="detail-' + safeId + '">'
        +   '<div class="detail-content">'
        +     '<div class="detail-grid">'
        +       detailRows
        +     '</div>'
        +   '</div>'
        + '</div>'
        + '</div>';
}

function renderDownloads(downloads) {
    var container = document.getElementById('downloads-list');
    if (!container) return;

    var filtered = filterDownloads(downloads, currentFilter);

    // Update filter tab counts and active state
    var allCount = downloads.length;
    var downloadingCount = downloads.filter(function(d) { return d.status === 'Downloading' || d.status === 'Queued'; }).length;
    var completedCount = downloads.filter(function(d) { return d.status === 'Completed'; }).length;
    var failedCount = downloads.filter(function(d) { return d.status === 'Failed' || d.status === 'Stopped' || d.status === 'Paused'; }).length;

    var tabs = document.querySelectorAll('.filter-tab');
    tabs.forEach(function(tab) {
        var f = tab.getAttribute('data-filter');
        tab.classList.toggle('active', f === currentFilter);
        var countEl = tab.querySelector('.count');
        if (countEl) {
            if (f === 'all') countEl.textContent = allCount;
            else if (f === 'downloading') countEl.textContent = downloadingCount;
            else if (f === 'completed') countEl.textContent = completedCount;
            else if (f === 'failed') countEl.textContent = failedCount;
        }
    });

    if (filtered.length === 0) {
        safeRender(container, '<div class="empty-state">'
            + '<svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">'
            + '<path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4"/>'
            + '<polyline points="7 10 12 15 17 10"/>'
            + '<line x1="12" y1="15" x2="12" y2="3"/>'
            + '</svg>'
            + '<p>No downloads yet</p>'
            + '<span>Add a URL above to start downloading</span>'
            + '</div>');
        lastDownloads = filtered;
        return;
    }

    // Check if the set of download IDs or their statuses changed — if so, full rebuild
    var currentIds = filtered.map(function(d) { return d.id + ':' + d.status; }).join(',');
    var prevIds = lastDownloads.map(function(d) { return d.id + ':' + d.status; }).join(',');

    if (currentIds !== prevIds) {
        // Full rebuild
        var html = filtered.map(buildDownloadItem).join('');
        safeRender(container, html);
        // Restore open menu
        if (openMenuId) {
            var menu = document.getElementById('delete-menu-' + openMenuId);
            if (menu) menu.classList.add('show');
        }
        // Restore expanded detail
        if (expandedId !== null) {
            var detail = document.getElementById('detail-' + expandedId);
            if (detail) detail.classList.add('open');
        }
    } else {
        // Incremental update — only update changing values in-place
        filtered.forEach(function(d) {
            var el = container.querySelector('[data-id="' + CSS.escape(d.id) + '"]');
            if (!el) return;

            var progress = Math.min(d.progress, 100);
            if (d.status === 'Completed' && d.total_size === 0 && d.downloaded_size > 0) {
                progress = 100;
            }
            var isActive = d.status === 'Downloading';

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
                speedEl.textContent = isActive ? formatSpeed(d.speed) : '--';
            }

            // Update ETA
            var etaEl = el.querySelector('.eta');
            if (etaEl) {
                etaEl.textContent = (isActive && d.eta) ? d.eta : '';
            }
        });
    }

    lastDownloads = filtered;
}

function updateStats(downloads) {
    var active = downloads.filter(function(d) { return d.status === 'Downloading'; }).length;
    var completed = downloads.filter(function(d) { return d.status === 'Completed'; }).length;
    var totalSpeed = downloads.reduce(function(sum, d) {
        return sum + (d.status === 'Downloading' ? (d.speed || 0) : 0);
    }, 0);

    var el;
    el = document.getElementById('active-count');
    if (el) el.textContent = active;
    el = document.getElementById('completed-count');
    if (el) el.textContent = completed;
    el = document.getElementById('total-speed');
    if (el) el.textContent = formatSpeed(totalSpeed);
}

// ─── Data Loading ───────────────────────────────────

async function loadDownloads() {
    try {
        var downloads = await apiRequest('/downloads');
        renderDownloads(downloads);
        updateStats(downloads);
    } catch (e) {
        console.error('Failed to load downloads:', e);
    }
}

async function loadSettings() {
    try {
        var settings = await apiRequest('/settings');
        var el;
        el = document.getElementById('settings-dir');
        if (el) el.value = settings.download_dir;
        el = document.getElementById('settings-max-concurrent');
        if (el) el.value = settings.max_concurrent;
        el = document.getElementById('settings-connections');
        if (el) el.value = settings.max_connections_per_file;
    } catch (e) {
        console.error('Failed to load settings:', e);
    }
}

// ─── Actions ────────────────────────────────────────

async function addDownload(url) {
    try {
        await apiRequest('/downloads', {
            method: 'POST',
            body: JSON.stringify({ url: url })
        });
        lastDownloads = [];
        loadDownloads();
    } catch (e) {
        console.error('Failed to add download:', e);
    }
}

function toggleDeleteMenu(event, id) {
    event.stopPropagation();
    // Close all other menus
    document.querySelectorAll('.delete-menu.show').forEach(function(m) { m.classList.remove('show'); });

    if (openMenuId === id) {
        openMenuId = null;
        return;
    }

    var menu = document.getElementById('delete-menu-' + id);
    if (menu) {
        menu.classList.add('show');
        openMenuId = id;
    }
}

async function deleteDownload(id, deleteFiles) {
    openMenuId = null;
    try {
        var qs = deleteFiles ? '?delete_files=true' : '';
        await apiRequest('/downloads/' + encodeURIComponent(id) + qs, { method: 'DELETE' });
        lastDownloads = [];
        loadDownloads();
    } catch (e) {
        console.error('Failed to delete download:', e);
    }
}

async function pauseDownload(id) {
    try {
        await apiRequest('/downloads/' + encodeURIComponent(id) + '/pause', { method: 'POST' });
        lastDownloads = [];
        loadDownloads();
    } catch (e) {
        console.error('Failed to pause download:', e);
    }
}

async function cancelDownload(id) {
    try {
        await apiRequest('/downloads/' + encodeURIComponent(id) + '/cancel', { method: 'POST' });
        lastDownloads = [];
        loadDownloads();
    } catch (e) {
        console.error('Failed to cancel download:', e);
    }
}

async function saveSettings() {
    var settings = {
        download_dir: document.getElementById('settings-dir').value,
        max_concurrent: parseInt(document.getElementById('settings-max-concurrent').value),
        max_connections_per_file: parseInt(document.getElementById('settings-connections').value),
        chunk_size: 131072,
        username: 'admin',
        port: 8080,
    };

    try {
        await apiRequest('/settings', {
            method: 'PUT',
            body: JSON.stringify(settings)
        });
    } catch (e) {
        console.error('Failed to save settings:', e);
    }
}

// ─── Navigation ─────────────────────────────────────

function navigate(hash) {
    var routes = {
        '#downloads': showDownloads,
        '#settings': showSettings
    };

    var render = routes[hash] || showDownloads;
    lastDownloads = [];
    currentFilter = 'all';
    expandedId = null;
    safeRender(document.getElementById('main-content'), render());

    // Update active nav
    document.querySelectorAll('.nav-item').forEach(function(item) {
        item.classList.toggle('active', item.getAttribute('href') === hash);
    });

    // Bind forms after render
    bindForms();

    // Load data
    if (hash === '#settings') {
        loadSettings();
    } else {
        loadDownloads();
    }
}

function bindForms() {
    var addForm = document.getElementById('add-download-form');
    if (addForm) {
        addForm.addEventListener('submit', function(e) {
            e.preventDefault();
            var input = document.getElementById('download-url');
            if (input.value.trim()) {
                addDownload(input.value.trim());
                input.value = '';
            }
        });
    }

    var settingsForm = document.getElementById('settings-form');
    if (settingsForm) {
        settingsForm.addEventListener('submit', function(e) {
            e.preventDefault();
            saveSettings();
        });
    }
}

// ─── Auth ───────────────────────────────────────────

function logout() {
    token = '';
    localStorage.removeItem('dload_token');
    if (refreshInterval) {
        clearInterval(refreshInterval);
        refreshInterval = null;
    }
    document.getElementById('login-modal').style.display = 'flex';
}

// ─── Init ───────────────────────────────────────────

function init() {
    var loginForm = document.getElementById('login-form');
    if (loginForm) {
        loginForm.addEventListener('submit', async function(e) {
            e.preventDefault();
            var username = document.getElementById('login-username').value;
            var password = document.getElementById('login-password').value;

            try {
                var result = await apiRequest('/auth/login', {
                    method: 'POST',
                    body: JSON.stringify({ username: username, password: password })
                });

                if (result.success) {
                    token = result.token;
                    localStorage.setItem('dload_token', token);
                    document.getElementById('login-modal').style.display = 'none';
                    startApp();
                } else {
                    document.getElementById('login-password').value = '';
                    document.getElementById('login-password').focus();
                }
            } catch (e) {
                console.error('Login failed:', e);
            }
        });
    }

    if (!token) {
        document.getElementById('login-modal').style.display = 'flex';
        return;
    }

    startApp();
}

function startApp() {
    navigate(window.location.hash || '#downloads');
    window.addEventListener('hashchange', function() { navigate(window.location.hash); });

    if (refreshInterval) clearInterval(refreshInterval);
    refreshInterval = setInterval(loadDownloads, 1000);
}

// Close delete menus when clicking outside
document.addEventListener('click', function(e) {
    if (!e.target.closest('.delete-dropdown')) {
        document.querySelectorAll('.delete-menu.show').forEach(function(m) { m.classList.remove('show'); });
        openMenuId = null;
    }
});

document.addEventListener('DOMContentLoaded', init);

const API_BASE = '/api';
let token = localStorage.getItem('dload_token') || '';
let refreshInterval = null;
let lastDownloadsJson = '';

// ─── API ────────────────────────────────────────────

async function apiRequest(endpoint, options = {}) {
    const headers = {
        'Content-Type': 'application/json',
        ...(token && { 'Authorization': `Bearer ${token}` }),
        ...options.headers
    };

    const response = await fetch(`${API_BASE}${endpoint}`, {
        ...options,
        headers
    });

    if (response.status === 401) {
        logout();
        throw new Error('Unauthorized');
    }

    return response.json();
}

// ─── Safe DOM helpers ───────────────────────────────

function escapeHtml(str) {
    const div = document.createElement('div');
    div.textContent = str;
    return div.innerHTML;
}

// All dynamic data inserted into templates is escaped via escapeHtml()
// to prevent XSS. The templates themselves are static strings defined
// in this file — no user input flows into them unescaped.

function safeRender(container, html) {
    container.innerHTML = html;
}

// ─── Views ──────────────────────────────────────────

function showDashboard() {
    return `
        <div class="page-header">
            <h1>Dashboard</h1>
            <p>Monitor and manage your downloads</p>
        </div>

        <div class="stats-grid">
            <div class="stat-card">
                <div class="stat-label">Active</div>
                <div class="stat-value accent" id="active-count">0</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Completed</div>
                <div class="stat-value info" id="completed-count">0</div>
            </div>
            <div class="stat-card">
                <div class="stat-label">Speed</div>
                <div class="stat-value warning" id="total-speed">0 B/s</div>
            </div>
        </div>

        <div class="add-section">
            <h3>New Download</h3>
            <form id="add-download-form" class="input-group">
                <input type="url" id="download-url" placeholder="Paste URL — http, ftp, sftp, magnet, or .torrent" required>
                <button type="submit" class="btn-primary">Download</button>
            </form>
        </div>

        <div class="section-header">
            <span class="section-title">Downloads</span>
        </div>
        <div id="downloads-list"></div>
    `;
}

function showTorrents() {
    return `
        <div class="page-header">
            <h1>Torrents</h1>
            <p>Manage torrent downloads</p>
        </div>

        <div class="add-section">
            <h3>Add Torrent</h3>
            <form id="add-torrent-form" class="input-group">
                <input type="url" id="torrent-url" placeholder="Paste magnet link or .torrent URL" required>
                <button type="submit" class="btn-primary">Add Torrent</button>
            </form>
        </div>

        <div class="section-header">
            <span class="section-title">Active Torrents</span>
        </div>
        <div id="torrents-list"></div>
    `;
}

function showSettings() {
    return `
        <div class="page-header">
            <h1>Settings</h1>
            <p>Configure your download manager</p>
        </div>

        <form id="settings-form" class="settings-grid">
            <div class="form-field">
                <label for="settings-dir">Download Directory</label>
                <input type="text" id="settings-dir" value="/data">
                <span class="hint">Path inside the container where files are saved</span>
            </div>
            <div class="form-field">
                <label for="settings-max-concurrent">Max Concurrent Downloads</label>
                <input type="number" id="settings-max-concurrent" value="3" min="1" max="10">
            </div>
            <div class="form-field">
                <label for="settings-connections">Max Connections Per File</label>
                <input type="number" id="settings-connections" value="4" min="1" max="16">
            </div>
            <div class="settings-actions">
                <button type="submit" class="btn-primary">Save Settings</button>
            </div>
        </form>
    `;
}

// ─── Rendering ──────────────────────────────────────

function renderDownloads(downloads, containerId) {
    containerId = containerId || 'downloads-list';
    const container = document.getElementById(containerId);
    if (!container) return;

    if (downloads.length === 0) {
        safeRender(container, `
            <div class="empty-state">
                <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4"/>
                    <polyline points="7 10 12 15 17 10"/>
                    <line x1="12" y1="15" x2="12" y2="3"/>
                </svg>
                <p>No downloads yet</p>
                <span>Add a URL above to start downloading</span>
            </div>
        `);
        return;
    }

    // All dynamic values (d.filename, d.url, d.id, etc.) are escaped
    var html = downloads.map(function(d, i) {
        var statusClass = escapeHtml(d.status.toLowerCase());
        var progressClass = statusClass === 'completed' ? 'completed' : statusClass === 'failed' ? 'failed' : '';
        var safeId = escapeHtml(d.id);
        var safeName = escapeHtml(d.filename);
        var safeUrl = escapeHtml(d.url);
        var safeProtocol = escapeHtml(d.protocol);
        var safeStatus = escapeHtml(d.status);
        var progress = Math.min(d.progress, 100);
        // Show speed only for active downloads
        var displaySpeed = (d.status === 'Downloading') ? formatSpeed(d.speed) : '--';
        // Completed with unknown total: show downloaded as total, 100%
        if (d.status === 'Completed' && d.total_size === 0 && d.downloaded_size > 0) {
            progress = 100;
        }

        return '<div class="download-item ' + statusClass + '">'
            + '<div class="download-top">'
            +   '<div>'
            +     '<div class="download-name">' + safeName + '</div>'
            +     '<div class="download-url" title="' + safeUrl + '">' + safeUrl + '</div>'
            +   '</div>'
            +   '<div class="download-meta">'
            +     '<span class="protocol-badge">' + safeProtocol + '</span>'
            +     '<span class="status-badge ' + statusClass + '">' + safeStatus + '</span>'
            +     '<div class="delete-dropdown">'
            +       '<button class="delete-btn" onclick="toggleDeleteMenu(\'' + safeId + '\')" title="Remove">'
            +         '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">'
            +           '<path d="M3 6h18"/><path d="M19 6v14a2 2 0 01-2 2H7a2 2 0 01-2-2V6m3 0V4a2 2 0 012-2h4a2 2 0 012 2v2"/>'
            +         '</svg>'
            +       '</button>'
            +       '<div class="delete-menu" id="delete-menu-' + safeId + '">'
            +         '<button onclick="deleteDownload(\'' + safeId + '\', false)">Remove from list</button>'
            +         '<button class="danger" onclick="deleteDownload(\'' + safeId + '\', true)">Delete from disk</button>'
            +       '</div>'
            +     '</div>'
            +   '</div>'
            + '</div>'
            + '<div class="download-progress-row">'
            +   '<div class="progress-bar">'
            +     '<div class="progress-fill ' + progressClass + '" style="width: ' + progress + '%"></div>'
            +   '</div>'
            +   '<div class="download-stats">'
            +     '<span>' + formatSize(d.downloaded_size) + (d.total_size > 0 ? ' / ' + formatSize(d.total_size) : '') + '</span>'
            +     '<span class="speed">' + displaySpeed + '</span>'
            +     '<span class="percent">' + progress.toFixed(1) + '%</span>'
            +   '</div>'
            + '</div>'
            + '</div>';
    }).join('');

    safeRender(container, html);
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
        var json = JSON.stringify(downloads);

        // Skip re-render if nothing changed — prevents flicker
        if (json === lastDownloadsJson) return;
        lastDownloadsJson = json;

        var hash = window.location.hash || '#dashboard';

        if (hash === '#torrents') {
            var torrents = downloads.filter(function(d) { return d.protocol === 'Torrent'; });
            renderDownloads(torrents, 'torrents-list');
        } else {
            renderDownloads(downloads, 'downloads-list');
            updateStats(downloads);
        }
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
        loadDownloads();
    } catch (e) {
        console.error('Failed to add download:', e);
    }
}

function toggleDeleteMenu(id) {
    // Close all other menus first
    document.querySelectorAll('.delete-menu.show').forEach(function(m) { m.classList.remove('show'); });
    var menu = document.getElementById('delete-menu-' + id);
    if (menu) menu.classList.toggle('show');
}

async function deleteDownload(id, deleteFiles) {
    try {
        var qs = deleteFiles ? '?delete_files=true' : '';
        await apiRequest('/downloads/' + encodeURIComponent(id) + qs, { method: 'DELETE' });
        lastDownloadsJson = ''; // Force re-render
        loadDownloads();
    } catch (e) {
        console.error('Failed to delete download:', e);
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

// ─── Navigation ─────────────────────────────────────

function navigate(hash) {
    var routes = {
        '#dashboard': showDashboard,
        '#downloads': showDashboard,
        '#torrents': showTorrents,
        '#settings': showSettings
    };

    var render = routes[hash] || showDashboard;
    lastDownloadsJson = ''; // Reset cache on navigation
    safeRender(document.getElementById('main-content'), render());

    // Update active nav
    document.querySelectorAll('.nav-item').forEach(function(item) {
        item.classList.toggle('active', item.getAttribute('href') === hash);
    });

    // Bind forms after render
    bindForms();

    // Load data
    if (hash === '#dashboard' || hash === '#downloads' || hash === '#torrents') {
        loadDownloads();
    } else if (hash === '#settings') {
        loadSettings();
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

    var torrentForm = document.getElementById('add-torrent-form');
    if (torrentForm) {
        torrentForm.addEventListener('submit', function(e) {
            e.preventDefault();
            var input = document.getElementById('torrent-url');
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
    // Login form
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
    navigate(window.location.hash || '#dashboard');
    window.addEventListener('hashchange', function() { navigate(window.location.hash); });

    if (refreshInterval) clearInterval(refreshInterval);
    refreshInterval = setInterval(loadDownloads, 2000);
}

// Close delete menus when clicking outside
document.addEventListener('click', function(e) {
    if (!e.target.closest('.delete-dropdown')) {
        document.querySelectorAll('.delete-menu.show').forEach(function(m) { m.classList.remove('show'); });
    }
});

document.addEventListener('DOMContentLoaded', init);

const API_BASE = '/api';
let token = localStorage.getItem('dload_token') || '';

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

function showDashboard() {
    return `
        <div class="dashboard">
            <h2>Dashboard</h2>
            <div class="stats-grid">
                <div class="stat-card">
                    <h3>Active</h3>
                    <p id="active-count">0</p>
                </div>
                <div class="stat-card">
                    <h3>Completed</h3>
                    <p id="completed-count">0</p>
                </div>
                <div class="stat-card">
                    <h3>Total Speed</h3>
                    <p id="total-speed">0 MB/s</p>
                </div>
            </div>
            
            <div class="add-download">
                <h3>Add Download</h3>
                <form id="add-download-form">
                    <input type="url" id="download-url" placeholder="Enter URL (http://, ftp://, sftp://, magnet:, .torrent)" required>
                    <button type="submit">Download</button>
                </form>
            </div>
            
            <div class="active-downloads">
                <h3>Active Downloads</h3>
                <div id="downloads-list"></div>
            </div>
        </div>
    `;
}

function showTorrents() {
    return `
        <div class="torrents">
            <h2>Torrents</h2>
            <div class="add-download">
                <h3>Add Torrent</h3>
                <form id="add-torrent-form">
                    <input type="url" id="torrent-url" placeholder="Enter magnet link or torrent URL" required>
                    <button type="submit">Add Torrent</button>
                </form>
            </div>
            <div id="torrents-list"></div>
        </div>
    `;
}

function showSettings() {
    return `
        <div class="settings">
            <h2>Settings</h2>
            <form id="settings-form">
                <label>
                    Download Directory
                    <input type="text" id="settings-dir" value="/data">
                </label>
                <label>
                    Max Concurrent Downloads
                    <input type="number" id="settings-max-concurrent" value="3" min="1" max="10">
                </label>
                <label>
                    Max Connections Per File
                    <input type="number" id="settings-connections" value="4" min="1" max="16">
                </label>
                <button type="submit">Save Settings</button>
            </form>
        </div>
    `;
}

async function loadDownloads() {
    try {
        const downloads = await apiRequest('/downloads');
        renderDownloads(downloads);
        updateStats(downloads);
    } catch (e) {
        console.error('Failed to load downloads:', e);
    }
}

function renderDownloads(downloads) {
    const container = document.getElementById('downloads-list');
    if (!container) return;
    
    if (downloads.length === 0) {
        container.innerHTML = '<p class="empty">No downloads yet</p>';
        return;
    }
    
    container.innerHTML = downloads.map(d => `
        <div class="download-item" data-id="${d.id}">
            <div class="download-info">
                <h4>${d.filename}</h4>
                <p class="url">${d.url}</p>
                <span class="protocol">${d.protocol}</span>
            </div>
            <div class="download-progress">
                <div class="progress-bar">
                    <div class="progress-fill" style="width: ${d.progress}%"></div>
                </div>
                <span class="progress-text">${d.progress.toFixed(1)}%</span>
            </div>
            <div class="download-stats">
                <span>${formatSize(d.downloaded_size)} / ${formatSize(d.total_size)}</span>
                <span>${formatSpeed(d.speed)}</span>
                <span class="status ${d.status.toLowerCase()}">${d.status}</span>
            </div>
            <button class="delete-btn" onclick="deleteDownload('${d.id}')">×</button>
        </div>
    `).join('');
}

function formatSize(bytes) {
    if (bytes === 0) return '0 B';
    const k = 1024;
    const sizes = ['B', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

function formatSpeed(bytesPerSec) {
    return formatSize(bytesPerSec) + '/s';
}

async function addDownload(url) {
    await apiRequest('/downloads', {
        method: 'POST',
        body: JSON.stringify({ url })
    });
    loadDownloads();
}

async function deleteDownload(id) {
    await apiRequest(`/downloads/${id}`, { method: 'DELETE' });
    loadDownloads();
}

function updateStats(downloads) {
    const active = downloads.filter(d => d.status === 'Downloading').length;
    const completed = downloads.filter(d => d.status === 'Completed').length;
    const totalSpeed = downloads.reduce((sum, d) => sum + d.speed, 0);
    
    document.getElementById('active-count').textContent = active;
    document.getElementById('completed-count').textContent = completed;
    document.getElementById('total-speed').textContent = formatSpeed(totalSpeed);
}

document.getElementById('login-form')?.addEventListener('submit', async (e) => {
    e.preventDefault();
    const username = document.getElementById('login-username').value;
    const password = document.getElementById('login-password').value;
    
    const result = await apiRequest('/auth/login', {
        method: 'POST',
        body: JSON.stringify({ username, password })
    });
    
    if (result.success) {
        token = result.token;
        localStorage.setItem('dload_token', token);
        document.getElementById('login-modal').style.display = 'none';
        init();
    } else {
        alert('Login failed');
    }
});

function logout() {
    token = '';
    localStorage.removeItem('dload_token');
    document.getElementById('login-modal').style.display = 'flex';
}

function navigate(hash) {
    const routes = {
        '#dashboard': showDashboard,
        '#downloads': showDashboard,
        '#torrents': showTorrents,
        '#settings': showSettings
    };
    
    const render = routes[hash] || showDashboard;
    document.getElementById('main-content').innerHTML = render();
    
    if (hash === '#dashboard' || hash === '#downloads') {
        loadDownloads();
    }
}

async function init() {
    if (!token) {
        document.getElementById('login-modal').style.display = 'flex';
        return;
    }
    
    navigate(window.location.hash || '#dashboard');
    window.addEventListener('hashchange', () => navigate(window.location.hash));
    setInterval(loadDownloads, 2000);
}

document.addEventListener('DOMContentLoaded', init);

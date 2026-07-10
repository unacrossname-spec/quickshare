// QuickShare - Tauri v2 Frontend
// Lazily resolve __TAURI__ — may not be injected at script parse time
function tauriInvoke(cmd, args) {
  const fn = window.__TAURI__?.core?.invoke;
  if (!fn) return Promise.reject(new Error('Tauri not ready'));
  return fn(cmd, args);
}
function tauriListen(evt, cb) {
  const fn = window.__TAURI__?.event?.listen;
  if (!fn) return;
  return fn(evt, cb);
}
function canInvoke() { return !!window.__TAURI__?.core?.invoke; }

let currentPage = 'discover';
let transfers = [];
let history = [];
let knownPeers = [];
let selectedTarget = ''; // device IP to send to

// ── Settings state ──
let appSettings = {
  compress: true,
  bundle: true,
  notificationsEnabled: true,
  port: 8877,
};

// ── Init ──
document.addEventListener('DOMContentLoaded', async () => {
  setupGlobalListeners();
  setupDropZone();

  // Wait for either bootstrap data (injected by Rust) or Tauri IPC
  const initResult = await waitForInit(5000);
  if (!initResult.source) return showMockMode();

  if (initResult.source === 'bootstrap') {
    // Use data injected directly into the webview by Rust
    const boot = initResult.data;
    document.getElementById('device-name').textContent = boot.name || '未知';
    document.getElementById('device-ip').textContent = pickLanIp(boot.ips) || '未知';
    document.getElementById('settings-save-dir').textContent = boot.saveDir || '默认';
  } else if (initResult.source === 'tauri') {
    // Use Tauri IPC to fetch data
    const [info, transfersResult, historyResult, settingsResult] = await Promise.all([
      tauriInvoke('get_local_info').catch(e => { console.error('init:', e); return null; }),
      tauriInvoke('get_transfers').catch(e => { console.error('load transfers:', e); return []; }),
      tauriInvoke('get_history').catch(e => { console.error('load history:', e); return []; }),
      tauriInvoke('get_settings').catch(e => { console.error('load settings:', e); return null; }),
    ]);

    if (info) {
      document.getElementById('device-name').textContent = info.name;
      document.getElementById('device-ip').textContent = pickLanIp(info.ips) || '未知';
      document.getElementById('settings-save-dir').textContent = info.save_dir;
    }

    if (transfersResult) {
      transfers = transfersResult;
      updateTransferUI();
    }
    if (historyResult) {
      history = historyResult;
      renderHistory();
    }

    if (settingsResult) {
      appSettings.compress = settingsResult.compress;
      appSettings.bundle = settingsResult.bundle;
      appSettings.notificationsEnabled = settingsResult.notificationsEnabled;
      appSettings.port = settingsResult.port;
      syncSettingsUI();
      syncPortInput();
    }
  }

  // Port input change handler
  const portInput = document.querySelector('#page-settings input[type="text"]');
  if (portInput) {
    portInput.addEventListener('change', () => {
      const port = parseInt(portInput.value, 10);
      if (port > 0 && port < 65536 && port !== appSettings.port) {
        appSettings.port = port;
        if (canInvoke()) tauriInvoke('update_settings', { appSettings }).catch(() => {});
      }
    });
  }

  setupTauriEvents();
});

async function waitForInit(maxMs) {
  const step = 50;
  for (let i = 0; i < maxMs / step; i++) {
    if (window.__BOOTSTRAP_DATA) return { source: 'bootstrap', data: window.__BOOTSTRAP_DATA };
    if (window.__TAURI__?.core?.invoke) return { source: 'tauri' };
    await new Promise(r => setTimeout(r, step));
  }
  return { source: null };
}

function showMockMode() {
  document.getElementById('device-name').textContent = '本地设备';
  document.getElementById('device-ip').textContent = '离线模式';
  document.getElementById('settings-save-dir').textContent = '/tmp/quickshare';
  // Show no-devices (no scan running, no peers found)
  document.getElementById('discover-status')?.classList.add('hidden');
  document.getElementById('no-devices')?.classList.remove('hidden');
  document.getElementById('device-grid')?.classList.add('hidden');
}

// ── Page switching ──
function switchPage(page) {
  currentPage = page;
  document.querySelectorAll('.page').forEach(el => el.classList.add('hidden'));
  const target = document.getElementById(`page-${page}`);
  if (target) target.classList.remove('hidden');
  document.querySelectorAll('.nav-item').forEach(el =>
    el.classList.toggle('active', el.dataset.page === page)
  );
  const titles = { discover: '设备发现', transfers: '传输队列', history: '历史记录', settings: '设置' };
  document.getElementById('page-title').textContent = titles[page] || 'QuickShare';
  if (page === 'transfers') updateTransferUI();
  if (page === 'history') renderHistory();
}

// ── Discovery ──
async function scanDevices() {
  const grid = document.getElementById('device-grid');
  const status = document.getElementById('discover-status');
  const noDev = document.getElementById('no-devices');
  grid.classList.add('hidden');
  noDev.classList.add('hidden');
  status.classList.remove('hidden');

  // Stop scanning animation after 5s max (frontend timeout)
  const timeout = new Promise(r => setTimeout(() => r('timeout'), 5000));

  let devices = [];
  if (canInvoke()) {
    const scan = tauriInvoke('scan_devices').then(d => d, () => []);
    const result = await Promise.race([scan, timeout]);
    devices = result === 'timeout' ? [] : result;
    knownPeers = devices.map(d => ({
      name: d.name,
      ip: `${d.ip}:${d.port}`,
      icon: 'desktop_windows',
    }));
  } else {
    await timeout;
  }

  status.classList.add('hidden');
  if (knownPeers.length === 0) {
    noDev.classList.remove('hidden');
    grid.classList.add('hidden');
  } else {
    noDev.classList.add('hidden');
    showPeers();
  }
}

function showPeers() {
  const grid = document.getElementById('device-grid');
  document.getElementById('no-devices')?.classList.add('hidden');
  grid.innerHTML = knownPeers.map(p => `
    <div class="device-card bg-surface-container-lowest border border-outline-variant p-5 rounded-2xl flex items-center gap-4 ${p.ip === selectedTarget ? 'border-primary' : ''}">
      <div class="w-12 h-12 bg-surface-container-high rounded-full flex items-center justify-center text-on-surface-variant">
        <span class="material-symbols-outlined">${p.icon || 'desktop_windows'}</span>
      </div>
      <div class="flex-1 min-w-0">
        <h4 class="font-medium truncate">${escHtml(p.name)}</h4>
        <p class="text-xs text-outline truncate">${p.ip}</p>
      </div>
      <button class="send-btn px-4 py-2 text-xs font-medium rounded-full transition-colors ${p.ip === selectedTarget ? 'bg-primary text-white' : 'bg-primary-container text-on-primary-container hover:bg-primary hover:text-white'}">${p.ip === selectedTarget ? '已选择' : '发送'}</button>
    </div>
  `).join('');
  grid.querySelectorAll('.send-btn').forEach((btn, i) =>
    btn.addEventListener('click', () => selectTarget(knownPeers[i]))
  );
  grid.classList.remove('hidden');
}

function selectTarget(peer) {
  selectedTarget = peer.ip;
  showPeers();
  document.getElementById('no-devices')?.classList.add('hidden');
  document.getElementById('selected-target')?.classList.remove('hidden');
  if (document.getElementById('selected-target')) {
    document.getElementById('selected-target').textContent = `目标设备: ${peer.name} (${peer.ip})`;
  }
}

function manualConnect() {
  const addr = document.getElementById('manual-addr').value.trim();
  if (addr) {
    const targetAddr = addr.includes(':') ? addr : `${addr}:8877`;
    selectedTarget = targetAddr;
    if (!knownPeers.find(p => p.ip === targetAddr)) {
      knownPeers.push({ name: `手动连接 (${addr})`, ip: targetAddr, icon: 'desktop_windows' });
      showPeers();
    }
    document.getElementById('selected-target').textContent = `目标设备: ${targetAddr}`;
    document.getElementById('selected-target').classList.remove('hidden');
    document.getElementById('no-devices')?.classList.add('hidden');
    switchPage('transfers');
  }
}

// ── Drop zone ──
function setupDropZone() {
  const zone = document.getElementById('drop-zone');
  if (!zone) return;

  zone.addEventListener('click', () => {
    document.getElementById('file-picker')?.click();
  });
  zone.addEventListener('dragenter', e => { e.preventDefault(); zone.classList.add('drag-over'); });
  zone.addEventListener('dragover', e => { e.preventDefault(); zone.classList.add('drag-over'); });
  zone.addEventListener('dragleave', e => {
    e.preventDefault();
    if (!zone.contains(e.relatedTarget)) zone.classList.remove('drag-over');
  });
  zone.addEventListener('drop', e => {
    e.preventDefault();
    zone.classList.remove('drag-over');
    const files = e.dataTransfer?.files;
    if (files?.length) {
      const path = files[0].path || files[0].name;
      handleFilePath(path, files[0].size || 0, files[0].name);
    }
  });
}

function handleFilePick(input) {
  if (!input.files?.length) return;
  const file = input.files[0];
  handleFilePath(file.path || file.name, file.size || 0, file.name);
  input.value = '';
}

function handleFilePath(filePath, fileSize, fileName) {
  if (!selectedTarget) {
    const ip = prompt('请输入目标设备 IP:port\n例如 192.168.1.100:8877');
    if (!ip) return;
    selectedTarget = ip;
  }

  if (!canInvoke()) {
    addTransfer(`[${selectedTarget}] ${fileName}`, fileSize || 1000000);
    switchPage('transfers');
    return;
  }

  addTransfer(`${fileName} → ${selectedTarget}`, fileSize || 0);
  const tempId = transfers[transfers.length - 1].id;
  switchPage('transfers');

  tauriInvoke('send_files', {
    opts: { addr: selectedTarget, path: filePath, compress: appSettings.compress, bundle: appSettings.bundle }
  }).then(realId => {
    // Replace temp ID with backend-generated UUID so progress events match
    if (realId) {
      const t = transfers.find(x => x.id === tempId);
      if (t) t.id = realId;
    }
  }).catch(e => {
    console.error('send failed:', e);
    const t = transfers.find(x => x.id === tempId);
    if (t) t.status = 'failed';
    updateTransferUI();
    alert('发送失败: ' + e);
  });
}

// ── Global event delegation ──
function setupGlobalListeners() {
  document.addEventListener('click', e => {
    const btn = e.target.closest('[data-page]');
    if (btn) { switchPage(btn.dataset.page); return; }

    const toggle = e.target.closest('.toggle');
    if (toggle) {
      toggle.classList.toggle('on');
      const setting = toggle.dataset.setting;
      if (setting && setting in appSettings) {
        appSettings[setting] = toggle.classList.contains('on');
        if (canInvoke()) tauriInvoke('update_settings', { appSettings });
      }
      return;
    }

    if (e.target.closest('[data-action="scan"]')) { scanDevices(); return; }
    if (e.target.closest('[data-action="manual-connect"]')) { manualConnect(); return; }
    if (e.target.closest('[data-action="clear-history"]')) { clearHistory(); return; }
    if (e.target.closest('[data-action="pick-save-dir"]')) { pickSaveDir(); return; }
    if (e.target.closest('[data-action="file-picker-trigger"]')) {
      document.getElementById('file-picker')?.click(); return;
    }
    const cancelBtn = e.target.closest('[data-action="cancel-transfer"]');
    if (cancelBtn) { cancelTransfer(cancelBtn.dataset.id); return; }
  });

  document.querySelectorAll('.bottom-nav-btn').forEach(el => {
    el.addEventListener('click', () => switchPage(el.dataset.page));
  });
}

// ── Transfer management ──
function addTransfer(name, total) {
  const id = Date.now().toString();
  transfers.push({ id, file_name: name, total, sent: 0, status: 'active' });
  updateTransferUI();

  if (!canInvoke()) simulateTransfer(id);
  return id;
}

function simulateTransfer(id) {
  const t = transfers.find(x => x.id === id);
  if (!t) return;
  const step = Math.max(1, Math.floor(t.total / 30));
  const iv = setInterval(() => {
    t.sent = Math.min(t.total, t.sent + step);
    if (currentPage === 'transfers') updateTransferUI();
    if (t.sent >= t.total) {
      t.status = 'completed';
      history.unshift({
        id: t.id, file_name: t.file_name, peer: '', direction: 'sent',
        size: t.total, status: 'completed', timestamp: new Date().toLocaleTimeString()
      });
      clearInterval(iv);
      if (currentPage === 'transfers') updateTransferUI();
    }
  }, 200);
}

function cancelTransfer(id) {
  if (canInvoke()) {
    tauriInvoke('cancel_transfer', { id })
      .then(() => {
        transfers = transfers.filter(t => t.id !== id);
        updateTransferUI();
      })
      .catch(() => {}); // leave it in UI so user can retry
  } else {
    transfers = transfers.filter(t => t.id !== id);
    updateTransferUI();
  }
}

// ── Transfer UI ──
function updateTransferUI() {
  const active = document.getElementById('active-transfer');
  const empty = document.getElementById('transfer-empty');
  const pending = document.getElementById('transfer-pending');
  const activeItems = transfers.filter(t => t.status === 'active');
  const pendingItems = transfers.filter(t => t.status !== 'active');

  if (transfers.length === 0) {
    active?.classList.add('hidden'); empty?.classList.remove('hidden');
    pending?.classList.add('hidden'); return;
  }
  active?.classList.remove('hidden');
  empty?.classList.add('hidden');

  const cards = document.getElementById('transfer-cards');
  cards.innerHTML = activeItems.map(t => {
    const pct = t.total > 0 ? Math.round((t.sent / t.total) * 100) : 0;
    return `<div class="bg-surface-container-lowest border border-outline-variant p-5 rounded-2xl">
      <div class="flex items-center gap-4 mb-4">
        <div class="w-10 h-10 bg-primary-container/20 rounded flex items-center justify-center text-primary">
          <span class="material-symbols-outlined">description</span>
        </div>
        <div class="flex-1 min-w-0">
          <p class="font-medium truncate">${escHtml(t.file_name)}</p>
          <p class="text-xs text-outline">传输中...</p>
        </div>
        <button class="w-9 h-9 rounded-full border border-outline-variant flex items-center justify-center hover:bg-error-container hover:text-error transition-colors" data-action="cancel-transfer" data-id="${t.id}">
          <span class="material-symbols-outlined text-sm">close</span>
        </button>
      </div>
      <div class="progress-bar"><div class="progress-bar-fill" style="width:${pct}%"></div></div>
      <div class="mt-2 flex justify-between text-xs text-outline">
        <span class="text-primary font-medium">${pct}%</span>
        <span>${fmtSize(t.sent)} / ${fmtSize(t.total)}</span>
      </div>
    </div>`;
  }).join('');

  if (pendingItems.length > 0) {
    pending?.classList.remove('hidden');
    document.getElementById('pending-list').innerHTML = pendingItems.map(t =>
      `<div class="flex items-center gap-3 p-3 hover:bg-surface-container-low rounded-lg transition-colors">
        <span class="material-symbols-outlined text-outline">description</span>
        <div class="flex-1 min-w-0">
          <p class="text-sm truncate">${escHtml(t.file_name)}</p>
          <p class="text-xs text-outline">${fmtSize(t.total)} • ${statusText(t.status)}</p>
        </div>
      </div>`
    ).join('');
  } else {
    pending?.classList.add('hidden');
  }
}

// ── History ──
function renderHistory() {
  const tbody = document.getElementById('history-body');
  const empty = document.getElementById('history-empty');
  const table = document.getElementById('history-table');
  if (history.length === 0) {
    table?.classList.add('hidden'); empty?.classList.remove('hidden'); return;
  }
  table?.classList.remove('hidden'); empty?.classList.add('hidden');
  tbody.innerHTML = history.slice(0, 50).map(h =>
    `<tr class="hover:bg-surface-container-lowest transition-colors">
      <td class="px-6 py-4"><div class="flex items-center gap-3">
        <div class="w-10 h-10 bg-secondary-container rounded-lg flex items-center justify-center"><span class="material-symbols-outlined">description</span></div>
        <div><p class="text-sm font-medium">${escHtml(h.file_name)}</p><p class="text-xs text-outline">${h.direction === 'sent' ? '发出' : '接收'}</p></div>
      </div></td>
      <td class="px-6 py-4 text-sm">${fmtSize(h.size)}</td>
      <td class="px-6 py-4 text-sm">${h.peer || '-'}</td>
      <td class="px-6 py-4">${statusBadge(h.status)}</td>
      <td class="px-6 py-4 text-sm text-outline text-right">${fmtTime(h.timestamp)}</td>
    </tr>`
  ).join('');
}

async function clearHistory() {
  history = [];
  if (canInvoke()) {
    try { await tauriInvoke('clear_history'); } catch (e) { console.error('clear history:', e); }
  }
  renderHistory();
}

// ── Settings UI sync ──
function syncSettingsUI() {
  document.querySelectorAll('.toggle[data-setting]').forEach(el => {
    const key = el.dataset.setting;
    el.classList.toggle('on', !!appSettings[key]);
  });
}

function syncPortInput() {
  const input = document.querySelector('#page-settings input[type="text"]');
  if (input) input.value = appSettings.port || 8877;
}

// ── Tauri Events ──
function setupTauriEvents() {
  const l = tauriListen;
  l('transfer-progress', e => {
    const { id, sent, total } = e.payload;
    const t = transfers.find(x => x.id === id);
    if (t) { t.sent = sent; t.total = total; }
    if (currentPage === 'transfers') updateTransferUI();
  });
  l('transfer-complete', async () => {
    try { transfers = await tauriInvoke('get_transfers') || []; } catch {}
    try { history = await tauriInvoke('get_history') || []; } catch {}
    if (currentPage === 'transfers') updateTransferUI();
  });
  l('receive-complete', async () => {
    try { history = await tauriInvoke('get_history') || []; } catch {}
    if (currentPage === 'history') renderHistory();
  });
}

// ── Settings ──
async function pickSaveDir() {
  if (!canInvoke()) return alert('开发模式下不可用');
  try {
    const { open } = await importFailsafe('@tauri-apps/plugin-dialog', 'open');
    if (open) {
      const dir = await open({ directory: true });
      if (dir) {
        await tauriInvoke('update_settings', { saveDir: dir });
        document.getElementById('settings-save-dir').textContent = dir;
      }
    }
  } catch (e) { console.warn('settings dialog:', e); }
}

// ── Utility ──
async function importFailsafe(pkg, fn) {
  try { const mod = await import(pkg); return { [fn]: mod[fn] }; }
  catch { return { [fn]: null }; }
}

function escHtml(s) { const d = document.createElement('div'); d.textContent = s; return d.innerHTML; }
function pickLanIp(ips) {
  if (!ips?.length) return '';
  // Prefer 192.168.x.x (most common home/office LAN)
  const lan = ips.find(ip => ip.startsWith('192.168.'));
  if (lan) return lan;
  // Then 10.x.x.x (corporate networks)
  const p10 = ips.find(ip => ip.startsWith('10.'));
  if (p10) return p10;
  // RFC 1918: 172.16.0.0 – 172.31.255.255 is valid private range
  const p172 = ips.find(ip => {
    if (!ip.startsWith('172.')) return false;
    const octet = parseInt(ip.split('.')[1], 10);
    return octet >= 16 && octet <= 31;
  });
  if (p172) return p172;
  // Fallback: filter out loopback and Docker default bridge (172.17.x.x)
  return ips.find(ip => !ip.startsWith('127.') && !ip.startsWith('172.17.')) || ips[0];
}
function fmtSize(bytes) {
  if (bytes >= 1e9) return (bytes / 1e9).toFixed(1) + ' GB';
  if (bytes >= 1e6) return (bytes / 1e6).toFixed(1) + ' MB';
  if (bytes >= 1e3) return (bytes / 1e3).toFixed(1) + ' KB';
  return bytes + ' B';
}
function fmtTime(ts) {
  // Timestamp is epoch seconds from Rust backend
  const n = parseInt(ts, 10);
  if (!n) return ts; // fallback for old-format timestamps
  return new Date(n * 1000).toLocaleTimeString();
}
function statusText(s) { return { completed: '已完成', failed: '失败', cancelled: '已取消', active: '传输中' }[s] || s; }
function statusBadge(s) {
  const m = { completed: 'bg-primary-container text-on-primary-container', failed: 'bg-error-container text-on-error-container', cancelled: 'bg-surface-container-high text-on-surface-variant', active: 'bg-primary-container text-on-primary-container' };
  return `<span class="inline-flex items-center gap-1 px-3 py-1 ${m[s] || m.completed} rounded-full text-xs font-medium">${statusText(s)}</span>`;
}

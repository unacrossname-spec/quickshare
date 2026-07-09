// QuickShare - Tauri v2 Frontend
const T = window.__TAURI__;
const invoke = T?.core?.invoke || (() => { throw new Error('not in Tauri'); });
const listen = T?.event?.listen;
const isTauri = !!T;

let currentPage = 'discover';
let transfers = [];
let history = [];
let knownPeers = [];
let selectedTarget = ''; // device IP to send to

// ── Init ──
document.addEventListener('DOMContentLoaded', async () => {
  setupGlobalListeners();
  setupDropZone();
  if (!isTauri) return showMockMode();

  try {
    const info = await invoke('get_local_info');
    document.getElementById('device-name').textContent = info.name;
    document.getElementById('device-ip').textContent = info.ips.join(', ') || '未知';
    document.getElementById('settings-save-dir').textContent = info.save_dir;
  } catch (e) { console.error('init:', e); }

  try {
    transfers = await invoke('get_transfers') || [];
    history = await invoke('get_history') || [];
    updateTransferUI();
    renderHistory();
  } catch (e) { console.error('load state:', e); }

  setupTauriEvents();
});

function showMockMode() {
  document.getElementById('device-name').textContent = 'Dev-Machine';
  document.getElementById('device-ip').textContent = '192.168.1.100';
  document.getElementById('settings-save-dir').textContent = '/tmp/quickshare';
  document.getElementById('discover-status').classList.add('hidden');
  showPeers();
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
  await new Promise(r => setTimeout(r, 1500));
  status.classList.add('hidden');
  grid.classList.remove('hidden');
  noDev.classList.remove('hidden');
  showPeers();
}

function showPeers() {
  const grid = document.getElementById('device-grid');
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
  // Show feedback briefly
  document.getElementById('no-devices')?.classList.add('hidden');
  document.getElementById('selected-target')?.classList.remove('hidden');
  if (document.getElementById('selected-target')) {
    document.getElementById('selected-target').textContent = `目标设备: ${peer.name} (${peer.ip})`;
  }
}

function manualConnect() {
  const addr = document.getElementById('manual-addr').value.trim();
  if (addr) {
    selectedTarget = addr;
    addPeer(`设备 (${addr})`, addr);
    startSend();
  }
}

// ── Drop zone ──
function setupDropZone() {
  const zone = document.getElementById('drop-zone');
  if (!zone) return;

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
      // In Tauri, dropped file has .path extension with full path
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
    // No target selected - ask or redirect
    const ip = prompt('请输入目标设备 IP:port\n例如 192.168.1.100:8877');
    if (!ip) return;
    selectedTarget = ip;
  }

  if (!isTauri) {
    addTransfer(`[${selectedTarget}] ${fileName}`, fileSize || 1000000);
    switchPage('transfers');
    return;
  }

  // Tauri mode: send file
  addTransfer(`${fileName} → ${selectedTarget}`, fileSize || 0);
  switchPage('transfers');

  invoke('send_files', {
    opts: { addr: selectedTarget, path: filePath, compress: true, bundle: true }
  }).catch(e => {
    console.error('send failed:', e);
    // Update transfer status
    const t = transfers.find(x => x.file_name.includes(fileName));
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
    if (toggle) { toggle.classList.toggle('on'); return; }

    if (e.target.closest('[data-action="scan"]')) { scanDevices(); return; }
    if (e.target.closest('[data-action="manual-connect"]')) { manualConnect(); return; }
    if (e.target.closest('[data-action="clear-history"]')) { clearHistory(); return; }
    if (e.target.closest('[data-action="pick-save-dir"]')) { pickSaveDir(); return; }
    if (e.target.closest('[data-action="file-picker-trigger"]')) {
      document.getElementById('file-picker')?.click(); return;
    }
    // Cancel transfer button
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

  if (!isTauri) simulateTransfer(id);
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
  if (isTauri) invoke('cancel_transfer', { id }).catch(() => {});
  transfers = transfers.filter(t => t.id !== id);
  updateTransferUI();
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
      <td class="px-6 py-4 text-sm text-outline text-right">${h.timestamp}</td>
    </tr>`
  ).join('');
}

function clearHistory() { history = []; renderHistory(); }

// ── Tauri Events ──
function setupTauriEvents() {
  if (!listen) return;
  listen('transfer-progress', e => {
    const { id, sent, total } = e.payload;
    const t = transfers.find(x => x.id === id);
    if (t) { t.sent = sent; t.total = total; }
    if (currentPage === 'transfers') updateTransferUI();
  });
  listen('transfer-complete', async () => {
    try { transfers = await invoke('get_transfers') || []; } catch {}
    try { history = await invoke('get_history') || []; } catch {}
    if (currentPage === 'transfers') updateTransferUI();
  });
  listen('receive-complete', async () => {
    try { history = await invoke('get_history') || []; } catch {}
    if (currentPage === 'history') renderHistory();
  });
}

// ── Settings ──
async function pickSaveDir() {
  if (!isTauri) return alert('开发模式下不可用');
  try {
    const { open } = await importFailsafe('@tauri-apps/plugin-dialog', 'open');
    if (open) {
      const dir = await open({ directory: true });
      if (dir) {
        await invoke('update_settings', { saveDir: dir });
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
function fmtSize(bytes) {
  if (bytes >= 1e9) return (bytes / 1e9).toFixed(1) + ' GB';
  if (bytes >= 1e6) return (bytes / 1e6).toFixed(1) + ' MB';
  if (bytes >= 1e3) return (bytes / 1e3).toFixed(1) + ' KB';
  return bytes + ' B';
}
function statusText(s) { return { completed: '已完成', failed: '失败', cancelled: '已取消', active: '传输中' }[s] || s; }
function statusBadge(s) {
  const m = { completed: 'bg-primary-container text-on-primary-container', failed: 'bg-error-container text-on-error-container', cancelled: 'bg-surface-container-high text-on-surface-variant', active: 'bg-primary-container text-on-primary-container' };
  return `<span class="inline-flex items-center gap-1 px-3 py-1 ${m[s] || m.completed} rounded-full text-xs font-medium">${statusText(s)}</span>`;
}

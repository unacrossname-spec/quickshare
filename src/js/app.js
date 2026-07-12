// QuickShare - Tauri v2 Frontend
// Tauri v2 always injects __TAURI_INTERNALS__ (the internal IPC bridge).
// __TAURI__ is the public API only available when @tauri-apps/api npm
// package is installed. We target __TAURI_INTERNALS__ directly to avoid
// the npm dependency.
function tauriInvoke(cmd, args) {
  const fn = window.__TAURI_INTERNALS__?.invoke;
  if (!fn) return Promise.reject(new Error('Tauri not ready'));
  return fn(cmd, args);
}
function tauriListen(evt, cb) {
  const internals = window.__TAURI_INTERNALS__;
  if (!internals) return;
  const handler = internals.transformCallback(cb, false);
  // IMPORTANT: EventTarget requires { kind: 'Any' } (serde tag format).
  // Without the target parameter, plugin:event|listen silently fails.
  return internals.invoke('plugin:event|listen', {
    event: evt,
    target: { kind: 'Any' },
    handler,
  }).catch(e => {
    console.error('[tauriListen] failed:', evt, e);
    // Critical failure — without event listeners the app can't receive
    // transfer progress or incoming transfer requests.
    const st = document.getElementById('init-status');
    if (st) st.textContent = '事件监听失败，请重启应用';
  });
}
function canInvoke() { return !!window.__TAURI_INTERNALS__?.invoke; }

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
  encrypted: false,
  password: '',
};

// ── Transfer speed tracking ──
let transferSpeeds = {}; // id → {lastBytes, lastTime, bytesPerSec}
let pendingProgress = {}; // realId → {sent, total} — buffered progress for in-flight sends
let speedUnit = localStorage.getItem('qs_speedUnit') || 'MB/s';

// ── Init ──
// IMPORTANT: Register event listeners BEFORE any async init to avoid
// missing events (e.g., incoming-transfer) that arrive during startup.
document.addEventListener('DOMContentLoaded', async () => {
  setupTauriEvents();
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
      appSettings.encrypted = settingsResult.encrypted || false;
      appSettings.password = settingsResult.password || '';
      syncSettingsUI();
      syncPortInput();
    } else {
      // Backend unavailable — use defaults (encrypted is already false)
      syncSettingsUI();
    }
  }

  // Auto-scan LAN devices on startup
  if (canInvoke()) scanDevices();

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

  // Speed unit selector
  const speedSel = document.getElementById('speed-unit-select');
  if (speedSel) {
    syncSpeedUnitUI();
    speedSel.addEventListener('change', () => {
      speedUnit = speedSel.value;
      localStorage.setItem('qs_speedUnit', speedUnit);
      if (currentPage === 'transfers') updateTransferUI();
    });
  }

  // Encryption password field — password is stored locally and sent
  // with the transfer request, NOT saved to backend settings on every keystroke.
  syncPasswordUI();
  const pwdInput = document.getElementById('encryption-password');
  if (pwdInput) {
    pwdInput.addEventListener('input', () => {
      appSettings.password = pwdInput.value;
    });
  }
  // Save button: explicitly persist password to backend
  document.getElementById('password-save')?.addEventListener('click', () => {
    if (!canInvoke()) return;
    const btn = document.getElementById('password-save');
    tauriInvoke('update_settings', { appSettings }).then(() => {
      if (btn) { btn.textContent = '已保存'; setTimeout(() => { btn.textContent = '保存'; }, 1500); }
    }).catch(() => {
      if (btn) { btn.textContent = '失败'; setTimeout(() => { btn.textContent = '保存'; }, 1500); }
    });
  });
  document.getElementById('password-toggle-vis')?.addEventListener('click', () => {
    const pwd = document.getElementById('encryption-password');
    if (!pwd) return;
    const btn = document.getElementById('password-toggle-vis');
    if (pwd.type === 'password') {
      pwd.type = 'text';
      if (btn) btn.textContent = '隐藏';
    } else {
      pwd.type = 'password';
      if (btn) btn.textContent = '显示';
    }
  });

  // ── At this point the app is fully initialized ──
});

async function waitForInit(maxMs) {
  const step = 50;
  for (let i = 0; i < maxMs / step; i++) {
    if (window.__BOOTSTRAP_DATA) return { source: 'bootstrap', data: window.__BOOTSTRAP_DATA };
    if (window.__TAURI_INTERNALS__?.invoke) return { source: 'tauri' };
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
  const hint = document.getElementById('discovery-hint');
  grid.classList.add('hidden');
  noDev.classList.add('hidden');
  status.classList.remove('hidden');

  // IPC is always available via __TAURI_INTERNALS__ (no npm package needed)
  const scan = tauriInvoke('scan_devices').then(d => d, () => []);
  const timeout = new Promise(r => setTimeout(() => r('timeout'), 5000));
  const result = await Promise.race([scan, timeout]);
  const devices = result === 'timeout' ? [] : result;
  if (result === 'timeout') {
    if (hint) hint.textContent = '扫描超时，请检查网络连接后重试';
  }

  knownPeers = devices.map(d => ({
    name: d.name,
    ip: `${d.ip}:${d.port}`,
    icon: 'desktop_windows',
  }));

  status.classList.add('hidden');
  if (knownPeers.length === 0) {
    noDev.classList.remove('hidden');
    grid.classList.add('hidden');
    if (hint) hint.textContent = '未发现其他设备。请确保对方也在运行 QuickShare，且防火墙允许 UDP 8879 端口';
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
      <button class="send-btn px-4 py-2 text-xs font-medium rounded-full transition-colors ${p.ip === selectedTarget ? 'bg-primary text-white' : 'bg-primary-container text-on-primary-container hover:bg-primary hover:text-white'}">${p.ip === selectedTarget ? '已选择' : '选择'}</button>
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

  // Click → open native file dialog via Tauri IPC (pick_file command).
  zone.addEventListener('click', async () => {
    if (!canInvoke()) {
      // Fallback: try HTML file input
      document.getElementById('file-picker')?.click();
      return;
    }
    try {
      const picked = await tauriInvoke('pick_file');
      if (picked && picked.path) {
        handleFilePath(picked.path, picked.size, picked.name);
      }
    } catch (e) {
      console.error('pick_file failed:', e);
      // Fallback to HTML file input on error
      document.getElementById('file-picker')?.click();
    }
  });

  zone.addEventListener('dragenter', e => { e.preventDefault(); zone.classList.add('drag-over'); });
  zone.addEventListener('dragover', e => { e.preventDefault(); zone.classList.add('drag-over'); });
  zone.addEventListener('dragleave', e => {
    e.preventDefault();
    if (!zone.contains(e.relatedTarget)) zone.classList.remove('drag-over');
  });
  // NOTE: File drop is handled by Tauri's native DragDrop event (see setupTauriEvents).
  // The HTML5 drop handler below is a fallback for when Tauri events are not available.
  zone.addEventListener('drop', e => {
    e.preventDefault();
    zone.classList.remove('drag-over');
    // If Tauri file-dropped event hasn't fired (e.g. mock mode), try HTML5 API
    const files = e.dataTransfer?.files;
    if (files?.length) {
      const file = files[0];
      handleFilePath(file.path || file.name, file.size || 0, file.name);
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
  console.log('[handleFilePath] path:', filePath, 'size:', fileSize, 'target:', selectedTarget);

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

  console.log('[handleFilePath] invoking send_files...');
  tauriInvoke('send_files', {
    opts: { addr: selectedTarget, path: filePath, compress: appSettings.compress, bundle: appSettings.bundle, encrypted: appSettings.encrypted, password: appSettings.password }
  }).then(realId => {
    console.log('[handleFilePath] send_files returned:', realId);
    if (realId) {
      const t = transfers.find(x => x.id === tempId);
      if (t) {
        t.id = realId;
        // Replay any progress that arrived before we knew the real ID
        if (pendingProgress[realId]) {
          const p = pendingProgress[realId];
          t.sent = p.sent;
          t.total = p.total;
          delete pendingProgress[realId];
        }
      }
      // Speed tracking: remap from tempId to realId
      if (transferSpeeds[tempId]) {
        transferSpeeds[realId] = transferSpeeds[tempId];
        delete transferSpeeds[tempId];
      }
    }
    updateTransferUI();
  }).catch(e => {
    console.error('send failed:', e);
    delete transferSpeeds[tempId];
    const t = transfers.find(x => x.id === tempId);
    if (t) { t.status = 'failed'; updateTransferUI(); }
    // Check if transfer was already completed (race): only alert if still pending
    if (transfers.find(x => x.id === tempId)) {
      alert('发送失败: ' + e);
    }
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
        if (canInvoke()) {
          const save = () => tauriInvoke('update_settings', { appSettings }).catch(() => {});
          // Wait for CSS transition to finish before IPC so WebKitGTK
          // compositor isn't interrupted mid-animation.
          const dur = getComputedStyle(toggle).transitionDuration;
          if (dur && dur !== '0s') {
            toggle.addEventListener('transitionend', save, { once: true });
          } else {
            save();
          }
        }
      }
      return;
    }

    if (e.target.closest('[data-action="scan"]')) { scanDevices(); return; }
    if (e.target.closest('[data-action="manual-connect"]')) { manualConnect(); return; }
    if (e.target.closest('[data-action="clear-history"]')) { clearHistory(); return; }
    if (e.target.closest('[data-action="pick-save-dir"]')) { pickSaveDir(); return; }
    if (e.target.closest('[data-action="file-picker-trigger"]')) {
      // Click is handled by setupDropZone's own listener; no-op here.
      return;
    }
    const extLink = e.target.closest('[data-action="open-url"]');
    if (extLink) {
      e.preventDefault();
      const url = extLink.dataset.url;
      if (canInvoke()) {
        tauriInvoke('plugin:shell|open', { path: url }).catch(() => {});
      }
      return;
    }
    const cancelBtn = e.target.closest('[data-action="cancel-transfer"]');
    if (cancelBtn) { cancelTransfer(cancelBtn.dataset.id); return; }
    const acceptBtn = e.target.closest('[data-action="accept-transfer"]');
    if (acceptBtn) { respondTransfer(acceptBtn.dataset.id, true); return; }
    const declineBtn = e.target.closest('[data-action="decline-transfer"]');
    if (declineBtn) { respondTransfer(declineBtn.dataset.id, false); return; }
  });

  document.querySelectorAll('.bottom-nav-btn').forEach(el => {
    el.addEventListener('click', () => switchPage(el.dataset.page));
  });
}

// ── Transfer management ──
function addTransfer(name, total) {
  const id = Date.now().toString();
  transfers.push({ id, file_name: name, total, sent: 0, status: 'active', direction: 'sent' });
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
  const t = transfers.find(x => x.id === id);
  delete transferSpeeds[id];
  // For pending incoming transfers, use respond_transfer to decline
  if (t && t.status === 'pending' && t.direction === 'received') {
    respondTransfer(id, false);
    return;
  }
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
  const container = document.getElementById('transfer-cards');
  const empty = document.getElementById('transfer-empty');
  const active = document.getElementById('active-transfer');
  const pending = document.getElementById('transfer-pending');

  if (!transfers.length) {
    active?.classList.add('hidden');
    pending?.classList.add('hidden');
    empty?.classList.remove('hidden');
    return;
  }

  active?.classList.remove('hidden');
  empty?.classList.add('hidden');

  const nowActive = transfers.filter(t => t.status === 'active');
  const nowPending = transfers.filter(t => t.status === 'pending');
  const nowDone = transfers.filter(t => t.status === 'completed' || t.status === 'failed' || t.status === 'cancelled');

  // ── Active & Pending combined cards (with progress bars) ──
  const activeAndPending = [...nowActive, ...nowPending];
  let activeHtml = '';
  if (activeAndPending.length) {
    activeHtml = activeAndPending.map(t => {
      const isPending = t.status === 'pending';
      const isIncoming = t.direction === 'received';
      const pct = t.total > 0 ? Math.round((t.sent / t.total) * 100) : 0;
      const icon = isIncoming ? 'download' : 'upload_file';
      const statusLabel = isPending
        ? (isIncoming ? '等待确认...' : '等待连接...')
        : (isIncoming ? `接收中 ${pct}%` : `发送中 ${pct}%`);
      const bgColor = isIncoming ? 'bg-tertiary-container/30' : 'bg-primary-container/20';
      const textColor = isIncoming ? 'text-tertiary' : 'text-primary';
      const barColor = isIncoming ? 'bg-tertiary' : 'bg-primary';
      const barReady = isIncoming ? 'bg-tertiary-container' : 'bg-primary-container';

      return `<div class="bg-surface-container-lowest border border-outline-variant p-5 rounded-2xl mb-4">
        <div class="flex items-center gap-4 mb-4">
          <div class="w-10 h-10 ${bgColor} rounded flex items-center justify-center ${textColor}">
            <span class="material-symbols-outlined">${icon}</span>
          </div>
          <div class="flex-1 min-w-0">
            <p class="font-medium truncate">${escHtml(t.file_name)}</p>
            <p class="text-xs text-outline">
              <span class="inline-flex items-center gap-1">
                ${isIncoming ? '📥 接收' : '📤 发送'}
                • ${statusLabel}
              </span>
            </p>
          </div>
          ${isPending && isIncoming
            ? `<div class="flex gap-2 shrink-0">
                <button class="px-3 py-1.5 bg-primary text-white rounded-full text-xs font-medium hover:opacity-90" data-action="accept-transfer" data-id="${t.id}">接收</button>
                <button class="px-3 py-1.5 border border-outline-variant rounded-full text-xs hover:bg-error-container hover:text-error" data-action="decline-transfer" data-id="${t.id}">拒绝</button>
              </div>`
            : (t.status === 'active'
              ? `<button class="w-9 h-9 rounded-full border border-outline-variant flex items-center justify-center hover:bg-error-container hover:text-error transition-colors shrink-0" data-action="cancel-transfer" data-id="${t.id}">
                  <span class="material-symbols-outlined text-sm">close</span>
                </button>`
              : '')
          }
        </div>
        ${isPending
          ? `<div class="flex items-center justify-center py-2 text-xs text-outline">
              <span class="animate-pulse">等待${isIncoming ? '确认接收' : '连接'}...</span>
            </div>`
          : `<div class="progress-bar"><div class="progress-bar-fill ${barColor}" style="width:${pct}%"></div></div>
            <div class="mt-2 flex justify-between items-center text-xs text-outline">
              <span class="${textColor} font-medium">${pct}%</span>
              <span>${fmtSpeed(transferSpeeds[t.id]?.bytesPerSec || 0)}</span>
              <span>${fmtSize(t.sent)} / ${fmtSize(t.total)}</span>
            </div>`
        }
      </div>`;
    }).join('');
  }

  // If no active/pending but there are done items, show "done" section
  const cards = document.getElementById('transfer-cards');
  if (cards) cards.innerHTML = activeHtml;

  // ── Completed / Failed / Cancelled ──
  if (nowDone.length > 0) {
    pending?.classList.remove('hidden');
    document.getElementById('pending-list').innerHTML = nowDone.map(t => {
      const isIncoming = t.direction === 'received';
      const icon = t.status === 'completed' ? 'check_circle' : t.status === 'failed' ? 'error' : 'cancel';
      const color = t.status === 'completed'
        ? 'text-primary'
        : t.status === 'failed' ? 'text-error' : 'text-on-surface-variant';
      return `<div class="flex items-center gap-3 p-3 hover:bg-surface-container-low rounded-lg transition-colors">
        <span class="material-symbols-outlined ${color}">${icon}</span>
        <div class="flex-1 min-w-0">
          <p class="text-sm truncate">${escHtml(t.file_name)}</p>
          <p class="text-xs text-outline">${isIncoming ? '📥' : '📤'} ${fmtSize(t.total)} • ${statusText(t.status)}</p>
        </div>
      </div>`;
    }).join('');
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
      <td class="px-6 py-4 text-sm text-outline">${h.speed ? fmtSpeed(h.speed) : '-'}</td>
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

function syncSpeedUnitUI() {
  const sel = document.getElementById('speed-unit-select');
  if (sel) sel.value = speedUnit;
}

function syncPasswordUI() {
  const pwd = document.getElementById('encryption-password');
  if (pwd) pwd.value = appSettings.password || '';
}

// ── Tauri Events ──
function setupTauriEvents() {
  const l = tauriListen;
  l('transfer-progress', e => {
    const { id, sent, total } = e.payload;
    const t = transfers.find(x => x.id === id);
    if (t) {
      t.sent = sent; t.total = total;
    } else {
      // Buffer progress for in-flight sends whose real UUID isn't known yet
      pendingProgress[id] = { sent, total };
      return;
    }

    // Speed tracking
    const now = performance.now();
    const prev = transferSpeeds[id];
    if (prev && prev.lastBytes !== undefined && sent > prev.lastBytes) {
      const deltaBytes = sent - prev.lastBytes;
      const deltaSec = (now - prev.lastTime) / 1000;
      if (deltaSec > 0) {
        const instant = deltaBytes / deltaSec;
        prev.bytesPerSec = prev.bytesPerSec
          ? prev.bytesPerSec * 0.7 + instant * 0.3
          : instant;
      }
    } else {
      transferSpeeds[id] = { lastBytes: sent, lastTime: now, bytesPerSec: prev?.bytesPerSec || 0 };
    }
    if (prev) { prev.lastBytes = sent; prev.lastTime = now; }

    if (currentPage === 'transfers') updateTransferUI();
  });
  // If notifications are enabled and the tab is not focused, notify the user.
  function maybeNotify(title, body) {
    if (!appSettings.notificationsEnabled) return;
    if (document.hasFocus()) return; // don't notify if user is looking at the app
    try {
      if (Notification.permission === 'granted') {
        new Notification(title, { body, icon: 'icons/128x128.png' });
      } else if (Notification.permission === 'default') {
        Notification.requestPermission().then(p => {
          if (p === 'granted') new Notification(title, { body, icon: 'icons/128x128.png' });
        });
      }
    } catch (_) { /* Notification API not available */ }
  }

  l('transfer-complete', async () => {
    // Preserve direction from local array before re-fetching
    const oldDirs = {};
    transfers.forEach(t => { if (t.direction) oldDirs[t.id] = t.direction; });
    try { transfers = await tauriInvoke('get_transfers') || []; } catch {}
    transfers.forEach(t => { if (oldDirs[t.id]) t.direction = oldDirs[t.id]; });
    try { history = await tauriInvoke('get_history') || []; } catch {}
    // Clean up speed tracking for completed transfers
    const activeIds = new Set(transfers.map(t => t.id));
    Object.keys(transferSpeeds).forEach(id => { if (!activeIds.has(id)) delete transferSpeeds[id]; });
    if (currentPage === 'transfers') updateTransferUI();
    // Notify if user isn't looking at the app
    const done = transfers.filter(x => x.status === 'completed');
    if (done.length > 0) {
      maybeNotify('传输完成', `${done[0].file_name} 已发送完毕`);
    }
  });
  l('receive-complete', async (e) => {
    // Check for decryption/password errors from the backend
    const payload = e.payload || {};
    if (payload.error) {
      alert('接收失败：' + payload.error);
    }

    const oldDirs = {};
    transfers.forEach(t => { if (t.direction) oldDirs[t.id] = t.direction; });
    try { transfers = await tauriInvoke('get_transfers') || []; } catch {}
    transfers.forEach(t => { if (oldDirs[t.id]) t.direction = oldDirs[t.id]; });
    try { history = await tauriInvoke('get_history') || []; } catch {}
    const activeIds = new Set(transfers.map(t => t.id));
    Object.keys(transferSpeeds).forEach(id => { if (!activeIds.has(id)) delete transferSpeeds[id]; });
    if (currentPage === 'transfers') updateTransferUI();
    if (currentPage === 'history') renderHistory();
    const lastRecv = transfers.filter(x => x.direction === 'received' && x.status === 'completed');
    if (lastRecv.length > 0) {
      maybeNotify('接收完成', `${lastRecv[0].file_name} 已接收完毕`);
    }
  });
  // Native file drag-and-drop (handled by Rust → emitted as 'file-dropped')
  l('file-dropped', e => {
    const files = Array.isArray(e.payload) ? e.payload : [e.payload];
    for (const f of files) {
      if (f && f.path) {
        handleFilePath(f.path, f.size || 0, f.name || 'unknown');
      }
    }
    if (files.length > 0 && currentPage !== 'transfers') switchPage('transfers');
  });
  // Incoming transfer request — someone wants to send us a file
  l('incoming-transfer', e => {
    console.log('[incoming-transfer] received:', JSON.stringify(e));
    const payload = e.payload || e;
    const { request_id, peer, file_name, file_size } = payload;
    console.log('[incoming-transfer] request_id:', request_id, 'peer:', peer, 'file:', file_name);

    // Add a pending transfer entry
    transfers.push({
      id: request_id,
      file_name: `${file_name} (来自 ${peer})`,
      total: file_size,
      sent: 0,
      status: 'pending',
      direction: 'received',
    });
    if (currentPage !== 'transfers') switchPage('transfers');
    updateTransferUI();

    // Show confirmation dialog with inline styles (no CSS dependency)
    showIncomingDialog(request_id, peer, file_name, file_size);
  });
}

// ── Incoming Transfer Confirmation Dialog ──
// Uses ONLY inline styles (no Tailwind classes) to guarantee visibility
// regardless of the Tailwind CDN state or CSS load order.
function showIncomingDialog(requestId, peer, fileName, fileSize) {
  // Remove any existing dialog
  const old = document.getElementById('incoming-dialog');
  if (old) old.remove();

  const overlay = document.createElement('div');
  overlay.id = 'incoming-dialog';
  overlay.style.cssText =
    'position:fixed;top:0;left:0;right:0;bottom:0;' +
    'background:rgba(0,0,0,0.55);' +
    'display:flex;align-items:center;justify-content:center;' +
    'z-index:99999;font-family:-apple-system,BlinkMacSystemFont,sans-serif;';

  overlay.innerHTML =
    '<div style="background:#fff;border-radius:1.5rem;padding:2rem;max-width:26rem;width:90%;' +
    'box-shadow:0 1rem 3rem rgba(0,0,0,0.35);text-align:center;">' +
      '<div style="font-size:3rem;line-height:1;margin-bottom:1rem;">📥</div>' +
      '<h3 style="font-size:1.25rem;font-weight:700;margin:0 0 0.25rem;color:#191c1e;">收到传输请求</h3>' +
      '<p style="font-size:0.875rem;color:#6b7a7d;margin:0 0 1.5rem;">来自 ' + escHtml(peer) + '</p>' +
      '<div style="background:#f2f4f6;border-radius:0.75rem;padding:1rem;margin-bottom:1.5rem;text-align:left;">' +
        '<div style="display:flex;justify-content:space-between;margin-bottom:0.5rem;font-size:0.875rem;">' +
          '<span style="color:#6b7a7d;">文件名称</span>' +
          '<span style="font-weight:600;color:#191c1e;max-width:60%;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">' + escHtml(fileName) + '</span>' +
        '</div>' +
        '<div style="display:flex;justify-content:space-between;font-size:0.875rem;">' +
          '<span style="color:#6b7a7d;">文件大小</span>' +
          '<span style="font-weight:600;color:#191c1e;">' + fmtSize(fileSize) + '</span>' +
        '</div>' +
      '</div>' +
      '<div style="display:flex;gap:0.75rem;">' +
        '<button id="incoming-decline" style="flex:1;padding:0.75rem;border:1px solid #bac9cc;border-radius:6.25rem;' +
        'font-size:0.9375rem;font-weight:500;background:#fff;color:#191c1e;cursor:pointer;">拒绝</button>' +
        '<button id="incoming-accept" style="flex:1;padding:0.75rem;border:none;border-radius:6.25rem;' +
        'font-size:0.9375rem;font-weight:600;background:#006875;color:#fff;cursor:pointer;">接收</button>' +
      '</div>' +
    '</div>';
  document.body.appendChild(overlay);

  overlay.querySelector('#incoming-accept').addEventListener('click', () => {
    overlay.remove();
    respondTransfer(requestId, true);
  });
  overlay.querySelector('#incoming-decline').addEventListener('click', () => {
    overlay.remove();
    respondTransfer(requestId, false);
  });
  // Click outside to decline
  overlay.addEventListener('click', e => {
    if (e.target === overlay) {
      overlay.remove();
      respondTransfer(requestId, false);
    }
  });
}

async function respondTransfer(requestId, accept) {
  // Remove any confirmation dialog
  const dialog = document.getElementById('incoming-dialog');
  if (dialog) dialog.remove();

  if (!canInvoke()) {
    const t = transfers.find(x => x.id === requestId);
    if (t) t.status = accept ? 'active' : 'cancelled';
    updateTransferUI();
    return;
  }
  try {
    await tauriInvoke('respond_transfer', { requestId, accept });
    // Update local transfer status instead of re-fetching from backend
    // (backend TransferState has no direction field, so re-fetch would lose it)
    const t = transfers.find(x => x.id === requestId);
    if (t) {
      t.status = accept ? 'active' : 'cancelled';
      t.sent = 0;
    }
    updateTransferUI();
  } catch (e) {
    console.error('respond_transfer:', e);
  }
}

// ── Settings ──
async function pickSaveDir() {
  if (!canInvoke()) return alert('保存目录选择在离线模式下不可用');
  try {
    const dir = await tauriInvoke('pick_folder');
    if (dir) {
      await tauriInvoke('update_settings', { saveDir: dir });
      document.getElementById('settings-save-dir').textContent = dir;
    }
  } catch (e) { console.warn('pick folder:', e); }
}

// ── Utility ──
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
  // Filter out loopback, link-local (169.254), and RFC 2544 benchmarking
  // range (198.18-19.x — used by WSL2/Docker Desktop virtual adapters)
  return ips.find(ip =>
    !ip.startsWith('127.')
    && !ip.startsWith('169.254.')
    && !ip.startsWith('198.18.')
    && !ip.startsWith('198.19.')
    && !ip.startsWith('172.17.')
  ) || ips[0];
}

const SPEED_UNITS = [
  { key: 'B/s',   divisor: 1 },
  { key: 'KB/s',  divisor: 1e3 },
  { key: 'MB/s',  divisor: 1e6 },
  { key: 'GB/s',  divisor: 1e9 },
  { key: 'Mbps',  divisor: 1e6 / 8 },
  { key: 'Gbps',  divisor: 1e9 / 8 },
];

function fmtSpeed(bytesPerSec) {
  const unit = SPEED_UNITS.find(u => u.key === speedUnit) || SPEED_UNITS[2];
  const val = bytesPerSec / unit.divisor;
  if (val < 0.01) return '<0.01 ' + unit.key;
  if (val < 10) return val.toFixed(2) + ' ' + unit.key;
  if (val < 100) return val.toFixed(1) + ' ' + unit.key;
  return Math.round(val) + ' ' + unit.key;
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

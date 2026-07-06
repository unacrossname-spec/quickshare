// QuickShare App - Frontend Logic

const invoke = window.__TAURI__?.core?.invoke;
const listen = window.__TAURI__?.event?.listen;
const isTauri = !!invoke;

let currentPage = 'discover';
let transfers = [];

// ── Init ──
document.addEventListener('DOMContentLoaded', async () => {
  if (isTauri) {
    setupTauriEvents();
    loadLocalInfo();
    scanDevices();
  } else {
    // Dev mode: show mock data
    document.getElementById('device-name').textContent = 'Dev-Machine';
    document.getElementById('device-ip').textContent = '192.168.1.100';
    showMockDevices();
  }
});

// ── Page switching ──
function switchPage(page) {
  currentPage = page;
  document.querySelectorAll('.page').forEach(el => el.classList.add('hidden'));
  document.getElementById(`page-${page}`).classList.remove('hidden');
  document.querySelectorAll('.nav-item').forEach(el => {
    el.classList.toggle('active', el.dataset.page === page);
  });

  const titles = { discover: '设备发现', transfers: '传输队列', history: '历史记录', settings: '设置' };
  document.getElementById('page-title').textContent = titles[page] || 'QuickShare';

  if (page === 'transfers') updateTransferUI();
  if (page === 'discovery') scanDevices();
}

// ── Local info ──
async function loadLocalInfo() {
  if (!isTauri) return;
  try {
    const info = await invoke('get_local_info');
    document.getElementById('device-name').textContent = info.name;
    document.getElementById('device-ip').textContent = info.ips.join(', ') || '未知';
    document.getElementById('settings-save-dir').textContent = info.save_dir;
  } catch (e) {
    console.error('get_local_info:', e);
  }
}

// ── Device discovery ──
async function scanDevices() {
  const grid = document.getElementById('device-grid');
  const status = document.getElementById('discover-status');
  const noDev = document.getElementById('no-devices');

  grid.classList.add('hidden');
  noDev.classList.add('hidden');
  status.classList.remove('hidden');

  if (isTauri) {
    // In Tauri, we'd call a command to scan LAN
    // For now, show manual entry
    await new Promise(r => setTimeout(r, 2000));
  } else {
    await new Promise(r => setTimeout(r, 2000));
  }

  status.classList.add('hidden');

  // If no devices found, show manual entry
  noDev.classList.remove('hidden');
}

function showMockDevices() {
  const grid = document.getElementById('device-grid');
  const status = document.getElementById('discover-status');
  const noDev = document.getElementById('no-devices');

  status.classList.add('hidden');
  noDev.classList.add('hidden');

  const devices = [
    { name: 'MacBook Pro', ip: '192.168.1.12', icon: 'laptop_mac', type: '笔记本电脑', online: true },
    { name: 'Windows PC', ip: '192.168.0.104', icon: 'desktop_windows', type: '桌面电脑', online: true },
    { name: 'iPhone 15', ip: '192.168.1.15', icon: 'smartphone', type: '手机', online: true },
    { name: 'Workstation', ip: '192.168.1.5', icon: 'desktop_windows', type: '桌面电脑', online: false },
  ];

  grid.innerHTML = devices.map(d => `
    <div class="device-card bg-surface-container-lowest border border-outline-variant p-5 rounded-2xl flex items-center gap-4 cursor-pointer ${d.online ? 'hover:border-primary' : 'opacity-50'}">
      <div class="w-12 h-12 bg-surface-container-high rounded-full flex items-center justify-center text-on-surface-variant">
        <span class="material-symbols-outlined">${d.icon}</span>
      </div>
      <div class="flex-1 min-w-0">
        <h4 class="font-medium truncate">${d.name}</h4>
        <p class="text-xs text-outline truncate">${d.ip}</p>
        <p class="text-xs text-outline">${d.type}</p>
      </div>
      ${d.online
        ? `<button class="px-4 py-2 bg-primary-container text-on-primary-container text-xs font-medium rounded-full hover:bg-primary hover:text-white transition-colors" onclick="sendTo('${d.ip}')">发送</button>`
        : `<span class="text-xs text-outline font-medium">离线</span>`}
    </div>
  `).join('');
  grid.classList.remove('hidden');
}

function manualConnect() {
  const addr = document.getElementById('manual-addr').value.trim();
  if (!addr) return;
  sendTo(addr);
}

// ── Send file ──
async function sendTo(addr) {
  // In a real app, this would open a file picker dialog
  // For now, just alert
  alert(`发送到 ${addr}\n\n文件选择功能通过 Tauri dialog 实现。\n开发模式下暂不可用。`);
}

// ── Transfer UI ──
function updateTransferUI() {
  const active = document.getElementById('active-transfer');
  const empty = document.getElementById('transfer-empty');
  const pending = document.getElementById('transfer-pending');

  if (transfers.length === 0) {
    active.classList.add('hidden');
    empty.classList.remove('hidden');
    pending.classList.add('hidden');
    return;
  }

  active.classList.remove('hidden');
  empty.classList.add('hidden');

  const activeItems = transfers.filter(t => t.status === 'active');
  const pendingItems = transfers.filter(t => t.status !== 'active');

  const cards = document.getElementById('transfer-cards');
  cards.innerHTML = activeItems.map(t => {
    const pct = t.total > 0 ? Math.round((t.sent / t.total) * 100) : 0;
    return `
      <div class="bg-surface-container-lowest border border-outline-variant p-5 rounded-2xl">
        <div class="flex items-center gap-4 mb-4">
          <div class="w-10 h-10 bg-primary-container/20 rounded flex items-center justify-center text-primary">
            <span class="material-symbols-outlined">description</span>
          </div>
          <div class="flex-1 min-w-0">
            <p class="font-medium truncate">${t.file_name}</p>
            <p class="text-xs text-outline">发送中...</p>
          </div>
          <button class="w-9 h-9 rounded-full border border-outline-variant flex items-center justify-center hover:bg-error-container hover:text-error transition-colors" onclick="cancelTransfer('${t.id}')">
            <span class="material-symbols-outlined text-sm">close</span>
          </button>
        </div>
        <div class="progress-bar"><div class="progress-bar-fill" style="width:${pct}%"></div></div>
        <div class="mt-2 flex justify-between text-xs text-outline">
          <span class="text-primary font-medium">${pct}%</span>
          <span>${formatSize(t.sent)} / ${formatSize(t.total)}</span>
        </div>
      </div>
    `;
  }).join('');

  if (pendingItems.length > 0) {
    pending.classList.remove('hidden');
    const list = document.getElementById('pending-list');
    list.innerHTML = pendingItems.map(t => `
      <div class="flex items-center gap-3 p-3 hover:bg-surface-container-low rounded-lg transition-colors">
        <span class="material-symbols-outlined text-outline">description</span>
        <div class="flex-1 min-w-0">
          <p class="text-sm truncate">${t.file_name}</p>
          <p class="text-xs text-outline">${formatSize(t.total)} • ${statusText(t.status)}</p>
        </div>
      </div>
    `).join('');
  } else {
    document.getElementById('pending-list').innerHTML = '';
    pending.classList.add('hidden');
  }
}

function cancelTransfer(id) {
  if (isTauri) {
    invoke('cancel_transfer', { id }).catch(console.error);
  }
  transfers = transfers.filter(t => t.id !== id);
  updateTransferUI();
}

// ── Tauri events ──
function setupTauriEvents() {
  if (!listen) return;
  listen('transfer-progress', e => {
    const { id, sent, total } = e.payload;
    const t = transfers.find(t => t.id === id);
    if (t) { t.sent = sent; t.total = total; }
    if (currentPage === 'transfers') updateTransferUI();
  });
  listen('transfer-complete', e => {
    // Update or remove completed transfer
    updateTransferUI();
  });
  listen('receive-complete', e => {
    const { peer, file, count } = e.payload;
    // Show notification or update history
  });
}

// ── Utility ──
function formatSize(bytes) {
  if (bytes >= 1e9) return (bytes / 1e9).toFixed(1) + ' GB';
  if (bytes >= 1e6) return (bytes / 1e6).toFixed(1) + ' MB';
  if (bytes >= 1e3) return (bytes / 1e3).toFixed(1) + ' KB';
  return bytes + ' B';
}

function statusText(s) {
  return { completed: '已完成', failed: '失败', cancelled: '已取消', active: '传输中' }[s] || s;
}

// OSAgent Launcher - Frontend
(function () {
  'use strict';

  const { invoke, listen, window: tauriWindow } = window.__TAURI__;

  // DOM Elements
  const $ = (sel) => document.querySelector(sel);
  const statusCard = $('#status-card');
  const statusLabel = $('#status-label');
  const statusPid = $('#status-pid');
  const pathBinary = $('#path-binary');
  const pathConfig = $('#path-config');
  const logContainer = $('#log-container');

  const btnStart = $('#btn-start');
  const btnStop = $('#btn-stop');
  const btnRestart = $('#btn-restart');
  const btnOpenUi = $('#btn-open-ui');
const btnBuild = $('#btn-build');
  const btnMinimize = $('#btn-minimize');
  const btnClose = $('#btn-close');
  const btnClearLog = $('#btn-clear-log');

  let isRunning = false;
  let logs = [];

  // --- Window Dragging ---
  // Use Tauri window.startDragging() for custom title bar drag
  const titlebar = document.querySelector('.titlebar');
  titlebar.addEventListener('mousedown', (e) => {
    if (e.target.closest('.titlebar-controls')) return; // Don't drag on buttons
    tauriWindow.appWindow.startDragging();
  });

  // --- UI Updates ---

  function updateStatus(running, pid) {
    isRunning = running;
    statusCard.className = 'status-card ' + (running ? 'running' : 'stopped');
    statusLabel.textContent = running ? 'Running' : 'Stopped';
    statusPid.textContent = pid ? `PID: ${pid}` : 'PID: -';

    btnStart.disabled = running;
    btnStop.disabled = !running;
    btnRestart.disabled = !running;
  }

  function addLog(level, message) {
    const time = new Date().toLocaleTimeString('en-GB', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
    });

    const entry = { time, level, message };
    logs.push(entry);
    if (logs.length > 300) logs.shift();

    renderLogEntry(entry);
  }

  function renderLogEntry(entry) {
    const div = document.createElement('div');
    div.className = 'log-entry';
    div.innerHTML =
      '<span class="log-time">' +
      escapeHtml(entry.time) +
      '</span>' +
      '<span class="log-level ' +
      entry.level +
      '">' +
      entry.level.toUpperCase() +
      '</span>' +
      '<span class="log-msg">' +
      escapeHtml(entry.message) +
      '</span>';

    logContainer.appendChild(div);
    logContainer.scrollTop = logContainer.scrollHeight;
  }

  function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  // --- Tauri Commands ---

  async function getStatus() {
    try {
      const status = await invoke('get_status');
      updateStatus(status.running, status.pid);
      pathBinary.textContent = status.osagent_path;
      pathConfig.textContent = status.config_path;
    } catch (e) {
      addLog('error', 'Failed to get status: ' + e);
    }
  }

  async function startAgent() {
    addLog('info', 'Starting OSAgent...');
    try {
      const status = await invoke('start_osagent');
      updateStatus(status.running, status.pid);
      addLog('info', 'OSAgent started (PID: ' + status.pid + ')');
    } catch (e) {
      addLog('error', 'Start failed: ' + e);
    }
  }

  async function stopAgent() {
    addLog('info', 'Stopping OSAgent...');
    try {
      const status = await invoke('stop_osagent');
      updateStatus(false, null);
      addLog('info', 'OSAgent stopped');
    } catch (e) {
      addLog('error', 'Stop failed: ' + e);
    }
  }

  async function restartAgent() {
    addLog('info', 'Restarting OSAgent...');
    try {
      await invoke('stop_osagent');
      await new Promise((r) => setTimeout(r, 500));
      const status = await invoke('start_osagent');
      updateStatus(status.running, status.pid);
      addLog('info', 'OSAgent restarted (PID: ' + status.pid + ')');
    } catch (e) {
      addLog('error', 'Restart failed: ' + e);
    }
  }

  function normalizeEntry(raw) {
    return {
      time: raw.time || raw.timestamp || new Date().toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' }),
      level: raw.level || 'info',
      message: raw.message || '',
    };
  }

  async function loadLogs() {
    try {
      const existingLogs = await invoke('get_logs');
      existingLogs.forEach((l) => {
        const entry = normalizeEntry(l);
        logs.push(entry);
        renderLogEntry(entry);
      });
    } catch (e) {
      // no existing logs
    }
  }

  // --- Event Listeners ---

  btnStart.addEventListener('click', startAgent);
  btnStop.addEventListener('click', stopAgent);
  btnRestart.addEventListener('click', restartAgent);

  btnOpenUi.addEventListener('click', async () => {
    try {
      await invoke('open_web_ui');
    } catch (e) {
      addLog('error', 'Failed to open Web UI: ' + e);
    }
  });

let buildPollInterval = null;
  let buildPollLogCount = 0;

  function stopBuildPolling() {
    if (buildPollInterval) {
      clearInterval(buildPollInterval);
      buildPollInterval = null;
    }
    btnBuild.disabled = false;
  }

  async function flushBuildLogs() {
    const allLogs = await invoke('get_logs');
    while (buildPollLogCount < allLogs.length) {
      const entry = normalizeEntry(allLogs[buildPollLogCount]);
      logs.push(entry);
      if (logs.length > 300) logs.shift();
      renderLogEntry(entry);
      buildPollLogCount++;
    }
  }

  async function pollBuild() {
    try {
      await flushBuildLogs();

      const building = await invoke('get_build_running');
      if (!building) {
        // Do one final flush — the Rust side joins reader threads before
        // clearing build_running, so all output is in state by now.
        await flushBuildLogs();
        stopBuildPolling();
      }
    } catch (e) {
      stopBuildPolling();
    }
  }

  btnBuild.addEventListener('click', async () => {
    addLog('info', 'Starting build...');
    btnBuild.disabled = true;
    try {
      // Capture the Rust state log count BEFORE the build adds anything,
      // so polling starts from the right index in the Rust state array.
      const logsBefore = await invoke('get_logs');
      buildPollLogCount = logsBefore.length;
      await invoke('build_osagent');
      buildPollInterval = setInterval(pollBuild, 500);
    } catch (e) {
      addLog('error', 'Build failed: ' + e);
      btnBuild.disabled = false;
    }
  });

  btnMinimize.addEventListener('click', async () => {
    try {
      await invoke('minimize_window');
    } catch (e) {
      // ignore
    }
  });

  btnClose.addEventListener('click', async () => {
    try {
      await invoke('hide_to_tray');
    } catch (e) {
      // ignore
    }
  });

  btnClearLog.addEventListener('click', () => {
    logs = [];
    logContainer.innerHTML = '';
  });

  // --- Tauri Events ---

  console.log('listen function:', typeof listen, listen);
  if (typeof listen === 'function') {
    listen('osagent-status-changed', (event) => {
      const status = event.payload;
      updateStatus(status.running, status.pid);
    });

    // Real-time log lines from osagent stdout/stderr
    listen('log-line', (event) => {
      const entry = normalizeEntry(event.payload);
      logs.push(entry);
      if (logs.length > 300) logs.shift();
      renderLogEntry(entry);
    });

    // Build completion event
    listen('build-completed', (event) => {
      btnBuild.disabled = false;
      addLog('info', 'Build process finished');
    });

    // Debug: test event listener
    listen('test-event', (event) => {
      addLog('info', 'TEST EVENT: ' + event.payload);
    });
  }

  // --- Init ---

  getStatus();
  loadLogs();
})();

// OSAgent Launcher - Frontend
(function () {
  'use strict';

  const tauri = window.__TAURI__ || {};
  const invoke = tauri.core?.invoke ?? tauri.invoke;
  const listen = tauri.event?.listen ?? tauri.listen;
  const tauriWindow = tauri.window;

  const WHISPER_MODELS = [
    { id: 'tiny',   name: 'Tiny',   size: '75 MB',  note: 'Fastest' },
    { id: 'base',   name: 'Base',   size: '142 MB', note: 'Balanced' },
    { id: 'small',  name: 'Small',  size: '466 MB', note: 'Accurate' },
    { id: 'medium', name: 'Medium', size: '1.5 GB', note: 'Best accuracy' }
  ];

  const PIPER_VOICES = {
    en: [
      { id: 'en_US-libritts-high',  name: 'LibriTTS', quality: 'high'   },
      { id: 'en_US-lessac-medium',  name: 'Lessac',   quality: 'medium' },
      { id: 'en_US-amy-medium',     name: 'Amy',      quality: 'medium' },
      { id: 'en_GB-semaine-medium', name: 'Semaine',  quality: 'medium' }
    ],
    de: [{ id: 'de_DE-thorsten-medium', name: 'Thorsten', quality: 'medium' }],
    fr: [{ id: 'fr_FR-siwis-medium',    name: 'Siwis',    quality: 'medium' }],
    es: [{ id: 'es_ES-sharvard-medium', name: 'Sharvard', quality: 'medium' }]
  };

  const state = {
    isRunning: false,
    logs: [],
    setup: null,
    providers: {},
    providerOrder: [],
    providerFilter: '',
    modelFilter: '',
    providerValidation: {
      status: 'idle',
      message: 'Test the provider before saving.',
      signature: ''
    },
    logPollInterval: null,
    logSyncCount: 0,
    buildPollInterval: null,
    buildPollLogCount: 0,
    buildProfile: 'release',
    runProfile: 'release',
    buildProgress: null,
    buildUnlisten: null,
    wizard: {
      step: 0,
      provider_type: '',
      model: '',
      auth_mode: 'api_key',
      api_key: '',
      workspace_path: '',
      password_enabled: true,
      password: '',
      confirm_password: '',
      stt_mode: 'browser',
      stt_whisper_model: 'base',
      tts_mode: 'browser',
      tts_piper_language: 'en',
      tts_piper_voice: 'en_US-libritts-high',
      discord_enabled: false,
      discord_token: '',
      discord_user_id: ''
    },
    devicePoll: null,
    voiceUnlisten: null
  };

  const $ = (selector) => document.querySelector(selector);
  const $$ = (selector) => Array.from(document.querySelectorAll(selector));

  const els = {
    titlebar: $('.titlebar'),
    setupView: $('#setup-view'),
    finishView: $('#finish-view'),
    dashboardView: $('#dashboard-view'),
    setupNotice: $('#setup-notice'),
    setupError: $('#setup-error'),
    setupConfigPath: $('#setup-config-path'),
    setupBinaryStatus: $('#setup-binary-status'),
    wizardTabs: $$('#wizard-steps .wizard-pill'),
    wizardPanels: $$('.wizard-panel'),
    wizardBack: $('#wizard-back'),
    wizardNext: $('#wizard-next'),
    wizardSave: $('#wizard-save'),
    providerSearch: $('#provider-search'),
    providerDesc: $('#provider-desc'),
    providerSelect: $('#provider-select'),
    modelSearch: $('#model-search'),
    providerModel: $('#provider-model'),
    providerAuthApi: $('#provider-auth-api'),
    providerAuthSignin: $('#provider-auth-signin'),
    providerAuthToggle: $('#provider-auth-toggle'),
    providerAuthNote: $('#provider-auth-note'),
    providerApiKeyWrap: $('#provider-api-key-wrap'),
    providerSignInRow: $('#provider-signin-row'),
    btnProviderSignIn: $('#btn-provider-signin'),
    deviceCodeHint: $('#device-code-hint'),
    deviceCodeUrl: $('#device-code-url'),
    deviceCodeValue: $('#device-code-value'),
    deviceCodeStatus: $('#device-code-status'),
    providerSigninHint: $('#provider-signin-hint'),
    providerApiKey: $('#provider-api-key'),
    providerKeyLabel: $('#provider-key-label'),
    providerKeyHelp: $('#provider-key-help'),
    providerSavedKeyStatus: $('#provider-saved-key-status'),
    providerTestResult: $('#provider-test-result'),
    btnTestProvider: $('#btn-test-provider'),
    workspacePath: $('#workspace-path'),
    browseWorkspace: $('#btn-browse-workspace'),
    passwordEnabled: $('#password-enabled'),
    passwordFields: $('#password-fields'),
    passwordInput: $('#password-input'),
    passwordConfirm: $('#password-confirm'),
    reviewProvider: $('#review-provider'),
    reviewModel: $('#review-model'),
    reviewWorkspace: $('#review-workspace'),
    reviewConfig: $('#review-config'),
    finishProvider: $('#finish-provider'),
    finishWorkspace: $('#finish-workspace'),
    finishSecurity: $('#finish-security'),
    statusCard: $('#status-card'),
    statusLabel: $('#status-label'),
    statusPid: $('#status-pid'),
    pathBinary: $('#path-binary'),
    pathConfig: $('#path-config'),
    dashboardProvider: $('#dashboard-provider'),
    dashboardWorkspace: $('#dashboard-workspace'),
    dashboardSecurity: $('#dashboard-security'),
    logContainer: $('#log-container'),
    btnStart: $('#btn-start'),
    btnStop: $('#btn-stop'),
    btnRestart: $('#btn-restart'),
    btnOpenUi: $('#btn-open-ui'),
    btnOpenSetup: $('#btn-open-setup'),
    btnBuild: $('#btn-build'),
    buildProfileToggle: $('#build-profile-toggle'),
    runProfileToggle: $('#run-profile-toggle'),
    buildProgressSection: $('#build-progress-section'),
    buildProgressLabel: $('#build-progress-label'),
    buildProgressFill: $('#build-progress-fill'),
    buildProgressStats: $('#build-progress-stats'),
    btnFinishOpenUi: $('#btn-finish-open-ui'),
    btnFinishDashboard: $('#btn-finish-dashboard'),
    btnMinimize: $('#btn-minimize'),
    btnClose: $('#btn-close'),
    btnClearLog: $('#btn-clear-log'),
    // Voice wizard
    sttModeToggle: $('#stt-mode-toggle'),
    sttModelCards: $('#stt-model-cards'),
    ttsModeToggle: $('#tts-mode-toggle'),
    ttsLocalOptions: $('#tts-local-options'),
    ttsLangSelect: $('#tts-lang-select'),
    ttsVoiceCards: $('#tts-voice-cards'),
    voiceDownloadSection: $('#voice-download-section'),
    voiceWizProgressWrap: $('#voice-wiz-progress-wrap'),
    voiceWizProgressLabel: $('#voice-wiz-progress-label'),
    voiceWizProgressFill: $('#voice-wiz-progress-fill'),
    voiceWizProgressText: $('#voice-wiz-progress-text'),
    btnDownloadVoice: $('#btn-download-voice'),
    voiceWizReady: $('#voice-wiz-ready'),
    // Voice dashboard
    voicePanel: $('#voice-panel'),
    voiceStatusChip: $('#voice-status-chip'),
    voiceSttStatus: $('#voice-stt-status'),
    voiceTtsStatus: $('#voice-tts-status'),
    voiceDashProgressWrap: $('#voice-dash-progress-wrap'),
    voiceDashProgressLabel: $('#voice-dash-progress-label'),
    voiceDashProgressFill: $('#voice-dash-progress-fill'),
    voiceDashProgressText: $('#voice-dash-progress-text'),
    btnInstallVoiceDash: $('#btn-install-voice-dash'),
    // Discord
    discordGuideToggle: $('#discord-guide-toggle'),
    discordGuideContent: $('#discord-guide-content'),
    discordToken: $('#discord-token'),
    discordUserId: $('#discord-user-id'),
    discordEnabled: $('#discord-enabled'),
    discordReview: $('#review-discord'),
    btnOpenDiscordDev: $('#btn-open-discord-dev')
  };

  function currentProvider() {
    const provider = state.providers[state.wizard.provider_type];
    if (provider) return provider;
    const firstId = state.providerOrder[0];
    return firstId ? state.providers[firstId] : null;
  }

  function providerDisplayName(providerType) {
    const provider = state.providers[providerType];
    return provider ? provider.name : providerType || '-';
  }

  function hasSavedKeyForSelectedProvider() {
    return Boolean(
      state.setup &&
      state.setup.api_key_configured &&
      state.setup.provider_type === state.wizard.provider_type
    );
  }

  function currentProviderValidationSignature() {
    return [
      state.wizard.provider_type,
      state.wizard.model,
      state.wizard.auth_mode,
      state.wizard.api_key ? `typed:${state.wizard.api_key}` : `saved:${hasSavedKeyForSelectedProvider()}`
    ].join('|');
  }

  function showView(viewName) {
    els.setupView.classList.toggle('hidden', viewName !== 'setup');
    els.finishView.classList.toggle('hidden', viewName !== 'finish');
    els.dashboardView.classList.toggle('hidden', viewName !== 'dashboard');
    if (viewName === 'dashboard') checkVoiceStatus();
  }

  function setSetupError(message) {
    if (!message) {
      els.setupError.textContent = '';
      els.setupError.classList.add('hidden');
      return;
    }
    els.setupError.textContent = message;
    els.setupError.classList.remove('hidden');
  }

  function setSetupNotice(message) {
    if (!message) {
      els.setupNotice.textContent = '';
      els.setupNotice.classList.add('hidden');
      return;
    }
    els.setupNotice.textContent = message;
    els.setupNotice.classList.remove('hidden');
  }

  function updateStatus(running, pid) {
    state.isRunning = running;
    els.statusCard.className = 'status-card ' + (running ? 'running' : 'stopped');
    els.statusLabel.textContent = running ? 'Running' : 'Stopped';
    els.statusPid.textContent = pid ? `PID: ${pid}` : 'PID: -';
    els.btnStart.disabled = running;
    els.btnStop.disabled = !running;
    els.btnRestart.disabled = !running;
  }

  function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
  }

  function setProviderValidation(status, message) {
    state.providerValidation.status = status;
    state.providerValidation.message = message;
    state.providerValidation.signature = currentProviderValidationSignature();
    els.providerTestResult.className = `provider-test-result ${status}`;
    els.providerTestResult.textContent = message;
  }

  function resetProviderValidation() {
    state.providerValidation.signature = '';
    if (state.wizard.auth_mode === 'oauth') {
      setProviderValidation('idle', 'Sign in first. You can pick a model after this step or later in the web UI.');
      state.providerValidation.signature = '';
      return;
    }
    if (hasSavedKeyForSelectedProvider()) {
      setProviderValidation('idle', 'A saved key exists for this provider. Leave blank to reuse it, then test connection.');
      state.providerValidation.signature = '';
      return;
    }
    setProviderValidation('idle', 'Test the provider before saving.');
    state.providerValidation.signature = '';
  }

  function hasFreshProviderValidation() {
    if (state.wizard.auth_mode === 'oauth') return true;
    return (
      state.providerValidation.status === 'success' &&
      state.providerValidation.signature === currentProviderValidationSignature()
    );
  }

  function providerSupportsSignIn(provider) {
    return Boolean(provider && provider.oauth);
  }

  function setProviderAuthMode(mode) {
    const provider = currentProvider();
    const allowSignIn = providerSupportsSignIn(provider);
    if (mode === 'oauth' && !allowSignIn) {
      mode = 'api_key';
    }
    if (mode === 'api_key' && state.devicePoll) {
      // cancel any in-progress device code flow when switching to API key mode
      clearInterval(state.devicePoll.timer);
      state.devicePoll = null;
      if (els.deviceCodeHint) els.deviceCodeHint.classList.add('hidden');
      if (els.providerSigninHint) els.providerSigninHint.classList.remove('hidden');
      if (els.btnProviderSignIn) {
        els.btnProviderSignIn.disabled = false;
        els.btnProviderSignIn.textContent = 'Sign in with Provider';
      }
    }
    state.wizard.auth_mode = mode;

    els.providerAuthSignin.classList.toggle('hidden', !allowSignIn);
    els.providerApiKeyWrap.classList.toggle('hidden', mode !== 'api_key');
    els.providerSignInRow.classList.toggle('hidden', mode !== 'oauth');
    els.btnTestProvider.classList.toggle('hidden', mode !== 'api_key');
    els.btnProviderSignIn.disabled = false;

    els.providerAuthNote.textContent = 'Sign in is only available for providers with OAuth support.';
    els.providerAuthNote.classList.toggle('hidden', allowSignIn);

    els.providerAuthApi.classList.toggle('active', mode === 'api_key');
    els.providerAuthSignin.classList.toggle('active', mode === 'oauth');

    resetProviderValidation();
    updateReview();
  }

  function providerFilterMatch(provider, query) {
    if (!query) return true;
    const value = `${provider.name} ${provider.id} ${provider.description}`.toLowerCase();
    return value.includes(query);
  }

  function renderProviderSelect() {
    const query = state.providerFilter.trim().toLowerCase();
    const filteredIds = state.providerOrder.filter((id) => providerFilterMatch(state.providers[id], query));
    const selectedId = filteredIds.includes(state.wizard.provider_type)
      ? state.wizard.provider_type
      : (filteredIds[0] || state.providerOrder[0] || '');

    els.providerSelect.innerHTML = '';
    filteredIds.forEach((id) => {
      const provider = state.providers[id];
      const option = document.createElement('option');
      option.value = provider.id;
      option.textContent = `${provider.name} (${provider.id})`;
      els.providerSelect.appendChild(option);
    });

    if (!filteredIds.length && state.providerOrder.length) {
      state.providerOrder.forEach((id) => {
        const provider = state.providers[id];
        const option = document.createElement('option');
        option.value = provider.id;
        option.textContent = `${provider.name} (${provider.id})`;
        els.providerSelect.appendChild(option);
      });
    }

    if (selectedId) {
      state.wizard.provider_type = selectedId;
      els.providerSelect.value = selectedId;
    }
    renderModelSelect();
    updateProviderFields();

    const selectedProvider = currentProvider();
    if (selectedProvider && selectedProvider.id === 'ollama' && (!selectedProvider.models || !selectedProvider.models.length)) {
      refreshCurrentProviderModels();
    }
  }

  function renderModelSelect() {
    const provider = currentProvider();
    if (!provider) {
      els.providerModel.innerHTML = '';
      state.wizard.model = '';
      return;
    }

    const query = state.modelFilter.trim().toLowerCase();
    const filtered = query
      ? provider.models.filter((m) => `${m.name} ${m.id}`.toLowerCase().includes(query))
      : provider.models;
    const modelsToShow = filtered.length ? filtered : provider.models;

    els.providerModel.innerHTML = '';
    const placeholder = document.createElement('option');
    placeholder.value = '';
    placeholder.textContent = 'Use provider default (choose later)';
    els.providerModel.appendChild(placeholder);

    if (!modelsToShow.length && provider.id === 'ollama') {
      const offline = document.createElement('option');
      offline.value = '';
      offline.textContent = 'No Ollama models found (is Ollama running?)';
      offline.disabled = true;
      els.providerModel.appendChild(offline);
    }

    modelsToShow.forEach((model) => {
      const option = document.createElement('option');
      option.value = model.id;
      option.textContent = model.name;
      els.providerModel.appendChild(option);
    });

    const validCurrent = modelsToShow.some((m) => m.id === state.wizard.model);
    state.wizard.model = validCurrent ? state.wizard.model : '';
    els.providerModel.value = state.wizard.model;
  }

  async function refreshCurrentProviderModels() {
    const provider = currentProvider();
    if (!provider || provider.id !== 'ollama') {
      renderModelSelect();
      return;
    }

    try {
      const models = await invoke('discover_setup_provider_models', {
        payload: { provider_type: provider.id }
      });
      provider.models = Array.isArray(models) ? models : [];
    } catch (_error) {
      provider.models = [];
    }

    renderModelSelect();
  }

  function updateProviderFields() {
    const provider = currentProvider();
    if (!provider) return;

    els.providerKeyLabel.textContent = provider.key_label;
    els.providerApiKey.placeholder = provider.key_placeholder;
    els.providerKeyHelp.textContent = hasSavedKeyForSelectedProvider()
      ? `${provider.key_help} Leave this blank to keep the saved key already stored in your config.`
      : provider.key_help;
    els.providerSavedKeyStatus.textContent = hasSavedKeyForSelectedProvider()
      ? 'Saved key found and ready to reuse'
      : 'No saved key for this provider yet';

    if (!providerSupportsSignIn(provider) && state.wizard.auth_mode !== 'api_key') {
      state.wizard.auth_mode = 'api_key';
    }
    setProviderAuthMode(state.wizard.auth_mode);
    updateProviderDesc();
  }

  function updatePasswordFields() {
    els.passwordFields.classList.toggle('hidden', !state.wizard.password_enabled);
  }

  function updateReview() {
    const provider = currentProvider();
    els.reviewProvider.textContent = provider ? provider.name : '-';
    els.reviewModel.textContent = state.wizard.model || 'Provider default (change later)';
    els.reviewWorkspace.textContent = state.wizard.workspace_path || 'No folder selected';
    els.discordReview.textContent = state.wizard.discord_enabled
      ? (state.wizard.discord_token ? 'Enabled' : 'No token')
      : 'Disabled';
    els.reviewConfig.textContent = state.setup ? state.setup.config_path : '-';
  }

  function updateProviderDesc() {
    const provider = currentProvider();
    if (els.providerDesc) {
      els.providerDesc.textContent = provider ? provider.description : '';
    }
  }

  function updateFinishSummary(setup) {
    els.finishProvider.textContent = providerDisplayName(setup.provider_type);
    els.finishWorkspace.textContent = setup.workspace_path || '-';
    els.finishSecurity.textContent = setup.password_enabled ? 'Password required' : 'No password';
  }

  function updateWizardTabs() {
    els.wizardTabs.forEach((tab) => {
      const step = Number(tab.dataset.step);
      const isActive = step === state.wizard.step;
      const isDone = step < state.wizard.step;
      tab.classList.toggle('active', isActive);
      tab.classList.toggle('done', isDone);
      const numEl = tab.querySelector('.wizard-pill-num');
      if (numEl) numEl.textContent = isDone ? '✓' : String(step + 1);
    });

    els.wizardPanels.forEach((panel) => {
      panel.classList.toggle('active', Number(panel.dataset.stepPanel) === state.wizard.step);
    });

    els.wizardBack.disabled = state.wizard.step === 0;
    const onLastStep = state.wizard.step === els.wizardPanels.length - 1;
    els.wizardNext.classList.toggle('hidden', onLastStep);
    els.wizardSave.classList.toggle('hidden', !onLastStep);
    updateReview();
  }

  function hydrateWizardFields() {
    state.modelFilter = '';
    if (els.modelSearch) els.modelSearch.value = '';
    renderProviderSelect();
    updateProviderDesc();
    els.providerApiKey.value = state.wizard.api_key;
    els.workspacePath.value = state.wizard.workspace_path;
    els.passwordEnabled.checked = state.wizard.password_enabled;
    els.passwordInput.value = state.wizard.password;
    els.passwordConfirm.value = state.wizard.confirm_password;
    els.discordEnabled.checked = state.wizard.discord_enabled;
    els.discordToken.value = state.wizard.discord_token;
    els.discordUserId.value = state.wizard.discord_user_id;
    updatePasswordFields();
    updateWizardTabs();
  }

  function updateDashboardSetup(setup) {
    els.dashboardProvider.textContent = providerDisplayName(setup.provider_type || 'Not configured');
    els.dashboardWorkspace.textContent = setup.workspace_path || '-';
    els.dashboardSecurity.textContent = setup.password_enabled ? 'Password required' : 'No password';
  }

  function applySetupState(setup) {
    state.setup = setup;
    els.setupConfigPath.textContent = setup.config_path;
    els.setupBinaryStatus.textContent = setup.osagent_found ? 'Found and ready to launch' : 'OSAgent binary not found yet';
    updateDashboardSetup(setup);
    updateFinishSummary(setup);

    const fallbackProvider = state.providers[setup.provider_type] ? setup.provider_type : (state.providerOrder[0] || '');
    state.wizard.provider_type = fallbackProvider;
    state.wizard.workspace_path = setup.workspace_path || state.wizard.workspace_path;
    state.wizard.password_enabled = setup.password_enabled;
    state.wizard.password = '';
    state.wizard.confirm_password = '';
    state.wizard.api_key = '';
    state.wizard.model = '';
    setProviderAuthMode('api_key');
    resetProviderValidation();

    let notice = '';
    if (!setup.osagent_found) {
      notice = 'OSA is not built yet. You can save setup now and build later from the control panel.';
    } else if (setup.has_config && setup.needs_setup) {
      notice = 'Your existing config looks incomplete. Saving here will replace it with this setup.';
    } else if (setup.has_config) {
      notice = 'Existing setup found. You can keep saved keys by leaving the API key field blank.';
    } else {
      notice = 'First run detected. The launcher will create config.toml for you.';
    }
    setSetupNotice(notice);
    hydrateWizardFields();
    refreshCurrentProviderModels();

    if (setup.needs_setup) {
      state.wizard.step = 0;
      updateWizardTabs();
      showView('setup');
    } else {
      showView('dashboard');
    }
  }

  function openSetupWizard() {
    if (state.setup) {
      state.wizard.provider_type = state.providers[state.setup.provider_type]
        ? state.setup.provider_type
        : (state.providerOrder[0] || '');
      state.wizard.model = '';
      state.wizard.workspace_path = state.setup.workspace_path || state.wizard.workspace_path;
      state.wizard.password_enabled = state.setup.password_enabled;
      state.wizard.password = '';
      state.wizard.confirm_password = '';
      state.wizard.api_key = '';
      state.wizard.discord_enabled = false;
      state.wizard.discord_token = '';
      state.wizard.discord_user_id = '';
      state.wizard.step = 0;
      setProviderAuthMode('api_key');
      resetProviderValidation();
      setSetupNotice('Reconfigure provider, model, workspace, and security settings.');
      setSetupError('');
      hydrateWizardFields();
      refreshCurrentProviderModels();
    }
    showView('setup');
  }

  function validateCurrentStep() {
    const provider = currentProvider();
    if (!provider) return 'No providers available.';

    if (
      state.wizard.step === 1 &&
      state.wizard.auth_mode === 'api_key' &&
      provider.api_key_required &&
      !state.wizard.api_key.trim() &&
      !hasSavedKeyForSelectedProvider()
    ) {
      return 'Enter an API key to continue.';
    }
    if (
      state.wizard.step === 1 &&
      state.wizard.auth_mode === 'oauth' &&
      state.providerValidation.status !== 'success' &&
      !hasSavedKeyForSelectedProvider()
    ) {
      return 'Sign in to continue.';
    }
    if (state.wizard.step === 3 && !state.wizard.workspace_path.trim()) {
      return 'Choose a workspace folder to continue.';
    }
    if (state.wizard.step === 5 && state.wizard.discord_enabled && !state.wizard.discord_token.trim()) {
      // Discord enabled but no token entered - won't save Discord section, existing config preserved
    }
    if (state.wizard.step === 6 && state.wizard.password_enabled) {
      if (!state.wizard.password) return 'Enter a password or turn password protection off.';
      if (state.wizard.password !== state.wizard.confirm_password) return 'Passwords do not match yet.';
    }
    return '';
  }

  async function getStatus() {
    try {
      const status = await invoke('get_status');
      updateStatus(status.running, status.pid);
      els.pathBinary.textContent = status.osagent_path;
      els.pathConfig.textContent = status.config_path;
    } catch (error) {
      addLog('error', 'Failed to get status: ' + error);
    }
    try {
      const binaryStatus = await invoke('get_binary_status');
      const dbg = binaryStatus.debug_exists;
      const rel = binaryStatus.release_exists;
      let badge = '';
      if (dbg && rel) badge = 'debug & release';
      else if (rel) badge = 'release only';
      else if (dbg) badge = 'debug only';
      else badge = 'not built';
      if (els.setupBinaryStatus) {
        els.setupBinaryStatus.textContent = badge;
      }
    } catch (_e) {}
  }

  async function loadProviderCatalog() {
    try {
      const providers = await invoke('get_setup_provider_catalog');
      state.providers = {};
      state.providerOrder = [];
      providers.forEach((provider) => {
        state.providers[provider.id] = provider;
        state.providerOrder.push(provider.id);
      });
      if (!state.wizard.provider_type && state.providerOrder.length) {
        state.wizard.provider_type = state.providerOrder[0];
      }
      state.wizard.model = '';
      renderProviderSelect();
      await refreshCurrentProviderModels();
    } catch (error) {
      setSetupError('Failed to load provider catalog: ' + error);
    }
  }

  async function loadSetupState() {
    try {
      const setup = await invoke('get_setup_state');
      applySetupState(setup);
    } catch (error) {
      setSetupError('Failed to inspect setup state: ' + error);
    }
  }

  function normalizeEntry(raw) {
    return {
      time:
        raw.time ||
        raw.timestamp ||
        new Date().toLocaleTimeString('en-GB', {
          hour: '2-digit',
          minute: '2-digit',
          second: '2-digit'
        }),
      level: raw.level || 'info',
      message: raw.message || ''
    };
  }

  function updateBuildProgressFromLog(message) {
    if (!message) return;

    const buildingMatch = message.match(/^Building\s+\[[^\]]*\]\s+(\d+)\/(\d+):\s+(.+)$/);
    if (buildingMatch) {
      const current = Number(buildingMatch[1]) || 0;
      const total = Number(buildingMatch[2]) || 0;
      const crateName = buildingMatch[3]
        .replace(/[.\u2026]+$/g, '')
        .replace(/â¦/g, '')
        .trim();
      const pct = total > 0 ? Math.max(2, Math.min(99, Math.round((current / total) * 100))) : 0;

      if (els.buildProgressSection) els.buildProgressSection.classList.remove('hidden');
      if (els.buildProgressLabel) els.buildProgressLabel.textContent = crateName ? 'Building ' + crateName + '...' : 'Building...';
      if (els.buildProgressFill) els.buildProgressFill.style.width = pct + '%';
      if (els.buildProgressStats) els.buildProgressStats.textContent = current + ' / ' + total + ' units';
      return;
    }

    const compilingMatch = message.match(/^Compiling\s+(\S+)/);
    if (compilingMatch) {
      if (els.buildProgressSection) els.buildProgressSection.classList.remove('hidden');
      if (els.buildProgressLabel) els.buildProgressLabel.textContent = 'Compiling ' + compilingMatch[1] + '...';
      if (els.buildProgressFill && (!els.buildProgressFill.style.width || els.buildProgressFill.style.width === '0%')) {
        els.buildProgressFill.style.width = '2%';
      }
      return;
    }

    if (/^Build completed successfully/.test(message)) {
      if (els.buildProgressLabel) els.buildProgressLabel.textContent = 'Build complete';
      if (els.buildProgressFill) els.buildProgressFill.style.width = '100%';
      return;
    }

    if (/^Build failed/.test(message)) {
      if (els.buildProgressLabel) els.buildProgressLabel.textContent = 'Build failed';
      if (els.buildProgressFill) els.buildProgressFill.style.width = '100%';
    }
  }

  function renderLogEntry(entry) {
    updateBuildProgressFromLog(entry.message);
    const div = document.createElement('div');
    div.className = 'log-entry';
    div.innerHTML =
      '<span class="log-time">' +
      escapeHtml(entry.time) +
      '</span>' +
      '<span class="log-level ' +
      escapeHtml(entry.level) +
      '">' +
      escapeHtml(entry.level.toUpperCase()) +
      '</span>' +
      '<span class="log-msg">' +
      escapeHtml(entry.message) +
      '</span>';
    els.logContainer.appendChild(div);
    els.logContainer.scrollTop = els.logContainer.scrollHeight;
  }

  function addLog(level, message) {
    const time = new Date().toLocaleTimeString('en-GB', {
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit'
    });
    const entry = { time, level, message };
    state.logs.push(entry);
    if (state.logs.length > 300) state.logs.shift();
    renderLogEntry(entry);
  }

  async function loadLogs() {
    try {
      state.logs = [];
      els.logContainer.innerHTML = '';
      const existingLogs = await invoke('get_logs');
      existingLogs.forEach((raw) => {
        const entry = normalizeEntry(raw);
        state.logs.push(entry);
        renderLogEntry(entry);
      });
      state.logSyncCount = existingLogs.length;
    } catch (_error) {
      addLog('warn', 'No existing logs found yet');
    }
  }

  async function syncLogsFromBackend() {
    try {
      const allLogs = await invoke('get_logs');

      if (allLogs.length < state.logSyncCount) {
        state.logSyncCount = 0;
      }

      while (state.logSyncCount < allLogs.length) {
        const entry = normalizeEntry(allLogs[state.logSyncCount]);
        state.logs.push(entry);
        if (state.logs.length > 300) state.logs.shift();
        renderLogEntry(entry);
        state.logSyncCount += 1;
      }
    } catch (_error) {}
  }

  function startLogPolling() {
    if (state.logPollInterval) return;
    state.logPollInterval = setInterval(() => {
      syncLogsFromBackend();
    }, 1000);
  }

  async function openWebUi() {
    await invoke('open_web_ui');
  }

  async function startAgent() {
    addLog('info', 'Starting OSA (' + state.runProfile + ')...');
    try {
      const status = await invoke('start_osagent', { profile: state.runProfile });
      updateStatus(status.running, status.pid);
      addLog('info', 'OSA started (PID: ' + status.pid + ')');
      return status;
    } catch (error) {
      addLog('error', 'Start failed: ' + error);
      throw error;
    }
  }

  async function stopAgent() {
    addLog('info', 'Stopping OSA...');
    try {
      await invoke('stop_osagent');
      updateStatus(false, null);
      addLog('info', 'OSA stopped');
    } catch (error) {
      addLog('error', 'Stop failed: ' + error);
    }
  }

  async function restartAgent() {
    addLog('info', 'Restarting OSA...');
    try {
      const status = await invoke('restart_osagent');
      updateStatus(status.running, status.pid);
      addLog('info', 'OSA restarted (PID: ' + status.pid + ')');
    } catch (error) {
      addLog('error', 'Restart failed: ' + error);
    }
  }

  function stopBuildPolling() {
    if (state.buildPollInterval) {
      clearInterval(state.buildPollInterval);
      state.buildPollInterval = null;
    }
    if (state.buildUnlisten) {
      state.buildUnlisten();
      state.buildUnlisten = null;
    }
    els.btnBuild.disabled = false;
  }

  async function flushBuildLogs() {
    const allLogs = await invoke('get_logs');
    while (state.buildPollLogCount < allLogs.length) {
      const entry = normalizeEntry(allLogs[state.buildPollLogCount]);
      state.logs.push(entry);
      if (state.logs.length > 300) state.logs.shift();
      renderLogEntry(entry);
      state.buildPollLogCount += 1;
    }
    if (state.logSyncCount < state.buildPollLogCount) {
      state.logSyncCount = state.buildPollLogCount;
    }
  }

  async function pollBuild() {
    try {
      await flushBuildLogs();
      const building = await invoke('get_build_running');
      if (!building) {
        await flushBuildLogs();
        stopBuildPolling();
      }
    } catch (_error) {
      stopBuildPolling();
    }
  }

  async function saveAndStartFromWizard() {
    const validationError = validateCurrentStep();
    if (validationError) {
      setSetupError(validationError);
      return;
    }

    setSetupError('');
    els.wizardSave.disabled = true;

    if (state.wizard.auth_mode === 'api_key' && !hasFreshProviderValidation()) {
      els.wizardSave.textContent = 'Testing connection...';
      try {
        const result = await invoke('validate_setup_provider', {
          payload: {
            provider_type: state.wizard.provider_type,
            api_key: state.wizard.api_key
          }
        });
        setProviderValidation(result.ok ? 'success' : 'error', result.message);
        if (!result.ok) {
          setSetupError('Provider test failed: ' + result.message);
          els.wizardSave.disabled = false;
          els.wizardSave.textContent = 'Save and Start OSA';
          return;
        }
      } catch (e) {
        setSetupError('Provider test error: ' + String(e));
        els.wizardSave.disabled = false;
        els.wizardSave.textContent = 'Save and Start OSA';
        return;
      }
    }

    els.wizardSave.textContent = 'Saving...';

    try {
      const setup = await invoke('save_setup_config', {
        payload: {
          provider_type: state.wizard.provider_type,
          model: state.wizard.model,
          auth_mode: state.wizard.auth_mode,
          api_key: state.wizard.auth_mode === 'api_key' ? state.wizard.api_key : '',
          workspace_path: state.wizard.workspace_path,
          password_enabled: state.wizard.password_enabled,
          password: state.wizard.password,
          stt_mode: state.wizard.stt_mode,
          stt_whisper_model: state.wizard.stt_whisper_model,
          tts_mode: state.wizard.tts_mode,
          tts_piper_language: state.wizard.tts_piper_language,
          tts_piper_voice: state.wizard.tts_piper_voice,
          discord_enabled: state.wizard.discord_enabled,
          discord_token: state.wizard.discord_token,
          discord_allowed_users: state.wizard.discord_user_id
        }
      });

      applySetupState(setup);
      addLog('info', 'Setup saved. Launching OSA...');

      try {
        await startAgent();
        showView('finish');
        try {
          await openWebUi();
          addLog('info', 'Setup complete. Opened the Web UI in your browser.');
        } catch (_error) {
          addLog('warn', 'OSA started, but the browser did not open automatically.');
        }
      } catch (_error) {
        showView('dashboard');
        addLog('warn', 'Setup finished, but OSA still needs attention before it can run.');
      }
    } catch (error) {
      setSetupError(String(error));
    } finally {
      els.wizardSave.disabled = false;
      els.wizardSave.textContent = 'Save and Start OSA';
    }
  }

  async function testProviderConnection() {
    setSetupError('');
    const provider = currentProvider();
    if (!provider) return;
    if (state.wizard.auth_mode !== 'api_key') {
      setProviderValidation('idle', 'Sign-in mode selected. API key test is skipped.');
      updateReview();
      return;
    }

    if (provider.api_key_required && !state.wizard.api_key.trim() && !hasSavedKeyForSelectedProvider()) {
      setSetupError('Enter an API key or keep an existing saved one before testing.');
      return;
    }

    els.btnTestProvider.disabled = true;
    setProviderValidation('running', 'Testing provider connection...');

    try {
      const result = await invoke('validate_setup_provider', {
        payload: {
          provider_type: state.wizard.provider_type,
          api_key: state.wizard.api_key
        }
      });
      setProviderValidation(result.ok ? 'success' : 'error', result.message);
      updateReview();
    } catch (error) {
      setProviderValidation('error', String(error));
      updateReview();
    } finally {
      els.btnTestProvider.disabled = false;
    }
  }

  // ── Voice helpers ────────────────────────────────────────

  function renderWhisperModelCards() {
    if (!els.sttModelCards) return;
    els.sttModelCards.innerHTML = '';
    WHISPER_MODELS.forEach((m) => {
      const card = document.createElement('div');
      card.className = 'voice-model-card' + (m.id === state.wizard.stt_whisper_model ? ' selected' : '');
      card.innerHTML =
        '<div class="voice-model-card-name">' + escapeHtml(m.name) + '</div>' +
        '<div class="voice-model-card-meta">' + escapeHtml(m.size) + ' — ' + escapeHtml(m.note) + '</div>';
      card.addEventListener('click', () => {
        state.wizard.stt_whisper_model = m.id;
        renderWhisperModelCards();
      });
      els.sttModelCards.appendChild(card);
    });
  }

  function renderPiperVoiceCards() {
    if (!els.ttsVoiceCards) return;
    const lang = state.wizard.tts_piper_language;
    const voices = PIPER_VOICES[lang] || PIPER_VOICES['en'];
    if (!state.wizard.tts_piper_voice || !voices.find((v) => v.id === state.wizard.tts_piper_voice)) {
      state.wizard.tts_piper_voice = voices[0].id;
    }
    els.ttsVoiceCards.innerHTML = '';
    voices.forEach((v) => {
      const card = document.createElement('div');
      card.className = 'voice-model-card' + (v.id === state.wizard.tts_piper_voice ? ' selected' : '');
      card.innerHTML =
        '<div class="voice-model-card-name">' + escapeHtml(v.name) + '</div>' +
        '<div class="voice-model-card-meta">' + escapeHtml(v.quality) + '</div>';
      card.addEventListener('click', () => {
        state.wizard.tts_piper_voice = v.id;
        renderPiperVoiceCards();
      });
      els.ttsVoiceCards.appendChild(card);
    });
  }

  function updateVoiceDownloadSection() {
    const needsLocal = state.wizard.stt_mode === 'local' || state.wizard.tts_mode === 'local';
    if (els.voiceDownloadSection) {
      els.voiceDownloadSection.classList.toggle('hidden', !needsLocal);
    }
  }

  async function checkVoiceStatus() {
    try {
      const status = await invoke('check_voice_status');
      const whisperOk = status.whisper_installed;
      const piperOk = status.piper_installed;

      if (els.voiceSttStatus) {
        els.voiceSttStatus.textContent = whisperOk
          ? ('Installed' + (status.whisper_model ? ' (' + status.whisper_model + ')' : ''))
          : 'Not installed';
      }
      if (els.voiceTtsStatus) {
        els.voiceTtsStatus.textContent = piperOk
          ? ('Installed' + (status.piper_voice ? ' (' + status.piper_voice + ')' : ''))
          : 'Not installed';
      }
      const allReady = whisperOk && piperOk;
      if (els.voiceStatusChip) {
        els.voiceStatusChip.textContent = allReady ? 'Voice ready' : 'Not installed';
        els.voiceStatusChip.classList.toggle('ready', allReady);
      }

      // Show install button only when something is missing
      const needsInstall =
        (state.wizard.stt_mode === 'local' && !whisperOk) ||
        (state.wizard.tts_mode === 'local' && !piperOk);
      if (els.btnInstallVoiceDash) {
        els.btnInstallVoiceDash.classList.toggle('hidden', !needsInstall && (whisperOk || piperOk));
      }

      // Update wizard ready check
      const wizardReady =
        (state.wizard.stt_mode !== 'local' || whisperOk) &&
        (state.wizard.tts_mode !== 'local' || piperOk);
      if (els.voiceWizReady) {
        els.voiceWizReady.classList.toggle('hidden', !wizardReady || !(state.wizard.stt_mode === 'local' || state.wizard.tts_mode === 'local'));
      }
    } catch (_e) {
      // Voice check failed — ignore silently
    }
  }

  async function downloadVoice(prefix) {
    const isWizard = prefix === 'wiz';
    const progressWrap = isWizard ? els.voiceWizProgressWrap : els.voiceDashProgressWrap;
    const progressLabel = isWizard ? els.voiceWizProgressLabel : els.voiceDashProgressLabel;
    const progressFill = isWizard ? els.voiceWizProgressFill : els.voiceDashProgressFill;
    const progressText = isWizard ? els.voiceWizProgressText : els.voiceDashProgressText;
    const downloadBtn = isWizard ? els.btnDownloadVoice : els.btnInstallVoiceDash;

    if (downloadBtn) downloadBtn.disabled = true;
    if (progressWrap) progressWrap.classList.remove('hidden');
    if (progressLabel) progressLabel.textContent = 'Starting download...';
    if (progressFill) progressFill.style.width = '0%';
    if (progressText) progressText.textContent = '0%';

    // Listen for progress events
    if (state.voiceUnlisten) {
      state.voiceUnlisten();
      state.voiceUnlisten = null;
    }

    if (typeof listen === 'function') {
      state.voiceUnlisten = await listen('voice-progress', (event) => {
        const p = event.payload;
        if (!p) return;
        const pct = Math.round((p.progress || 0) * 100);
        if (progressLabel) progressLabel.textContent = (p.model_id || '') + ' — ' + (p.stage || '');
        if (progressFill) progressFill.style.width = pct + '%';
        if (progressText) progressText.textContent = pct + '%';

        if (p.stage === 'complete') {
          if (state.voiceUnlisten) { state.voiceUnlisten(); state.voiceUnlisten = null; }
          checkVoiceStatus();
          if (isWizard && els.voiceWizReady) {
            els.voiceWizReady.classList.remove('hidden');
          }
          if (progressWrap) setTimeout(() => progressWrap.classList.add('hidden'), 2000);
        }
      });
    }

    try {
      await invoke('install_voice', {
        payload: {
          install_whisper: state.wizard.stt_mode === 'local',
          whisper_model: state.wizard.stt_whisper_model || 'base',
          install_piper: state.wizard.tts_mode === 'local',
          piper_voice: state.wizard.tts_piper_voice || 'en_US-libritts-high'
        }
      });
      await checkVoiceStatus();
    } catch (e) {
      if (progressLabel) progressLabel.textContent = 'Error: ' + String(e);
    } finally {
      if (downloadBtn) downloadBtn.disabled = false;
    }
  }

  function bindVoiceWizardEvents() {
    if (els.sttModeToggle) {
      els.sttModeToggle.querySelectorAll('.voice-mode-btn').forEach((btn) => {
        btn.addEventListener('click', () => {
          const mode = btn.dataset.mode;
          state.wizard.stt_mode = mode;
          els.sttModeToggle.querySelectorAll('.voice-mode-btn').forEach((b) =>
            b.classList.toggle('active', b.dataset.mode === mode));
          if (els.sttModelCards) els.sttModelCards.classList.toggle('hidden', mode !== 'local');
          if (mode === 'local') renderWhisperModelCards();
          updateVoiceDownloadSection();
        });
      });
    }

    if (els.ttsModeToggle) {
      els.ttsModeToggle.querySelectorAll('.voice-mode-btn').forEach((btn) => {
        btn.addEventListener('click', () => {
          const mode = btn.dataset.mode;
          state.wizard.tts_mode = mode;
          els.ttsModeToggle.querySelectorAll('.voice-mode-btn').forEach((b) =>
            b.classList.toggle('active', b.dataset.mode === mode));
          if (els.ttsLocalOptions) els.ttsLocalOptions.classList.toggle('hidden', mode !== 'local');
          if (mode === 'local') renderPiperVoiceCards();
          updateVoiceDownloadSection();
        });
      });
    }

    if (els.ttsLangSelect) {
      els.ttsLangSelect.addEventListener('change', (e) => {
        state.wizard.tts_piper_language = e.target.value;
        state.wizard.tts_piper_voice = '';
        renderPiperVoiceCards();
      });
    }

    if (els.btnDownloadVoice) {
      els.btnDownloadVoice.addEventListener('click', () => downloadVoice('wiz'));
    }

    // Discord collapsible toggle
    if (els.discordGuideToggle) {
      els.discordGuideToggle.addEventListener('click', () => {
        els.discordGuideToggle.classList.toggle('active');
        els.discordGuideContent.classList.toggle('open');
      });
    }

    // Open Discord developer portal
    if (els.btnOpenDiscordDev) {
      els.btnOpenDiscordDev.addEventListener('click', async () => {
        try {
          if (window.__TAURI__ && window.__TAURI__.shell) {
            await window.__TAURI__.shell.open('https://discord.com/developers');
          } else {
            window.open('https://discord.com/developers', '_blank');
          }
        } catch (_e) {}
      });
    }
  }

  // ────────────────────────────────────────────────────────

  function bindWizardEvents() {
    els.wizardTabs.forEach((tab) => {
      tab.addEventListener('click', () => {
        const targetStep = Number(tab.dataset.step);
        if (targetStep <= state.wizard.step) {
          setSetupError('');
          state.wizard.step = targetStep;
          updateWizardTabs();
        }
      });
    });

    els.providerSearch.addEventListener('input', (event) => {
      state.providerFilter = event.target.value || '';
      renderProviderSelect();
      resetProviderValidation();
      updateReview();
    });

    els.providerSelect.addEventListener('change', async (event) => {
      state.wizard.provider_type = event.target.value;
      state.modelFilter = '';
      state.wizard.model = '';
      if (els.modelSearch) els.modelSearch.value = '';
      await refreshCurrentProviderModels();
      updateProviderFields();
      resetProviderValidation();
      updateReview();
    });

    if (els.modelSearch) {
      els.modelSearch.addEventListener('input', (event) => {
        state.modelFilter = event.target.value || '';
        renderModelSelect();
      });
    }

    els.providerModel.addEventListener('change', (event) => {
      state.wizard.model = event.target.value;
      if (state.wizard.auth_mode === 'api_key') {
        resetProviderValidation();
      }
      updateReview();
    });

    els.providerAuthApi.addEventListener('click', () => {
      setProviderAuthMode('api_key');
    });

    els.providerAuthSignin.addEventListener('click', () => {
      setProviderAuthMode('oauth');
    });

    els.btnProviderSignIn.addEventListener('click', () => {
      const provider = currentProvider();
      const flowType = provider && provider.oauth ? provider.oauth.flow_type : 'pkce';

      if (flowType === 'device_code') {
        startDeviceCodeFlow();
      } else {
        els.btnProviderSignIn.disabled = true;
        els.btnProviderSignIn.textContent = 'Waiting for browser...';
        invoke('start_setup_oauth', { payload: { provider_type: state.wizard.provider_type } })
          .then(() => {
            els.btnProviderSignIn.disabled = false;
            els.btnProviderSignIn.textContent = 'Sign in with Provider';
            setSetupNotice('Sign-in complete. Pick a model now or leave it for the web UI.');
            setSetupError('');
            setProviderValidation('success', 'Signed in successfully.');
            if (state.wizard.step === 1) {
              state.wizard.step = 2;
              updateWizardTabs();
            }
            updateReview();
          })
          .catch((error) => {
            els.btnProviderSignIn.disabled = false;
            els.btnProviderSignIn.textContent = 'Sign in with Provider';
            setSetupError(String(error));
            setProviderValidation('error', 'Could not start sign-in flow.');
            updateReview();
          });
      }
    });

    function stopDeviceCodePolling() {
      if (state.devicePoll) {
        clearInterval(state.devicePoll.timer);
        state.devicePoll = null;
      }
      els.deviceCodeHint.classList.add('hidden');
      els.providerSigninHint.classList.remove('hidden');
      els.btnProviderSignIn.disabled = false;
      els.btnProviderSignIn.textContent = 'Sign in with Provider';
    }

    function startDeviceCodeFlow() {
      stopDeviceCodePolling();
      els.btnProviderSignIn.disabled = true;
      els.btnProviderSignIn.textContent = 'Starting...';
      setSetupError('');

      invoke('start_device_code_oauth', { payload: { provider_type: state.wizard.provider_type } })
        .then((result) => {
          els.deviceCodeValue.textContent = result.user_code;
          els.deviceCodeUrl.textContent = result.verification_uri.replace(/^https?:\/\//, '');
          els.deviceCodeStatus.textContent = 'Waiting for you to authorize...';
          els.deviceCodeHint.classList.remove('hidden');
          els.providerSigninHint.classList.add('hidden');
          els.btnProviderSignIn.textContent = 'Cancel';
          els.btnProviderSignIn.disabled = false;

          // open the verification URL in the browser
          try { window.__TAURI__.shell.open(result.verification_uri); } catch (_) {}

          const intervalMs = (result.interval || 5) * 1000;
          const pollTimer = setInterval(() => {
            invoke('poll_device_code_oauth', {
              payload: {
                provider_type: state.wizard.provider_type,
                device_code: result.device_code
              }
            })
              .then((pollResult) => {
                if (pollResult.status === 'success') {
                  stopDeviceCodePolling();
                  setProviderValidation('success', 'Signed in successfully.');
                  setSetupNotice('Device sign-in complete. Pick a model now or leave it for the web UI.');
                  if (state.wizard.step === 1) {
                    state.wizard.step = 2;
                    updateWizardTabs();
                  }
                  updateReview();
                } else if (pollResult.status === 'error') {
                  stopDeviceCodePolling();
                  setSetupError('Sign-in failed: ' + pollResult.message);
                  setProviderValidation('error', 'Sign-in failed.');
                  updateReview();
                } else {
                  els.deviceCodeStatus.textContent = pollResult.message || 'Waiting for authorization...';
                }
              })
              .catch((err) => {
                stopDeviceCodePolling();
                setSetupError('Poll error: ' + String(err));
              });
          }, intervalMs);

          state.devicePoll = { timer: pollTimer, device_code: result.device_code };

          // if user clicks cancel, stopDeviceCodePolling is called at the start of next click
        })
        .catch((err) => {
          els.btnProviderSignIn.disabled = false;
          els.btnProviderSignIn.textContent = 'Sign in with Provider';
          setSetupError(String(err));
          setProviderValidation('error', 'Could not start sign-in.');
        });
    }

    els.providerApiKey.addEventListener('input', (event) => {
      state.wizard.api_key = event.target.value;
      resetProviderValidation();
      updateReview();
    });

    els.btnTestProvider.addEventListener('click', testProviderConnection);

    els.workspacePath.addEventListener('input', (event) => {
      state.wizard.workspace_path = event.target.value;
      updateReview();
    });

    els.browseWorkspace.addEventListener('click', async () => {
      try {
        const folder = await invoke('browse_workspace_folder');
        if (folder) {
          state.wizard.workspace_path = folder;
          els.workspacePath.value = folder;
          updateReview();
        }
      } catch (error) {
        setSetupError('Could not open the folder picker: ' + error);
      }
    });

    els.passwordEnabled.addEventListener('change', (event) => {
      state.wizard.password_enabled = event.target.checked;
      updatePasswordFields();
      updateReview();
    });

    els.passwordInput.addEventListener('input', (event) => {
      state.wizard.password = event.target.value;
    });

    els.passwordConfirm.addEventListener('input', (event) => {
      state.wizard.confirm_password = event.target.value;
    });

    els.discordEnabled.addEventListener('change', (event) => {
      state.wizard.discord_enabled = event.target.checked;
      updateReview();
    });

    els.discordToken.addEventListener('input', (event) => {
      state.wizard.discord_token = event.target.value;
      updateReview();
    });

    els.discordUserId.addEventListener('input', (event) => {
      state.wizard.discord_user_id = event.target.value;
    });

    els.wizardBack.addEventListener('click', () => {
      if (state.wizard.step > 0) {
        setSetupError('');
        state.wizard.step -= 1;
        updateWizardTabs();
      }
    });

    els.wizardNext.addEventListener('click', () => {
      const validationError = validateCurrentStep();
      if (validationError) {
        setSetupError(validationError);
        return;
      }

      setSetupError('');
      if (state.wizard.step < els.wizardPanels.length - 1) {
        state.wizard.step += 1;
        updateWizardTabs();
      }
    });

    els.wizardSave.addEventListener('click', saveAndStartFromWizard);
  }

  function bindDashboardEvents() {
    els.btnStart.addEventListener('click', () => startAgent().catch(() => {}));
    els.btnStop.addEventListener('click', stopAgent);
    els.btnRestart.addEventListener('click', restartAgent);

    els.btnOpenUi.addEventListener('click', async () => {
      try {
        await openWebUi();
      } catch (error) {
        addLog('error', 'Failed to open Web UI: ' + error);
      }
    });

    els.btnOpenSetup.addEventListener('click', openSetupWizard);

    if (els.buildProfileToggle) {
      els.buildProfileToggle.querySelectorAll('.profile-btn').forEach((btn) => {
        btn.addEventListener('click', () => {
          const profile = btn.dataset.profile;
          state.buildProfile = profile;
          els.buildProfileToggle.querySelectorAll('.profile-btn').forEach((b) =>
            b.classList.toggle('active', b.dataset.profile === profile));
        });
      });
    }

    if (els.runProfileToggle) {
      els.runProfileToggle.querySelectorAll('.profile-btn').forEach((btn) => {
        btn.addEventListener('click', () => {
          const profile = btn.dataset.profile;
          state.runProfile = profile;
          els.runProfileToggle.querySelectorAll('.profile-btn').forEach((b) =>
            b.classList.toggle('active', b.dataset.profile === profile));
        });
      });
    }

    if (els.btnBuild) {
      els.btnBuild.addEventListener('click', async () => {
        addLog('info', 'Starting build (' + state.buildProfile + ')...');
        els.btnBuild.disabled = true;
        if (els.buildProgressSection) els.buildProgressSection.classList.remove('hidden');
        if (els.buildProgressLabel) els.buildProgressLabel.textContent = 'Preparing...';
        if (els.buildProgressFill) els.buildProgressFill.style.width = '0%';
        if (els.buildProgressStats) els.buildProgressStats.textContent = '';

        try {
          await invoke('build_osagent', { profile: state.buildProfile });
        } catch (error) {
          addLog('error', 'Build failed: ' + error);
          els.btnBuild.disabled = false;
          if (els.buildProgressSection) els.buildProgressSection.classList.add('hidden');
        }
      });
    }

    els.btnFinishOpenUi.addEventListener('click', async () => {
      try {
        await openWebUi();
      } catch (error) {
        addLog('error', 'Failed to open Web UI: ' + error);
      }
    });

    els.btnFinishDashboard.addEventListener('click', () => {
      showView('dashboard');
    });

    if (els.btnInstallVoiceDash) {
      els.btnInstallVoiceDash.addEventListener('click', () => downloadVoice('dash'));
    }

    els.btnClearLog.addEventListener('click', async () => {
      state.logs = [];
      els.logContainer.innerHTML = '';
      try {
        const allLogs = await invoke('get_logs');
        state.logSyncCount = allLogs.length;
      } catch (_error) {
        state.logSyncCount = 0;
      }
    });

    els.btnMinimize.addEventListener('click', async () => {
      try {
        await invoke('minimize_window');
      } catch (_error) {}
    });

    els.btnClose.addEventListener('click', async () => {
      try {
        await invoke('hide_to_tray');
      } catch (_error) {}
    });
  }

  function bindTitlebarDrag() {
    if (!els.titlebar) return;
    const appWindow = tauriWindow?.appWindow ?? tauriWindow?.getCurrentWindow?.() ?? tauri.window?.getCurrentWindow?.();
    if (!appWindow) {
      console.warn('Tauri window API not available for dragging');
      return;
    }
    els.titlebar.addEventListener('mousedown', (event) => {
      if (event.target.closest('.titlebar-controls')) return;
      if (event.target.closest('button')) return;
      appWindow.startDragging().catch(() => {});
    });
  }

  async function bindTauriEvents() {
    if (typeof listen !== 'function') return;

    listen('osagent-status-changed', (event) => {
      const status = event.payload;
      updateStatus(status.running, status.pid);
    });

    listen('log-line', (event) => {
      const entry = normalizeEntry(event.payload);
      state.logs.push(entry);
      if (state.logs.length > 300) state.logs.shift();
      state.logSyncCount += 1;
      renderLogEntry(entry);
    });

    listen('setup-state-changed', (event) => {
      applySetupState(event.payload);
    });

    await listen('build-progress', (event) => {
      const p = event.payload;
      console.log('build-progress event:', p);
      if (!p) return;
      state.buildProgress = p;

      if (p.current_crate) {
        if (els.buildProgressLabel) els.buildProgressLabel.textContent = 'Compiling ' + p.current_crate + '...';
      }
      if (els.buildProgressStats) {
        let stats = p.compiling + ' crates';
        if (p.warnings > 0) stats += ', ' + p.warnings + ' warnings';
        if (p.errors > 0) stats += ', ' + p.errors + ' errors';
        els.buildProgressStats.textContent = stats;
      }
      if (p.finished) {
        if (p.success) {
          if (els.buildProgressLabel) els.buildProgressLabel.textContent = 'Build complete';
          if (els.buildProgressFill) els.buildProgressFill.style.width = '100%';
          addLog('info', 'Build completed successfully (' + p.profile + ')');
        } else {
          if (els.buildProgressLabel) els.buildProgressLabel.textContent = 'Build failed';
          if (els.buildProgressFill) els.buildProgressFill.style.width = '100%';
          addLog('error', 'Build failed (' + p.profile + ')');
        }
        els.btnBuild.disabled = false;
      }
    });
  }

  async function init() {
    bindTitlebarDrag();
    bindWizardEvents();
    bindVoiceWizardEvents();
    bindDashboardEvents();
    await bindTauriEvents();
    await loadProviderCatalog();
    await getStatus();
    await loadSetupState();
    await loadLogs();
    startLogPolling();
  }

  init();
})();

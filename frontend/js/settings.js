window.OSA = window.OSA || {};

OSA.labelThinkingOption = function(value) {
    switch ((value || '').toLowerCase()) {
        case 'auto': return 'Auto';
        case 'off': return 'Off';
        case 'minimal': return 'Minimal';
        case 'low': return 'Low';
        case 'medium': return 'Medium';
        case 'high': return 'High';
        case 'max': return 'Max';
        case 'xhigh': return 'X-High';
        default: return value;
    }
};

OSA.applyThinkingStateToSelect = function(selectId, state, selectedValue) {
    const select = document.getElementById(selectId);
    if (!select) return;

    const options = state?.options || ['auto'];
    select.innerHTML = options.map(function(option) {
        return '<option value="' + OSA.escapeHtml(option) + '">' + OSA.escapeHtml(OSA.labelThinkingOption(option)) + '</option>';
    }).join('');

    const fallback = state?.selected || 'auto';
    select.value = options.includes(selectedValue) ? selectedValue : fallback;
};

OSA.updateThinkingHint = function(state) {
    const hint = document.getElementById('setting-thinking-hint');
    if (!hint) return;

    const options = (state?.options || []).filter(function(option) { return option !== 'auto'; }).map(OSA.labelThinkingOption);
    hint.textContent = options.length
        ? ('Active model: ' + state.provider_id + '/' + state.model + ' - available: ' + options.join(', '))
        : ('Active model: ' + state.provider_id + '/' + state.model + ' - no provider-specific thinking controls exposed');
};

OSA.getActiveThinkingSelection = function() {
    return document.getElementById('header-thinking-level')?.value
        || document.getElementById('setting-thinking-level')?.value
        || OSA.getCachedConfig()?.agent?.thinking_level
        || 'auto';
};

OSA.refreshThinkingOptions = async function(providerId, model, selectedValue) {
    try {
        const params = new URLSearchParams();
        if (providerId) params.set('provider_id', providerId);
        if (model) params.set('model', model);
        const suffix = params.toString() ? ('?' + params.toString()) : '';
        const state = await OSA.getJson('/api/reasoning/options' + suffix);
        const value = selectedValue || state.selected || 'auto';
        OSA.applyThinkingStateToSelect('setting-thinking-level', state, value);
        OSA.applyThinkingStateToSelect('header-thinking-level', state, value);
        OSA.updateThinkingHint(state);
    } catch (error) {
        console.error('Failed to load thinking options:', error);
        OSA.applyThinkingStateToSelect('setting-thinking-level', { options: ['auto'], selected: 'auto' }, 'auto');
        OSA.applyThinkingStateToSelect('header-thinking-level', { options: ['auto'], selected: 'auto' }, 'auto');
    }
};

OSA.persistThinkingLevel = async function(value, providerId, model) {
    const errorDiv = document.getElementById('settings-error');
    if (errorDiv) errorDiv.classList.add('hidden');

    let cfg = OSA.getCachedConfig();
    if (!cfg) {
        cfg = await OSA.getJson('/api/config');
    }

    const next = {
        ...cfg,
        agent: {
            ...(cfg.agent || {}),
            thinking_level: value || 'auto'
        }
    };

    const res = await OSA.fetchWithAuth('/api/config', {
        method: 'PUT',
        body: JSON.stringify(next)
    });
    if (!res.ok) {
        const data = await res.json().catch(() => ({}));
        throw new Error(data.error || `HTTP ${res.status}`);
    }

    OSA.setCachedConfig(next);
    await OSA.refreshThinkingOptions(providerId, model, next.agent.thinking_level);
};

OSA.handleQuickThinkingChange = async function(event) {
    try {
        const providerId = OSA.currentModelProviderId || OSA.getCachedConfig()?.default_provider || '';
        const model = OSA.currentModelId || OSA.getCachedConfig()?.default_model || '';
        await OSA.persistThinkingLevel(event.target.value, providerId, model);
    } catch (error) {
        console.error('Failed to update thinking level:', error);
        alert(error.message || 'Failed to update thinking level');
        await OSA.refreshThinkingOptions();
    }
};

OSA.applyThinkingVisibilitySetting = function(enabled) {
    OSA.setShowThinkingBlocks(enabled);
    const checkbox = document.getElementById('setting-show-thinking-blocks');
    if (checkbox) checkbox.checked = enabled;
    const currentSession = OSA.getCurrentSession();
    if (currentSession && currentSession.id) {
        OSA.selectSession(currentSession.id).catch(error => {
            console.error('Failed to refresh session after thinking visibility change:', error);
        });
    }
};

OSA.onThinkingVisibilityToggleChange = function() {
    const checkbox = document.getElementById('setting-show-thinking-blocks');
    OSA.applyThinkingVisibilitySetting(checkbox ? checkbox.checked : true);
};

OSA.DEFAULT_IDENTITY = "You are OSA, a technical workspace agent optimized for software engineering. Provide precise, actionable assistance for code analysis, debugging, and file operations.";

OSA.DEFAULT_PRIORITIES = "- Answer directly from knowledge when confident\n- Use tools only when uncertain or when current data is required\n- Arithmetic: work step by step, don't rely on memory\n- Keep tool calls minimal and purposeful\n- One tool call is often enough for simple tasks";

OSA.onCustomIdentityToggleChange = function() {
    const checkbox = document.getElementById('setting-use-custom-identity');
    const field = document.getElementById('custom-identity-field');
    const textarea = document.getElementById('setting-custom-identity');
    if (field) {
        const enabled = !!(checkbox && checkbox.checked);
        field.classList.toggle('hidden', !enabled);
        // Populate with default if empty and being enabled
        if (enabled && textarea && !textarea.value.trim()) {
            textarea.value = OSA.DEFAULT_IDENTITY;
        }
    }
};

OSA.onCustomPrioritiesToggleChange = function() {
    const checkbox = document.getElementById('setting-use-custom-priorities');
    const field = document.getElementById('custom-priorities-field');
    const textarea = document.getElementById('setting-custom-priorities');
    if (field) {
        const enabled = !!(checkbox && checkbox.checked);
        field.classList.toggle('hidden', !enabled);
        // Populate with default if empty and being enabled
        if (enabled && textarea && !textarea.value.trim()) {
            textarea.value = OSA.DEFAULT_PRIORITIES;
        }
    }
};

OSA.openSettings = async function() {
    document.getElementById('settings-modal').classList.remove('hidden');
    requestAnimationFrame(function() {
        OSA.loadSettings();
    });
};

OSA.closeSettings = function() {
    document.getElementById('settings-modal').classList.add('hidden');
    document.getElementById('settings-error').classList.add('hidden');
};

OSA.loadSettings = async function() {
    try {
        const res = await OSA.fetchWithAuth('/api/config');
        const config = await res.json();
        if (!res.ok) throw new Error(config.error || `HTTP ${res.status}`);
        OSA.setCachedConfig(config);
        await OSA.loadWorkspaces();
        
        document.getElementById('setting-base-url').value = config.provider?.base_url || '';
        document.getElementById('setting-model').value = config.provider?.model || '';
        const discord = config.discord || {};
        document.getElementById('setting-discord-enabled').value = discord.enabled ? 'true' : 'false';
        document.getElementById('setting-discord-token').value = discord.token || '';
        document.getElementById('setting-discord-allowed-users').value = (discord.allowed_users || []).join('\n');
        document.getElementById('setting-max-tokens').value = config.agent?.max_tokens || 4096;
        document.getElementById('setting-temperature').value = config.agent?.temperature || 0.7;
        document.getElementById('setting-show-thinking-blocks').checked = OSA.getShowThinkingBlocks();
        
        // Load custom prompt sections
        const customIdentity = config.agent?.custom_identity || '';
        const customPriorities = config.agent?.custom_priorities || [];
        document.getElementById('setting-use-custom-identity').checked = !!customIdentity;
        document.getElementById('setting-custom-identity').value = customIdentity;
        document.getElementById('custom-identity-field').classList.toggle('hidden', !customIdentity);
        document.getElementById('setting-use-custom-priorities').checked = customPriorities.length > 0;
        document.getElementById('setting-custom-priorities').value = customPriorities.join('\n');
        document.getElementById('custom-priorities-field').classList.toggle('hidden', customPriorities.length === 0);
        await OSA.refreshThinkingOptions(
            OSA.currentModelProviderId || config.default_provider,
            OSA.currentModelId || config.default_model || config.provider?.model || '',
            config.agent?.thinking_level || 'auto'
        );
        const memEnabled = config.agent?.memory_enabled === true;
        document.getElementById('setting-memory-enabled').checked = memEnabled;
        document.getElementById('setting-memory-file').value = config.agent?.memory_file || '~/.osagent/memories.json';
        document.getElementById('memory-file-field').classList.toggle('hidden', !memEnabled);
        document.getElementById('memory-add-form').classList.toggle('hidden', !memEnabled);
        const decisionMemEnabled = config.agent?.decision_memory_enabled !== false;
        document.getElementById('setting-decision-memory-enabled').checked = decisionMemEnabled;
        document.getElementById('setting-decision-memory-file').value = config.agent?.decision_memory_file || '~/.osagent/decision_memories.json';
        document.getElementById('decision-memory-file-field').classList.toggle('hidden', !decisionMemEnabled);
        
        const voice = OSA.normalizeVoiceConfig(config.voice || {});
        document.getElementById('setting-voice-enabled').checked = !!voice.enabled;
        OSA.setVoiceProviderToggle('stt-provider-toggle', 'setting-stt-provider', voice.stt_provider || 'browser');
        OSA.setVoiceProviderToggle('tts-provider-toggle', 'setting-tts-provider', voice.tts_provider || 'browser');
        document.getElementById('setting-voice-language').value = voice.language || 'en';
        document.getElementById('setting-auto-send').checked = !!voice.auto_send;
        document.getElementById('setting-auto-speak').checked = !!voice.auto_speak;
        document.getElementById('setting-voice-speed').value = voice.voice_speed || 1.0;
        
        document.getElementById('setting-password-enabled').checked = config.server?.password_enabled || false;
        
        const bind = config.server?.bind || '127.0.0.1';
        const port = config.server?.port || 8765;
        const corsAllowedOrigins = Array.isArray(config.server?.cors_allowed_origins)
            ? config.server.cors_allowed_origins
            : [];
        const isLan = bind === '0.0.0.0';
        const isCustom = bind !== '127.0.0.1' && bind !== '0.0.0.0';
        
        document.getElementById('setting-lan-enabled').checked = isLan;
        document.getElementById('setting-port').value = port;
        document.getElementById('setting-cors-allowed-origins').value = corsAllowedOrigins.join('\n');
        
        if (isCustom) {
            document.getElementById('setting-bind').value = bind;
            document.getElementById('custom-network-fields').classList.remove('hidden');
        } else {
            document.getElementById('custom-network-fields').classList.add('hidden');
        }
        
        OSA.updateLanAddressDisplay();
        OSA.updateFirewallWarning();
        document.getElementById('network-restart-notice').classList.add('hidden');
        
        const experimental = config.experimental || {};
        document.getElementById('setting-experimental-workflows').checked = experimental.workflows_enabled || false;
        OSA.updateWorkflowButtonVisibility(experimental.workflows_enabled);
        
        await OSA.loadMemories();
        await OSA.loadVoiceInstallStatus();
        await OSA.loadDiscordBotStatus();
        await OSA.renderSettingsProviders();
        OSA.loadDoctorStatus();
    } catch (error) {
        console.error('Failed to load settings:', error);
    }
};

OSA.saveSettings = async function() {
    const errorDiv = document.getElementById('settings-error');
    errorDiv.classList.add('hidden');
    
    const cachedConfig = OSA.getCachedConfig();
    if (!cachedConfig) {
        errorDiv.textContent = 'No config loaded';
        errorDiv.classList.remove('hidden');
        return;
    }
    
    const newConfig = { ...cachedConfig };
    let allowedDiscordUsers = [];
    
    try {
        allowedDiscordUsers = (document.getElementById('setting-discord-allowed-users').value || '')
            .split(/[\n,]/)
            .map(v => v.trim())
            .filter(Boolean)
            .map(v => {
                if (!/^\d+$/.test(v)) throw new Error(`Invalid Discord user ID: ${v}`);
                return Number(v);
            });
    } catch (error) {
        errorDiv.textContent = error.message;
        errorDiv.classList.remove('hidden');
        return;
    }

    const lanEnabled = document.getElementById('setting-lan-enabled').checked;
    const corsAllowedOrigins = (document.getElementById('setting-cors-allowed-origins').value || '')
        .split(/[\n,]/)
        .map(v => v.trim())
        .filter(Boolean);
    let bindAddr = '127.0.0.1';
    if (lanEnabled) bindAddr = '0.0.0.0';
    else if (document.getElementById('custom-network-fields').classList.contains('hidden') === false) {
        bindAddr = document.getElementById('setting-bind').value || '127.0.0.1';
    }

    newConfig.server = {
        ...newConfig.server,
        bind: bindAddr,
        port: parseInt(document.getElementById('setting-port').value) || 8765,
        password_enabled: document.getElementById('setting-password-enabled').checked,
        cors_allowed_origins: corsAllowedOrigins
    };
    newConfig.provider = {
        ...newConfig.provider,
        base_url: document.getElementById('setting-base-url').value,
        model: document.getElementById('setting-model').value
    };
    newConfig.discord = {
        ...(newConfig.discord || {}),
        enabled: document.getElementById('setting-discord-enabled').value === 'true',
        token: document.getElementById('setting-discord-token').value || '',
        allowed_users: allowedDiscordUsers
    };
    // Process custom priorities: split by newline and filter empty lines
    const customPrioritiesText = document.getElementById('setting-custom-priorities').value || '';
    const customPriorities = customPrioritiesText
        .split('\n')
        .map(line => line.trim())
        .filter(line => line.length > 0);
    
    const useCustomIdentity = document.getElementById('setting-use-custom-identity').checked;
    const customIdentity = useCustomIdentity ? (document.getElementById('setting-custom-identity').value || '').trim() : null;
    
    const useCustomPriorities = document.getElementById('setting-use-custom-priorities').checked;
    
    newConfig.agent = {
        ...newConfig.agent,
        max_tokens: parseInt(document.getElementById('setting-max-tokens').value) || 4096,
        temperature: parseFloat(document.getElementById('setting-temperature').value) || 0.7,
        thinking_level: document.getElementById('setting-thinking-level').value || 'auto',
        memory_enabled: document.getElementById('setting-memory-enabled').checked,
        memory_file: document.getElementById('setting-memory-file').value || '~/.osagent/memories.json',
        decision_memory_enabled: document.getElementById('setting-decision-memory-enabled').checked,
        decision_memory_file: document.getElementById('setting-decision-memory-file').value || '~/.osagent/decision_memories.json',
        custom_identity: customIdentity || null,
        custom_priorities: useCustomPriorities && customPriorities.length > 0 ? customPriorities : null
    };
    const previousVoice = OSA.normalizeVoiceConfig(newConfig.voice || {});
    newConfig.voice = OSA.normalizeVoiceConfig({
        ...previousVoice,
        enabled: document.getElementById('setting-voice-enabled').checked,
        stt_provider: document.getElementById('setting-stt-provider').value,
        tts_provider: document.getElementById('setting-tts-provider').value,
        language: document.getElementById('setting-voice-language').value || 'en',
        auto_send: document.getElementById('setting-auto-send').checked,
        auto_speak: document.getElementById('setting-auto-speak').checked,
        voice_speed: parseFloat(document.getElementById('setting-voice-speed').value) || 1.0,
        whisper_model: previousVoice?.whisper_model || null,
        piper_voice: previousVoice?.piper_voice || null
    });
    newConfig.experimental = {
        workflows_enabled: document.getElementById('setting-experimental-workflows').checked
    };
    
    try {
        const res = await OSA.fetchWithAuth('/api/config', {
            method: 'PUT',
            body: JSON.stringify(newConfig)
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        OSA.setCachedConfig(newConfig);
        OSA.updateWorkflowButtonVisibility(!!newConfig.experimental?.workflows_enabled);
        await OSA.refreshThinkingOptions(undefined, undefined, newConfig.agent.thinking_level);
        OSA.setVoiceConfig(newConfig.voice);
        OSA.updateVoiceButtons();
        OSA.closeSettings();
    } catch (error) {
        errorDiv.textContent = error.message;
        errorDiv.classList.remove('hidden');
    }
};

OSA.setVoiceProviderToggle = function(toggleId, hiddenId, value) {
    const hidden = document.getElementById(hiddenId);
    if (!hidden) return;

    const normalizedValue = hiddenId === 'setting-stt-provider'
        ? OSA.normalizeSttProvider(value)
        : OSA.normalizeTtsProvider(value);

    hidden.value = normalizedValue;

    if (hiddenId === 'setting-stt-provider') {
        const checkbox = document.getElementById('setting-stt-local');
        if (checkbox) checkbox.checked = normalizedValue === 'whisper-local';
    } else if (hiddenId === 'setting-tts-provider') {
        const checkbox = document.getElementById('setting-tts-local');
        if (checkbox) checkbox.checked = normalizedValue === 'piper-local';
    }
};

OSA.bindVoiceProviderToggles = function() {
    const toggleMap = [
        {
            checkboxId: 'setting-stt-local',
            hiddenId: 'setting-stt-provider',
            onValue: 'whisper-local',
            offValue: 'browser'
        },
        {
            checkboxId: 'setting-tts-local',
            hiddenId: 'setting-tts-provider',
            onValue: 'piper-local',
            offValue: 'browser'
        }
    ];

    toggleMap.forEach(function(entry) {
        const checkbox = document.getElementById(entry.checkboxId);
        if (!checkbox || checkbox.dataset.bound === 'true') return;
        checkbox.addEventListener('change', function() {
            OSA.setVoiceProviderToggle(entry.checkboxId, entry.hiddenId, checkbox.checked ? entry.onValue : entry.offValue);
        });
        checkbox.dataset.bound = 'true';
    });
};

OSA.loadVoiceInstallStatus = async function() {
    const statusDiv = document.getElementById('voice-install-status');
    if (!statusDiv) return;
    try {
        const res = await OSA.fetchWithAuth('/api/voice/status');
        const data = await res.json();
        statusDiv.innerHTML = `
            <div class="install-status-grid">
                <div class="install-item">
                    <span class="${data.whisper_installed ? 'installed' : 'not-installed'}">${data.whisper_installed ? '✓' : '○'} Whisper</span>
                    ${data.whisper_model ? `<small>${data.whisper_model}</small>` : ''}
                </div>
                <div class="install-item">
                    <span class="${data.piper_installed ? 'installed' : 'not-installed'}">${data.piper_installed ? '✓' : '○'} Piper TTS</span>
                    ${data.piper_voice ? `<small>${data.piper_voice}</small>` : ''}
                </div>
            </div>
        `;
    } catch (error) {
        statusDiv.innerHTML = '<span class="not-installed">Failed to load status</span>';
    }
};

OSA.installVoiceModels = async function() {
    const btn = document.querySelector('.btn-install');
    const statusDiv = document.getElementById('voice-install-status');
    const sttProvider = document.getElementById('setting-stt-provider').value;
    const ttsProvider = document.getElementById('setting-tts-provider').value;
    const language = document.getElementById('setting-voice-language').value || 'en';
    
    btn.disabled = true;
    btn.textContent = 'Installing...';
    statusDiv.innerHTML = '<span class="not-installed">Downloading models...</span>';
    
    try {
        const res = await OSA.fetchWithAuth('/api/voice/install', {
            method: 'POST',
            body: JSON.stringify({
                install_whisper: sttProvider === 'whisper-local',
                whisper_model: 'base',
                install_piper: ttsProvider === 'piper-local',
                language
            })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadVoiceInstallStatus();
        btn.textContent = 'Install Complete!';
        setTimeout(() => { btn.textContent = 'Install Local Models'; btn.disabled = false; }, 2000);
    } catch (error) {
        statusDiv.innerHTML = `<span class="not-installed">Error: ${error.message}</span>`;
        btn.textContent = 'Install Local Models';
        btn.disabled = false;
    }
};

OSA.switchSettingsTab = async function(tabId) {
    document.querySelectorAll('.settings-sidebar-item').forEach(item => {
        item.classList.toggle('active', item.dataset.tab === tabId);
    });
    document.querySelectorAll('.settings-pane').forEach(pane => {
        pane.classList.toggle('active', pane.id === `pane-${tabId}`);
    });
    const sel = document.getElementById('settings-tab-select');
    if (sel) sel.value = tabId;
    if (tabId === 'models' || tabId === 'provider') {
        const catalogList = document.getElementById('model-catalog-list');
        if (catalogList) {
            catalogList.innerHTML = '<div class="model-empty">Loading...</div>';
        }
        requestAnimationFrame(function() {
            OSA.renderSettingsProviders();
        });
    } else if (tabId === 'voice') {
        const browser = document.getElementById('voice-models-browser');
        if (browser) {
            browser.innerHTML = '<div class="loading-placeholder">Loading models...</div>';
        }
        try {
            await OSA.loadVoiceModels();
            OSA.renderVoiceModelBrowser();
        } catch (error) {
            console.error('Failed to render voice models:', error);
            if (browser) {
                browser.innerHTML = `<div class="model-empty">Failed to load voice models: ${OSA.escapeHtml(error.message || 'Unknown error')}</div>`;
            }
        }
    } else if (tabId === 'doctor') {
        await OSA.loadDoctorStatus();
    } else if (tabId === 'skills') {
        await OSA.loadSkillsUI();
    }
};

OSA.fetchDoctorJson = async function(url) {
    try {
        const res = await OSA.fetchWithAuth(url);
        const data = await res.json().catch(() => ({}));
        if (!res.ok) {
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        return { ok: true, data };
    } catch (error) {
        return { ok: false, error: error.message || 'Unknown error' };
    }
};

OSA.renderDoctorStatus = function(checks) {
    const grid = document.getElementById('doctor-status-grid');
    if (!grid) return;

    if (!Array.isArray(checks) || checks.length === 0) {
        grid.innerHTML = '<div class="doctor-status-empty">No health checks available.</div>';
        return;
    }

    grid.innerHTML = checks.map(function(check) {
        const state = check.state || 'warn';
        const badgeLabel = state === 'ok' ? 'OK' : (state === 'error' ? 'Error' : 'Warn');
        const actions = Array.isArray(check.actions)
            ? check.actions.map(function(action) {
                return `<button type="button" class="btn-ghost btn-ghost-compact doctor-status-action" onclick="${action.onclick}">${OSA.escapeHtml(action.label)}</button>`;
            }).join('')
            : '';

        return `
            <div class="doctor-status-card ${OSA.escapeHtml(state)}">
                <div class="doctor-status-header">
                    <div class="doctor-status-title">${OSA.escapeHtml(check.title || 'Check')}</div>
                    <span class="doctor-status-badge ${OSA.escapeHtml(state)}">${badgeLabel}</span>
                </div>
                <div class="doctor-status-detail">${OSA.escapeHtml(check.detail || '')}</div>
                ${actions ? `<div class="doctor-status-actions">${actions}</div>` : ''}
            </div>
        `;
    }).join('');
};

OSA.loadDoctorStatus = async function() {
    const grid = document.getElementById('doctor-status-grid');
    if (!grid) return;

    grid.innerHTML = '<div class="doctor-status-empty">Running health checks...</div>';

    let config = OSA.getCachedConfig();
    if (!config) {
        const configResult = await OSA.fetchDoctorJson('/api/config');
        if (configResult.ok) {
            config = configResult.data;
            OSA.setCachedConfig(config);
        }
    }

    const [authResult, providersResult, voiceResult, discordResult, updateResult] = await Promise.all([
        OSA.fetchDoctorJson('/api/auth/status'),
        OSA.fetchDoctorJson('/api/providers'),
        OSA.fetchDoctorJson('/api/voice/status'),
        OSA.fetchDoctorJson('/api/discord/status'),
        OSA.fetchDoctorJson('/api/update/status'),
    ]);

    const checks = [];

    if (!authResult.ok) {
        checks.push({
            title: 'Auth',
            state: 'error',
            detail: `Failed to read auth status: ${authResult.error}`,
            actions: [{ label: 'Security', onclick: "switchSettingsTab('security')" }],
        });
    } else {
        const required = !!authResult.data.required;
        checks.push({
            title: 'Auth',
            state: required ? 'ok' : 'warn',
            detail: required
                ? 'Password protection is enabled for the web UI.'
                : 'Password protection is disabled for the web UI.',
            actions: [{ label: 'Security', onclick: "switchSettingsTab('security')" }],
        });
    }

    if (!providersResult.ok) {
        checks.push({
            title: 'Providers',
            state: 'error',
            detail: `Failed to read provider status: ${providersResult.error}`,
            actions: [{ label: 'Models', onclick: "switchSettingsTab('models')" }],
        });
    } else {
        const providers = Array.isArray(providersResult.data.providers) ? providersResult.data.providers : [];
        if (providers.length === 0) {
            checks.push({
                title: 'Providers',
                state: 'warn',
                detail: 'No providers are configured yet.',
                actions: [{ label: 'Connect Provider', onclick: "switchSettingsTab('models')" }],
            });
        } else {
            const active = providers.find(function(provider) { return provider.is_default; }) || providers[0];
            const activeModel = active.model || providersResult.data.default_model || 'provider default';
            checks.push({
                title: 'Providers',
                state: 'ok',
                detail: `${providers.length} configured. Active route: ${active.id} / ${activeModel}.`,
                actions: [{ label: 'Manage Models', onclick: "switchSettingsTab('models')" }],
            });
        }
    }

    if (!voiceResult.ok) {
        checks.push({
            title: 'Voice',
            state: 'error',
            detail: `Failed to read voice status: ${voiceResult.error}`,
            actions: [{ label: 'Voice Settings', onclick: "switchSettingsTab('voice')" }],
        });
    } else {
        const voiceData = voiceResult.data || {};
        const voiceEnabled = !!config?.voice?.enabled;
        const whisperInstalled = !!voiceData.whisper_installed;
        const piperInstalled = !!voiceData.piper_installed;
        const state = (voiceEnabled && (whisperInstalled || piperInstalled)) ? 'ok' : 'warn';
        const detailParts = [];
        detailParts.push(voiceEnabled ? 'Voice features enabled.' : 'Voice features currently disabled.');
        detailParts.push(`Whisper: ${whisperInstalled ? 'installed' : 'missing'}.`);
        detailParts.push(`Piper: ${piperInstalled ? 'installed' : 'missing'}.`);
        checks.push({
            title: 'Voice',
            state,
            detail: detailParts.join(' '),
            actions: [{ label: 'Voice Settings', onclick: "switchSettingsTab('voice')" }],
        });
    }

    if (!discordResult.ok) {
        checks.push({
            title: 'Discord',
            state: 'error',
            detail: `Failed to read Discord status: ${discordResult.error}`,
            actions: [{ label: 'Discord Settings', onclick: "switchSettingsTab('discord')" }],
        });
    } else {
        const discord = discordResult.data || {};
        let state = 'warn';
        let detail = 'Discord integration is disabled.';
        if (!discord.available) {
            detail = 'Discord support is not available in this build.';
        } else if (discord.running) {
            state = 'ok';
            detail = 'Discord bot is running.';
        } else if (discord.enabled && discord.configured) {
            detail = 'Discord bot is configured but not running.';
        } else if (discord.enabled && !discord.configured) {
            state = 'error';
            detail = 'Discord bot is enabled but token is missing.';
        }
        checks.push({
            title: 'Discord',
            state,
            detail,
            actions: [{ label: 'Discord Settings', onclick: "switchSettingsTab('discord')" }],
        });
    }

    if (!updateResult.ok) {
        checks.push({
            title: 'Updates',
            state: 'error',
            detail: `Failed to read update status: ${updateResult.error}`,
            actions: [{ label: 'Updates', onclick: "switchSettingsTab('updates')" }],
        });
    } else {
        const updateData = updateResult.data || {};
        const isReady = updateData.status === 'ready';
        checks.push({
            title: 'Updates',
            state: isReady ? 'warn' : 'ok',
            detail: isReady
                ? `Update ready to install${updateData.version ? ` (v${updateData.version})` : ''}.`
                : 'No downloaded update pending installation.',
            actions: [{ label: 'Updates', onclick: "switchSettingsTab('updates')" }],
        });
    }

    const bind = config?.server?.bind || '127.0.0.1';
    const port = config?.server?.port || 8765;
    const passwordEnabled = !!config?.server?.password_enabled;
    const networkOpen = bind === '0.0.0.0';
    const networkState = networkOpen && !passwordEnabled ? 'error' : (networkOpen ? 'warn' : 'ok');
    const networkDetail = networkOpen
        ? `Listening on all interfaces (${bind}:${port}). ${passwordEnabled ? 'Password is enabled.' : 'Password is disabled.'}`
        : `Listening on local interface (${bind}:${port}).`;
    checks.push({
        title: 'Network',
        state: networkState,
        detail: networkDetail,
        actions: [{ label: 'Security', onclick: "switchSettingsTab('security')" }],
    });

    OSA.renderDoctorStatus(checks);
};

OSA.changePassword = async function() {
    const errorDiv = document.getElementById('password-error');
    const successDiv = document.getElementById('password-success');
    errorDiv.classList.add('hidden');
    successDiv.classList.add('hidden');
    
    const oldPassword = document.getElementById('setting-current-password').value;
    const newPassword = document.getElementById('setting-new-password').value;
    const confirmPassword = document.getElementById('setting-confirm-password').value;
    
    if (!newPassword) {
        errorDiv.textContent = 'New password is required';
        errorDiv.classList.remove('hidden');
        return;
    }
    if (newPassword !== confirmPassword) {
        errorDiv.textContent = 'Passwords do not match';
        errorDiv.classList.remove('hidden');
        return;
    }
    if (newPassword.length < 4) {
        errorDiv.textContent = 'Password must be at least 4 characters';
        errorDiv.classList.remove('hidden');
        return;
    }
    
    try {
        const res = await OSA.fetchWithAuth('/api/auth/password', {
            method: 'POST',
            body: JSON.stringify({ old_password: oldPassword, new_password: newPassword })
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        successDiv.classList.remove('hidden');
        document.getElementById('setting-current-password').value = '';
        document.getElementById('setting-new-password').value = '';
        document.getElementById('setting-confirm-password').value = '';
    } catch (error) {
        errorDiv.textContent = error.message;
        errorDiv.classList.remove('hidden');
    }
};

OSA.onLanToggleChange = function() {
    const lanEnabled = document.getElementById('setting-lan-enabled').checked;
    const customFields = document.getElementById('custom-network-fields');
    
    if (lanEnabled) {
        customFields.classList.add('hidden');
    }
    
    OSA.updateFirewallWarning();
    OSA.updateLanAddressDisplay();
    document.getElementById('network-restart-notice').classList.remove('hidden');
};

OSA.onPasswordToggleChange = function() {
    // Just needs the toggle state which is automatically updated
};

OSA.onPortChange = function() {
    OSA.updateLanAddressDisplay();
    OSA.updateFirewallWarning();
    const lanEnabled = document.getElementById('setting-lan-enabled').checked;
    const customHidden = document.getElementById('custom-network-fields')?.classList.contains('hidden');
    if (lanEnabled || !customHidden) {
        document.getElementById('network-restart-notice').classList.remove('hidden');
    }
};

OSA.onNetworkSettingsChange = function() {
    document.getElementById('network-restart-notice').classList.remove('hidden');
};

OSA.updateLanAddressDisplay = async function() {
    const lanSection = document.getElementById('lan-address-section');
    const lanAddressDisplay = document.getElementById('lan-address-display');
    const lanEnabled = document.getElementById('setting-lan-enabled').checked;
    
    if (lanEnabled) {
        try {
            const netInfo = await OSA.getJson('/api/network');
            if (lanAddressDisplay && netInfo.lan_url) {
                lanAddressDisplay.textContent = netInfo.lan_url;
            }
        } catch (e) {
            const port = document.getElementById('setting-port')?.value || 8765;
            if (lanAddressDisplay) {
                lanAddressDisplay.textContent = `http://<your-lan-ip>:${port}`;
            }
        }
        if (lanSection) {
            lanSection.classList.remove('hidden');
        }
    } else {
        if (lanSection) {
            lanSection.classList.add('hidden');
        }
    }
};

OSA.copyLanAddress = function() {
    const address = document.getElementById('lan-address-display')?.textContent;
    if (address) {
        navigator.clipboard.writeText(address).then(() => {
            const btn = document.querySelector('.btn-copy');
            if (btn) {
                btn.classList.add('copied');
                btn.textContent = 'Copied!';
                setTimeout(() => {
                    btn.classList.remove('copied');
                    btn.textContent = 'Copy';
                }, 2000);
            }
        });
    }
};

OSA.updateFirewallWarning = function() {
    const warning = document.getElementById('firewall-warning');
    if (!warning) return;
    const lanEnabled = document.getElementById('setting-lan-enabled').checked;
    const customHidden = document.getElementById('custom-network-fields')?.classList.contains('hidden');
    const port = document.getElementById('setting-port')?.value || '8765';
    const portSpan = document.getElementById('warning-port');
    if (portSpan) portSpan.textContent = port;
    
    if (lanEnabled || !customHidden) {
        warning.classList.remove('hidden');
    } else {
        warning.classList.add('hidden');
    }
};

OSA.restartServer = async function() {
    const btn = document.getElementById('btn-restart-server');
    const successDiv = document.getElementById('restart-success');
    btn.disabled = true;
    btn.textContent = 'Restarting...';
    successDiv.classList.add('hidden');
    
    try {
        const res = await OSA.fetchWithAuth('/api/admin/restart', {
            method: 'POST'
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        successDiv.classList.remove('hidden');
        setTimeout(() => { location.reload(); }, 3000);
    } catch (error) {
        btn.disabled = false;
        btn.textContent = 'Restart Server';
        alert('Failed to restart: ' + error.message);
    }
};

OSA.renderDiscordBotStatus = function(status, message) {
    const statusEl = document.getElementById('discord-bot-status');
    const messageEl = document.getElementById('discord-bot-message');
    const startBtn = document.getElementById('btn-discord-start');
    const stopBtn = document.getElementById('btn-discord-stop');
    if (!statusEl || !messageEl || !startBtn || !stopBtn) return;

    statusEl.classList.remove('is-running', 'is-stopped', 'is-unavailable');

    if (!status.available) {
        statusEl.textContent = 'Discord support is unavailable in this build';
        statusEl.classList.add('is-unavailable');
        startBtn.disabled = true;
        stopBtn.disabled = true;
    } else if (status.running) {
        statusEl.textContent = 'Bot is running';
        statusEl.classList.add('is-running');
        startBtn.disabled = true;
        stopBtn.disabled = false;
    } else {
        const enabled = status.enabled ? 'enabled' : 'disabled';
        const configured = status.configured ? 'token saved' : 'token missing';
        statusEl.textContent = `Bot is stopped (${enabled}, ${configured})`;
        statusEl.classList.add('is-stopped');
        startBtn.disabled = !status.enabled || !status.configured;
        stopBtn.disabled = true;
    }

    messageEl.textContent = message || '';
};

OSA.loadDiscordBotStatus = async function(message) {
    try {
        const res = await OSA.fetchWithAuth('/api/discord/status');
        const status = await res.json();
        if (!res.ok) throw new Error(status.error || `HTTP ${res.status}`);
        OSA.renderDiscordBotStatus(status, message);
    } catch (error) {
        OSA.renderDiscordBotStatus({ available: false, enabled: false, configured: false, running: false }, error.message);
    }
};

OSA.startDiscordBot = async function() {
    const startBtn = document.getElementById('btn-discord-start');
    const stopBtn = document.getElementById('btn-discord-stop');
    if (!startBtn || !stopBtn) return;

    startBtn.disabled = true;
    stopBtn.disabled = true;

    try {
        const res = await OSA.fetchWithAuth('/api/discord/start', {
            method: 'POST'
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadDiscordBotStatus(data.message || 'Discord bot starting');
    } catch (error) {
        await OSA.loadDiscordBotStatus(error.message);
    }
};

OSA.stopDiscordBot = async function() {
    const startBtn = document.getElementById('btn-discord-start');
    const stopBtn = document.getElementById('btn-discord-stop');
    if (!startBtn || !stopBtn) return;

    startBtn.disabled = true;
    stopBtn.disabled = true;

    try {
        const res = await OSA.fetchWithAuth('/api/discord/stop', {
            method: 'POST'
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadDiscordBotStatus(data.message || 'Discord bot stopped');
    } catch (error) {
        await OSA.loadDiscordBotStatus(error.message);
    }
};

OSA.updateWorkflowButtonVisibility = function(enabled) {
    const btn = document.getElementById('workflow-btn');
    if (btn) {
        if (enabled) {
            btn.classList.remove('hidden');
        } else {
            btn.classList.add('hidden');
        }
    }
};

OSA.refreshWorkflowAvailability = async function() {
    try {
        let config = OSA.getCachedConfig();
        if (!config) {
            const res = await OSA.fetchWithAuth('/api/config');
            if (!res.ok) {
                const data = await res.json().catch(() => ({}));
                throw new Error(data.error || `HTTP ${res.status}`);
            }
            config = await res.json();
            OSA.setCachedConfig(config);
        }

        OSA.updateWorkflowButtonVisibility(!!config?.experimental?.workflows_enabled);
    } catch (error) {
        console.error('Failed to refresh workflow availability:', error);
        OSA.updateWorkflowButtonVisibility(false);
    }
};

OSA.getTheme = function() {
    return localStorage.getItem('osagent-theme') || 'dark';
};

OSA.getChatAlignment = function() {
    return localStorage.getItem('osagent-chat-alignment') || 'split';
};

OSA.setChatAlignment = function(alignment) {
    const normalized = alignment === 'left' ? 'left' : 'split';
    localStorage.setItem('osagent-chat-alignment', normalized);
    OSA.applyChatAlignment(normalized);
    const select = document.getElementById('setting-chat-alignment');
    if (select) select.value = normalized;
};

OSA.applyChatAlignment = function(alignment) {
    document.documentElement.setAttribute('data-chat-alignment', alignment === 'left' ? 'left' : 'split');
};

OSA.setTheme = function(theme) {
    localStorage.setItem('osagent-theme', theme);
    OSA.applyTheme(theme);
    const radio = document.querySelector(`input[name="theme"][value="${theme}"]`);
    if (radio) radio.checked = true;
};

OSA.applyTheme = function(theme) {
    document.documentElement.setAttribute('data-theme', theme);
};

// --- Accent color ---

OSA._hexToRgb = function(hex) {
    const h = hex.replace('#', '');
    const r = parseInt(h.substring(0, 2), 16);
    const g = parseInt(h.substring(2, 4), 16);
    const b = parseInt(h.substring(4, 6), 16);
    return { r, g, b };
};

OSA.getAccent = function() {
    return localStorage.getItem('osagent-accent') || '#7c7cff';
};

OSA.setAccent = function(hex) {
    localStorage.setItem('osagent-accent', hex);
    OSA.applyAccent(hex);
    document.querySelectorAll('.accent-swatch[data-color]').forEach(el => {
        el.classList.toggle('active', el.dataset.color.toLowerCase() === hex.toLowerCase());
    });
    const picker = document.getElementById('accent-color-picker');
    if (picker) picker.value = hex;
};

OSA.applyAccent = function(hex) {
    const { r, g, b } = OSA._hexToRgb(hex);
    // Darken by 35% for gradient end stop (replaces color-mix which doesn't update reactively with var())
    const dr = Math.round(r * 0.65);
    const dg = Math.round(g * 0.65);
    const db = Math.round(b * 0.65);
    const darkHex = '#' + [dr, dg, db].map(v => v.toString(16).padStart(2, '0')).join('');
    const root = document.documentElement;
    root.style.setProperty('--accent', hex);
    root.style.setProperty('--accent-dark', darkHex);
    root.style.setProperty('--accent-dim', `rgba(${r}, ${g}, ${b}, 0.15)`);
    root.style.setProperty('--accent-glow', `rgba(${r}, ${g}, ${b}, 0.35)`);
};

OSA.initTheme = function() {
    const theme = OSA.getTheme();
    OSA.applyTheme(theme);
    const radio = document.querySelector(`input[name="theme"][value="${theme}"]`);
    if (radio) radio.checked = true;

    const chatAlignment = OSA.getChatAlignment();
    OSA.applyChatAlignment(chatAlignment);
    const chatAlignmentSelect = document.getElementById('setting-chat-alignment');
    if (chatAlignmentSelect) chatAlignmentSelect.value = chatAlignment;

    const accent = OSA.getAccent();
    OSA.applyAccent(accent);
    const picker = document.getElementById('accent-color-picker');
    if (picker) picker.value = accent;
    document.querySelectorAll('.accent-swatch[data-color]').forEach(el => {
        el.classList.toggle('active', el.dataset.color.toLowerCase() === accent.toLowerCase());
        el.addEventListener('click', () => OSA.setAccent(el.dataset.color));
    });
};

OSA.onMemoryToggleChange = function() {
    const enabled = document.getElementById('setting-memory-enabled').checked;
    document.getElementById('memory-file-field').classList.toggle('hidden', !enabled);
    document.getElementById('memory-add-form').classList.toggle('hidden', !enabled);
    if (enabled) OSA.loadMemories();
};

OSA.onDecisionMemoryToggleChange = function() {
    const enabled = document.getElementById('setting-decision-memory-enabled').checked;
    document.getElementById('decision-memory-file-field').classList.toggle('hidden', !enabled);
};

OSA.loadMemories = async function() {
    const list = document.getElementById('memory-list');
    if (!list) return;
    try {
        const res = await OSA.fetchWithAuth('/api/memories');
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        if (!data.enabled) {
            list.innerHTML = '<div class="decision-meta">Enable memory to view and manage memories.</div>';
            return;
        }
        if (!data.memories || data.memories.length === 0) {
            list.innerHTML = '<div class="decision-meta">No memories recorded yet. The agent will add memories automatically, or you can add them manually below.</div>';
            return;
        }
        list.innerHTML = data.memories.map(m => {
            const tagStr = m.tags && m.tags.length ? `<span class="decision-meta" style="margin-left:4px">[${OSA.escapeHtml(m.tags.join(', '))}]</span>` : '';
            const sourceLabel = m.source === 'agent' ? 'Recorded by agent' : 'Added by user';
            const encodedTitle = encodeURIComponent(m.title || '').replace(/'/g, '%27');
            const encodedContent = encodeURIComponent(m.content || '').replace(/'/g, '%27');
            const encodedTags = encodeURIComponent((m.tags || []).join(', ')).replace(/'/g, '%27');
            return `
            <div class="decision-item">
                <div class="decision-body">
                    <div class="decision-key">${OSA.escapeHtml(m.title)}${tagStr}</div>
                    <div class="decision-value" style="white-space:pre-wrap">${OSA.escapeHtml(m.content)}</div>
                    <div class="decision-meta">${sourceLabel}</div>
                </div>
                <div style="display:flex;gap:6px;flex-shrink:0">
                    <button type="button" class="btn-ghost" style="font-size:12px" onclick="OSA.openMemoryEdit('${m.id}', '${encodedTitle}', '${encodedContent}', '${encodedTags}')">Edit</button>
                    <button type="button" class="btn-danger" onclick="OSA.deleteMemory('${m.id}')">Delete</button>
                </div>
            </div>`;
        }).join('');
    } catch (error) {
        if (list) list.innerHTML = `<div class="decision-meta">Failed to load memories: ${OSA.escapeHtml(error.message)}</div>`;
    }
};

OSA.addMemory = async function() {
    const title = document.getElementById('memory-title').value.trim();
    const content = document.getElementById('memory-content').value.trim();
    const tagsRaw = document.getElementById('memory-tags').value.trim();
    const tags = tagsRaw ? tagsRaw.split(',').map(t => t.trim()).filter(Boolean) : [];
    if (!title || !content) { alert('Title and content are required.'); return; }
    try {
        const res = await OSA.fetchWithAuth('/api/memories', {
            method: 'POST',
            body: JSON.stringify({ title, content, tags })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        document.getElementById('memory-title').value = '';
        document.getElementById('memory-content').value = '';
        document.getElementById('memory-tags').value = '';
        await OSA.loadMemories();
    } catch (error) {
        alert(`Failed to add memory: ${error.message}`);
    }
};

OSA.deleteMemory = async function(id) {
    if (!confirm('Delete this memory?')) return;
    try {
        const res = await OSA.fetchWithAuth(`/api/memories/${id}`, {
            method: 'DELETE'
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        await OSA.loadMemories();
    } catch (error) {
        alert(`Failed to delete memory: ${error.message}`);
    }
};

OSA.openMemoryEdit = function(id, title, content, tags) {
    const decode = value => {
        try {
            return decodeURIComponent(value || '');
        } catch (_) {
            return value || '';
        }
    };
    document.getElementById('edit-memory-id').value = id;
    document.getElementById('edit-memory-title').value = decode(title);
    document.getElementById('edit-memory-content').value = decode(content);
    document.getElementById('edit-memory-tags').value = decode(tags);
    document.getElementById('memory-edit-modal').classList.remove('hidden');
};

OSA.closeMemoryEdit = function() {
    document.getElementById('memory-edit-modal').classList.add('hidden');
};

OSA.saveMemoryEdit = async function() {
    const id = document.getElementById('edit-memory-id').value;
    const title = document.getElementById('edit-memory-title').value.trim();
    const content = document.getElementById('edit-memory-content').value.trim();
    const tagsRaw = document.getElementById('edit-memory-tags').value.trim();
    const tags = tagsRaw ? tagsRaw.split(',').map(t => t.trim()).filter(Boolean) : [];
    try {
        const res = await OSA.fetchWithAuth(`/api/memories/${id}`, {
            method: 'PUT',
            body: JSON.stringify({ title, content, tags })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        OSA.closeMemoryEdit();
        await OSA.loadMemories();
    } catch (error) {
        alert(`Failed to save memory: ${error.message}`);
    }
};

OSA.pendingUpdateTag = null;
OSA.pendingUpdateVersion = null;
OSA.currentVersion = null;

OSA.checkForUpdates = async function() {
    const btn = document.getElementById('btn-check-update');
    const statusDisplay = document.getElementById('update-status-display');
    const versionRow = document.getElementById('update-version-row');
    const latestVersion = document.getElementById('update-latest-version');
    const channel = document.getElementById('update-channel-select')?.value || 'stable';
    
    btn.disabled = true;
    btn.textContent = 'Checking...';
    statusDisplay.className = 'update-status-display checking';
    statusDisplay.querySelector('.update-status-text').textContent = 'Checking for updates...';
    versionRow.classList.add('hidden');
    document.getElementById('btn-download-update')?.classList.add('hidden');
    document.getElementById('btn-install-update')?.classList.add('hidden');
    document.getElementById('btn-view-release')?.classList.add('hidden');
    document.getElementById('update-release-notes')?.classList.add('hidden');
    
    try {
        const result = await OSA.getJson('/api/update/check?channel=' + encodeURIComponent(channel));
        
        btn.disabled = false;
        btn.textContent = 'Check for Updates';
        
        if (!OSA.currentVersion) {
            OSA.currentVersion = result.current_version;
            document.getElementById('update-current-version').textContent = result.current_version;
        }
        
        if (result.update_available) {
            const latest = result.latest_version || 'unknown';
            latestVersion.textContent = latest;
            versionRow.classList.remove('hidden');
            statusDisplay.className = 'update-status-display update-available';
            statusDisplay.querySelector('.update-status-text').textContent = 'Update available: v' + latest;
            
            OSA.pendingUpdateTag = result.release_url?.split('/tag/')[1] || latest;
            OSA.pendingUpdateVersion = latest;
            
            const downloadBtn = document.getElementById('btn-download-update');
            if (downloadBtn) {
                downloadBtn.classList.remove('hidden');
                downloadBtn.disabled = false;
                downloadBtn.textContent = 'Download Update';
            }
            
            const viewRelease = document.getElementById('btn-view-release');
            if (viewRelease && result.release_url) {
                viewRelease.href = result.release_url;
                viewRelease.classList.remove('hidden');
            }
            
            if (result.release_notes) {
                const notesDiv = document.getElementById('update-release-notes');
                const notesContent = document.getElementById('release-notes-content');
                if (notesDiv && notesContent) {
                    notesContent.textContent = result.release_notes;
                    notesDiv.classList.remove('hidden');
                }
            }
        } else {
            statusDisplay.className = 'update-status-display up-to-date';
            statusDisplay.querySelector('.update-status-text').textContent = 'You are up to date!';
        }
    } catch (error) {
        btn.disabled = false;
        btn.textContent = 'Check for Updates';
        statusDisplay.className = 'update-status-display error';
        statusDisplay.querySelector('.update-status-text').textContent = 'Error checking for updates: ' + (error.message || 'Unknown error');
    }
};

OSA.downloadUpdate = async function() {
    if (!OSA.pendingUpdateTag) {
        alert('No update to download. Please check for updates first.');
        return;
    }
    
    const btn = document.getElementById('btn-download-update');
    const progressContainer = document.getElementById('update-progress-container');
    const progressFill = document.getElementById('update-progress-fill');
    const progressText = document.getElementById('update-progress-text');
    const channel = document.getElementById('update-channel-select')?.value || 'stable';
    
    btn.disabled = true;
    btn.textContent = 'Downloading...';
    progressContainer.classList.remove('hidden');
    progressFill.style.width = '0%';
    progressText.textContent = '0%';
    
    try {
        const response = await OSA.fetchWithAuth('/api/update/download', {
            method: 'POST',
            body: JSON.stringify({ tag: OSA.pendingUpdateTag, channel: channel })
        });
        
        const result = await response.json();
        
        if (!response.ok) {
            throw new Error(result.error || 'Download failed');
        }
        
        progressFill.style.width = '100%';
        progressText.textContent = '100%';
        
        const installBtn = document.getElementById('btn-install-update');
        if (installBtn) {
            installBtn.classList.remove('hidden');
            installBtn.disabled = false;
            installBtn.textContent = 'Install & Restart';
        }
        
        btn.classList.add('hidden');
    } catch (error) {
        btn.disabled = false;
        btn.textContent = 'Download Update';
        const statusDisplay = document.getElementById('update-status-display');
        statusDisplay.className = 'update-status-display error';
        statusDisplay.querySelector('.update-status-text').textContent = 'Download failed: ' + (error.message || 'Unknown error');
    }
};

OSA.installUpdate = async function() {
    if (!OSA.pendingUpdateTag) {
        alert('No update to install. Please download an update first.');
        return;
    }
    
    const btn = document.getElementById('btn-install-update');
    btn.disabled = true;
    btn.textContent = 'Restarting...';
    
    try {
        const response = await OSA.fetchWithAuth('/api/update/install', {
            method: 'POST',
            body: JSON.stringify({ tag: OSA.pendingUpdateTag })
        });
        
        const result = await response.json();
        
        if (!response.ok) {
            throw new Error(result.error || 'Install failed');
        }
        
        const statusDisplay = document.getElementById('update-status-display');
        statusDisplay.className = 'update-status-display checking';
        statusDisplay.querySelector('.update-status-text').textContent = 'Restarting... Please wait.';
        
        setTimeout(function() {
            window.location.reload();
        }, 3000);
    } catch (error) {
        btn.disabled = false;
        btn.textContent = 'Install & Restart';
        const statusDisplay = document.getElementById('update-status-display');
        statusDisplay.className = 'update-status-display error';
        statusDisplay.querySelector('.update-status-text').textContent = 'Install failed: ' + (error.message || 'Unknown error');
    }
};

OSA.loadUpdateStatus = async function() {
    try {
        const result = await OSA.getJson('/api/update/status');
        
        if (result.tag && result.status === 'ready') {
            OSA.pendingUpdateTag = result.tag;
            OSA.pendingUpdateVersion = result.version;
            
            const installBtn = document.getElementById('btn-install-update');
            if (installBtn) {
                installBtn.classList.remove('hidden');
                installBtn.disabled = false;
                installBtn.textContent = 'Install & Restart';
            }
            
            const statusDisplay = document.getElementById('update-status-display');
            statusDisplay.className = 'update-status-display update-available';
            statusDisplay.querySelector('.update-status-text').textContent = 'Update ready: v' + result.version;
            
            const versionRow = document.getElementById('update-version-row');
            const latestVersion = document.getElementById('update-latest-version');
            latestVersion.textContent = result.version;
            versionRow.classList.remove('hidden');
        }
    } catch (error) {
        console.error('Failed to load update status:', error);
    }
};

OSA.initUpdatesPane = function() {
    const versionDisplay = document.getElementById('update-current-version');
    if (versionDisplay && !OSA.currentVersion) {
        OSA.getJson('/api/update/check?channel=stable').then(function(result) {
            OSA.currentVersion = result.current_version;
            versionDisplay.textContent = result.current_version;
        }).catch(function() {
            versionDisplay.textContent = 'Unknown';
        });
    }
};

window.openSettings = OSA.openSettings;
window.closeSettings = OSA.closeSettings;
window.saveSettings = OSA.saveSettings;
window.installVoiceModels = OSA.installVoiceModels;
window.switchSettingsTab = OSA.switchSettingsTab;

document.addEventListener('DOMContentLoaded', OSA.bindVoiceProviderToggles);

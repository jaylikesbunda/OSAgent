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
    // Update status is scoped to the open Updates pane. In particular, do not
    // leave a download poll running after the user closes Settings.
    OSA.stopUpdatePolling?.();
    // The voice-model progress stream is only useful while Settings is open;
    // otherwise it holds an SSE connection for the whole page lifetime.
    if (typeof OSA.stopProgressListener === 'function') {
        OSA.stopProgressListener();
    }
};

OSA.loadSettings = async function() {
    try {
        const res = await OSA.fetchWithAuth('/api/config');
        const config = await res.json();
        if (!res.ok) throw new Error(config.error || `HTTP ${res.status}`);
        OSA.setCachedConfig(config);
        await OSA.loadWorkspaces();

        const discord = config.discord || {};
        document.getElementById('setting-discord-enabled').checked = discord.enabled === true;
        document.getElementById('setting-discord-token').value = discord.token || '';
        document.getElementById('setting-discord-community-mode').checked = discord.community_mode === true;
        document.getElementById('setting-discord-allow-community-members').checked = discord.allow_community_members === true;
        document.getElementById('setting-discord-community-context').value = discord.community_context || '';
        document.getElementById('setting-discord-docs-url').value = discord.docs_url || '';
        document.getElementById('setting-discord-github-repo').value = discord.github_repo || '';
        document.getElementById('setting-discord-github-token').value = discord.github_token || '';
        document.getElementById('setting-discord-github-channel').value = discord.github_tracking_channel || '';
        document.getElementById('setting-discord-allowed-users').value = (discord.allowed_users || []).join('\n');
        document.getElementById('setting-discord-allowed-roles').value = (discord.allowed_roles || []).join('\n');
        document.getElementById('setting-discord-allowed-guilds').value = (discord.allowed_guilds || []).join('\n');
        document.getElementById('setting-discord-allowed-channels').value = (discord.allowed_channels || []).join('\n');
        document.getElementById('setting-discord-allow-dms').checked = discord.allow_dms === true;
        document.getElementById('setting-discord-trusted-users').value = (discord.trusted_users || []).join('\n');
        document.getElementById('setting-discord-trusted-roles').value = (discord.trusted_roles || []).join('\n');
        document.getElementById('setting-discord-trusted-guilds').value = (discord.trusted_guilds || []).join('\n');
        document.getElementById('setting-discord-trusted-channels').value = (discord.trusted_channels || []).join('\n');
        document.getElementById('setting-discord-music-enabled').checked = discord.music_enabled === true;
        document.getElementById('setting-discord-yt-dlp-path').value = discord.yt_dlp_path || '';
        document.getElementById('setting-discord-yt-dlp-extra-args').value = discord.yt_dlp_extra_args || '';
        document.getElementById('setting-discord-music-max-queue').value = discord.music_max_queue || 50;
        document.getElementById('setting-discord-music-max-duration').value = discord.music_max_duration_secs || 0;
        document.getElementById('setting-discord-music-auto-leave').value = discord.music_auto_leave_secs ?? 300;
        document.getElementById('setting-discord-piped-instances').value = (discord.piped_instances || []).join('\n');
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
        document.getElementById('setting-memory-capture-mode').value = config.agent?.memory_capture_mode || 'review';
        document.getElementById('memory-file-field').classList.toggle('hidden', !memEnabled);
        document.getElementById('memory-add-form').classList.toggle('hidden', !memEnabled);
        document.getElementById('memory-suggestions-group').classList.toggle('hidden', !memEnabled);
        document.getElementById('decision-list-group').classList.toggle('hidden', config.agent?.decision_memory_enabled === false);
        document.getElementById('decision-suggestions-group').classList.toggle('hidden', config.agent?.decision_memory_enabled === false);
        const decisionMemEnabled = config.agent?.decision_memory_enabled !== false;
        document.getElementById('setting-decision-memory-enabled').checked = decisionMemEnabled;
        OSA.updateMemoryHeaderStatus();
        document.getElementById('setting-decision-memory-file').value = config.agent?.decision_memory_file || '~/.osagent/decision_memories.json';
        document.getElementById('setting-decision-capture-mode').value = config.agent?.decision_capture_mode || 'review';
        document.getElementById('decision-memory-file-field').classList.toggle('hidden', !decisionMemEnabled);
        
        const voice = OSA.normalizeVoiceConfig(config.voice || {});
        document.getElementById('setting-voice-enabled').checked = !!voice.enabled;
        document.getElementById('setting-stt-provider').value = voice.stt_provider || 'browser';
        document.getElementById('setting-tts-provider').value = voice.tts_provider || 'browser';
        await OSA.populateSettingsDevicePickers();
        document.getElementById('setting-voice-language').value = voice.language || 'en';
        document.getElementById('setting-auto-send').checked = !!voice.auto_send;
        document.getElementById('setting-auto-speak').checked = !!voice.auto_speak;
        document.getElementById('setting-speak-tool-progress').checked = !!voice.speak_tool_progress;
        document.getElementById('setting-silence-auto-stop').checked = voice.silence_auto_stop !== false;
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
        await OSA.loadMemorySuggestions();
        await OSA.loadDecisions();
        await OSA.loadDecisionSuggestions();
        await OSA.loadVoiceInstallStatus();
        await OSA.loadDiscordBotStatus();
        await OSA.renderDiscordActiveModel();
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
    // The active provider route is managed live through /api/model and
    // /api/providers/switch (the model picker, the Discord model card, and
    // /model set all persist immediately). The settings form has no provider
    // fields, so echo the fresh route instead of the cached snapshot — the
    // server ignores these fields on PUT, but this keeps the local cache
    // from going stale when settings are saved after a model switch.
    if (typeof OSA.currentModelId === 'string' && OSA.currentModelId) {
        newConfig.default_model = OSA.currentModelId;
    }
    if (typeof OSA.currentModelProviderId === 'string' && OSA.currentModelProviderId) {
        newConfig.default_provider = OSA.currentModelProviderId;
    }
    let allowedDiscordUsers = [];
    let allowedDiscordRoles = [];
    let allowedDiscordGuilds = [];
    let allowedDiscordChannels = [];
    let trustedDiscordUsers = [];
    let trustedDiscordRoles = [];
    let trustedDiscordGuilds = [];
    let trustedDiscordChannels = [];
    
    try {
        allowedDiscordUsers = (document.getElementById('setting-discord-allowed-users').value || '')
            .split(/[\n,]/)
            .map(v => v.trim())
            .filter(Boolean)
            .map(v => {
                if (!/^\d+$/.test(v)) throw new Error(`Invalid Discord user ID: ${v}`);
                // Keep the id as a string: Discord snowflakes are larger than
                // Number.MAX_SAFE_INTEGER, so Number(v) silently rounds them
                // (420155234833268737 becomes 420155234833268740).
                return v;
            });
        const parseDiscordIds = (id, label) => (document.getElementById(id).value || '')
            .split(/[\n,]/)
            .map(v => v.trim())
            .filter(Boolean)
            .map(v => {
                if (!/^\d+$/.test(v)) throw new Error(`Invalid Discord ${label} ID: ${v}`);
                return v;
            });
        allowedDiscordRoles = parseDiscordIds('setting-discord-allowed-roles', 'role');
        allowedDiscordGuilds = parseDiscordIds('setting-discord-allowed-guilds', 'server');
        allowedDiscordChannels = parseDiscordIds('setting-discord-allowed-channels', 'channel');
        trustedDiscordUsers = parseDiscordIds('setting-discord-trusted-users', 'trusted user');
        trustedDiscordRoles = parseDiscordIds('setting-discord-trusted-roles', 'trusted role');
        trustedDiscordGuilds = parseDiscordIds('setting-discord-trusted-guilds', 'trusted server');
        trustedDiscordChannels = parseDiscordIds('setting-discord-trusted-channels', 'trusted channel');
        const trackingChannel = document.getElementById('setting-discord-github-channel').value.trim();
        if (trackingChannel && !/^\d+$/.test(trackingChannel)) {
            throw new Error(`Invalid Discord announcement channel ID: ${trackingChannel}`);
        }
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
        ...(newConfig.provider || {})
    };
    newConfig.discord = {
        ...(newConfig.discord || {}),
        enabled: document.getElementById('setting-discord-enabled').checked,
        token: document.getElementById('setting-discord-token').value || '',
        community_mode: document.getElementById('setting-discord-community-mode').checked,
        allow_community_members: document.getElementById('setting-discord-allow-community-members').checked,
        community_context: document.getElementById('setting-discord-community-context').value || '',
        docs_url: document.getElementById('setting-discord-docs-url').value || '',
        github_repo: document.getElementById('setting-discord-github-repo').value.trim(),
        github_token: document.getElementById('setting-discord-github-token').value || '',
        github_tracking_channel: document.getElementById('setting-discord-github-channel').value.trim() || null,
        allowed_users: allowedDiscordUsers,
        allowed_roles: allowedDiscordRoles,
        allowed_guilds: allowedDiscordGuilds,
        allowed_channels: allowedDiscordChannels,
        allow_dms: document.getElementById('setting-discord-allow-dms').checked,
        trusted_users: trustedDiscordUsers,
        trusted_roles: trustedDiscordRoles,
        trusted_guilds: trustedDiscordGuilds,
        trusted_channels: trustedDiscordChannels,
        music_enabled: document.getElementById('setting-discord-music-enabled').checked,
        yt_dlp_path: document.getElementById('setting-discord-yt-dlp-path').value || '',
        yt_dlp_extra_args: document.getElementById('setting-discord-yt-dlp-extra-args').value || '',
        music_max_queue: parseInt(document.getElementById('setting-discord-music-max-queue').value) || 50,
        music_max_duration_secs: parseInt(document.getElementById('setting-discord-music-max-duration').value) || 0,
        music_auto_leave_secs: parseInt(document.getElementById('setting-discord-music-auto-leave').value) || 300,
        piped_instances: (document.getElementById('setting-discord-piped-instances').value || '').split(/[\n,]/).map(v=>v.trim()).filter(Boolean)
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
        memory_capture_mode: document.getElementById('setting-memory-capture-mode').value || 'review',
        decision_memory_enabled: document.getElementById('setting-decision-memory-enabled').checked,
        decision_memory_file: document.getElementById('setting-decision-memory-file').value || '~/.osagent/decision_memories.json',
        decision_capture_mode: document.getElementById('setting-decision-capture-mode').value || 'review',
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
        speak_tool_progress: document.getElementById('setting-speak-tool-progress').checked,
        silence_auto_stop: document.getElementById('setting-silence-auto-stop').checked,
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
        OSA.closeSettings();
        // Saving other settings must not move the route: confirm the live
        // trigger still matches what we just kept, then refresh the Discord
        // card from the server.
        if (typeof OSA.loadModel === 'function') {
            await OSA.loadModel().catch(function(error) {
                console.error('Post-save model refresh failed:', error);
            });
        } else if (typeof OSA.renderDiscordActiveModel === 'function') {
            await OSA.renderDiscordActiveModel().catch(function(error) {
                console.error('Post-save model refresh failed:', error);
            });
        }
    } catch (error) {
        errorDiv.textContent = error.message;
        errorDiv.classList.remove('hidden');
        return;
    }

    try {
        OSA.updateWorkflowButtonVisibility(!!newConfig.experimental?.workflows_enabled);
        await OSA.refreshThinkingOptions(undefined, undefined, newConfig.agent.thinking_level);
        OSA.setVoiceConfig(newConfig.voice);
        OSA.updateVoiceButtons();
    } catch (refreshError) {
        console.error('Post-save refresh failed:', refreshError);
    }
};

// Device lists change when hardware is plugged in or mic permission is granted.
// Rebuild the Voice pickers whenever the browser reports a change.
OSA.bindVoiceDeviceListeners = function() {
    if (!navigator.mediaDevices?.addEventListener) return;
    navigator.mediaDevices.addEventListener('devicechange', function() {
        if (document.getElementById('setting-input-device') || document.getElementById('setting-output-device')) {
            OSA.populateSettingsDevicePickers();
        }
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
    if (tabId === 'models') {
        const catalogList = document.getElementById('model-catalog-list');
        if (catalogList) {
            catalogList.innerHTML = '<div class="model-empty">Loading...</div>';
        }
        OSA.renderLocalServers();
        requestAnimationFrame(function() {
            OSA.renderSettingsProviders();
        });
    } else if (tabId === 'voice') {
        const browser = document.getElementById('voice-models-browser');
        if (browser) {
            browser.innerHTML = '<div class="loading-placeholder">Loading models...</div>';
        }
        await OSA.populateSettingsDevicePickers();
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
    } else if (tabId === 'mcp') {
        await OSA.loadMcpUI();
    } else if (tabId === 'updates') {
        // The update API is independent of the settings/config load. Make
        // sure the pane initializes every time it is selected, including a
        // pane selected before the first config request has completed.
        await OSA.initUpdatesPane();
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

// Discord has no per-server model: the bot answers with the same active
// provider route as the desktop app, so the existing model picker in the
// Models tab is the only switch. This card mirrors that route inside the
// Discord tab instead of duplicating the picker.
OSA.renderDiscordActiveModel = async function() {
    const el = document.getElementById('discord-active-model');
    if (!el) return;

    const showRoute = function(providerId, model) {
        el.innerHTML = '<div><div class="provider-route-kicker">Active route (shared with Discord)</div>' +
            '<div class="provider-route-title">' + OSA.escapeHtml(providerId || 'none') + '</div>' +
            '<div class="provider-route-meta">Model: <strong>' + OSA.escapeHtml(model || 'none') + '</strong></div></div>';
    };

    try {
        const res = await OSA.fetchWithAuth('/api/model');
        const data = await res.json().catch(function() { return {}; });
        if (!res.ok) throw new Error(data.error || ('HTTP ' + res.status));
        const cached = OSA.getCachedConfig ? OSA.getCachedConfig() : null;
        if (cached) {
            OSA.setCachedConfig({
                ...cached,
                default_provider: data.provider_id || cached.default_provider,
                default_model: data.model || cached.default_model
            });
        }
        showRoute(data.provider_id, data.model);
    } catch (error) {
        const cached = OSA.getCachedConfig ? OSA.getCachedConfig() : null;
        if (cached && (cached.default_provider || cached.default_model)) {
            showRoute(cached.default_provider, cached.default_model);
        } else {
            el.innerHTML = '<div><div class="provider-route-title">No active route</div>' +
                '<div class="provider-route-meta">' + OSA.escapeHtml(error.message || 'Could not load the active model.') + '</div></div>';
        }
    }
};

OSA.openDiscordModelPicker = function() {
    if (typeof OSA.switchSettingsTab === 'function') {
        OSA.switchSettingsTab('models');
    } else if (typeof switchSettingsTab === 'function') {
        switchSettingsTab('models');
    }
    setTimeout(function() {
        const search = document.getElementById('model-catalog-search');
        if (search) search.focus();
    }, 100);
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

OSA.exportDiscordConfig = async function() {
    const btn = document.getElementById('btn-discord-export');
    const msgEl = document.getElementById('discord-import-message');
    if (msgEl) msgEl.textContent = '';
    if (btn) btn.disabled = true;
    try {
        const res = await OSA.fetchWithAuth('/api/discord/export');
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
        const url = URL.createObjectURL(blob);
        const a = document.createElement('a');
        a.href = url;
        a.download = `osagent-discord-${new Date().toISOString().slice(0, 10)}.json`;
        document.body.appendChild(a);
        a.click();
        a.remove();
        URL.revokeObjectURL(url);
        if (msgEl) {
            msgEl.textContent = 'Discord config exported — contains bot token, store securely.';
            msgEl.style.color = 'var(--success)';
        }
    } catch (error) {
        if (msgEl) {
            msgEl.textContent = error.message || 'Export failed';
            msgEl.style.color = 'var(--error)';
        }
    } finally {
        if (btn) btn.disabled = false;
        setTimeout(() => { if (msgEl) msgEl.textContent = ''; }, 4000);
    }
};

OSA.triggerDiscordImport = function() {
    const input = document.getElementById('discord-import-file');
    if (input) input.click();
};

OSA.importDiscordConfig = async function(event) {
    const file = event.target.files && event.target.files[0];
    const msgEl = document.getElementById('discord-import-message');
    const btn = document.getElementById('btn-discord-import');
    if (!file) return;
    if (msgEl) msgEl.textContent = '';
    if (btn) btn.disabled = true;
    try {
        const text = await file.text();
        let data;
        try {
            data = JSON.parse(text);
        } catch (e) {
            throw new Error('Invalid JSON file');
        }
        const res = await OSA.fetchWithAuth('/api/discord/import', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(data)
        });
        const result = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(result.error || `HTTP ${res.status}`);
        if (msgEl) {
            msgEl.textContent = result.message || 'Discord config imported — saving and reloading...';
            msgEl.style.color = 'var(--success)';
        }
        // Reload settings to reflect imported values
        await OSA.loadSettings();
        await OSA.loadDiscordBotStatus('Discord config imported');
    } catch (error) {
        if (msgEl) {
            msgEl.textContent = error.message || 'Import failed';
            msgEl.style.color = 'var(--error)';
        }
    } finally {
        if (btn) btn.disabled = false;
        // Reset file input so same file can be selected again
        event.target.value = '';
        setTimeout(() => { if (msgEl) msgEl.textContent = ''; }, 5000);
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

OSA.PALETTE_CHOICES = [
    'charcoal-red',
    'midnight-purple',
    'ocean-blue',
    'daylight',
    'coral-dusk',
    'sea-glass',
    'violet-night',
    'peach-sorbet',
    'coral-cream',
    'sea-mist',
    'violet-lavender',
    'peach-cream',
];

OSA.FONT_PRESETS = {
    'inter-playfair': {
        sans: "'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
        serif: "'Playfair Display', Georgia, 'Times New Roman', serif"
    },
    'manrope-lora': {
        sans: "'Manrope', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
        serif: "'Lora', Georgia, 'Times New Roman', serif"
    },
    'plex-merriweather': {
        sans: "'IBM Plex Sans', -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
        serif: "'Merriweather', Georgia, 'Times New Roman', serif"
    },
    'system-georgia': {
        sans: "-apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif",
        serif: "Georgia, 'Times New Roman', serif"
    }
};

OSA.getPalette = function() {
    const saved = localStorage.getItem('osagent-palette');
    if (saved && OSA.PALETTE_CHOICES.includes(saved)) return saved;

    const legacyTheme = localStorage.getItem('osagent-theme');
    if (legacyTheme === 'light') return 'daylight';
    if (legacyTheme === 'blue') return 'ocean-blue';
    if (legacyTheme === 'dark') return 'midnight-purple';

    return 'charcoal-red';
};

OSA.setPalette = function(palette) {
    const next = OSA.PALETTE_CHOICES.includes(palette) ? palette : 'charcoal-red';
    localStorage.setItem('osagent-palette', next);
    OSA.applyPalette(next);
};

OSA.applyPalette = function(palette) {
    const next = OSA.PALETTE_CHOICES.includes(palette) ? palette : 'charcoal-red';
    document.documentElement.setAttribute('data-palette', next);
    document.querySelectorAll('.palette-swatch[data-palette]').forEach(function(el) {
        el.classList.toggle('active', el.dataset.palette === next);
    });
};

OSA.getFontPreset = function() {
    const saved = localStorage.getItem('osagent-font-preset');
    if (saved && OSA.FONT_PRESETS[saved]) return saved;
    return 'inter-playfair';
};

OSA.setFontPreset = function(preset) {
    const next = OSA.FONT_PRESETS[preset] ? preset : 'inter-playfair';
    localStorage.setItem('osagent-font-preset', next);
    OSA.applyFontPreset(next);
    const select = document.getElementById('setting-font');
    if (select) select.value = next;
};

OSA.applyFontPreset = function(preset) {
    const next = OSA.FONT_PRESETS[preset] ? preset : 'inter-playfair';
    const root = document.documentElement;
    root.setAttribute('data-font', next);
    root.style.setProperty('--font-sans', OSA.FONT_PRESETS[next].sans);
    root.style.setProperty('--font-serif', OSA.FONT_PRESETS[next].serif);
    root.style.setProperty('--sans', 'var(--font-sans)');
    root.style.setProperty('--serif', 'var(--font-serif)');
};

OSA.initTheme = function() {
    const palette = OSA.getPalette();
    OSA.applyPalette(palette);

    const fontPreset = OSA.getFontPreset();
    OSA.applyFontPreset(fontPreset);
    const fontSelect = document.getElementById('setting-font');
    if (fontSelect) fontSelect.value = fontPreset;

    const chatAlignment = OSA.getChatAlignment();
    OSA.applyChatAlignment(chatAlignment);
    const chatAlignmentSelect = document.getElementById('setting-chat-alignment');
    if (chatAlignmentSelect) chatAlignmentSelect.value = chatAlignment;
};

OSA.updateMemoryHeaderStatus = function() {
    const status = document.getElementById('memory-header-status');
    if (!status) return;
    const memoryEnabled = document.getElementById('setting-memory-enabled')?.checked === true;
    const decisionEnabled = document.getElementById('setting-decision-memory-enabled')?.checked === true;
    document.getElementById('memory-control-card')?.classList.toggle('is-disabled', !memoryEnabled);
    document.getElementById('decision-control-card')?.classList.toggle('is-disabled', !decisionEnabled);
    if (memoryEnabled && decisionEnabled) {
        status.textContent = 'Memory active';
        status.className = 'memory-header-status active';
    } else if (memoryEnabled || decisionEnabled) {
        status.textContent = decisionEnabled ? 'Decisions active' : 'Memory active';
        status.className = 'memory-header-status partial';
    } else {
        status.textContent = 'Memory off';
        status.className = 'memory-header-status';
    }
};

OSA.onMemoryToggleChange = function() {
    const enabled = document.getElementById('setting-memory-enabled').checked;
    document.getElementById('memory-file-field').classList.toggle('hidden', !enabled);
    document.getElementById('memory-add-form').classList.toggle('hidden', !enabled);
    document.getElementById('memory-suggestions-group').classList.toggle('hidden', !enabled);
    OSA.updateMemoryHeaderStatus();
    if (enabled) {
        OSA.loadMemories();
        OSA.loadMemorySuggestions();
        OSA.loadDecisionSuggestions();
    }
};

OSA.onDecisionMemoryToggleChange = function() {
    const enabled = document.getElementById('setting-decision-memory-enabled').checked;
    document.getElementById('decision-memory-file-field').classList.toggle('hidden', !enabled);
    document.getElementById('decision-list-group').classList.toggle('hidden', !enabled);
    document.getElementById('decision-suggestions-group').classList.toggle('hidden', !enabled);
    OSA.updateMemoryHeaderStatus();
    if (enabled) {
        OSA.loadDecisions();
        OSA.loadDecisionSuggestions();
    }
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
            const scope = m.scope === 'global' ? 'global' : `workspace:${m.workspace_id || 'current'}`;
            const category = m.category ? `<span class="decision-meta" style="margin-left:4px">${OSA.escapeHtml(m.category)}</span>` : '';
            const scopeLabel = `<span class="decision-meta" style="margin-left:4px">${OSA.escapeHtml(scope)}</span>`;
            const confirmation = m.confirmed === false
                ? '<span class="decision-meta" style="margin-left:4px;color:var(--warning-color,#d97706)">unconfirmed</span>'
                : '';
            const sourceLabel = m.source === 'agent' ? 'Recorded by agent' : 'Added by user';
            const encodedTitle = encodeURIComponent(m.title || '').replace(/'/g, '%27');
            const encodedContent = encodeURIComponent(m.content || '').replace(/'/g, '%27');
            const encodedTags = encodeURIComponent((m.tags || []).join(', ')).replace(/'/g, '%27');
            const encodedScope = m.scope === 'global' ? 'global' : 'workspace';
            return `
            <div class="decision-item">
                <div class="decision-body">
                    <div class="decision-key">${OSA.escapeHtml(m.title)}${tagStr}${category}${scopeLabel}${confirmation}</div>
                    <div class="decision-value" style="white-space:pre-wrap">${OSA.escapeHtml(m.content)}</div>
                    <div class="decision-meta">${sourceLabel}</div>
                </div>
                <div style="display:flex;gap:6px;flex-shrink:0">
                    <button type="button" class="btn-ghost" style="font-size:12px" onclick="OSA.openMemoryEdit('${m.id}', '${encodedTitle}', '${encodedContent}', '${encodedTags}', '${encodedScope}')">Edit</button>
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
    const scope = document.getElementById('memory-scope').value;
    if (!title || !content) { alert('Title and content are required.'); return; }
    try {
        const res = await OSA.fetchWithAuth('/api/memories', {
            method: 'POST',
            body: JSON.stringify({ title, content, tags, scope })
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

OSA.openMemoryEdit = function(id, title, content, tags, scope) {
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
    document.getElementById('edit-memory-scope').value = scope === 'global' ? 'global' : 'workspace';
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
    const scope = document.getElementById('edit-memory-scope').value;
    try {
        const res = await OSA.fetchWithAuth(`/api/memories/${id}`, {
            method: 'PUT',
            body: JSON.stringify({ title, content, tags, scope })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        OSA.closeMemoryEdit();
        await OSA.loadMemories();
    } catch (error) {
        alert(`Failed to save memory: ${error.message}`);
    }
};

OSA.loadMemorySuggestions = async function() {
    const list = document.getElementById('memory-suggestions-list');
    if (!list) return;
    try {
        const res = await OSA.fetchWithAuth('/api/memories/suggestions');
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);

        const statusFilter = (document.getElementById('memory-suggestions-filter')?.value || 'pending').toLowerCase();
        const filtered = (data.suggestions || []).filter(s => statusFilter === 'all' ? true : (s.status || 'pending') === statusFilter);
        if (filtered.length === 0) {
            const label = statusFilter === 'all' ? 'memory suggestions' : `${statusFilter} memory suggestions`;
            list.innerHTML = `<div class="decision-meta">No ${OSA.escapeHtml(label)}.</div>`;
            return;
        }

        list.innerHTML = filtered.map(s => {
            const tags = s.tags && s.tags.length
                ? `<span class="decision-meta" style="margin-left:4px">[${OSA.escapeHtml(s.tags.join(', '))}]</span>`
                : '';
            const rationale = s.rationale
                ? `<div class="decision-meta">Reason: ${OSA.escapeHtml(s.rationale)}</div>`
                : '';
            const statusBadge = `<span class="decision-meta" style="margin-left:6px">${OSA.escapeHtml(s.status || 'pending')}</span>`;
            const resolution = s.resolved_by
                ? `<div class="decision-meta">Resolved by ${OSA.escapeHtml(s.resolved_by)}${s.resolution_note ? `: ${OSA.escapeHtml(s.resolution_note)}` : ''}</div>`
                : '';
            const actions = (s.status || 'pending') === 'pending'
                ? `<div style="display:flex;gap:6px;flex-shrink:0">
                    <button type="button" class="btn-action" onclick="OSA.approveMemorySuggestion('${s.id}')">Approve</button>
                    <button type="button" class="btn-danger" onclick="OSA.rejectMemorySuggestion('${s.id}')">Reject</button>
                </div>`
                : '';
            return `
            <div class="decision-item">
                <div class="decision-body">
                    <div class="decision-key">${OSA.escapeHtml(s.title)}${tags}${statusBadge}</div>
                    <div class="decision-value" style="white-space:pre-wrap">${OSA.escapeHtml(s.content)}</div>
                    ${rationale}
                    ${resolution}
                </div>
                ${actions}
            </div>`;
        }).join('');
    } catch (error) {
        list.innerHTML = `<div class="decision-meta">Failed to load memory suggestions: ${OSA.escapeHtml(error.message)}</div>`;
    }
};

OSA.approveMemorySuggestion = async function(id) {
    try {
        const res = await OSA.fetchWithAuth(`/api/memories/suggestions/${id}/approve`, {
            method: 'POST'
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadMemorySuggestions();
        await OSA.loadMemories();
    } catch (error) {
        alert(`Failed to approve memory suggestion: ${error.message}`);
    }
};

OSA.rejectMemorySuggestion = async function(id) {
    const reason = prompt('Optional rejection reason:') || '';
    try {
        const res = await OSA.fetchWithAuth(`/api/memories/suggestions/${id}/reject`, {
            method: 'POST',
            body: JSON.stringify({ reason })
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadMemorySuggestions();
    } catch (error) {
        alert(`Failed to reject memory suggestion: ${error.message}`);
    }
};

OSA.loadDecisions = async function() {
    const list = document.getElementById('decision-list');
    if (!list) return;
    try {
        const res = await OSA.fetchWithAuth('/api/decisions');
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        if (!data.enabled) {
            list.innerHTML = '<div class="decision-meta">Enable decision memory to view approved decisions.</div>';
            return;
        }
        if (!data.decisions || data.decisions.length === 0) {
            list.innerHTML = '<div class="decision-meta">No approved decisions.</div>';
            return;
        }
        list.innerHTML = data.decisions.map(decision => `
            <div class="decision-item">
                <div class="decision-body">
                    <div class="decision-key">${OSA.escapeHtml(decision.key)}</div>
                    <div class="decision-value">${OSA.escapeHtml(decision.value)}</div>
                </div>
                <button type="button" class="btn-danger" onclick="OSA.deleteDecision('${decision.id}')">Delete</button>
            </div>`).join('');
    } catch (error) {
        list.innerHTML = `<div class="decision-meta">Failed to load decisions: ${OSA.escapeHtml(error.message)}</div>`;
    }
};

OSA.deleteDecision = async function(id) {
    if (!confirm('Delete this approved decision?')) return;
    try {
        const res = await OSA.fetchWithAuth(`/api/decisions/${id}`, { method: 'DELETE' });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadDecisions();
    } catch (error) {
        alert(`Failed to delete decision: ${error.message}`);
    }
};

OSA.loadDecisionSuggestions = async function() {
    const list = document.getElementById('decision-suggestions-list');
    if (!list) return;
    try {
        const res = await OSA.fetchWithAuth('/api/decisions/suggestions');
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);

        const statusFilter = (document.getElementById('decision-suggestions-filter')?.value || 'pending').toLowerCase();
        const filtered = (data.suggestions || []).filter(s => statusFilter === 'all' ? true : (s.status || 'pending') === statusFilter);
        if (filtered.length === 0) {
            const label = statusFilter === 'all' ? 'decision suggestions' : `${statusFilter} decision suggestions`;
            list.innerHTML = `<div class="decision-meta">No ${OSA.escapeHtml(label)}.</div>`;
            return;
        }

        list.innerHTML = filtered.map(s => {
            const rationale = s.rationale
                ? `<div class="decision-meta">Reason: ${OSA.escapeHtml(s.rationale)}</div>`
                : '';
            const statusBadge = `<span class="decision-meta" style="margin-left:6px">${OSA.escapeHtml(s.status || 'pending')}</span>`;
            const resolution = s.resolved_by
                ? `<div class="decision-meta">Resolved by ${OSA.escapeHtml(s.resolved_by)}${s.resolution_note ? `: ${OSA.escapeHtml(s.resolution_note)}` : ''}</div>`
                : '';
            const actions = (s.status || 'pending') === 'pending'
                ? `<div style="display:flex;gap:6px;flex-shrink:0">
                    <button type="button" class="btn-action" onclick="OSA.approveDecisionSuggestion('${s.id}')">Approve</button>
                    <button type="button" class="btn-danger" onclick="OSA.rejectDecisionSuggestion('${s.id}')">Reject</button>
                </div>`
                : '';
            return `
            <div class="decision-item">
                <div class="decision-body">
                    <div class="decision-key">${OSA.escapeHtml(s.key)}${statusBadge}</div>
                    <div class="decision-value">${OSA.escapeHtml(s.value)}</div>
                    ${rationale}
                    ${resolution}
                </div>
                ${actions}
            </div>`;
        }).join('');
    } catch (error) {
        list.innerHTML = `<div class="decision-meta">Failed to load decision suggestions: ${OSA.escapeHtml(error.message)}</div>`;
    }
};

OSA.approveDecisionSuggestion = async function(id) {
    try {
        const res = await OSA.fetchWithAuth(`/api/decisions/suggestions/${id}/approve`, {
            method: 'POST'
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadDecisions();
        await OSA.loadDecisionSuggestions();
    } catch (error) {
        alert(`Failed to approve decision suggestion: ${error.message}`);
    }
};

OSA.rejectDecisionSuggestion = async function(id) {
    const reason = prompt('Optional rejection reason:') || '';
    try {
        const res = await OSA.fetchWithAuth(`/api/decisions/suggestions/${id}/reject`, {
            method: 'POST',
            body: JSON.stringify({ reason })
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadDecisions();
        await OSA.loadDecisionSuggestions();
    } catch (error) {
        alert(`Failed to reject decision suggestion: ${error.message}`);
    }
};

OSA.pendingUpdateTag = null;
OSA.pendingUpdateVersion = null;
OSA.currentVersion = null;
OSA._updatePollGeneration = 0;
OSA._updatePollTimer = null;
OSA._updatePaneInitialized = false;
OSA._updateStartupChecked = false;
OSA._updateStartupToastPending = false;
OSA._updateState = {
    status: 'idle',
    phase: 'idle',
    retryAction: null,
    releaseUrl: '',
    releaseNotes: ''
};

OSA.UPDATE_POLL_INTERVAL = 1000;
OSA.getUpdateState = function() {
    return OSA._updateState;
};

OSA.getUpdateActionButton = function() {
    return document.getElementById('btn-check-update');
};

OSA.getUpdateChannel = function() {
    return document.getElementById('update-channel-select')?.value || 'stable';
};

// Unlike getJson(), update endpoints need to reject an error JSON body even
// when the HTTP status is 200. The server has historically returned both
// shapes, and treating `{ error: ... }` as a successful download/install is
// particularly dangerous because it can make the UI offer a restart that was
// never staged.
OSA.fetchUpdateJson = async function(url, options, fallbackMessage) {
    const response = await OSA.fetchWithAuth(url, options);
    const data = await response.json().catch(() => ({}));
    const ok = response.ok === undefined ? true : response.ok;
    if (!ok || data?.error || data?.status === 'error' || data?.status === 'failed') {
        const error = new Error(data?.error || data?.message || fallbackMessage || `HTTP ${response.status || 'error'}`);
        error.updateData = data;
        throw error;
    }
    return data || {};
};

OSA.labelForUpdateProgress = function(percent, bytesDownloaded, totalBytes) {
    let label = Math.round(percent) + '%';
    const downloaded = Number(bytesDownloaded);
    const total = Number(totalBytes);
    if (Number.isFinite(downloaded) && downloaded >= 0 && Number.isFinite(total) && total > 0) {
        label += ' (' + Math.max(0, Math.round(downloaded)) + ' / ' + Math.round(total) + ' bytes)';
    }
    return label;
};

OSA.setUpdateProgress = function(progress, bytesDownloaded, totalBytes, message) {
    const container = document.getElementById('update-progress-container');
    const fill = document.getElementById('update-progress-fill');
    const text = document.getElementById('update-progress-text');
    if (!container || !fill || !text) return;

    let percent = null;
    if (Number.isFinite(Number(progress))) {
        const value = Number(progress);
        // Accept both the API's 0..100 form and the common 0..1 form.
        percent = value > 0 && value <= 1 ? value * 100 : value;
        percent = Math.max(0, Math.min(100, percent));
    }

    const downloaded = Number(bytesDownloaded);
    const total = Number(totalBytes);
    const hasBytes = Number.isFinite(downloaded) && downloaded >= 0
        && Number.isFinite(total) && total > 0;
    if (percent === null && hasBytes) {
        percent = Math.max(0, Math.min(100, (downloaded / total) * 100));
    }
    if (percent === null) percent = 0;

    fill.style.width = percent + '%';
    container.setAttribute('aria-valuenow', String(Math.round(percent)));
    container.setAttribute('aria-valuetext', message || OSA.labelForUpdateProgress(percent, bytesDownloaded, totalBytes));
    let label = OSA.labelForUpdateProgress(percent, bytesDownloaded, totalBytes);
    if (message) label += ' — ' + message;
    text.textContent = label;
    container.classList.remove('hidden');
    container.setAttribute('aria-busy', 'true');
};

OSA.hideUpdateProgress = function() {
    const container = document.getElementById('update-progress-container');
    if (container) {
        container.classList.add('hidden');
        container.setAttribute('aria-busy', 'false');
        container.setAttribute('aria-valuenow', '0');
    }
};

OSA.setUpdateStatusText = function(message, stateName) {
    const display = document.getElementById('update-status-display');
    if (!display) return;
    display.className = 'update-status-display' + (stateName ? ' ' + stateName : '');
    const text = display.querySelector('.update-status-text');
    if (text) text.textContent = message;
};

OSA.renderUpdateAction = function() {
    const button = OSA.getUpdateActionButton();
    if (!button) return;
    const state = OSA._updateState;
    const busy = state.phase === 'checking' || state.phase === 'downloading' || state.phase === 'installing' || state.phase === 'restarting';
    let label = 'Check for Updates';
    if (state.phase === 'available') label = 'Download Update';
    else if (state.phase === 'ready') label = 'Install & Restart';
    else if (state.phase === 'downloading') label = 'Downloading…';
    else if (state.phase === 'installing') label = 'Installing…';
    else if (state.phase === 'restarting') label = 'Restarting…';
    else if (state.phase === 'error') {
        label = state.retryAction === 'download' ? 'Retry Download'
            : state.retryAction === 'install' ? 'Retry Install'
            : 'Check for Updates';
    }
    button.textContent = label;
    button.disabled = busy || (state.phase === 'ready' && !OSA.pendingUpdateTag);
    button.setAttribute('aria-busy', String(busy));
    button.dataset.updateAction = state.phase === 'error'
        ? (state.retryAction || 'check')
        : state.phase;
};

OSA.getUpdateTag = function(data, version) {
    if (data?.latest_tag || data?.tag) return data.latest_tag || data.tag;
    if (!data?.update_available) return '';
    const safeReleaseUrl = OSA.safeUrl(data.release_url) || OSA._updateState.releaseUrl;
    if (safeReleaseUrl) {
        const match = safeReleaseUrl.match(/\/tag\/([^/?#]+)/);
        if (match) {
            try {
                return decodeURIComponent(match[1]);
            } catch (error) {
                return match[1];
            }
        }
    }
    return version || '';
};

OSA.renderUpdateRelease = function(data) {
    const state = OSA._updateState;
    if (Object.prototype.hasOwnProperty.call(data || {}, 'release_url')) {
        const safeReleaseUrl = OSA.safeUrl(data.release_url);
        state.releaseUrl = safeReleaseUrl;
    }
    const release = document.getElementById('btn-view-release');
    if (release) {
        if (state.releaseUrl) {
            release.href = state.releaseUrl;
            release.classList.remove('hidden');
        } else {
            release.classList.add('hidden');
            release.removeAttribute('href');
        }
    }

    // Release notes are rendered as text, never as HTML. This preserves
    // markdown/code formatting in the browser without allowing server output
    // to inject markup into Settings.
    if (Object.prototype.hasOwnProperty.call(data || {}, 'release_notes')) {
        state.releaseNotes = typeof data.release_notes === 'string' ? data.release_notes : '';
    }
    const notes = document.getElementById('update-release-notes');
    const notesContent = document.getElementById('release-notes-content');
    if (notes && notesContent) {
        notesContent.textContent = state.releaseNotes;
        notes.classList.toggle('hidden', !state.releaseNotes);
    }
};

OSA.renderUpdateStatus = function(data, fallbackPhase) {
    data = data || {};
    const state = OSA._updateState;
    const status = String(data.status || '').toLowerCase();
    const currentVersion = data.current_version || data.currentVersion;
    if (currentVersion) {
        OSA.currentVersion = currentVersion;
        const current = document.getElementById('update-current-version');
        if (current) current.textContent = currentVersion;
    }

    const version = data.latest_version || data.version || (data.tag ? String(data.tag).replace(/^v/, '') : '');
    if (version) {
        OSA.pendingUpdateVersion = version;
        const latest = document.getElementById('update-latest-version');
        const row = document.getElementById('update-version-row');
        if (latest) latest.textContent = version;
        if (row) row.classList.toggle('hidden', !(data.update_available || status === 'ready' || status === 'available' || fallbackPhase === 'ready'));
    }

    OSA.renderUpdateRelease(data);
    if (data.update_available) {
        // Replace, rather than retain, a tag from an older check. Otherwise a
        // second check without a tag could install the previous release.
        OSA.pendingUpdateTag = OSA.getUpdateTag(data, version);
    } else if (status === 'ready') {
        // A ready response is installable only when the server identifies the
        // exact staged tag. Do not guess from a version string.
        OSA.pendingUpdateTag = data.tag || '';
    } else if (data.tag) {
        OSA.pendingUpdateTag = data.tag;
    }

    if (status === 'ready') {
        state.status = 'ready';
        state.phase = 'ready';
        state.retryAction = null;
        OSA.setUpdateStatusText('Update ready: v' + (OSA.pendingUpdateVersion || version || 'unknown'), 'update-available');
        OSA.setUpdateProgress(data.progress === undefined ? 100 : data.progress, data.bytes_downloaded, data.total_bytes, data.message);
    } else if (status === 'checking' || status === 'in_progress') {
        state.status = status;
        state.phase = 'checking';
        state.retryAction = null;
        OSA.setUpdateStatusText(data.message || 'Checking for updates…', 'checking');
        OSA.hideUpdateProgress();
    } else if (status === 'downloading' || status === 'preparing') {
        state.status = status;
        state.phase = 'downloading';
        state.retryAction = null;
        OSA.setUpdateStatusText(data.message || 'Downloading update…', 'checking');
        OSA.setUpdateProgress(data.progress, data.bytes_downloaded, data.total_bytes, data.message);
    } else if (status === 'restarting' || status === 'installing') {
        state.status = status;
        state.phase = 'installing';
        state.retryAction = null;
        OSA.setUpdateStatusText(data.message || 'Restarting… Please wait.', 'checking');
        OSA.hideUpdateProgress();
    } else if (status === 'error' || status === 'failed' || data.error) {
        state.status = 'error';
        state.phase = 'error';
        state.retryAction = OSA.updateRetryAction(data, state.retryAction);
        OSA.setUpdateStatusText(data.error || data.message || 'Update failed.', 'error');
        OSA.hideUpdateProgress();
    } else if (data.update_available === true || status === 'available') {
        state.status = 'available';
        state.phase = 'available';
        state.retryAction = null;
        OSA.setUpdateStatusText('Update available: v' + (version || 'unknown'), 'update-available');
        OSA.hideUpdateProgress();
    } else {
        state.status = status || 'idle';
        state.phase = 'idle';
        state.retryAction = null;
        OSA.setUpdateStatusText('You are up to date!', 'up-to-date');
        OSA.hideUpdateProgress();
    }
    OSA.renderUpdateAction();
    return state;
};

OSA.updateRetryAction = function(data, fallback) {
    const text = String(data?.message || data?.error || '').toLowerCase();
    if (text.includes('install') || text.includes('launcher') || text.includes('handoff')) return 'install';
    if (text.includes('download') || text.includes('prepar') || text.includes('stage')) return 'download';
    if (data?.tag && data?.launcher_managed === true) return 'install';
    return fallback || 'check';
};

OSA.renderUpdateError = function(error, retryAction) {
    const state = OSA._updateState;
    state.status = 'error';
    state.phase = 'error';
    state.retryAction = OSA.updateRetryAction(error?.updateData, retryAction || state.retryAction || 'check');
    OSA.setUpdateStatusText((retryAction === 'download' ? 'Download failed: ' : retryAction === 'install' ? 'Install failed: ' : 'Error checking for updates: ')
        + (error?.message || 'Unknown error'), 'error');
    OSA.hideUpdateProgress();
    OSA.renderUpdateAction();
    return state;
};

OSA.maybeShowStartupUpdateToast = function(data) {
    const state = OSA._updateState;
    const version = data?.latest_version || data?.version || (data?.tag ? String(data.tag).replace(/^v/, '') : '') || OSA.pendingUpdateVersion;
    if (!version || (state.phase !== 'available' && state.phase !== 'ready')) return false;
    const key = 'osa-update-toast:' + version;
    try {
        if (window.sessionStorage?.getItem(key)) return false;
        window.sessionStorage?.setItem(key, '1');
    } catch (error) {
        // Storage is optional; a toast is still useful when it is unavailable.
    }
    if (typeof OSA.showToast === 'function') {
        OSA.showToast('OSAgent ' + version + ' is ' + (state.phase === 'ready' ? 'ready to install' : 'available') + '.', 'info');
    }
    return true;
};

OSA.stopUpdatePolling = function() {
    OSA._updatePollGeneration = (OSA._updatePollGeneration || 0) + 1;
    if (OSA._updatePollTimer) {
        clearTimeout(OSA._updatePollTimer);
        OSA._updatePollTimer = null;
    }
    if (OSA._updateRestartTimer) {
        clearTimeout(OSA._updateRestartTimer);
        OSA._updateRestartTimer = null;
    }
};

OSA.startUpdatePolling = function() {
    OSA.stopUpdatePolling();
    const generation = OSA._updatePollGeneration;
    const poll = async function() {
        if (generation !== OSA._updatePollGeneration) return;
        try {
            const result = await OSA.fetchUpdateJson('/api/update/status', undefined, 'Failed to read update status');
            if (generation !== OSA._updatePollGeneration) return;
            OSA.renderUpdateStatus(result);
            const status = String(result.status || '').toLowerCase();
            if (OSA._updateStartupToastPending && (status === 'available' || status === 'ready')) {
                OSA.maybeShowStartupUpdateToast(result);
                OSA._updateStartupToastPending = false;
            }
            if (status === 'checking' || status === 'downloading' || status === 'in_progress' || status === 'preparing') {
                OSA._updatePollTimer = setTimeout(poll, OSA.UPDATE_POLL_INTERVAL);
            } else {
                OSA._updatePollTimer = null;
            }
        } catch (error) {
            if (generation !== OSA._updatePollGeneration) return;
            OSA._updatePollTimer = null;
            if (error?.updateData) {
                OSA.renderUpdateStatus(error.updateData);
            } else {
                OSA.renderUpdateError(error, OSA._updateState.phase === 'downloading' ? 'download' : 'check');
            }
        }
    };
    OSA._updatePollTimer = setTimeout(poll, 0);
};

OSA.loadUpdateStatus = async function(options) {
    options = options || {};
    try {
        const result = await OSA.fetchUpdateJson('/api/update/status', undefined, 'Failed to read update status');
        OSA.renderUpdateStatus(result);
        if (!options.skipPoll) {
            const status = String(result.status || '').toLowerCase();
            if (status === 'checking' || status === 'downloading' || status === 'in_progress' || status === 'preparing') {
                OSA.startUpdatePolling();
            }
        }
        if (options.notify) OSA.maybeShowStartupUpdateToast(result);
        return result;
    } catch (error) {
        if (error?.updateData) {
            OSA.renderUpdateStatus(error.updateData);
            return error.updateData;
        }
        console.error('Failed to load update status:', error);
        OSA.renderUpdateError(error, 'check');
        return null;
    }
};

OSA.checkForUpdates = async function(options) {
    options = options || {};
    const channel = options.channel || OSA.getUpdateChannel();
    OSA._updateState.phase = 'checking';
    OSA._updateState.status = 'checking';
    OSA.setUpdateStatusText('Checking for updates…', 'checking');
    OSA.renderUpdateAction();

    try {
        const result = await OSA.fetchUpdateJson('/api/update/check?channel=' + encodeURIComponent(channel), undefined, 'Update check failed');
        OSA.renderUpdateStatus(result);
        if (result.update_available) {
            const safeReleaseUrl = OSA.safeUrl(result.release_url);
            OSA._updateState.releaseUrl = safeReleaseUrl;
            const release = document.getElementById('btn-view-release');
            if (release && safeReleaseUrl) {
                release.href = safeReleaseUrl;
                release.classList.remove('hidden');
            }
            if (options.notify) OSA.maybeShowStartupUpdateToast(result);
        }
        return result;
    } catch (error) {
        OSA.renderUpdateError(error, 'check');
        return null;
    }
};

OSA.downloadUpdate = async function() {
    if (!OSA.pendingUpdateTag) {
        OSA.renderUpdateError(new Error('No update is available to download. Check for updates first.'), 'check');
        return false;
    }
    const tag = OSA.pendingUpdateTag;
    OSA._updateState.phase = 'downloading';
    OSA._updateState.status = 'downloading';
    OSA.renderUpdateAction();
    OSA.setUpdateProgress(0, null, null, 'Starting download…');
    try {
        const result = await OSA.fetchUpdateJson('/api/update/download', {
            method: 'POST',
            body: JSON.stringify({ tag, channel: OSA.getUpdateChannel() })
        }, 'Download failed');
        OSA.renderUpdateStatus(result, 'downloading');
        const status = String(result.status || '').toLowerCase();
        if (status === 'downloading' || status === 'in_progress' || status === 'preparing') {
            OSA.startUpdatePolling();
        }
        return result;
    } catch (error) {
        OSA.renderUpdateError(error, 'download');
        return false;
    }
};

OSA.waitForUpdateRestart = async function(attempt) {
    attempt = Number(attempt) || 0;
    if (attempt >= 90) {
        window.location.reload();
        return;
    }
    try {
        const result = await OSA.fetchUpdateJson('/api/update/status', undefined, 'Update status unavailable during restart');
        const status = String(result.status || '').toLowerCase();
        OSA.renderUpdateStatus(result, status === 'installing' ? 'installing' : undefined);
        if (status === 'installing' || status === 'restarting' || status === 'downloading' || status === 'preparing') {
            OSA._updateRestartTimer = setTimeout(function() {
                OSA._updateRestartTimer = null;
                OSA.waitForUpdateRestart(attempt + 1);
            }, OSA.UPDATE_POLL_INTERVAL || 1000);
            return;
        }
        // The replacement process is answering again. Reloading now restores
        // the UI from its durable post-restart state instead of guessing a
        // fixed three-second delay.
        window.location.reload();
    } catch (error) {
        OSA._updateRestartTimer = setTimeout(function() {
            OSA._updateRestartTimer = null;
            OSA.waitForUpdateRestart(attempt + 1);
        }, OSA.UPDATE_POLL_INTERVAL || 1000);
    }
};

OSA.installUpdate = async function() {
    if (!OSA.pendingUpdateTag) {
        OSA.renderUpdateError(new Error('No prepared update is ready to install.'), 'check');
        return false;
    }
    const tag = OSA.pendingUpdateTag;
    OSA._updateState.phase = 'installing';
    OSA._updateState.status = 'installing';
    OSA.setUpdateStatusText('Installing update…', 'checking');
    OSA.renderUpdateAction();
    try {
        const result = await OSA.fetchUpdateJson('/api/update/install', {
            method: 'POST',
            body: JSON.stringify({ tag })
        }, 'Install failed');
        OSA.renderUpdateStatus(result, 'installing');
        OSA.waitForUpdateRestart(0);
        return result;
    } catch (error) {
        OSA.renderUpdateError(error, 'install');
        return false;
    }
};

OSA.handleUpdateAction = async function() {
    const state = OSA._updateState;
    let action = state.phase;
    if (state.phase === 'error') action = state.retryAction || 'check';
    if (action === 'ready' && !OSA.pendingUpdateTag) action = 'check';
    if (action === 'ready' || action === 'installing' || action === 'restarting') {
        if (action === 'restarting') return;
        return OSA.installUpdate();
    }
    if (action === 'downloading') return;
    if (action === 'available' || action === 'download') return OSA.downloadUpdate();
    return OSA.checkForUpdates();
};

OSA.onUpdateChannelChange = function() {
    if (document.getElementById('pane-updates')?.classList.contains('active')) {
        OSA.checkForUpdates();
    }
};

OSA.initUpdatesPane = async function() {
    OSA._updatePaneInitialized = true;
    const result = await OSA.loadUpdateStatus();
    const status = String(result?.status || 'idle').toLowerCase();
    if (status === 'idle' || status === 'cancelled') {
        await OSA.checkForUpdates();
    }
};

OSA.checkForUpdatesOnStartup = async function() {
    if (OSA._updateStartupChecked) return;
    OSA._updateStartupChecked = true;
    // The backend owns the configured startup/interval policy. Poll an
    // in-flight check instead of racing it with a second client-owned request.
    const result = await OSA.loadUpdateStatus({ notify: true });
    const status = String(result?.status || '').toLowerCase();
    if (status === 'checking' || status === 'downloading' || status === 'preparing') {
        OSA._updateStartupToastPending = true;
    } else if (!status || status === 'idle') {
        // The scheduled task is deliberately detached from router startup.
        // Re-read once after it has had a chance to enter its checking phase.
        setTimeout(function() {
            OSA.loadUpdateStatus({ notify: true }).then(function(delayed) {
                const delayedStatus = String(delayed?.status || '').toLowerCase();
                if (delayedStatus === 'checking' || delayedStatus === 'downloading' || delayedStatus === 'preparing') {
                    OSA._updateStartupToastPending = true;
                }
            });
        }, 1500);
    }
};

window.openSettings = OSA.openSettings;
window.closeSettings = OSA.closeSettings;

window.saveSettings = OSA.saveSettings;
window.installVoiceModels = OSA.installVoiceModels;
window.switchSettingsTab = OSA.switchSettingsTab;

document.addEventListener('DOMContentLoaded', OSA.bindVoiceDeviceListeners);

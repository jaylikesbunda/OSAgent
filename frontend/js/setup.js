window.OSA = window.OSA || {};

// First-run setup wizard.
//
// The launcher used to own this flow. Keeping it in the web UI means one
// place to change, and it works the same whether the user opens the native
// window or a browser. Everything here talks to the public API the Settings
// panes already use, so there is no second configuration path to maintain.

OSA.setupNeeded = async function() {
    try {
        const res = await OSA.fetchWithAuth('/api/setup/state');
        if (res.ok) {
            const data = await res.json();
            return !!data.needs_setup;
        }
    } catch (error) {
        console.debug('Setup state check failed:', error);
    }

    // Older servers do not have /api/setup/state. Fall back to the same
    // signals: at least one real provider entry plus a model and workspace.
    try {
        const res = await OSA.fetchWithAuth('/api/config');
        if (!res.ok) return false;
        const config = await res.json();
        const providers = Array.isArray(config.providers) ? config.providers : [];
        const hasWorkspace = !!(config.agent && String(config.agent.workspace || '').trim());
        return !(providers.length > 0 && config.default_model && hasWorkspace);
    } catch (error) {
        return false;
    }
};

OSA.showSetup = function(options) {
    document.getElementById('setup-view')?.classList.remove('hidden');
    document.getElementById('login-view')?.classList.add('hidden');
    document.getElementById('app-view')?.classList.add('hidden');
    OSA.Setup.mount(options);
};

OSA.openSetupWizard = function() {
    try {
        OSA.closeSettings?.();
    } catch (error) {}
    // Re-running from Settings is optional; first-run setup is not.
    OSA.showSetup({ cancelable: true });
};

OSA.Setup = (function() {
    const state = {
        step: 0,
        providers: [],
        connected: {},
        selected: null,
        models: [],
        selectedModel: '',
        workspace: '',
        authMode: 'key',
        pendingKey: '',
        cancelable: false,
        busy: false,
        oauthPending: false,
        oauthTimer: null,
        deviceTimer: null,
        mounted: false
    };

    const LOCAL_PROVIDERS = { ollama: true, unsloth: true };
    // Keep the first screen focused: the ones most people start with, in the
    // order the Settings provider list already uses.
    const POPULAR_PROVIDERS = {
        openrouter: 0,
        openai: 1,
        anthropic: 2,
        google: 3,
        groq: 4,
        deepseek: 5,
        xai: 6,
        ollama: 7,
        unsloth: 8
    };

    function el(id) {
        return document.getElementById(id);
    }

    function setHidden(node, hidden) {
        if (!node) return;
        node.classList.toggle('hidden', !!hidden);
    }

    function setError(node, message) {
        if (!node) return;
        node.textContent = message || '';
        node.classList.toggle('hidden', !message);
    }

    function providerId(provider) {
        return provider && (provider.id || provider.provider_id || '');
    }

    function providerName(provider) {
        return provider && (provider.name || provider.id || 'Provider');
    }

    function providerInitials(provider) {
        const name = providerName(provider).replace(/[^A-Za-z0-9 ]/g, ' ').trim();
        const words = name.split(/\s+/).filter(Boolean);
        if (words.length >= 2) {
            return (words[0][0] + words[1][0]).toUpperCase();
        }
        return name.slice(0, 3).toUpperCase() || 'AI';
    }

    async function requestJson(url, options) {
        const res = await OSA.fetchWithAuth(url, options || {});
        let data = null;
        try {
            data = await res.json();
        } catch (error) {
            data = null;
        }
        if (!res.ok) {
            throw new Error((data && data.error) || ('Request failed (HTTP ' + res.status + ')'));
        }
        return data || {};
    }

    function postJson(url, body) {
        return requestJson(url, {
            method: 'POST',
            body: JSON.stringify(body || {})
        });
    }

    function setBusy(busy) {
        state.busy = !!busy;
        const connect = el('setup-connect-btn');
        if (connect) {
            connect.disabled = state.busy;
            connect.textContent = state.busy ? 'Connecting...' : connect.dataset.label || 'Connect';
        }
        const oauth = el('setup-oauth-btn');
        if (oauth) oauth.disabled = state.busy;
        const cont = el('setup-continue');
        if (cont) cont.disabled = state.busy || !canContinue();
        updateFooter();
    }

    function canContinue() {
        if (state.step === 0) {
            return !!(state.selected && state.connected[providerId(state.selected)]);
        }
        if (state.step === 1) {
            return !!state.selectedModel;
        }
        if (state.step === 2) {
            return !!state.workspace.trim();
        }
        return false;
    }

    function updateFooter() {
        const back = el('setup-back');
        const cont = el('setup-continue');
        const hint = el('setup-footer-hint');
        if (back) {
            setHidden(back, state.step === 0);
            back.textContent = state.step === 0 ? 'Back' : 'Back';
        }
        if (cont) {
            const labels = ['Continue', 'Continue', 'Finish setup'];
            cont.textContent = state.busy
                ? (state.step === 2 ? 'Finishing...' : 'Working...')
                : labels[state.step] || 'Continue';
            cont.disabled = state.busy || !canContinue();
        }
        if (hint) {
            if (state.step === 0) {
                hint.textContent = state.selected && !state.connected[providerId(state.selected)]
                    ? 'Connect the provider to continue.'
                    : '';
            } else if (state.step === 1) {
                hint.textContent = state.selectedModel ? '' : 'Pick a model to continue.';
            } else {
                hint.textContent = '';
            }
        }
        document.querySelectorAll('[data-step-dot]').forEach(function(dot) {
            const index = Number(dot.dataset.stepDot);
            dot.classList.toggle('active', index === state.step);
            dot.classList.toggle('done', index < state.step);
        });
    }

    function showStep(step) {
        state.step = Math.max(0, Math.min(2, step));
        document.querySelectorAll('[data-step-pane]').forEach(function(pane) {
            pane.classList.toggle('active', Number(pane.dataset.stepPane) === state.step);
        });
        updateFooter();
        if (state.step === 1) {
            const search = el('setup-model-search');
            if (search) {
                search.value = '';
                window.setTimeout(function() { search.focus(); }, 30);
            }
        }
        if (state.step === 2) {
            const input = el('setup-workspace-path');
            if (input) window.setTimeout(function() { input.focus(); }, 30);
        }
    }

    function resetConnectForm() {
        setError(el('setup-connect-error'), '');
        const key = el('setup-api-key');
        if (key) key.value = '';
        const oauthStatus = el('setup-oauth-status');
        if (oauthStatus) oauthStatus.textContent = '';
        setHidden(el('setup-device-box'), true);
        setHidden(el('setup-oauth-cancel'), true);
        setHidden(el('setup-save-anyway'), true);
        state.pendingKey = '';
        cancelOAuth();
    }

    function applyAuthMode(mode) {
        const provider = state.selected;
        if (!provider) return;
        const oauthSupported = !!provider.oauth_supported;
        state.authMode = (mode === 'oauth' && oauthSupported) ? 'oauth' : 'key';

        const keyRow = el('setup-api-key-row');
        const oauthRow = el('setup-oauth-row');
        const connectBtn = el('setup-connect-btn');
        setHidden(keyRow, state.authMode !== 'key');
        setHidden(oauthRow, state.authMode !== 'oauth');
        setHidden(connectBtn, state.authMode !== 'key');
        setHidden(el('setup-use-key'), !(oauthSupported && state.authMode === 'oauth'));
        setHidden(el('setup-use-oauth'), !(oauthSupported && state.authMode === 'key'));

        if (state.authMode === 'oauth') {
            cancelOAuth();
            const statusNode = el('setup-oauth-status');
            if (statusNode) statusNode.textContent = '';
            const oauthBtn = el('setup-oauth-btn');
            if (oauthBtn) oauthBtn.textContent = 'Sign in with ' + providerName(provider);
        } else if (connectBtn) {
            connectBtn.dataset.label = 'Connect';
            connectBtn.textContent = state.busy ? 'Connecting...' : 'Connect';
            connectBtn.disabled = state.busy;
        }
        updateFooter();
    }

    function selectProvider(provider) {
        state.selected = provider;
        resetConnectForm();
        const id = providerId(provider);
        setHidden(el('setup-provider-picker'), true);
        setHidden(el('setup-provider-connect'), false);

        const name = providerName(provider);
        el('setup-connect-name').textContent = name;
        el('setup-connect-desc').textContent = provider.description || '';
        el('setup-connect-icon').textContent = providerInitials(provider);
        el('setup-connect-icon').className = 'setup-provider-icon icon-' + id.replace(/[^a-z0-9_-]/gi, '');

        const connected = state.connected[id];
        const keyRow = el('setup-api-key-row');
        const oauthRow = el('setup-oauth-row');
        const connectBtn = el('setup-connect-btn');
        const keyInput = el('setup-api-key');
        const hint = el('setup-connect-hint');
        const keyLink = el('setup-api-key-link');

        const oauthSupported = !!provider.oauth_supported;
        const local = !!LOCAL_PROVIDERS[id];

        if (connected) {
            state.authMode = 'key';
            setHidden(keyRow, true);
            setHidden(oauthRow, true);
            setHidden(el('setup-use-key'), true);
            setHidden(el('setup-use-oauth'), true);
            setHidden(connectBtn, false);
            connectBtn.dataset.label = 'Continue';
            connectBtn.textContent = 'Continue';
            connectBtn.disabled = false;
            hint.textContent = connected.model
                ? 'Connected. Current model: ' + connected.model
                : 'Connected.';
            state.selectedModel = connected.model || '';
            updateFooter();
        } else {
            hint.textContent = local
                ? 'No key needed for a local server. Leave this blank if it is unauthenticated.'
                : '';
            if (keyInput) {
                keyInput.value = '';
                keyInput.placeholder = local ? 'Optional' : 'Paste your key';
            }
            const keyLinkUrl = provider.api_key_url;
            if (keyLink && keyLinkUrl) {
                keyLink.href = OSA.safeUrl ? (OSA.safeUrl(keyLinkUrl) || '#') : keyLinkUrl;
                setHidden(keyLink, false);
            } else {
                setHidden(keyLink, true);
            }
            applyAuthMode(oauthSupported ? 'oauth' : 'key');
        }

        updateFooter();
        if (keyInput && !connected && !oauthSupported) {
            window.setTimeout(function() { keyInput.focus(); }, 30);
        }
    }

    function backToProviders() {
        cancelOAuth();
        state.selected = null;
        setHidden(el('setup-provider-connect'), true);
        setHidden(el('setup-provider-picker'), false);
        setError(el('setup-connect-error'), '');
        updateFooter();
        const search = el('setup-provider-search');
        if (search) window.setTimeout(function() { search.focus(); }, 30);
    }

    function renderProviders() {
        const list = el('setup-provider-list');
        const empty = el('setup-provider-empty');
        if (!list) return;
        const query = ((el('setup-provider-search') || {}).value || '').trim().toLowerCase();

        const providers = state.providers.filter(function(provider) {
            if (!query) return true;
            const haystack = [
                providerName(provider),
                providerId(provider),
                provider.description || ''
            ].join(' ').toLowerCase();
            return haystack.indexOf(query) !== -1;
        });

        providers.sort(function(a, b) {
            const aId = providerId(a);
            const bId = providerId(b);
            const aConnected = state.connected[aId] ? 0 : 1;
            const bConnected = state.connected[bId] ? 0 : 1;
            if (aConnected !== bConnected) return aConnected - bConnected;

            const aRank = POPULAR_PROVIDERS[aId] === undefined ? 99 : POPULAR_PROVIDERS[aId];
            const bRank = POPULAR_PROVIDERS[bId] === undefined ? 99 : POPULAR_PROVIDERS[bId];
            if (aRank !== bRank) return aRank - bRank;
            return providerName(a).localeCompare(providerName(b));
        });

        const fragment = document.createDocumentFragment();
        providers.forEach(function(provider) {
            const id = providerId(provider);
            const card = document.createElement('button');
            card.type = 'button';
            card.className = 'setup-provider-card';
            card.dataset.providerId = id;

            const icon = document.createElement('div');
            icon.className = 'setup-provider-icon icon-' + id.replace(/[^a-z0-9_-]/gi, '');
            icon.textContent = providerInitials(provider);

            const copy = document.createElement('div');
            copy.className = 'setup-provider-copy';
            const name = document.createElement('div');
            name.className = 'setup-provider-name';
            name.textContent = providerName(provider);
            const desc = document.createElement('div');
            desc.className = 'setup-provider-desc';
            desc.textContent = provider.description || '';
            copy.appendChild(name);
            copy.appendChild(desc);

            card.appendChild(icon);
            card.appendChild(copy);

            if (state.connected[id]) {
                const badge = document.createElement('span');
                badge.className = 'setup-provider-badge';
                badge.textContent = 'Connected';
                card.appendChild(badge);
            }

            card.addEventListener('click', function() {
                if (state.connected[id] && state.connected[id].model) {
                    state.selected = provider;
                    state.selectedModel = state.connected[id].model;
                    loadModels().then(function() { showStep(1); });
                    return;
                }
                selectProvider(provider);
            });

            fragment.appendChild(card);
        });

        list.replaceChildren(fragment);
        setHidden(empty, providers.length > 0);
    }

    async function addProvider(id, apiKey) {
        const body = {
            provider_id: id,
            is_default: true
        };
        if (apiKey) body.api_key = apiKey;
        const data = await postJson('/api/providers', body);
        state.connected[id] = state.connected[id] || { id: id };
        if (data && data.provider_id) state.connected[data.provider_id] = state.connected[data.provider_id] || { id: data.provider_id };
        return data;
    }

    async function connectWithKey() {
        if (!state.selected || state.busy) return;
        const id = providerId(state.selected);
        const key = ((el('setup-api-key') || {}).value || '').trim();
        const errorNode = el('setup-connect-error');
        setError(errorNode, '');

        if (!key && !LOCAL_PROVIDERS[id]) {
            setError(errorNode, 'Paste an API key to continue.');
            (el('setup-api-key') || {}).focus?.();
            return;
        }

        state.pendingKey = key;
        setHidden(el('setup-save-anyway'), true);
        setBusy(true);
        try {
            if (key) {
                const result = await postJson('/api/providers/validate', {
                    provider_id: id,
                    api_key: key
                });
                if (result && result.valid === false) {
                    // Some valid endpoints (custom gateways, proxies) do not
                    // answer the probe even though they handle chats. Leave a
                    // way through instead of dead-ending the wizard.
                    setError(
                        errorNode,
                        (result.error || 'That key did not validate.') + ' You can still save it if you know it works.'
                    );
                    setHidden(el('setup-save-anyway'), false);
                    return;
                }
            }
            await addProvider(id, key);
            state.connected[id] = state.connected[id] || { id: id };
            state.connected[id].model = state.selected.model || '';
            state.pendingKey = '';
            await afterConnected();
        } catch (error) {
            setError(errorNode, error.message || String(error));
            if (key) setHidden(el('setup-save-anyway'), false);
        } finally {
            setBusy(false);
        }
    }

    async function saveKeyAnyway() {
        if (!state.selected || state.busy) return;
        const id = providerId(state.selected);
        const errorNode = el('setup-connect-error');
        setError(errorNode, '');
        setBusy(true);
        try {
            await addProvider(id, state.pendingKey);
            state.connected[id] = state.connected[id] || { id: id };
            state.connected[id].model = state.selected.model || '';
            state.pendingKey = '';
            setHidden(el('setup-save-anyway'), true);
            await afterConnected();
        } catch (error) {
            setError(errorNode, error.message || String(error));
        } finally {
            setBusy(false);
        }
    }

    async function afterConnected() {
        state.selectedModel = '';
        await loadModels();
        showStep(1);
    }

    function cancelOAuth() {
        state.oauthPending = false;
        if (state.oauthTimer) {
            clearTimeout(state.oauthTimer);
            state.oauthTimer = null;
        }
        if (state.deviceTimer) {
            clearTimeout(state.deviceTimer);
            state.deviceTimer = null;
        }
        setHidden(el('setup-oauth-cancel'), true);
    }

    async function connectWithOAuth() {
        if (!state.selected || state.busy) return;
        const id = providerId(state.selected);
        const statusNode = el('setup-oauth-status');
        const errorNode = el('setup-connect-error');
        setError(errorNode, '');
        setHidden(el('setup-device-box'), true);
        if (statusNode) statusNode.textContent = 'Starting sign-in...';

        try {
            const flow = await postJson('/api/oauth/' + encodeURIComponent(id) + '/start', {});
            state.oauthPending = true;
            setHidden(el('setup-oauth-cancel'), false);

            if (flow.flow_type === 'device_code') {
                if (statusNode) statusNode.textContent = 'Waiting for authorization...';
                el('setup-device-code').textContent = flow.user_code || '';
                const link = el('setup-device-link');
                if (link) {
                    link.href = OSA.safeUrl ? (OSA.safeUrl(flow.verification_uri) || '#') : (flow.verification_uri || '#');
                }
                setHidden(el('setup-device-box'), false);
                state.deviceFlow = {
                    deviceCode: flow.device_code,
                    interval: Math.max(3000, (flow.interval || 5) * 1000)
                };
                pollDeviceCode();
                return;
            }

            if (!flow.auth_url) {
                throw new Error('The provider did not return a sign-in URL.');
            }
            await postJson('/api/oauth/' + encodeURIComponent(id) + '/open-browser', {
                auth_url: flow.auth_url
            });
            if (statusNode) statusNode.textContent = 'Waiting for sign-in in your browser...';
            pollOAuthStatus();
        } catch (error) {
            cancelOAuth();
            if (statusNode) statusNode.textContent = '';
            setError(errorNode, error.message || String(error));
        }
    }

    async function pollOAuthStatus(attempt) {
        if (!state.oauthPending) return;
        const tries = (attempt || 0) + 1;
        if (tries > 150) {
            cancelOAuth();
            setError(el('setup-connect-error'), 'Timed out waiting for sign-in. Try again.');
            return;
        }

        state.oauthTimer = window.setTimeout(async function() {
            if (!state.oauthPending) return;
            try {
                const id = providerId(state.selected);
                const status = await requestJson('/api/oauth/' + encodeURIComponent(id) + '/status');
                if (status.status === 'active' || status.configured) {
                    await completeOAuth();
                    return;
                }
            } catch (error) {}
            pollOAuthStatus(tries);
        }, 2000);
    }

    async function pollDeviceCode() {
        if (!state.oauthPending || !state.deviceFlow) return;
        state.deviceTimer = window.setTimeout(async function() {
            if (!state.oauthPending || !state.deviceFlow) return;
            try {
                const id = providerId(state.selected);
                const result = await postJson('/api/oauth/' + encodeURIComponent(id) + '/device', {
                    device_code: state.deviceFlow.deviceCode
                });
                if (result.success && result.connected) {
                    await completeOAuth();
                    return;
                }
                if (result.pending) {
                    if (result.slow_down) {
                        state.deviceFlow.interval = Math.max(state.deviceFlow.interval + 3000, Math.round(state.deviceFlow.interval * 1.5));
                    }
                    pollDeviceCode();
                    return;
                }
                if (result.error) {
                    throw new Error(result.error);
                }
                pollDeviceCode();
            } catch (error) {
                cancelOAuth();
                setError(el('setup-connect-error'), error.message || 'Sign-in failed.');
                const statusNode = el('setup-oauth-status');
                if (statusNode) statusNode.textContent = '';
            }
        }, state.deviceFlow.interval);
    }

    async function completeOAuth() {
        const id = providerId(state.selected);
        cancelOAuth();
        const statusNode = el('setup-oauth-status');
        if (statusNode) statusNode.textContent = 'Connected. Finishing up...';
        try {
            await addProvider(id);
            state.connected[id] = state.connected[id] || { id: id };
            state.connected[id].model = state.selected.model || '';
            await afterConnected();
        } catch (error) {
            setError(el('setup-connect-error'), error.message || String(error));
            if (statusNode) statusNode.textContent = '';
        }
    }

    async function loadModels() {
        const provider = state.selected;
        if (!provider) return;
        const id = providerId(provider);
        const list = el('setup-model-list');
        if (list) list.replaceChildren();
        state.models = [];

        try {
            const models = await requestJson('/api/providers/models?provider_id=' + encodeURIComponent(id));
            state.models = Array.isArray(models) ? models : [];
        } catch (error) {
            state.models = [];
        }

        if (!state.models.length) {
            try {
                const catalog = await OSA.loadProviderCatalog?.(true);
                const entry = (catalog?.providers || []).find(function(item) { return item.id === id; });
                state.models = (entry && entry.models) || [];
            } catch (error) {}
        }

        state.models.sort(function(a, b) {
            const rank = { recommended: 0, popular: 1, fast: 2 };
            const ar = rank[a.category] === undefined ? 3 : rank[a.category];
            const br = rank[b.category] === undefined ? 3 : rank[b.category];
            if (ar !== br) return ar - br;
            return String(a.name || a.id).localeCompare(String(b.name || b.id));
        });

        const manual = el('setup-manual-model');
        setHidden(manual, state.models.length > 0);
        setHidden(el('setup-model-empty'), state.models.length > 0);

        const preferred = state.selectedModel
            || (state.connected[id] && state.connected[id].model)
            || provider.model
            || (state.models[0] && state.models[0].id)
            || '';
        if (preferred) {
            state.selectedModel = preferred;
            const manualInput = el('setup-model-input');
            if (manualInput) manualInput.value = preferred;
        }

        renderModels();
        updateFooter();
    }

    function renderModels() {
        const list = el('setup-model-list');
        if (!list) return;
        const query = ((el('setup-model-search') || {}).value || '').trim().toLowerCase();
        const models = state.models.filter(function(model) {
            if (!query) return true;
            return (String(model.name || '') + ' ' + String(model.id || '')).toLowerCase().indexOf(query) !== -1;
        });

        const fragment = document.createDocumentFragment();
        models.slice(0, 400).forEach(function(model) {
            const button = document.createElement('button');
            button.type = 'button';
            button.className = 'setup-model-row';
            button.setAttribute('role', 'radio');
            button.setAttribute('aria-checked', model.id === state.selectedModel ? 'true' : 'false');
            button.classList.toggle('selected', model.id === state.selectedModel);

            const copy = document.createElement('div');
            copy.className = 'setup-model-copy';
            const name = document.createElement('span');
            name.className = 'setup-model-name';
            name.textContent = model.name || model.id;
            const id = document.createElement('span');
            id.className = 'setup-model-id';
            id.textContent = model.id;
            copy.appendChild(name);
            copy.appendChild(id);

            const meta = document.createElement('span');
            meta.className = 'setup-model-meta';
            if (model.context_window) {
                meta.textContent = Math.round(model.context_window / 1000) + 'k ctx';
            }

            button.appendChild(copy);
            button.appendChild(meta);
            button.addEventListener('click', function() {
                state.selectedModel = model.id;
                const manualInput = el('setup-model-input');
                if (manualInput) manualInput.value = model.id;
                renderModels();
                updateFooter();
            });
            fragment.appendChild(button);
        });

        list.replaceChildren(fragment);
        if (models.length === 0 && state.models.length > 0) {
            const empty = document.createElement('p');
            empty.className = 'setup-empty';
            empty.textContent = 'No models match that search.';
            list.appendChild(empty);
        }
    }

    function updateManualModel() {
        if (state.models.length) return;
        state.selectedModel = ((el('setup-model-input') || {}).value || '').trim();
        updateFooter();
    }

    async function browseWorkspace() {
        const input = el('setup-workspace-path');
        if (!input) return;
        try {
            const tauriDialog = window.__TAURI__ && window.__TAURI__.dialog;
            if (tauriDialog && typeof tauriDialog.open === 'function') {
                const picked = await tauriDialog.open({
                    directory: true,
                    title: 'Choose workspace folder',
                    multiple: false
                });
                if (typeof picked === 'string' && picked) {
                    input.value = picked;
                    state.workspace = picked;
                    updateFooter();
                }
                return;
            }
            const data = await requestJson('/api/workspaces/browse');
            if (data.path) {
                input.value = data.path;
                state.workspace = data.path;
                updateFooter();
            }
        } catch (error) {
            if (String(error.message || error).indexOf('cancel') === -1) {
                setError(el('setup-finish-error'), error.message || String(error));
            }
        }
    }

    async function saveWorkspace() {
        const path = state.workspace.trim();
        if (!path) throw new Error('Choose a workspace folder first.');

        let workspaces = [];
        try {
            const data = await requestJson('/api/workspaces');
            workspaces = data.workspaces || [];
        } catch (error) {}

        const target = workspaces.find(function(workspace) { return workspace.id === 'default'; })
            || workspaces[0]
            || null;
        const id = target ? target.id : 'default';
        const name = target ? (target.name || 'Default Workspace') : 'Default Workspace';
        const body = {
            id: id,
            name: name,
            paths: [{ path: path, permission: 'read_write' }],
            description: target ? target.description : null
        };

        try {
            await postJson('/api/workspaces/' + encodeURIComponent(id), body);
        } catch (error) {
            if (/not found/i.test(String(error.message || ''))) {
                await postJson('/api/workspaces', body);
            } else {
                throw error;
            }
        }

        await postJson('/api/workspaces/active', { workspace_id: id });
    }

    async function finish() {
        if (state.busy) return;
        const errorNode = el('setup-finish-error');
        setError(errorNode, '');
        setBusy(true);
        try {
            if (!state.selectedModel) throw new Error('Pick a model first.');
            const provider = state.selected;
            const id = providerId(provider);
            if (!id) throw new Error('Choose a provider first.');

            await postJson('/api/providers/switch', {
                provider_id: id,
                model: state.selectedModel
            });
            await saveWorkspace();

            await OSA.showApp({ force: true });
            if (typeof OSA.loadProviderCatalog === 'function') {
                OSA.loadProviderCatalog(true).catch(function() {});
            }
            OSA.loadModel?.();
            OSA.showToast?.('Setup complete', 'success');
        } catch (error) {
            setError(errorNode, error.message || String(error));
        } finally {
            setBusy(false);
        }
    }

    async function continueStep() {
        if (state.busy || !canContinue()) return;
        if (state.step === 0) {
            const provider = state.selected;
            const id = providerId(provider);
            if (!state.connected[id]) {
                if (provider && provider.oauth_supported) {
                    connectWithOAuth();
                } else {
                    connectWithKey();
                }
                return;
            }
            state.selectedModel = state.connected[id].model || '';
            await loadModels();
            showStep(1);
            return;
        }
        if (state.step === 1) {
            showStep(2);
            return;
        }
        finish();
    }

    function goBack() {
        if (state.step === 0) return;
        showStep(state.step - 1);
    }

    async function loadAll() {
        state.connected = {};
        state.selected = null;
        state.selectedModel = '';
        state.models = [];
        state.workspace = '';
        setHidden(el('setup-provider-connect'), true);
        setHidden(el('setup-provider-picker'), false);
        setError(el('setup-connect-error'), '');
        setError(el('setup-finish-error'), '');
        const list = el('setup-provider-list');
        if (list) list.replaceChildren();

        const [setupState, catalog, providersData] = await Promise.all([
            requestJson('/api/setup/state').catch(function() { return {}; }),
            requestJson('/api/providers/catalog').catch(function() { return { providers: [] }; }),
            requestJson('/api/providers').catch(function() { return { providers: [] }; })
        ]);

        state.providers = (catalog.providers || []).filter(function(provider) {
            return !!providerId(provider);
        });
        (providersData.providers || []).forEach(function(entry) {
            state.connected[entry.id] = entry;
            // Custom endpoints are saved entries but not catalog presets;
            // include them so re-running setup can keep using them.
            if (!state.providers.some(function(provider) { return providerId(provider) === entry.id; })) {
                state.providers.push({
                    id: entry.id,
                    name: entry.name || entry.id,
                    description: entry.provider_type || 'Configured provider',
                    oauth_supported: false
                });
            }
        });
        if (providersData.default_provider && state.connected[providersData.default_provider]) {
            state.connected[providersData.default_provider].is_default = true;
        }

        state.workspace = setupState.workspace || '';
        const workspaceInput = el('setup-workspace-path');
        if (workspaceInput) workspaceInput.value = state.workspace;

        const search = el('setup-provider-search');
        if (search) search.value = '';

        renderProviders();
        showStep(0);
    }

    function bind() {
        if (state.mounted) return;
        state.mounted = true;

        el('setup-provider-search')?.addEventListener('input', renderProviders);
        el('setup-model-search')?.addEventListener('input', renderModels);
        el('setup-model-input')?.addEventListener('input', updateManualModel);
        el('setup-workspace-path')?.addEventListener('input', function(event) {
            state.workspace = event.target.value || '';
            updateFooter();
        });
        el('setup-change-provider')?.addEventListener('click', backToProviders);
        el('setup-connect-btn')?.addEventListener('click', function() {
            const provider = state.selected;
            if (!provider) return;
            const id = providerId(provider);
            if (state.connected[id]) {
                state.selectedModel = state.connected[id].model || '';
                loadModels().then(function() { showStep(1); });
            } else {
                connectWithKey();
            }
        });
        el('setup-save-anyway')?.addEventListener('click', saveKeyAnyway);
        el('setup-oauth-btn')?.addEventListener('click', connectWithOAuth);
        el('setup-use-key')?.addEventListener('click', function() {
            applyAuthMode('key');
            window.setTimeout(function() { el('setup-api-key')?.focus(); }, 30);
        });
        el('setup-use-oauth')?.addEventListener('click', function() {
            applyAuthMode('oauth');
        });
        el('setup-oauth-cancel')?.addEventListener('click', function() {
            cancelOAuth();
            const statusNode = el('setup-oauth-status');
            if (statusNode) statusNode.textContent = 'Sign-in cancelled.';
        });
        el('setup-browse')?.addEventListener('click', browseWorkspace);
        el('setup-back')?.addEventListener('click', goBack);
        el('setup-continue')?.addEventListener('click', continueStep);
        el('setup-cancel')?.addEventListener('click', function() {
            if (!state.cancelable) return;
            cancelOAuth();
            OSA.showApp({ force: true });
        });
        el('setup-api-key')?.addEventListener('keydown', function(event) {
            if (event.key === 'Enter' && !state.busy) connectWithKey();
        });
        el('setup-model-input')?.addEventListener('keydown', function(event) {
            if (event.key === 'Enter' && !state.busy) continueStep();
        });
        el('setup-workspace-path')?.addEventListener('keydown', function(event) {
            if (event.key === 'Enter' && !state.busy) continueStep();
        });
    }

    async function mount(options) {
        state.cancelable = !!(options && options.cancelable);
        setHidden(el('setup-cancel'), !state.cancelable);
        bind();
        showStep(0);
        try {
            await loadAll();
        } catch (error) {
            setError(el('setup-connect-error'), 'Could not load setup: ' + (error.message || error));
        }
    }

    return {
        mount: mount,
        refresh: loadAll
    };
})();

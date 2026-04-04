window.OSA = window.OSA || {};

OSA.providerCatalog = { providers: [], all_models: [] };
OSA.modelDropdownOpen = false;
OSA.modelSearchQuery = '';
OSA.currentProviderId = null;
OSA.currentProviderApiKeyUrl = null;
OSA.currentProviderOAuthSupported = false;
OSA.pendingProviderModel = null;
OSA.validationDebounce = null;
OSA.expandedModels = {};
OSA.modalModelDropdownOpen = false;
OSA.modalModelSearchQuery = '';
OSA.modalProviderModels = [];
OSA.ollamaModelDebounce = null;

// ── Favourite Models ─────────────────────────────────────────

OSA.favourites = (function() {
    try { return JSON.parse(localStorage.getItem('osa_favourite_models') || '[]'); } catch (e) { return []; }
})();

OSA.saveFavourites = function() {
    localStorage.setItem('osa_favourite_models', JSON.stringify(OSA.favourites));
};

OSA.isFavourite = function(modelId, providerId) {
    return OSA.favourites.some(function(f) {
        return f.id === modelId && (!providerId || f.provider_id === providerId);
    });
};

OSA.toggleFavourite = function(modelId, providerId, modelName, providerName) {
    const idx = OSA.favourites.findIndex(function(f) {
        return f.id === modelId && f.provider_id === providerId;
    });
    if (idx >= 0) {
        OSA.favourites.splice(idx, 1);
    } else {
        OSA.favourites.push({ id: modelId, provider_id: providerId, name: modelName, provider_name: providerName });
    }
    OSA.saveFavourites();
    if (OSA.modelDropdownOpen) OSA.renderModelDropdown();
    const searchInput = document.getElementById('model-catalog-search');
    OSA.renderSettingsModelList(searchInput ? searchInput.value : '');
};

// ── Model Option HTML Builder ────────────────────────────────

OSA.buildModelOptionHtml = function(m, providerId, currentModel, opts) {
    const pid = m.provider_id || providerId;
    const isCurrent = m.id.toLowerCase() === (currentModel || '').toLowerCase()
        && (!OSA.currentModelProviderId || OSA.currentModelProviderId === pid);
    const isFav = OSA.isFavourite(m.id, pid);
    const ctx = m.context_window >= 1000000
        ? (m.context_window / 1000000).toFixed(0) + 'M'
        : m.context_window > 0 ? (m.context_window / 1000).toFixed(0) + 'K' : '?';
    const cat = m.category === 'recommended' ? ' style="color:var(--accent)"' :
                 m.category === 'fast' ? ' style="color:var(--text-muted)"' : '';

    const safeId = OSA.escapeHtml(m.id);
    const safePid = OSA.escapeHtml(pid);
    const safeName = OSA.escapeHtml(m.name);
    const safeProvName = OSA.escapeHtml(m.provider_name || '');

    const selectClick = (opts && opts.inSettings)
        ? 'OSA.selectModel(\'' + safeId + '\', \'' + safePid + '\'); OSA.closeSettings();'
        : 'OSA.selectModel(\'' + safeId + '\', \'' + safePid + '\')';

    const providerTag = (opts && opts.showProvider && (m.provider_name || ''))
        ? '<span class="model-option-provider-tag">' + safeProvName + '</span>'
        : '';

    const favBtn = '<span class="model-fav-btn' + (isFav ? ' is-fav' : '') + '" ' +
        'title="' + (isFav ? 'Remove from favourites' : 'Add to favourites') + '" ' +
        'onclick="event.stopPropagation(); OSA.toggleFavourite(\'' + safeId + '\', \'' + safePid + '\', \'' + safeName + '\', \'' + safeProvName + '\')">' +
        (isFav ? '★' : '☆') +
        '</span>';

    return '<div class="model-option' + (isCurrent ? ' active' : '') + '" onclick="' + selectClick + '">' +
        '<div class="model-option-info">' +
            '<span class="model-option-name">' + safeName + '</span>' +
            '<span class="model-option-id"' + cat + '>' + safeId + '</span>' +
        '</div>' +
        '<div class="model-option-meta">' +
            providerTag +
            '<span>' + ctx + ' ctx</span>' +
            (m.supports_tools ? '<span title="Tool calling">T</span>' : '') +
            (m.supports_vision ? '<span title="Vision">V</span>' : '') +
            favBtn +
        '</div>' +
    '</div>';
};

OSA.loadProviderCatalog = async function() {
    try {
        const data = await OSA.getJson('/api/providers/catalog');
        OSA.providerCatalog = data;
    } catch (error) {
        console.error('Failed to load provider catalog:', error);
    }
};

OSA.sortProvidersForDropdown = function(providers) {
    const providerOrder = ['OpenRouter', 'OpenAI', 'Anthropic', 'Google AI', 'Groq', 'DeepSeek', 'xAI', 'Ollama (Local)'];
    return [...(providers || [])].sort((a, b) => {
        if (!!a.connected !== !!b.connected) return a.connected ? -1 : 1;
        const ai = providerOrder.indexOf(a.name);
        const bi = providerOrder.indexOf(b.name);
        if (ai !== -1 && bi !== -1) return ai - bi;
        if (ai !== -1) return -1;
        if (bi !== -1) return 1;
        return a.name.localeCompare(b.name);
    });
};

// ── SVG Icons ───────────────────────────────────────────────

OSA.icons = {
    check: '<svg class="icon-inline" viewBox="0 0 16 16" width="12" height="12"><path fill="currentColor" d="M13.78 4.22a.75.75 0 0 1 0 1.06l-7.25 7.25a.75.75 0 0 1-1.06 0L2.22 9.28a.75.75 0 0 1 1.06-1.06L6 10.94l6.72-6.72a.75.75 0 0 1 1.06 0z"/></svg>',
    xmark: '<svg class="icon-inline" viewBox="0 0 16 16" width="12" height="12"><path fill="currentColor" d="M3.72 3.72a.75.75 0 0 1 1.06 0L8 6.94l3.22-3.22a.75.75 0 1 1 1.06 1.06L9.06 8l3.22 3.22a.75.75 0 1 1-1.06 1.06L8 9.06l-3.22 3.22a.75.75 0 0 1-1.06-1.06L6.94 8 3.72 4.78a.75.75 0 0 1 0-1.06z"/></svg>',
    spinner: '<span class="validation-spinner"></span>',
    external: '<svg class="icon-inline" viewBox="0 0 16 16" width="12" height="12"><path fill="currentColor" d="M3.75 2h3.5a.75.75 0 0 1 0 1.5h-3.5a.25.25 0 0 0-.25.25v8.5c0 .138.112.25.25.25h8.5a.25.25 0 0 0 .25-.25v-3.5a.75.75 0 0 1 1.5 0v3.5A1.75 1.75 0 0 1 12.25 14h-8.5A1.75 1.75 0 0 1 2 12.25v-8.5C2 2.784 2.784 2 3.75 2zm6.854-1h4.146a.25.25 0 0 1 .25.25v4.146a.25.25 0 0 1-.427.177L9.354 2.354a.25.25 0 0 1 0-.354l.283-.283a.25.25 0 0 1 .354 0l2.96 2.96V2.25a.25.25 0 0 1 .25-.25z"/></svg>'
};

// ── Header Model Dropdown ───────────────────────────────────

OSA.toggleModelDropdown = function() {
    OSA.modelDropdownOpen = !OSA.modelDropdownOpen;
    const dropdown = document.getElementById('model-dropdown');
    if (dropdown) {
        dropdown.classList.toggle('hidden', !OSA.modelDropdownOpen);
        if (OSA.modelDropdownOpen) {
            const searchInput = document.getElementById('model-search');
            if (searchInput) {
                searchInput.value = '';
                OSA.modelSearchQuery = '';
                searchInput.focus();
            }
            OSA.renderModelDropdown();
        }
    }
};

OSA.closeModelDropdown = function() {
    OSA.modelDropdownOpen = false;
    const dropdown = document.getElementById('model-dropdown');
    if (dropdown) dropdown.classList.add('hidden');
};

// Global aliases for inline onclick handlers in HTML
window.toggleModelDropdown = function() { OSA.toggleModelDropdown(); };
window.closeModelDropdown = function() { OSA.closeModelDropdown(); };
window.handleModelSearch = function(e) { OSA.handleModelSearch(e); };
window.handleModelDropdownKeydown = function(e) { OSA.handleModelDropdownKeydown(e); };
window.handleModelInputKeydown = function(e) { OSA.handleModelInputKeydown(e); };

OSA.handleModelInputKeydown = function(event) {
    const key = event.key || '';
    if (key === 'ArrowDown' || key === 'Enter' || key === ' ') {
        event.preventDefault();
        if (!OSA.modelDropdownOpen) {
            OSA.toggleModelDropdown();
        }
        return;
    }

    if (key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault();
        if (!OSA.modelDropdownOpen) {
            OSA.toggleModelDropdown();
        }
        const searchInput = document.getElementById('model-search');
        if (searchInput) {
            searchInput.value = key;
            OSA.modelSearchQuery = key;
            searchInput.focus();
            OSA.renderModelDropdown();
        }
    }
};

OSA.renderModelDropdown = async function() {
    const dropdown = document.getElementById('model-dropdown');
    if (!dropdown) return;

    const query = OSA.modelSearchQuery.trim();
    const currentModel = (OSA.currentModelId || document.getElementById('model-input')?.value || '').toLowerCase();

    if (query.length >= 1) {
        try {
            const models = await OSA.getJson(`/api/providers/search?q=${encodeURIComponent(query)}`);
            OSA.renderModelSearchResults(models, currentModel);
        } catch (e) {
            dropdown.querySelector('.model-dropdown-list').innerHTML = '<div class="model-empty">Search failed</div>';
        }
        return;
    }

    const { providers, all_models } = OSA.providerCatalog;
    let html = '';

    // Favourites group at top
    if (OSA.favourites.length > 0) {
        html += '<div class="model-group-title model-group-favs">★ Favourites</div>';
        for (const fav of OSA.favourites) {
            const full = (all_models || []).find(function(m) { return m.id === fav.id && m.provider_id === fav.provider_id; }) || {
                id: fav.id, name: fav.name, provider_id: fav.provider_id,
                provider_name: fav.provider_name, context_window: 0,
                supports_tools: false, supports_vision: false, category: ''
            };
            html += OSA.buildModelOptionHtml(full, fav.provider_id, currentModel, { showProvider: true });
        }
    }

    const sortedProviders = OSA.sortProvidersForDropdown(providers);

    for (const provider of sortedProviders) {
        const models = provider.models || [];
        if (models.length === 0) continue;
        html += '<div class="model-group-title">' + OSA.escapeHtml(provider.name) + '</div>';
        for (const m of models) {
            html += OSA.buildModelOptionHtml(m, provider.id, currentModel, {});
        }
    }

    if (!html) html = '<div class="model-empty">No models available</div>';
    dropdown.querySelector('.model-dropdown-list').innerHTML = html;
};

OSA.renderModelSearchResults = function(models, currentModel) {
    const dropdown = document.getElementById('model-dropdown');
    if (!dropdown) return;

    if (!models || models.length === 0) {
        dropdown.querySelector('.model-dropdown-list').innerHTML = '<div class="model-empty">No models found</div>';
        return;
    }

    const grouped = {};
    for (const m of models) {
        if (!grouped[m.provider_name]) grouped[m.provider_name] = [];
        grouped[m.provider_name].push(m);
    }

    const providersByName = Object.fromEntries((OSA.providerCatalog.providers || []).map(function(provider) {
        return [provider.name, provider];
    }));
    const orderedProviders = OSA.sortProvidersForDropdown(
        Object.keys(grouped).map(function(name) {
            return providersByName[name] || { name: name, connected: false };
        })
    );

    let html = '';
    for (const provider of orderedProviders) {
        const providerName = provider.name;
        const providerModels = grouped[providerName] || [];
        html += '<div class="model-group-title">' + OSA.escapeHtml(providerName) + '</div>';
        for (const m of providerModels) {
            html += OSA.buildModelOptionHtml(m, m.provider_id, currentModel, {});
        }
    }

    dropdown.querySelector('.model-dropdown-list').innerHTML = html;
};

OSA.selectModel = async function(modelId, providerId) {
    const provider = (OSA.providerCatalog.providers || []).find(function(item) { return item.id === providerId; });
    const configured = !!(provider && provider.connected) || await (async function() {
        try {
            const data = await OSA.getJson('/api/providers');
            return (data.providers || []).some(function(item) { return item.id === providerId; });
        } catch (error) {
            const cfg = OSA.getCachedConfig ? OSA.getCachedConfig() : null;
            return !!cfg?.providers?.some(function(item) { return item.provider_type === providerId; });
        }
    })();

    if (!configured) {
        OSA.closeModelDropdown();
        await OSA.openAddProviderModal(providerId, modelId);
        return;
    }

    OSA.currentModelId = modelId;
    OSA.currentModelProviderId = providerId;
    const input = document.getElementById('model-input');
    if (input) {
        const provider = (OSA.providerCatalog.providers || []).find(function(item) { return item.id === providerId; });
        input.value = provider ? provider.name + ' · ' + modelId : modelId;
        input.title = provider ? provider.name + ' / ' + modelId : modelId;
    }
    OSA.closeModelDropdown();

    try {
        const res = await fetch('/api/model', {
            method: 'POST',
            headers: { 'Authorization': 'Bearer ' + OSA.getToken(), 'Content-Type': 'application/json' },
            body: JSON.stringify({ model: modelId, provider_id: providerId })
        });
        const data = await res.json().catch(function() { return {}; });
        if (!res.ok) {
            throw new Error(data.error || 'Failed to switch model');
        }
        if (typeof OSA.refreshThinkingOptions === 'function') {
            const cachedConfig = OSA.getCachedConfig ? OSA.getCachedConfig() : null;
            const selected = cachedConfig?.agent?.thinking_level || 'auto';
            OSA.refreshThinkingOptions(providerId, modelId, selected);
        }
    } catch (error) {
        console.error('Failed to switch model:', error);
        alert(error.message || 'Failed to switch model');
    }
};

OSA.modelSearchDebounce = null;

OSA.handleModelSearch = function(event) {
    OSA.modelSearchQuery = event.target.value;
    if (OSA.modelSearchDebounce) clearTimeout(OSA.modelSearchDebounce);
    OSA.modelSearchDebounce = setTimeout(() => OSA.renderModelDropdown(), 200);
};

OSA.handleModelDropdownKeydown = function(event) {
    if (event.key === 'Escape') { OSA.closeModelDropdown(); event.stopPropagation(); }
};

// ── Modal Model Dropdown ────────────────────────────────────

OSA.toggleModalModelDropdown = function() {
    OSA.modalModelDropdownOpen = !OSA.modalModelDropdownOpen;
    const dropdown = document.getElementById('modal-model-dropdown');
    if (dropdown) {
        dropdown.classList.toggle('hidden', !OSA.modalModelDropdownOpen);
        if (OSA.modalModelDropdownOpen) {
            const searchInput = document.getElementById('modal-model-search');
            if (searchInput) { searchInput.value = ''; OSA.modalModelSearchQuery = ''; searchInput.focus(); }
        }
    }
};

OSA.closeModalModelDropdown = function() {
    OSA.modalModelDropdownOpen = false;
    const dropdown = document.getElementById('modal-model-dropdown');
    if (dropdown) dropdown.classList.add('hidden');
};

OSA.handleModalModelSearch = function(event) {
    OSA.modalModelSearchQuery = event.target.value;
    if (OSA.modelSearchDebounce) clearTimeout(OSA.modelSearchDebounce);
    OSA.modelSearchDebounce = setTimeout(() => OSA.renderModalModelDropdown(), 200);
};

OSA.handleModalDropdownKeydown = function(event) {
    if (event.key === 'Escape') { OSA.closeModalModelDropdown(); event.stopPropagation(); }
};

OSA.formatContextWindow = function(contextWindow) {
    if (!contextWindow || contextWindow <= 0) return '?';
    if (contextWindow >= 1000000) return (contextWindow / 1000000).toFixed(0) + 'M';
    return (contextWindow / 1000).toFixed(0) + 'K';
};

OSA.discoverProviderModels = async function(providerId, baseUrl) {
    const params = new URLSearchParams();
    params.set('provider_id', providerId || '');
    if (baseUrl) params.set('base_url', baseUrl);
    return OSA.getJson('/api/providers/models?' + params.toString());
};

OSA.refreshModalProviderModels = async function(provider, connectedEntry) {
    if (!provider) return;

    const list = document.getElementById('modal-model-list');
    const baseUrlInput = document.getElementById('provider-base-url');
    const selectedModelId = document.getElementById('provider-model').value;

    if (provider.id !== 'ollama') {
        OSA.modalProviderModels = provider.models || [];
        OSA.populateModalModelDropdown(OSA.modalProviderModels, provider.id);
        return;
    }

    if (list) list.innerHTML = '<div class="model-empty">Loading installed Ollama models...</div>';

    const baseUrl = (baseUrlInput && baseUrlInput.value.trim())
        || (connectedEntry && connectedEntry.base_url)
        || provider.base_url
        || '';

    try {
        const models = await OSA.discoverProviderModels(provider.id, baseUrl);
        OSA.modalProviderModels = Array.isArray(models) ? models : [];
    } catch (error) {
        OSA.modalProviderModels = [];
    }

    OSA.populateModalModelDropdown(OSA.modalProviderModels, provider.id);

    if (selectedModelId) {
        const selected = OSA.modalProviderModels.find(function(model) { return model.id === selectedModelId; });
        if (selected) {
            OSA.selectModalModel(selected.id, selected.name, provider.id);
            return;
        }
    }

    if (provider.id === 'ollama' && OSA.modalProviderModels.length === 0 && list) {
        list.innerHTML = '<div class="model-empty">No Ollama models available (is Ollama running?)</div>';
    }
};

OSA.renderModalModelDropdown = async function() {
    const list = document.getElementById('modal-model-list');
    if (!list) return;

    const query = OSA.modalModelSearchQuery.trim();
    const provider = OSA.providerCatalog.providers.find(p => p.id === OSA.currentProviderId);
    if (!provider) { list.innerHTML = '<div class="model-empty">No provider selected</div>'; return; }
    const modalModels = (OSA.modalProviderModels && OSA.modalProviderModels.length)
        ? OSA.modalProviderModels
        : (provider.models || []);

    if (provider.id === 'ollama') {
        const filtered = query
            ? modalModels.filter(function(model) {
                return (model.name || '').toLowerCase().includes(query.toLowerCase())
                    || (model.id || '').toLowerCase().includes(query.toLowerCase());
            })
            : modalModels;
        OSA.populateModalModelDropdown(filtered, provider.id);
        if (filtered.length === 0) {
            list.innerHTML = '<div class="model-empty">No Ollama models available (is Ollama running?)</div>';
        }
        return;
    }

    if (query.length >= 2) {
        try {
            const models = await OSA.getJson('/api/providers/search?q=' + encodeURIComponent(query));
            if (!models.length) { list.innerHTML = '<div class="model-empty">No models found</div>'; return; }
            let html = '';
            for (const m of models) {
                html += '<div class="model-option" onclick="OSA.selectModalModel(\'' + OSA.escapeHtml(m.id) + '\', \'' + OSA.escapeHtml(m.name) + '\', \'' + OSA.escapeHtml(m.provider_id) + '\')">' +
                    '<div class="model-option-info">' +
                        '<span class="model-option-name">' + OSA.escapeHtml(m.name) + '</span>' +
                        '<span class="model-option-id">' + OSA.escapeHtml(m.id) + '</span>' +
                    '</div>' +
                    '<div class="model-option-meta">' +
                        '<span>' + OSA.formatContextWindow(m.context_window) + ' ctx</span>' +
                        (m.supports_tools ? '<span title="Tool calling">T</span>' : '') +
                        (m.supports_vision ? '<span title="Vision">V</span>' : '') +
                    '</div>' +
                '</div>';
            }
            list.innerHTML = html;
            return;
        } catch (e) { list.innerHTML = '<div class="model-empty">Search failed</div>'; return; }
    }

    OSA.populateModalModelDropdown(modalModels, provider.id);
};

OSA.populateModalModelDropdown = function(models, providerId) {
    const list = document.getElementById('modal-model-list');
    if (!list) return;

    const categories = {};
    const catOrder = ['installed', 'recommended', 'popular', 'fast', 'reasoning', 'open', 'code'];
    for (const m of models) {
        const cat = m.category || 'other';
        if (!categories[cat]) categories[cat] = [];
        categories[cat].push(m);
    }

    let html = '';
    for (const cat of catOrder) {
        if (!categories[cat] || categories[cat].length === 0) continue;
        html += '<div class="model-group-title">' + cat.charAt(0).toUpperCase() + cat.slice(1) + '</div>';
        for (const m of categories[cat]) {
            html += '<div class="model-option" onclick="OSA.selectModalModel(\'' + OSA.escapeHtml(m.id) + '\', \'' + OSA.escapeHtml(m.name) + '\', \'' + OSA.escapeHtml(providerId) + '\')">' +
                '<div class="model-option-info">' +
                    '<span class="model-option-name">' + OSA.escapeHtml(m.name) + '</span>' +
                    '<span class="model-option-id">' + OSA.escapeHtml(m.id) + '</span>' +
                '</div>' +
                '<div class="model-option-meta">' +
                    '<span>' + OSA.formatContextWindow(m.context_window) + ' ctx</span>' +
                    (m.supports_tools ? '<span title="Tool calling">T</span>' : '') +
                    (m.supports_vision ? '<span title="Vision">V</span>' : '') +
                '</div>' +
            '</div>';
        }
    }
    list.innerHTML = html || '<div class="model-empty">No models</div>';
};

OSA.selectModalModel = function(modelId, modelName, providerId) {
    OSA.pendingProviderModel = modelId;
    OSA.currentModelId = modelId;
    OSA.currentModelProviderId = providerId;
    document.getElementById('provider-model').value = modelId;
    document.getElementById('modal-model-select-text').textContent = modelName;
    OSA.closeModalModelDropdown();
};

// ── Settings Model Filter ────────────────────────────────────

OSA.filterSettingsModels = function(query) {
    OSA.renderSettingsModelList(query || '');
};

OSA.renderSettingsModelList = function(query) {
    const catalogList = document.getElementById('all-models-list');
    if (!catalogList) return;

    const q = (query || '').toLowerCase().trim();
    const { providers } = OSA.providerCatalog;
    if (!providers || providers.length === 0) {
        catalogList.innerHTML = '<div class="model-empty">No providers available</div>';
        return;
    }

    const providerOrder = ['OpenRouter', 'OpenAI', 'Anthropic', 'Google AI', 'Groq', 'DeepSeek', 'xAI', 'Ollama (Local)'];
    const sortedProviders = [...providers].sort((a, b) => {
        const ai = providerOrder.indexOf(a.name);
        const bi = providerOrder.indexOf(b.name);
        if (ai !== -1 && bi !== -1) return ai - bi;
        if (ai !== -1) return -1;
        if (bi !== -1) return 1;
        return a.name.localeCompare(b.name);
    });

    const currentModel = (document.getElementById('model-input')?.value || '').toLowerCase();
    let html = '';

    for (const provider of sortedProviders) {
        let models = provider.models || [];
        if (q) {
            models = models.filter(m =>
                m.name.toLowerCase().includes(q) ||
                m.id.toLowerCase().includes(q) ||
                provider.name.toLowerCase().includes(q)
            );
        }
        if (models.length === 0) continue;

        const connectedBadge = provider.connected
            ? ' <span class="badge badge-apikey" style="font-size:10px;padding:1px 6px;vertical-align:middle">Connected</span>'
            : ' <button class="btn-ghost" onclick="OSA.openAddProviderModal(\'' + OSA.escapeHtml(provider.id) + '\')" style="padding:1px 8px;font-size:11px;vertical-align:middle;margin-left:2px">Connect</button>';

        html += '<div class="model-group-title">' + OSA.escapeHtml(provider.name) + connectedBadge + '</div>';

        for (const m of models) {
            html += OSA.buildModelOptionHtml(m, provider.id, currentModel, { inSettings: true });
        }
    }

    if (!html) {
        html = '<div class="model-empty">' + (q ? 'No models match "' + OSA.escapeHtml(query) + '"' : 'No models available') + '</div>';
    }

    catalogList.innerHTML = html;
};

OSA.renderRoutingOverview = function(catalog, providersData) {
    const summaryEl = document.getElementById('provider-routing-summary');
    const listEl = document.getElementById('provider-routing-list');
    if (!summaryEl && !listEl) return;

    const connected = providersData.providers || [];
    const activeId = providersData.default_provider || '';
    const activeModel = providersData.default_model || '';
    const providersById = Object.fromEntries((catalog.providers || []).map(function(provider) {
        return [provider.id, provider];
    }));
    const activeProvider = providersById[activeId] || null;

    if (summaryEl) {
        if (!activeId || !activeModel) {
            summaryEl.innerHTML = '<div class="provider-route-card"><div><div class="provider-route-title">No active connected route</div><div class="provider-route-meta">Connect a provider in the Models tab to start routing by provider + model.</div></div><button class="btn-action" onclick="switchSettingsTab(\'models\')">Open Models</button></div>';
        } else {
            const providerName = activeProvider ? activeProvider.name : activeId;
            summaryEl.innerHTML = '<div class="provider-route-card provider-route-card-active">' +
                '<div>' +
                    '<div class="provider-route-kicker">Active provider route</div>' +
                    '<div class="provider-route-title">' + OSA.escapeHtml(providerName) + '</div>' +
                    '<div class="provider-route-meta">Model: <strong>' + OSA.escapeHtml(activeModel) + '</strong></div>' +
                '</div>' +
                '<div class="provider-route-actions">' +
                    '<button class="btn-ghost" onclick="switchSettingsTab(\'models\')">Browse Models</button>' +
                    '<button class="btn-action" onclick="OSA.openAddProviderModal(\'' + OSA.escapeHtml(activeId) + '\', \'' + OSA.escapeHtml(activeModel) + '\')">Manage</button>' +
                '</div>' +
            '</div>';
        }
    }

    if (listEl) {
        if (!connected.length) {
            listEl.innerHTML = '<div class="model-empty">No connected providers yet</div>';
            return;
        }

        listEl.innerHTML = connected.map(function(entry) {
            const provider = providersById[entry.id] || null;
            const name = provider ? provider.name : entry.id;
            const status = provider && provider.oauth_supported ? 'OAuth' : 'API key';
            const activeBadge = entry.is_default ? '<span class="badge badge-apikey" style="opacity:0.7">active</span>' : '';
            const safeProviderId = OSA.escapeHtml(entry.id);
            const safeModelId = OSA.escapeHtml(entry.model || '');
            return `<div class="provider-route-list-item">
                <div class="provider-route-list-main">
                    <div class="provider-route-list-title">${OSA.escapeHtml(name)}${activeBadge}</div>
                    <div class="provider-route-list-meta">${OSA.escapeHtml(entry.model || 'provider default')} · ${OSA.escapeHtml(status)}</div>
                </div>
                <div class="provider-route-actions">
                    <button class="btn-ghost" onclick="OSA.selectModel('${safeModelId}', '${safeProviderId}')"${entry.model ? '' : ' disabled'}>Use</button>
                    <button class="btn-ghost" onclick="OSA.openAddProviderModal('${safeProviderId}', '${safeModelId}')">Edit</button>
                </div>
            </div>`;
        }).join('');
    }
};

// ── Settings Providers with Collapsible Models ──────────────

OSA.renderSettingsProviders = async function() {
    const catalogList = document.getElementById('model-catalog-list');
    if (!catalogList) return;

    catalogList.innerHTML = '<div class="model-empty">Loading...</div>';

    try {
        const [catalog, providersData] = await Promise.all([
            OSA.getJson('/api/providers/catalog'),
            OSA.getJson('/api/providers')
        ]);
        OSA.providerCatalog = catalog;
        OSA.renderRoutingOverview(catalog, providersData);

        // Build a map of connected provider configs (id → config entry)
        const connectedMap = {};
        for (const p of (providersData.providers || [])) {
            connectedMap[p.id] = p;
        }

        let catalogHtml = '';
        for (const provider of catalog.providers) {
            const modelCount = provider.models.length;
            const isExpanded = OSA.expandedModels[provider.id];
            const connectedEntry = connectedMap[provider.id];
            const isDefault = connectedEntry && connectedEntry.is_default;

            let statusBadge = '';
            if (provider.connected) {
                statusBadge = provider.oauth_supported
                    ? '<span class="badge badge-oauth">OAuth</span>'
                    : '<span class="badge badge-apikey">Connected</span>';
                if (isDefault) statusBadge += '<span class="badge badge-apikey" style="opacity:0.7">active</span>';
            } else {
                statusBadge = '<span class="badge badge-disconnected">Not connected</span>';
            }

            const safeProviderId = OSA.escapeHtml(provider.id);
            const safeConnectedModel = OSA.escapeHtml((connectedEntry && connectedEntry.model) || '');
            const connectBtn = provider.connected
                ? `<button class="btn-ghost" onclick="OSA.openAddProviderModal('${safeProviderId}', '${safeConnectedModel}')" style="padding:4px 12px;font-size:12px">Manage</button>`
                : '<button class="btn-action" onclick="OSA.openAddProviderModal(\'' + safeProviderId + '\')" style="padding:4px 12px;font-size:12px">Connect</button>';

            let modelSection = '';
            if (modelCount > 0) {
                const toggleIcon = isExpanded ? '&#9660;' : '&#9654;';
                const toggleClass = isExpanded ? 'expanded' : '';
                modelSection =
                    '<div class="provider-models-toggle ' + toggleClass + '" onclick="OSA.toggleProviderModels(\'' + OSA.escapeHtml(provider.id) + '\')">' +
                        '<span>' + modelCount + ' model' + (modelCount !== 1 ? 's' : '') + ' ' + toggleIcon + '</span>' +
                    '</div>' +
                    '<div class="provider-catalog-models ' + (isExpanded ? 'expanded' : '') + '" id="models-' + OSA.escapeHtml(provider.id) + '">' +
                        (isExpanded ? OSA.renderCategorizedModels(provider.models, provider.id) : '') +
                    '</div>';
            }

            // Show active model if this provider is connected and configured
            const activeMeta = connectedEntry
                ? '<div class="provider-catalog-meta" style="margin-top:2px">Route: <span style="color:var(--text-primary)">' + OSA.escapeHtml(connectedEntry.model || 'provider default') + '</span></div>'
                : '<div class="provider-catalog-meta">' + OSA.escapeHtml(provider.description) + '</div>';

            catalogHtml += '<div class="provider-catalog-item">' +
                '<div class="provider-catalog-header">' +
                    '<div class="provider-catalog-title">' +
                        '<span class="provider-catalog-name">' + OSA.escapeHtml(provider.name) + '</span>' +
                        statusBadge +
                    '</div>' +
                    connectBtn +
                '</div>' +
                activeMeta +
                modelSection +
            '</div>';
        }
        catalogList.innerHTML = catalogHtml || '<div class="model-empty">No providers available</div>';

        // Populate all-models list
        const filterInput = document.getElementById('model-catalog-search');
        OSA.renderSettingsModelList(filterInput ? filterInput.value : '');
    } catch (e) {
        catalogList.innerHTML = '<div class="model-empty">Failed to load catalog</div>';
    }
};

OSA.toggleProviderModels = function(providerId) {
    OSA.expandedModels[providerId] = !OSA.expandedModels[providerId];
    OSA.renderSettingsProviders();
};

OSA.renderCategorizedModels = function(models, providerId) {
    const categories = {};
    const catOrder = ['recommended', 'popular', 'fast', 'reasoning', 'open', 'code', 'custom'];

    for (const m of models) {
        const cat = m.category || 'other';
        if (!categories[cat]) categories[cat] = [];
        categories[cat].push(m);
    }

    let html = '';
    for (const cat of catOrder) {
        if (!categories[cat] || categories[cat].length === 0) continue;
        html += '<div class="model-category"><div class="model-category-title">' + cat.charAt(0).toUpperCase() + cat.slice(1) + '</div>';
        for (const m of categories[cat]) {
            const ctx = m.context_window >= 1000000 ? (m.context_window / 1000000).toFixed(0) + 'M' : (m.context_window / 1000).toFixed(0) + 'K';
            let badges = '';
            if (m.supports_tools) badges += '<span class="model-badge" title="Tool calling">T</span>';
            if (m.supports_vision) badges += '<span class="model-badge" title="Vision">V</span>';
            html += '<div class="provider-model-tag" onclick="OSA.selectModel(\'' + OSA.escapeHtml(m.id) + '\', \'' + OSA.escapeHtml(m.provider_id || providerId) + '\'); OSA.closeSettings();" title="' + OSA.escapeHtml(m.id) + '">' +
                OSA.escapeHtml(m.name) +
                '<span class="model-tag-meta">' + ctx + ' ctx ' + badges + '</span>' +
            '</div>';
        }
        html += '</div>';
    }

    for (const [cat, catModels] of Object.entries(categories)) {
        if (catOrder.includes(cat)) continue;
        html += '<div class="model-category"><div class="model-category-title">' + cat.charAt(0).toUpperCase() + cat.slice(1) + '</div>';
        for (const m of catModels) {
            const ctx = m.context_window >= 1000000 ? (m.context_window / 1000000).toFixed(0) + 'M' : (m.context_window / 1000).toFixed(0) + 'K';
            let badges = '';
            if (m.supports_tools) badges += '<span class="model-badge" title="Tool calling">T</span>';
            if (m.supports_vision) badges += '<span class="model-badge" title="Vision">V</span>';
            html += '<div class="provider-model-tag" onclick="OSA.selectModel(\'' + OSA.escapeHtml(m.id) + '\', \'' + OSA.escapeHtml(m.provider_id || providerId) + '\'); OSA.closeSettings();" title="' + OSA.escapeHtml(m.id) + '">' +
                OSA.escapeHtml(m.name) +
                '<span class="model-tag-meta">' + ctx + ' ctx ' + badges + '</span>' +
            '</div>';
        }
        html += '</div>';
    }
    return html || '<div class="model-empty">No models</div>';
};

// ── Sleek Add Provider Modal ────────────────────────────────

OSA.openAddProviderModal = async function(providerId, preferredModelId) {
    const modal = document.getElementById('add-provider-modal');
    if (!modal || !providerId) return;

    const [_, providersData] = await Promise.all([
        OSA.loadProviderCatalog(),
        OSA.getJson('/api/providers').catch(function() { return { providers: [] }; })
    ]);
    OSA.currentProviderId = providerId;
    OSA.pendingProviderModel = preferredModelId || null;

    const provider = OSA.providerCatalog.providers.find(p => p.id === providerId);
    if (!provider) return;
    const connectedEntry = (providersData.providers || []).find(function(item) { return item.id === providerId; }) || null;

    OSA.currentProviderOAuthSupported = provider.oauth_supported;
    OSA.currentProviderApiKeyUrl = provider.api_key_url || null;

    // Set header icon
    const iconMap = { openai: 'OAI', anthropic: 'ANT', google: 'GAI', openrouter: 'ORN', ollama: 'OLL', groq: 'GRQ', deepseek: 'DSK', xai: 'XAI' };
    const iconEl = document.getElementById('provider-modal-icon');
    iconEl.textContent = iconMap[providerId] || providerId.substring(0, 3).toUpperCase();
    iconEl.className = 'provider-modal-icon icon-' + providerId;

    document.getElementById('provider-modal-title').textContent = provider.name;
    document.getElementById('provider-modal-desc').textContent = provider.description || 'Connect your account';

    const oauthSection = document.getElementById('oauth-section');
    const apiKeySection = document.getElementById('api-key-section');
    const divider = document.getElementById('modal-divider');
    const apiKeyTitle = document.getElementById('api-key-title');
    const addBtn = document.getElementById('add-provider-btn');

    // Show OAuth section for OAuth-capable providers
    if (provider.oauth_supported) {
        oauthSection.classList.remove('hidden');
        apiKeySection.classList.remove('hidden');
        divider.classList.remove('hidden');
        apiKeyTitle.classList.remove('hidden');
        addBtn.textContent = 'Save Changes';
        await OSA.updateOAuthUI(providerId);
    } else {
        oauthSection.classList.add('hidden');
        apiKeySection.classList.remove('hidden');
        divider.classList.add('hidden');
        apiKeyTitle.classList.add('hidden');
        addBtn.textContent = 'Add Provider';
    }

    // Pre-fill base URL
    document.getElementById('provider-base-url').value = (connectedEntry && connectedEntry.base_url) || provider.base_url || '';

    // API key link
    const apiKeyLink = document.getElementById('api-key-link');
    const apiKeyLinkName = document.getElementById('api-key-link-name');
    if (provider.api_key_url) {
        apiKeyLink.classList.remove('hidden');
        apiKeyLinkName.textContent = provider.name;
        apiKeyLink.onclick = function() { window.open(provider.api_key_url, '_blank'); return false; };
    } else {
        apiKeyLink.classList.add('hidden');
    }

    // Reset fields
    document.getElementById('provider-api-key').value = '';
    document.getElementById('provider-default').checked = !!(connectedEntry && connectedEntry.is_default);
    document.getElementById('validation-badge').style.display = 'none';
    document.getElementById('modal-model-select-text').textContent = 'Use provider default (choose later)';
    document.getElementById('provider-model').value = '';
    OSA.closeModalModelDropdown();

    // Populate modal model dropdown
    await OSA.refreshModalProviderModels(provider, connectedEntry);
    const initialModelId = OSA.pendingProviderModel || (connectedEntry && connectedEntry.model) || '';
    if (initialModelId) {
        const selected = OSA.modalProviderModels.find(function(model) { return model.id === initialModelId; });
        if (selected) {
            OSA.selectModalModel(selected.id, selected.name, provider.id);
        } else {
            document.getElementById('provider-model').value = initialModelId;
            document.getElementById('modal-model-select-text').textContent = initialModelId;
        }
    }

    // Set up auto-validation and dynamic Ollama discovery
    document.getElementById('provider-api-key').oninput = function() { OSA.scheduleValidation(); };
    document.getElementById('provider-base-url').oninput = function() {
        OSA.scheduleValidation();
        if (provider.id === 'ollama') {
            if (OSA.ollamaModelDebounce) clearTimeout(OSA.ollamaModelDebounce);
            OSA.ollamaModelDebounce = setTimeout(function() {
                OSA.refreshModalProviderModels(provider, connectedEntry);
            }, 350);
        }
    };

    modal.classList.remove('hidden');
};

// ── API Key Validation ──────────────────────────────────────

OSA.scheduleValidation = function() {
    if (OSA.validationDebounce) clearTimeout(OSA.validationDebounce);
    const apiKey = document.getElementById('provider-api-key').value;
    if (apiKey.length < 5) { document.getElementById('validation-badge').style.display = 'none'; return; }
    OSA.validationDebounce = setTimeout(function() { OSA.validateApiKey(); }, 500);
};

OSA.validateApiKey = async function() {
    const apiKey = document.getElementById('provider-api-key').value;
    const baseUrl = document.getElementById('provider-base-url').value;
    const badge = document.getElementById('validation-badge');

    if (!apiKey) { badge.style.display = 'none'; return; }

    badge.style.display = 'inline-flex';
    badge.className = 'validation-badge validating';
    badge.innerHTML = OSA.icons.spinner + ' Checking...';

    try {
        const res = await fetch('/api/providers/validate', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ provider_id: OSA.currentProviderId, api_key: apiKey, base_url: baseUrl || undefined })
        });
        const data = await res.json();

        if (data.valid) {
            badge.className = 'validation-badge valid';
            badge.innerHTML = OSA.icons.check + ' Valid';
        } else {
            badge.className = 'validation-badge invalid';
            badge.innerHTML = OSA.icons.xmark + ' ' + OSA.escapeHtml(data.error || 'Invalid');
        }
    } catch (e) {
        badge.className = 'validation-badge invalid';
        badge.innerHTML = OSA.icons.xmark + ' Connection failed';
    }
};

// ── OAuth Functions ─────────────────────────────────────────

OSA.oauthProviders = null;
OSA.currentOAuthFlow = null;
OSA.deviceCodePollTimer = null;

OSA.loadOAuthProviders = async function() {
    if (OSA.oauthProviders) return OSA.oauthProviders;
    try {
        const res = await OSA.getJson('/api/oauth/providers');
        OSA.oauthProviders = res.providers || [];
    } catch (e) {
        OSA.oauthProviders = [];
    }
    return OSA.oauthProviders;
};

OSA.isOAuthProvider = function(providerId) {
    if (!OSA.oauthProviders) return false;
    return OSA.oauthProviders.some(p => p.id === providerId);
};

OSA.getProviderFlowType = function(providerId) {
    if (!OSA.oauthProviders) return null;
    const provider = OSA.oauthProviders.find(p => p.id === providerId);
    return provider ? provider.flow_type : null;
};

OSA.showOAuthView = function(view) {
    document.getElementById('oauth-connected-view').classList.add('hidden');
    document.getElementById('oauth-pkce-view').classList.add('hidden');
    document.getElementById('oauth-device-view').classList.add('hidden');
    document.getElementById('oauth-loading-view').classList.add('hidden');
    if (view) {
        document.getElementById('oauth-' + view + '-view').classList.remove('hidden');
    }
};

OSA.updateOAuthUI = async function(providerId) {
    const pid = providerId || OSA.currentProviderId;
    if (!pid) return;

    await OSA.loadOAuthProviders();
    const flowType = OSA.getProviderFlowType(pid);

    try {
        const status = await OSA.getJson('/api/oauth/' + pid + '/status');
        if (status.status === 'active' || status.configured) {
            document.getElementById('oauth-connected-account').textContent = status.account || 'Account connected';
            OSA.showOAuthView('connected');
            return;
        }
    } catch (e) {}

    if (flowType === 'device_code') {
        OSA.showOAuthView('device');
    } else {
        OSA.showOAuthView('pkce');
        const provider = OSA.oauthProviders.find(p => p.id === pid);
        document.getElementById('oauth-provider-name-pkce').textContent = provider ? provider.name : pid;
        document.getElementById('oauth-connect-text').textContent = 'Connect with ' + (provider ? provider.name : 'Provider');
    }
};

OSA.initiateOAuth = async function() {
    if (!OSA.currentProviderId) return;

    const loadingView = document.getElementById('oauth-loading-view');
    const pkceView = document.getElementById('oauth-pkce-view');
    const errorEl = document.getElementById('oauth-error-pkce');

    OSA.showOAuthView('loading');
    document.getElementById('oauth-loading-text').textContent = 'Starting OAuth...';

    // Open the popup window NOW while we still have the user gesture context.
    // Browsers block window.open() called after an await (async operation loses
    // the user gesture, causing the popup to open as about:blank or be blocked).
    const oauthWindow = window.open('', '_blank', 'width=600,height=700');

    try {
        const res = await fetch('/api/oauth/' + OSA.currentProviderId + '/start', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({})
        });
        const data = await res.json();

        if (data.error || !data.success) {
            if (oauthWindow) oauthWindow.close();
            throw new Error(data.error || 'Failed to start OAuth');
        }

        if (data.flow_type === 'pkce') {
            OSA.currentOAuthFlow = {
                type: 'pkce',
                codeVerifier: data.code_verifier,
                state: data.state
            };

            // Navigate the already-open window to the auth URL
            if (oauthWindow) {
                oauthWindow.location.href = data.auth_url;
                oauthWindow.focus();

                const pollTimer = setInterval(function() {
                    try {
                        if (oauthWindow.closed) {
                            clearInterval(pollTimer);
                            OSA.checkOAuthCallback(OSA.currentProviderId);
                        }
                    } catch (e) {}
                }, 500);

                window.oauthCallback = function(params) {
                    clearInterval(pollTimer);
                    if (oauthWindow && !oauthWindow.closed) oauthWindow.close();
                    delete window.oauthCallback;
                    if (params && params.success) {
                        OSA.onOAuthSuccess(OSA.currentProviderId);
                    } else if (params && params.error) {
                        OSA.showOAuthView('pkce');
                        errorEl.textContent = params.error;
                        errorEl.classList.remove('hidden');
                    }
                    OSA.currentOAuthFlow = null;
                };
            } else {
                OSA.showOAuthView('pkce');
                errorEl.textContent = 'Please allow popups for OAuth';
                errorEl.classList.remove('hidden');
            }
        } else if (data.flow_type === 'device_code') {
            OSA.currentOAuthFlow = {
                type: 'device_code',
                deviceCode: data.device_code,
                interval: (data.interval || 5) * 1000
            };

            document.getElementById('oauth-device-code-display').textContent = data.user_code;
            document.getElementById('oauth-device-link').href = data.verification_uri;
            document.getElementById('oauth-device-link').textContent = 'Open ' + (data.verification_uri || 'verification page');
            document.getElementById('oauth-device-status-text').textContent = 'Waiting for authorization...';
            OSA.showOAuthView('device');
            OSA.pollDeviceCode();
        }
    } catch (error) {
        if (oauthWindow && !oauthWindow.closed) oauthWindow.close();
        OSA.showOAuthView('pkce');
        errorEl.textContent = error.message;
        errorEl.classList.remove('hidden');
    }
};

OSA.pollDeviceCode = function() {
    if (!OSA.currentOAuthFlow || OSA.currentOAuthFlow.type !== 'device_code') return;

    const deviceCode = OSA.currentOAuthFlow.deviceCode;
    const interval = OSA.currentOAuthFlow.interval;

    OSA.deviceCodePollTimer = setTimeout(async function() {
        try {
            const res = await fetch('/api/oauth/' + OSA.currentProviderId + '/device', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ device_code: deviceCode })
            });
            const data = await res.json();

            if (data.success && data.connected) {
                OSA.onOAuthSuccess(OSA.currentProviderId);
                return;
            }

            if (data.pending) {
                document.getElementById('oauth-device-status-text').textContent = 'Waiting for authorization...';
                OSA.pollDeviceCode();
            } else if (data.error) {
                document.getElementById('oauth-device-status-text').textContent = 'Error: ' + data.error;
            }
        } catch (error) {
            document.getElementById('oauth-device-status-text').textContent = 'Error: ' + error.message;
        }
    }, interval);
};

OSA.cancelDeviceCode = function() {
    if (OSA.deviceCodePollTimer) {
        clearTimeout(OSA.deviceCodePollTimer);
        OSA.deviceCodePollTimer = null;
    }
    OSA.currentOAuthFlow = null;
    OSA.showOAuthView('pkce');
};

OSA.checkOAuthCallback = async function(providerId) {
    try {
        const status = await OSA.getJson('/api/oauth/' + providerId + '/status');
        if (status.status === 'active' || status.configured) {
            OSA.onOAuthSuccess(providerId);
        } else {
            OSA.updateOAuthUI(providerId);
        }
    } catch (error) {
        OSA.updateOAuthUI(providerId);
    }
};

OSA.onOAuthSuccess = async function(providerId) {
    if (OSA.deviceCodePollTimer) {
        clearTimeout(OSA.deviceCodePollTimer);
        OSA.deviceCodePollTimer = null;
    }
    OSA.currentOAuthFlow = null;

    OSA.updateOAuthUI(providerId);

    try {
        const res = await fetch('/api/providers', {
            method: 'POST',
            headers: { 'Authorization': 'Bearer ' + OSA.getToken(), 'Content-Type': 'application/json' },
            body: JSON.stringify({ provider_id: providerId, is_default: document.getElementById('provider-default').checked })
        });
        if (res.ok) {
            OSA.closeAddProviderModal();
            OSA.renderSettingsProviders();
        }
    } catch (error) {
        console.error('OAuth success handler error:', error);
    }
};

OSA.refreshOAuthToken = async function() {
    if (!OSA.currentProviderId) return;
    const refreshBtn = document.getElementById('oauth-refresh-btn');
    refreshBtn.disabled = true;
    refreshBtn.textContent = 'Refreshing...';
    try {
        const res = await fetch('/api/oauth/' + OSA.currentProviderId + '/refresh', {
            method: 'POST',
            headers: { 'Authorization': 'Bearer ' + OSA.getToken() }
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'Failed to refresh token');
        OSA.updateOAuthUI(OSA.currentProviderId);
        OSA.renderSettingsProviders();
    } catch (error) {
        alert('Failed to refresh token: ' + error.message);
    } finally {
        refreshBtn.disabled = false;
        refreshBtn.innerHTML = '<svg class="icon-inline" viewBox="0 0 16 16" width="14" height="14"><path fill="currentColor" d="M8 3a5 5 0 1 0 4.546 2.914.5.5 0 0 1 .908-.417A6 6 0 1 1 8 2v1z"/><path fill="currentColor" d="M8 4.466V.534a.25.25 0 0 1 .41-.192l2.36 1.966c.12.1.12.284 0 .384L8.41 4.658A.25.25 0 0 1 8 4.466z"/></svg> Refresh';
    }
};

OSA.disconnectOAuth = async function() {
    if (!OSA.currentProviderId) return;
    if (!confirm('Disconnect this OAuth provider? This will remove the provider and revoke the OAuth token.')) return;
    try {
        const res = await fetch('/api/oauth/' + OSA.currentProviderId, {
            method: 'DELETE',
            headers: { 'Authorization': 'Bearer ' + OSA.getToken() }
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'Failed to disconnect');
        OSA.oauthProviders = null;
        await OSA.updateOAuthUI(OSA.currentProviderId);
        OSA.renderSettingsProviders();
    } catch (error) {
        alert('Failed to disconnect: ' + error.message);
    }
};

// ── Add Provider ────────────────────────────────────────────

OSA.addProvider = async function() {
    if (!OSA.currentProviderId) return;

    const apiKey = document.getElementById('provider-api-key').value;
    const baseUrl = document.getElementById('provider-base-url').value;
    const model = document.getElementById('provider-model').value;
    const isDefault = document.getElementById('provider-default').checked;
    const addBtn = document.getElementById('add-provider-btn');

    // If OAuth is active, save config
    if (OSA.currentProviderOAuthSupported) {
        const connectedView = document.getElementById('oauth-connected-view');
        if (connectedView && !connectedView.classList.contains('hidden')) {
            addBtn.disabled = true;
            addBtn.textContent = 'Saving...';
            try {
                const res = await fetch('/api/providers', {
                    method: 'POST',
                    headers: { 'Authorization': 'Bearer ' + OSA.getToken(), 'Content-Type': 'application/json' },
                    body: JSON.stringify({ provider_id: OSA.currentProviderId, model: model || undefined, is_default: isDefault })
                });
                const data = await res.json();
                if (!res.ok) throw new Error(data.error || 'Failed to add provider');
                OSA.closeAddProviderModal();
                OSA.renderSettingsProviders();
            } catch (error) {
                alert('Failed to add provider: ' + error.message);
            } finally {
                addBtn.disabled = false;
                addBtn.textContent = 'Save Changes';
            }
            return;
        }
    }

    if (!apiKey && OSA.currentProviderId !== 'ollama') { alert('Please enter an API key'); return; }

    addBtn.disabled = true;
    addBtn.textContent = 'Adding...';

    try {
        const res = await fetch('/api/providers', {
            method: 'POST',
            headers: { 'Authorization': 'Bearer ' + OSA.getToken(), 'Content-Type': 'application/json' },
            body: JSON.stringify({
                provider_id: OSA.currentProviderId,
                api_key: apiKey,
                base_url: baseUrl || undefined,
                model: model || undefined,
                is_default: isDefault
            })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'Failed to add provider');
        OSA.closeAddProviderModal();
        OSA.renderSettingsProviders();
    } catch (error) {
        alert('Failed to add provider: ' + error.message);
    } finally {
        addBtn.disabled = false;
        addBtn.textContent = OSA.currentProviderOAuthSupported ? 'Save Changes' : 'Add Provider';
    }
};

// ── Modal Close ─────────────────────────────────────────────

OSA.closeAddProviderModal = function() {
    const modal = document.getElementById('add-provider-modal');
    if (modal) modal.classList.add('hidden');
    OSA.currentProviderId = null;
    OSA.currentProviderOAuthSupported = false;
    OSA.currentProviderApiKeyUrl = null;
    OSA.pendingProviderModel = null;
    OSA.modalProviderModels = [];
    if (OSA.ollamaModelDebounce) {
        clearTimeout(OSA.ollamaModelDebounce);
        OSA.ollamaModelDebounce = null;
    }
    const apiKeyInput = document.getElementById('provider-api-key');
    const baseUrlInput = document.getElementById('provider-base-url');
    const modelInput = document.getElementById('provider-model');
    const modelSelectText = document.getElementById('modal-model-select-text');
    const defaultProviderCheckbox = document.getElementById('provider-default');
    const validationBadge = document.getElementById('validation-badge');
    const oauthClientIdInput = document.getElementById('oauth-client-id');
    const oauthCardError = document.getElementById('oauth-card-error');

    if (apiKeyInput) apiKeyInput.value = '';
    if (baseUrlInput) baseUrlInput.value = '';
    if (modelInput) modelInput.value = '';
    if (modelSelectText) modelSelectText.textContent = 'Use provider default (choose later)';
    if (defaultProviderCheckbox) defaultProviderCheckbox.checked = false;
    if (validationBadge) validationBadge.style.display = 'none';
    if (oauthClientIdInput) oauthClientIdInput.value = '';
    if (oauthCardError) oauthCardError.classList.add('hidden');
    OSA.closeModalModelDropdown();
};

// ── Document click handler ──────────────────────────────────

document.addEventListener('click', function(event) {
    if (OSA.modelDropdownOpen && !event.target.closest('#model-dropdown') && !event.target.closest('.model-selector')) {
        OSA.closeModelDropdown();
    }
    if (OSA.modalModelDropdownOpen && !event.target.closest('#modal-model-select-wrapper')) {
        OSA.closeModalModelDropdown();
    }
});

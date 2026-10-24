window.OSA = window.OSA || {};

OSA.providerCatalog = { providers: [], all_models: [] };
OSA.modelDropdownOpen = false;
OSA.modelSearchQuery = '';
OSA.currentProviderId = null;
OSA.currentProviderApiKeyUrl = null;
OSA.currentProviderOAuthSupported = false;
OSA.pendingProviderModel = null;
OSA.validationDebounce = null;
OSA.currentProviderConnected = false;
OSA.modalModelDropdownOpen = false;
OSA.modalModelSearchQuery = '';
OSA.modalProviderModels = [];
OSA.modalModelSearchDebounce = null;
OSA.ollamaModelDebounce = null;
OSA.providerCatalogPromise = null;
OSA.modelPickerActiveIndex = -1;
OSA.modelSearchRequestId = 0;
OSA.modelOptionDelegationReady = false;
OSA.modelDropdownRenderCache = new Map();
OSA.modelDropdownCatalogIds = new WeakMap();
OSA.nextModelDropdownCatalogId = 1;

OSA.clearModelDropdownRenderCache = function() {
    OSA.modelDropdownRenderCache.clear();
};

OSA.modelDropdownCatalogKey = function() {
    const catalog = OSA.providerCatalog || {};
    let catalogId = 'none';
    if (catalog && typeof catalog === 'object') {
        catalogId = OSA.modelDropdownCatalogIds.get(catalog);
        if (!catalogId) {
            catalogId = String(OSA.nextModelDropdownCatalogId++);
            OSA.modelDropdownCatalogIds.set(catalog, catalogId);
        }
    }
    const favourites = (OSA.favourites || [])
        .map(function(item) { return (item.id || '') + '|' + (item.provider_id || ''); })
        .join(',');
    return [
        catalogId,
        OSA.providerCatalogFetchedAt || 0,
        OSA.currentModelProviderId || '',
        OSA.currentModelId || '',
        favourites,
        OSA.modelMenuSignature(),
    ].join('#');
};

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
    if (OSA.modelDropdownOpen) {
        // The list re-renders in place, so keep the user's scroll position.
        const list = document.querySelector('#model-dropdown .model-dropdown-list');
        OSA._pendingModelListScroll = list ? list.scrollTop : null;
        OSA.renderModelDropdown();
    }
};

// ── Model Option HTML Builder ────────────────────────────────

OSA.buildModelOptionHtml = function(m, providerId, currentModel, opts) {
    const pid = m.provider_id || providerId;
    // The composer marks the active route model, a settings picker marks the
    // value of its own field, so the provider can be overridden per caller.
    const currentProviderId = (opts && opts.currentProviderId !== undefined)
        ? opts.currentProviderId
        : OSA.currentModelProviderId;
    const isCurrent = m.id.toLowerCase() === (currentModel || '').toLowerCase()
        && (!currentProviderId || currentProviderId === pid);
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
    const attr = function(value) {
        return String(value || '')
            .replace(/&/g, '&amp;')
            .replace(/"/g, '&quot;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;');
    };

    const providerTag = (opts && opts.showProvider && (m.provider_name || ''))
        ? '<span class="model-option-provider-tag">' + safeProvName + '</span>'
        : '';

    const favBtn = '<button type="button" class="model-fav-btn' + (isFav ? ' is-fav' : '') + '" ' +
        'title="' + (isFav ? 'Remove from favourites' : 'Add to favourites') + '" ' +
        'data-model-action="favourite">' +
        (isFav ? '★' : '☆') +
        '</button>';

    return '<div class="model-option' + (isCurrent ? ' active' : '') + '" role="option" tabindex="-1" ' +
        'aria-selected="' + (isCurrent ? 'true' : 'false') + '" ' +
        'data-model-id="' + attr(m.id) + '" data-provider-id="' + attr(pid) + '" ' +
        'data-model-name="' + attr(m.name) + '" data-provider-name="' + attr(m.provider_name || '') + '" ' +
        'data-close-settings="' + ((opts && opts.inSettings) ? 'true' : 'false') + '">' +
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

// /api/providers/catalog sends each model once, in all_models. Settings and the
// provider cards still read provider.models, so rebuild those lists here.
OSA.hydrateProviderModels = function(catalog) {
    const byProvider = {};
    const models = catalog.all_models || [];
    for (let i = 0; i < models.length; i++) {
        const m = models[i];
        if (!byProvider[m.provider_id]) byProvider[m.provider_id] = [];
        byProvider[m.provider_id].push(m);
    }
    const providers = catalog.providers || [];
    for (let i = 0; i < providers.length; i++) {
        const p = providers[i];
        if (!p.models || p.models.length === 0) p.models = byProvider[p.id] || [];
    }
    return catalog;
};

OSA.loadProviderCatalog = async function(force = false) {
    if (!force && OSA.providerCatalog?.providers?.length) {
        return OSA.providerCatalog;
    }

    if (!force && OSA.providerCatalogPromise) {
        return OSA.providerCatalogPromise;
    }

    OSA.providerCatalogPromise = OSA.getJson('/api/providers/catalog')
        .then(function(data) {
            OSA.providerCatalog = OSA.hydrateProviderModels(data || { providers: [], all_models: [] });
            OSA.providerCatalogFetchedAt = Date.now();
            OSA.clearModelDropdownRenderCache();
            if (OSA.currentModelId) {
                OSA.setModelTrigger(OSA.currentModelProviderId || '', OSA.currentModelId);
            }
            return OSA.providerCatalog;
        })
        .catch(function(error) {
            console.error('Failed to load provider catalog:', error);
            return OSA.providerCatalog;
        })
        .finally(function() {
            OSA.providerCatalogPromise = null;
        });

    return OSA.providerCatalogPromise;
};

OSA.CATALOG_REFRESH_MS = 60000;

/// Fill a model picker select from the provider catalog. Options are
/// "provider_id:model" values; an empty option means "same as current
/// model". A stored value that is not in the catalog stays selectable.
/// Only connected providers are listed: the full catalog covers every
/// provider on models.dev (~9k models), and building that many options
/// locks the main thread for a long time on every Settings open.
OSA.populateModelSelect = async function(select, currentValue) {
    if (!select) return;
    const catalog = await OSA.loadProviderCatalog();
    const preferred = (currentValue || '').trim();
    // Rebuilding is skipped when neither the catalog nor the stored value
    // changed; selects keep their options for the lifetime of the pane.
    const signature = OSA.catalogSignature(catalog) + '|' + preferred;
    if (select.dataset.catalogSig === signature && select.options.length > 0) {
        select.value = preferred;
        return;
    }
    const providers = (catalog && catalog.providers) || [];
    const models = (catalog && catalog.all_models) || [];
    const nameById = {};
    const connected = {};
    for (let i = 0; i < providers.length; i++) {
        nameById[providers[i].id] = providers[i].name || providers[i].id;
        if (providers[i].connected) connected[providers[i].id] = true;
    }
    const fragment = document.createDocumentFragment();
    const defaultOption = document.createElement('option');
    defaultOption.value = '';
    defaultOption.textContent = 'Same as current model';
    fragment.appendChild(defaultOption);
    let hasPreferred = preferred === '';
    for (let i = 0; i < models.length; i++) {
        const model = models[i];
        if (!connected[model.provider_id]) continue;
        const value = model.provider_id + ':' + model.id;
        const option = document.createElement('option');
        option.value = value;
        option.textContent = (nameById[model.provider_id] || model.provider_id) + ' · ' + (model.name || model.id);
        if (value === preferred) hasPreferred = true;
        fragment.appendChild(option);
    }
    if (!hasPreferred) {
        const option = document.createElement('option');
        option.value = preferred;
        option.textContent = preferred;
        fragment.appendChild(option);
    }
    select.replaceChildren(fragment);
    select.dataset.catalogSig = signature;
    select.value = preferred;
};

// The server answers from its build-time models.dev snapshot until the live
// fetch lands, so the catalog pulled at page load can be a release behind.
// Re-pull it in the background while the user reads the list.
OSA.catalogSignature = function(catalog) {
    const providers = (catalog && catalog.providers) || [];
    const connected = providers.filter(function(p) { return p.connected; })
        .map(function(p) { return p.id; }).join(',');
    return connected + '#' + ((catalog && catalog.all_models) || []).length;
};

OSA.refreshProviderCatalogInBackground = function() {
    if (OSA.providerCatalogFetchedAt
        && Date.now() - OSA.providerCatalogFetchedAt < OSA.CATALOG_REFRESH_MS) {
        return;
    }
    const before = OSA.catalogSignature(OSA.providerCatalog);
    OSA.loadProviderCatalog(true).then(function(catalog) {
        // Almost always identical. Re-rendering regardless meant a second full
        // rebuild of the list moments after opening it.
        if (OSA.catalogSignature(catalog) === before) return;
        if (OSA.modelDropdownOpen && !OSA.modelSearchQuery.trim()) OSA.renderModelDropdown();
    });
};

OSA.sortProvidersForDropdown = function(providers) {
    const providerOrder = ['OpenRouter', 'OpenAI', 'Anthropic', 'Google AI', 'Groq', 'DeepSeek', 'xAI', 'Ollama (Local)', 'Unsloth (Local)'];
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


// ── Composer Model Dropdown ─────────────────────────────────

OSA.setModelTrigger = function(providerId, modelId) {
    const trigger = document.getElementById('model-input');
    const label = document.getElementById('model-input-label');
    if (!trigger || !label) return;
    const provider = (OSA.providerCatalog.providers || []).find(function(item) { return item.id === providerId; });
    const text = provider ? provider.name + ' · ' + (modelId || '') : (modelId || 'Select model');
    label.textContent = text;
    trigger.dataset.modelId = modelId || '';
    trigger.dataset.providerId = providerId || '';
    trigger.title = provider ? provider.name + ' / ' + (modelId || '') : (modelId || 'Browse models');
};

// Anchor a model popover to its trigger. The popover is positioned with
// `position: fixed`, so it can live on the body and escape any scrolling
// container it is shown inside. Shared by the composer dropdown and the
// settings model pickers.
OSA.positionModelPopover = function(popover, trigger, options) {
    if (!popover || !trigger) return;
    const opts = options || {};

    const margin = 8;
    const gap = 6;
    const triggerRect = trigger.getBoundingClientRect();
    const viewport = window.matchMedia?.('(max-width: 900px)').matches ? window.visualViewport : null;
    const viewportLeft = viewport?.offsetLeft || 0;
    const viewportTop = viewport?.offsetTop || 0;
    const viewportWidth = viewport?.width || window.innerWidth;
    const viewportBottom = viewportTop + (viewport?.height || window.innerHeight);
    const width = opts.width
        ? Math.min(opts.width, viewportWidth - margin * 2)
        : Math.min(opts.maxWidth || 380, viewportWidth - margin * 2);
    const availableBelow = viewportBottom - triggerRect.bottom - gap - margin;
    const availableAbove = triggerRect.top - viewportTop - gap - margin;
    const openBelow = availableBelow >= 240 || availableBelow >= availableAbove;
    const availableHeight = Math.max(80, openBelow ? availableBelow : availableAbove);
    const left = Math.min(
        Math.max(viewportLeft + margin, triggerRect.right - width),
        Math.max(viewportLeft + margin, viewportLeft + viewportWidth - width - margin)
    );

    popover.style.width = width + 'px';
    popover.style.left = left + 'px';
    popover.style.right = 'auto';
    popover.style.maxHeight = Math.min(opts.maxHeight || 520, availableHeight) + 'px';
    if (openBelow) {
        popover.style.top = (triggerRect.bottom + gap) + 'px';
        popover.style.bottom = 'auto';
    } else {
        popover.style.top = 'auto';
        popover.style.bottom = (window.innerHeight - triggerRect.top + gap) + 'px';
    }
};

OSA.positionModelDropdown = function() {
    const dropdown = document.getElementById('model-dropdown');
    const trigger = document.getElementById('model-input');
    if (!dropdown || !trigger || !OSA.modelDropdownOpen) return;
    OSA.positionModelPopover(dropdown, trigger);
};

// ── Composer dropdown instance ──────────────────────────────
//
// The chat bar and the Settings model pickers share one dropdown component
// (model-dropdown.js). The composer keeps its state in the OSA globals, so
// this adapter maps the component's state onto them and the rest of the page
// keeps reading the same values.
OSA.composerDropdownState = {
    get open() { return OSA.modelDropdownOpen; },
    set open(value) { OSA.modelDropdownOpen = value; },
    get searchQuery() { return OSA.modelSearchQuery; },
    set searchQuery(value) { OSA.modelSearchQuery = value; },
    get searchDebounce() { return OSA.modelSearchDebounce; },
    set searchDebounce(value) { OSA.modelSearchDebounce = value; },
    get searchRequestId() { return OSA.modelSearchRequestId; },
    set searchRequestId(value) { OSA.modelSearchRequestId = value; },
    get optionNodes() { return OSA.modelOptionNodes; },
    set optionNodes(value) { OSA.modelOptionNodes = value; },
    get activeIndex() { return OSA.modelPickerActiveIndex; },
    set activeIndex(value) { OSA.modelPickerActiveIndex = value; },
};

OSA.composerModelDropdown = new OSA.ModelDropdown({
    triggerId: 'model-input',
    dropdownId: 'model-dropdown',
    searchId: 'model-search',
    listSelector: '#model-dropdown .model-dropdown-list',
    state: OSA.composerDropdownState,
    // The composer wires its own handlers through the inline attributes in
    // index.html, so the component must not attach them a second time.
    autoBind: false,
    // providers.js already closes the composer dropdown on an outside click.
    closeOnOutsideClick: false,
    // The composer keeps its own active-row bookkeeping.
    manageActiveIndex: false,
    renderList: function() {
        return OSA.renderModelDropdown();
    },
    onOpen: function() {
        OSA.ensureModelOptionDelegation();
        OSA.modelPickerActiveIndex = -1;
        // Reset the list's cache key rather than overwriting its markup: the
        // render below restores the cached list, and a stale key used to skip
        // that restore and leave the "Loading" placeholder on screen.
        const list = document.querySelector('#model-dropdown .model-dropdown-list');
        if (list) list._osaModelDropdownCacheKey = '';
        OSA.refreshProviderCatalogInBackground();
    },
});
// Binds the viewport listeners, so the dropdown follows a scroll or a
// resize. The element handlers stay with the inline attributes in index.html.
OSA.composerModelDropdown.bind();

OSA.openModelDropdown = function(initialQuery) {
    OSA.composerModelDropdown.open(initialQuery);
};

OSA.toggleModelDropdown = function() {
    OSA.composerModelDropdown.toggle();
};

OSA.closeModelDropdown = function(restoreFocus = true) {
    OSA.composerModelDropdown.close(restoreFocus);
};

// Global aliases for inline onclick handlers in HTML
window.toggleModelDropdown = function() { OSA.toggleModelDropdown(); };
window.closeModelDropdown = function() { OSA.closeModelDropdown(); };
window.handleModelSearch = function(e) { OSA.handleModelSearch(e); };
window.handleModelDropdownKeydown = function(e) { OSA.handleModelDropdownKeydown(e); };
window.handleModelInputKeydown = function(e) { OSA.handleModelInputKeydown(e); };

OSA.handleModelInputKeydown = function(event) {
    OSA.composerModelDropdown.handleTriggerKeydown(event);
};

OSA.ensureModelOptionDelegation = function() {
    if (OSA.modelOptionDelegationReady) return;
    OSA.modelOptionDelegationReady = true;
    document.addEventListener('click', function(event) {
        // Scoped to the composer dropdown: the Settings model pickers render
        // the same rows, but a pick there records a value instead of
        // switching the live route.
        const option = event.target.closest('#model-dropdown .model-option[data-model-id]');
        if (!option) return;
        const modelId = option.dataset.modelId || '';
        const providerId = option.dataset.providerId || '';
        if (event.target.closest('[data-model-action="favourite"]')) {
            event.preventDefault();
            event.stopPropagation();
            OSA.toggleFavourite(
                modelId,
                providerId,
                option.dataset.modelName || modelId,
                option.dataset.providerName || ''
            );
            return;
        }
        OSA.selectModel(modelId, providerId);
        if (option.dataset.closeSettings === 'true') OSA.closeSettings();
    });
};
OSA.ensureModelOptionDelegation();

// Cached at render time. Re-querying per keypress meant walking every option
// in the list on each arrow key.
OSA.modelOptionNodes = [];

// Kept as the public alias for the highlighted row. The shared component owns
// the implementation, and its state adapter writes the same globals, so the
// composer keeps one source of truth for the active option.
OSA.setActiveModelOption = function(index) {
    OSA.composerModelDropdown.setActiveIndex(index);
};

OSA.afterModelDropdownRender = function() {
    if (!OSA.modelDropdownOpen) {
        OSA.modelOptionNodes = [];
        return;
    }
    const dropdown = document.getElementById('model-dropdown');
    if (!dropdown) return;
    OSA.modelOptionNodes = Array.from(dropdown.querySelectorAll('.model-option[data-model-id]'));
    OSA.modelPickerActiveIndex = OSA.modelOptionNodes.findIndex(function(option) {
        return option.classList.contains('active');
    });
    const list = dropdown.querySelector('.model-dropdown-list');
    if (list && OSA._pendingModelListScroll != null) {
        list.scrollTop = OSA._pendingModelListScroll;
        OSA._pendingModelListScroll = null;
    }
    OSA.positionModelDropdown();
};

OSA.renderModelDropdown = async function() {
    const dropdown = document.getElementById('model-dropdown');
    if (!dropdown) return;

    const query = OSA.modelSearchQuery.trim();
    const requestId = ++OSA.modelSearchRequestId;
    const currentModel = (OSA.currentModelId || document.getElementById('model-input')?.dataset.modelId || '').toLowerCase();
    // The model menu applies to browse and search alike. The active model and
    // favourites are never hidden (see OSA.modelShownInMenu).
    const hidden = OSA.modelMenuFilter();

    if (query.length >= 1) {
        try {
            const models = await OSA.getJson(`/api/providers/search?q=${encodeURIComponent(query)}`);
            if (!OSA.modelDropdownOpen || requestId !== OSA.modelSearchRequestId || query !== OSA.modelSearchQuery.trim()) return;
            OSA.renderModelSearchResults((models || []).filter(function(m) { return OSA.modelShownInMenu(hidden, m.provider_id, m); }), currentModel);
        } catch (e) {
            if (!OSA.modelDropdownOpen || requestId !== OSA.modelSearchRequestId) return;
            dropdown.querySelector('.model-dropdown-list').innerHTML = '<div class="model-empty">Search failed</div>';
        }
        return;
    }

    const cacheKey = OSA.modelDropdownCatalogKey();
    const cached = OSA.modelDropdownRenderCache.get(cacheKey);
    const list = dropdown.querySelector('.model-dropdown-list');
    if (cached && list) {
        // The dropdown remains mounted while closed. On a normal reopen this
        // avoids reparsing hundreds of option nodes and preserves focus state.
        if (list._osaModelDropdownCacheKey !== cacheKey) {
            list.innerHTML = cached.html;
            list._osaModelDropdownCacheKey = cacheKey;
        }
        OSA.afterModelDropdownRender();
        return;
    }

    if (!OSA.providerCatalog?.providers?.length) {
        if (list) {
            list.innerHTML = '<div class="model-empty">Loading models...</div>';
            list._osaModelDropdownCacheKey = '';
        }
        await OSA.loadProviderCatalog();
        if (!OSA.modelDropdownOpen || requestId !== OSA.modelSearchRequestId) return;
    }

    const { providers, all_models } = OSA.providerCatalog;
    let html = '';
    const renderedModelKeys = new Set();

    if (OSA.currentModelId) {
        const currentProvider = (providers || []).find(function(p) { return p.id === OSA.currentModelProviderId; });
        const currentFromAll = (all_models || []).find(function(m) {
            return m.id.toLowerCase() === OSA.currentModelId.toLowerCase()
                && (!OSA.currentModelProviderId || m.provider_id === OSA.currentModelProviderId);
        });
        if (currentFromAll) {
            html += '<div class="model-group-title" style="color:var(--accent)">Current</div>';
            html += OSA.buildModelOptionHtml(currentFromAll, currentFromAll.provider_id, currentModel, { showProvider: true });
            renderedModelKeys.add(currentFromAll.id + '|' + currentFromAll.provider_id);
        } else if (currentProvider) {
            html += '<div class="model-group-title" style="color:var(--accent)">Current</div>';
            html += OSA.buildModelOptionHtml({
                id: OSA.currentModelId,
                name: OSA.currentModelId,
                provider_id: OSA.currentModelProviderId || currentProvider.id,
                provider_name: currentProvider.name,
                context_window: 0,
                supports_tools: false,
                supports_vision: false,
                category: '',
            }, currentProvider.id, currentModel, { showProvider: true });
            renderedModelKeys.add(OSA.currentModelId.toLowerCase() + '|' + OSA.currentModelProviderId);
        }
    }

    // Only the Current entry hides a row from the provider groups below. A
    // favourite appears in its favourites block and again under its provider.
    const currentKeys = new Set(renderedModelKeys);

    if (OSA.favourites.length > 0) {
        html += '<div class="model-group-title model-group-favs">★ Favourites</div>';
        for (const fav of OSA.favourites) {
            const key = fav.id + '|' + fav.provider_id;
            if (renderedModelKeys.has(key.toLowerCase())) continue;
            const full = (all_models || []).find(function(m) { return m.id === fav.id && m.provider_id === fav.provider_id; }) || {
                id: fav.id, name: fav.name, provider_id: fav.provider_id,
                provider_name: fav.provider_name, context_window: 0,
                supports_tools: false, supports_vision: false, category: ''
            };
            html += OSA.buildModelOptionHtml(full, fav.provider_id, currentModel, { showProvider: true });
            renderedModelKeys.add(key.toLowerCase());
        }
    }

    const sortedProviders = OSA.sortProvidersForDropdown(providers);
    const providerRank = new Map(sortedProviders.map(function(provider, index) {
        return [provider.name, index];
    }));

    // Shared with the Settings model pickers so both menus apply the same
    // "show in model menu" filters.
    const connectedMap = Object.fromEntries((providers || []).map(function(p) {
        return [p.id, !!p.connected];
    }));
    const connectedModels = OSA.modelMenuVisibleModels(all_models, { connected: connectedMap });
    const grouped = {};
    for (var i = 0; i < connectedModels.length; i++) {
        var m = connectedModels[i];
        if (!grouped[m.provider_name || m.provider_id]) grouped[m.provider_name || m.provider_id] = [];
        grouped[m.provider_name || m.provider_id].push(m);
    }
    var connectedGroupKeys = Object.keys(grouped).sort(function(a, b) {
        const ai = providerRank.has(a) ? providerRank.get(a) : Number.MAX_SAFE_INTEGER;
        const bi = providerRank.has(b) ? providerRank.get(b) : Number.MAX_SAFE_INTEGER;
        return ai - bi || a.localeCompare(b);
    });
    for (var gi = 0; gi < connectedGroupKeys.length; gi++) {
        var groupKey = connectedGroupKeys[gi];
        var provider = providers.find(function(p) { return p.name === groupKey; });
        html += '<div class="model-group-title">' + OSA.escapeHtml(groupKey) + '</div>';
        var gModels = grouped[groupKey];
        for (var mi = 0; mi < gModels.length; mi++) {
            var gm = gModels[mi];
            var gKey = gm.id + '|' + gm.provider_id;
            if (currentKeys.has(gKey.toLowerCase())) continue;
            html += OSA.buildModelOptionHtml(gm, gm.provider_id, currentModel, {});
            renderedModelKeys.add(gKey.toLowerCase());
        }
    }

    // Models from providers you have not configured are reachable through the
    // search box. Rendering all of them here meant ~6k options and ~56k DOM
    // nodes on every open, for rows nobody scrolls to.
    var unconnectedCount = 0;
    var unconnectedProviders = 0;
    for (var pi = 0; pi < sortedProviders.length; pi++) {
        var provider2 = sortedProviders[pi];
        if (provider2.connected) continue;
        var pCount = (provider2.models || []).filter(function(m) { return OSA.modelShownInMenu(hidden, provider2.id, m); }).length;
        if (pCount === 0) continue;
        unconnectedProviders++;
        unconnectedCount += pCount;
    }
    if (unconnectedCount > 0) {
        html += '<div class="model-dropdown-hint">Type to search '
            + unconnectedCount.toLocaleString() + ' more models across '
            + unconnectedProviders + ' other providers</div>';
    }

    if (!html) html = '<div class="model-empty">No models available</div>';
    list.innerHTML = html;
    list._osaModelDropdownCacheKey = cacheKey;
    OSA.modelDropdownRenderCache.set(cacheKey, { html: html });
    // Keep a few catalog/model combinations warm without retaining old
    // dropdown markup indefinitely.
    while (OSA.modelDropdownRenderCache.size > 6) {
        OSA.modelDropdownRenderCache.delete(OSA.modelDropdownRenderCache.keys().next().value);
    }
    OSA.afterModelDropdownRender();
};

// Group models by provider name and order the groups the way the composer
// dropdown does: connected providers first, then the well-known ones, then
// the rest alphabetically. Shared by the composer dropdown and the settings
// model pickers so both list models in the same order.
OSA.groupModelsByProvider = function(models) {
    const grouped = new Map();
    for (const m of models || []) {
        const key = m.provider_name || m.provider_id;
        if (!grouped.has(key)) grouped.set(key, []);
        grouped.get(key).push(m);
    }

    const providersByName = Object.fromEntries(((OSA.providerCatalog || {}).providers || []).map(function(provider) {
        return [provider.name, provider];
    }));
    const orderedProviders = OSA.sortProvidersForDropdown(
        Array.from(grouped.keys()).map(function(name) {
            const catalogProvider = providersByName[name];
            const providerModels = grouped.get(name) || [];
            // Prefer the catalog's connected flag, but fall back to each model's
            // own `available` flag: search can run before the catalog has
            // loaded, and connected providers must still sort to the top.
            const connected = !!(catalogProvider && catalogProvider.connected)
                || providerModels.some(function(m) { return !!m.available; });
            return { name: name, connected: connected };
        })
    );

    return orderedProviders.map(function(provider) {
        return { name: provider.name, models: grouped.get(provider.name) || [] };
    });
};

OSA.renderModelSearchResults = function(models, currentModel) {
    const dropdown = document.getElementById('model-dropdown');
    if (!dropdown) return;

    if (!models || models.length === 0) {
        const list = dropdown.querySelector('.model-dropdown-list');
        list.innerHTML = '<div class="model-empty">No models found</div>';
        list._osaModelDropdownCacheKey = '';
        OSA.afterModelDropdownRender();
        return;
    }

    let html = '';
    for (const group of OSA.groupModelsByProvider(models)) {
        html += '<div class="model-group-title">' + OSA.escapeHtml(group.name) + '</div>';
        for (const m of group.models) {
            html += OSA.buildModelOptionHtml(m, m.provider_id, currentModel, {});
        }
    }

    const list = dropdown.querySelector('.model-dropdown-list');
    list.innerHTML = html;
    list._osaModelDropdownCacheKey = '';
    OSA.afterModelDropdownRender();
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

    const previousModelId = OSA.currentModelId;
    const previousProviderId = OSA.currentModelProviderId;
    OSA.currentModelId = modelId;
    OSA.currentModelProviderId = providerId;
    OSA.setModelTrigger(providerId, modelId);
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
        if (typeof OSA.renderDiscordActiveModel === 'function') {
            OSA.renderDiscordActiveModel();
        }
    } catch (error) {
        OSA.currentModelId = previousModelId;
        OSA.currentModelProviderId = previousProviderId;
        OSA.setModelTrigger(previousProviderId, previousModelId);
        console.error('Failed to switch model:', error);
        alert(error.message || 'Failed to switch model');
    }
};

OSA.modelSearchDebounce = null;

OSA.handleModelSearch = function(event) {
    OSA.composerModelDropdown.handleSearch(event);
};

OSA.handleModelDropdownKeydown = function(event) {
    OSA.composerModelDropdown.handleKeydown(event);
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
    if (OSA.modalModelSearchDebounce) clearTimeout(OSA.modalModelSearchDebounce);
    OSA.modalModelSearchDebounce = setTimeout(() => OSA.renderModalModelDropdown(), 150);
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

    if (provider.id !== 'ollama' && provider.id !== 'unsloth') {
        OSA.modalProviderModels = provider.models || [];
        OSA.populateModalModelDropdown(OSA.modalProviderModels, provider.id);
        return;
    }

    if (provider.id === 'ollama' && list) list.innerHTML = '<div class="model-empty">Loading installed Ollama models...</div>';
    if (provider.id === 'unsloth' && list) list.innerHTML = '<div class="model-empty">Loading installed Unsloth models...</div>';

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
    if (provider.id === 'unsloth' && OSA.modalProviderModels.length === 0 && list) {
        list.innerHTML = '<div class="model-empty">No Unsloth models available (is Unsloth running on :8888 or :8000?)</div>';
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

    if (provider.id === 'ollama' || provider.id === 'unsloth') {
        const filtered = query
            ? modalModels.filter(function(model) {
                return (model.name || '').toLowerCase().includes(query.toLowerCase())
                    || (model.id || '').toLowerCase().includes(query.toLowerCase());
            })
            : modalModels;
        OSA.populateModalModelDropdown(filtered, provider.id);
        if (filtered.length === 0) {
            const msg = provider.id === 'unsloth'
                ? 'No Unsloth models available (is Unsloth running on :8888 or :8000?)'
                : 'No Ollama models available (is Ollama running?)';
            list.innerHTML = '<div class="model-empty">' + msg + '</div>';
        }
        return;
    }

    if (query.length >= 2) {
        try {
            const models = await OSA.getJson('/api/providers/search?q=' + encodeURIComponent(query));
            if (!models.length) { list.innerHTML = '<div class="model-empty">No models found</div>'; return; }
            let html = '';
            for (const m of models) {
                html += '<div class="model-option" onclick="OSA.selectModalModel(' + OSA.jsArg(m.id) + ', ' + OSA.jsArg(m.name) + ', ' + OSA.jsArg(m.provider_id) + ')">' +
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
            html += '<div class="model-option" onclick="OSA.selectModalModel(' + OSA.jsArg(m.id) + ', ' + OSA.jsArg(m.name) + ', ' + OSA.jsArg(providerId) + ')">' +
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

// ── Settings Models: connected providers, model menu, add provider ───────
// Connected providers are the rows people manage. Each row carries its own
// model-menu switch and a model list that renders on first expand. The catalog
// is cached (loadProviderCatalog), so toggling and expanding never refetch it.

OSA.modelsConnectedMap = OSA.modelsConnectedMap || {};

OSA.modelsCurrentModelId = function() {
    return (OSA.currentModelId || document.getElementById('model-input')?.dataset.modelId || '').toLowerCase();
};

OSA.findCatalogProvider = function(providerId) {
    const providers = (OSA.providerCatalog && OSA.providerCatalog.providers) || [];
    return providers.find(function(p) { return p.id === providerId; }) || null;
};

// ── Model menu visibility ───────────────────────────────────
// Stored in config (PUT /api/providers/menu) and delivered with the catalog.
// The active model and favourites are never hidden, so the menu can never
// strand the user away from the model they are using.

OSA.modelMenu = function() {
    const menu = (OSA.providerCatalog && OSA.providerCatalog.model_menu) || {};
    return {
        hidden_providers: menu.hidden_providers || [],
        hidden_models: menu.hidden_models || [],
        hide_older_than_days: menu.hide_older_than_days || null,
    };
};

// Earliest release date (YYYY-MM-DD) still shown when hiding by age, or '' when off.
OSA.modelMenuCutoff = function(days) {
    if (!days) return '';
    const cutoff = new Date();
    cutoff.setDate(cutoff.getDate() - days);
    return cutoff.toISOString().slice(0, 10);
};

// True when a model's release date is older than the menu's age threshold.
// Models without a known date are never hidden by age.
OSA.modelIsTooOld = function(releaseDate) {
    const cutoff = OSA.modelMenuCutoff(OSA.modelMenu().hide_older_than_days);
    return !!(cutoff && releaseDate && releaseDate < cutoff);
};

OSA.modelMenuKey = function(providerId, modelId) {
    return providerId + ':' + modelId;
};

// Returns a predicate that says whether a model is kept out of the dropdown,
// either by an explicit choice or by the age threshold. Pass no release date
// to test explicit choices only. Built once per render, so a 6k-model catalog
// costs a few Set lookups per model.
OSA.modelMenuFilter = function() {
    const menu = OSA.modelMenu();
    const providers = new Set(menu.hidden_providers);
    const models = new Set(menu.hidden_models);
    const cutoff = OSA.modelMenuCutoff(menu.hide_older_than_days);
    return function(providerId, modelId, releaseDate) {
        if (providers.has(providerId) || models.has(OSA.modelMenuKey(providerId, modelId))) return true;
        return !!(cutoff && releaseDate && releaseDate < cutoff);
    };
};

// Favourites always show, and so does anything the dropdown would not hide.
OSA.modelShownInMenu = function(hidden, providerId, model) {
    return OSA.isFavourite(model.id, providerId) || !hidden(providerId, model.id, model.release_date);
};

// Models the model menu shows, and which the Settings model pickers must show
// too: connected providers only, minus whatever "show in model menu" hides.
// `always` keeps a model visible regardless of the filters — the composer uses
// it for the active model, the Settings pickers for their stored value.
OSA.modelMenuVisibleModels = function(models, options) {
    const opts = options || {};
    const connected = opts.connected || null;
    const always = typeof opts.always === 'function' ? opts.always : function() { return false; };
    const hidden = OSA.modelMenuFilter();
    return (models || []).filter(function(model) {
        if (connected && !connected[model.provider_id]) return false;
        if (always(model)) return true;
        return OSA.modelShownInMenu(hidden, model.provider_id, model);
    });
};

OSA.modelMenuSignature = function() {
    const menu = OSA.modelMenu();
    return menu.hidden_providers.join(',') + '|' + menu.hidden_models.join(',') + '|' + (menu.hide_older_than_days || '');
};

// Optimistic: the UI and the cached catalog change first, then the server
// confirms. A rejected save puts the previous menu back.
// `patch` holds only the fields that change; the rest of the menu carries over.
OSA.saveModelMenu = async function(patch, providerId, rerenderChecks) {
    const previous = OSA.modelMenu();
    const next = Object.assign({}, previous, patch);
    const apply = function(menu) {
        if (OSA.providerCatalog) OSA.providerCatalog.model_menu = menu;
        const cached = OSA.getCachedConfig ? OSA.getCachedConfig() : null;
        if (cached) OSA.setCachedConfig({ ...cached, model_menu: menu });
        OSA.clearModelDropdownRenderCache();
        OSA.syncModelMenuAgeSelect();
    };
    apply(next);
    OSA.refreshProviderRow(providerId, rerenderChecks);
    try {
        const res = await OSA.fetchWithAuth('/api/providers/menu', {
            method: 'PUT',
            body: JSON.stringify(next)
        });
        const data = await res.json().catch(function() { return {}; });
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
    } catch (error) {
        apply(previous);
        OSA.renderConnectedProviders();
        alert('Failed to update the model menu: ' + error.message);
    }
};

OSA.setProviderMenuVisible = function(providerId, visible) {
    const menu = OSA.modelMenu();
    const hidden = menu.hidden_providers.filter(function(id) { return id !== providerId; });
    if (!visible) hidden.push(providerId);
    OSA.saveModelMenu({ hidden_providers: hidden }, providerId, false);
};

OSA.setModelMenuVisible = function(providerId, modelId, visible) {
    const menu = OSA.modelMenu();
    const key = OSA.modelMenuKey(providerId, modelId);
    const hidden = menu.hidden_models.filter(function(k) { return k !== key; });
    if (!visible) hidden.push(key);
    OSA.saveModelMenu({ hidden_models: hidden }, providerId, false);
};

// days: 0 or null turns the age filter off.
OSA.setModelMenuAge = function(days) {
    const value = parseInt(days, 10);
    OSA.saveModelMenu({ hide_older_than_days: value > 0 ? value : null }, null, false);
};

// Bulk Show all / Hide all for one provider's catalog models.
OSA.setProviderModelsVisible = function(providerId, visible) {
    const menu = OSA.modelMenu();
    const keys = new Set(menu.hidden_models);
    const provider = OSA.findCatalogProvider(providerId);
    for (const m of ((provider && provider.models) || [])) {
        const key = OSA.modelMenuKey(providerId, m.id);
        if (visible) keys.delete(key); else keys.add(key);
    }
    OSA.saveModelMenu({ hidden_models: [...keys] }, providerId, true);
};

// ── Connected provider rows ─────────────────────────────────

OSA.providerAuthLabel = function(entry) {
    if (entry.api_key_preview === '(oauth)') return 'OAuth sign-in';
    if (entry.key_count > 1) return entry.key_count + ' API keys, auto-rotated';
    if (entry.key_count === 1) return 'API key saved';
    return 'No API key';
};

OSA.providerModelsSummary = function(providerId) {
    const provider = OSA.findCatalogProvider(providerId);
    const models = (provider && provider.models) || [];
    if (!models.length) return 'No model list';
    const hidden = OSA.modelMenuFilter();
    const shown = models.filter(function(m) { return OSA.modelShownInMenu(hidden, providerId, m); }).length;
    return models.length + ' models · ' + shown + ' in menu';
};

OSA.providerRowHtml = function(provider, entry) {
    const id = entry.id;
    const menuOn = OSA.modelMenu().hidden_providers.indexOf(id) === -1;
    return '<div class="provider-catalog-item provider-row" data-provider-id="' + OSA.escapeAttr(id) + '">'
        + '<div class="provider-catalog-header">'
        +   '<div class="provider-catalog-title">'
        +     '<span class="provider-catalog-name">' + OSA.escapeHtml(provider.name || id) + '</span>'
        +     (entry.is_default ? ' <span class="badge badge-apikey">Default</span>' : '')
        +   '</div>'
        +   '<button type="button" class="btn-ghost provider-row-btn" onclick="OSA.openAddProviderModal(' + OSA.jsArg(id) + ')">Edit</button>'
        + '</div>'
        + '<div class="provider-catalog-meta">'
        +   OSA.escapeHtml(OSA.providerAuthLabel(entry)) + ' · Model: ' + OSA.escapeHtml(entry.model || 'provider default')
        + '</div>'
        + '<div class="provider-row-controls">'
        +   '<label class="toggle-switch" title="Show this provider in the model menu">'
        +     '<input type="checkbox" aria-label="Show ' + OSA.escapeAttr(provider.name || id) + ' in model menu"'
        +       (menuOn ? ' checked' : '') + ' onchange="OSA.setProviderMenuVisible(' + OSA.jsArg(id) + ', this.checked)" />'
        +     '<span class="toggle-slider"></span>'
        +   '</label>'
        +   '<span>Show in model menu</span>'
        + '</div>'
        + '<details class="provider-row-models" data-provider-id="' + OSA.escapeAttr(id) + '" ontoggle="OSA.onProviderModelsToggle(this)">'
        +   '<summary><span class="provider-row-models-count">' + OSA.escapeHtml(OSA.providerModelsSummary(id)) + '</span></summary>'
        +   '<div class="provider-row-models-tools">'
        +     '<button type="button" class="btn-ghost btn-ghost-compact" onclick="OSA.setProviderModelsVisible(' + OSA.jsArg(id) + ', true)">Show all</button>'
        +     '<button type="button" class="btn-ghost btn-ghost-compact" onclick="OSA.setProviderModelsVisible(' + OSA.jsArg(id) + ', false)">Hide all</button>'
        +     '<div class="settings-field provider-row-models-filter"><input type="text" placeholder="Filter models..." autocomplete="off" oninput="OSA.applyProviderModelFilter(this.closest(\'details\'))" /></div>'
        +   '</div>'
        +   '<div class="provider-model-checks"></div>'
        + '</details>'
        + '</div>';
};

OSA.onProviderModelsToggle = function(details) {
    if (!details.open || details.dataset.rendered) return;
    OSA.renderProviderModelChecks(details);
};

OSA.renderProviderModelChecks = function(details) {
    const providerId = details.dataset.providerId;
    const list = details.querySelector('.provider-model-checks');
    if (!list) return;
    details.dataset.rendered = '1';
    const provider = OSA.findCatalogProvider(providerId);
    const models = (provider && provider.models) || [];
    if (!models.length) {
        list.innerHTML = '<div class="model-empty">This provider has not listed any models yet.</div>';
        return;
    }
    const hidden = OSA.modelMenuFilter();
    const current = OSA.modelsCurrentModelId();
    const currentProvider = OSA.currentModelProviderId || providerId;
    list.innerHTML = models.map(function(m) {
        const isCurrent = (m.id || '').toLowerCase() === current && currentProvider === providerId;
        // A model hidden by the age threshold cannot be ticked back on here.
        // Its box stays off and disabled, and the title says why.
        const explicit = hidden(providerId, m.id, null);
        const aged = !explicit && OSA.modelIsTooOld(m.release_date);
        const checked = !explicit && !aged;
        return '<div class="provider-model-check' + (aged ? ' is-aged' : '') + '" data-search="' + OSA.escapeAttr(((m.name || '') + ' ' + m.id).toLowerCase()) + '">'
            + '<input type="checkbox"' + (checked ? ' checked' : '') + (aged ? ' disabled' : '')
            +   ' title="' + (aged ? 'Older than the model menu age limit' : 'Show in model menu') + '"'
            +   ' onchange="OSA.setModelMenuVisible(' + OSA.jsArg(providerId) + ', ' + OSA.jsArg(m.id) + ', this.checked)" />'
            + '<button type="button" class="provider-model-use' + (isCurrent ? ' is-current' : '') + '" title="Use this model"'
            +   ' onclick="OSA.useModelFromSettings(' + OSA.jsArg(providerId) + ', ' + OSA.jsArg(m.id) + ')">'
            +   OSA.escapeHtml(m.name || m.id) + '</button>'
            + '<code>' + OSA.escapeHtml(m.id) + '</code>'
            + '</div>';
    }).join('');
    OSA.applyProviderModelFilter(details);
};

OSA.applyProviderModelFilter = function(details) {
    if (!details) return;
    const input = details.querySelector('.provider-row-models-filter input');
    const query = (input ? input.value : '').trim().toLowerCase();
    details.querySelectorAll('.provider-model-check').forEach(function(row) {
        row.classList.toggle('hidden', !!query && row.dataset.search.indexOf(query) === -1);
    });
};

OSA.useModelFromSettings = function(providerId, modelId) {
    OSA.closeSettings();
    OSA.selectModel(modelId, providerId);
};

// Updates one row in place so an expanded list keeps its scroll position.
OSA.refreshProviderRow = function(providerId, rerenderChecks) {
    // No provider id means the change touched every row (the age threshold).
    const rows = Array.from(document.querySelectorAll('.provider-row'));
    if (providerId == null) {
        rows.forEach(function(row) { OSA.refreshProviderRow(row.dataset.providerId, false); });
        return;
    }
    const row = rows.find(function(node) { return node.dataset.providerId === providerId; });
    if (!row) return;
    const summary = row.querySelector('.provider-row-models-count');
    if (summary) summary.textContent = OSA.providerModelsSummary(providerId);
    const menuToggle = row.querySelector('.provider-row-controls input');
    if (menuToggle) menuToggle.checked = OSA.modelMenu().hidden_providers.indexOf(providerId) === -1;
    const details = row.querySelector('.provider-row-models');
    if (rerenderChecks && details && details.dataset.rendered) OSA.renderProviderModelChecks(details);
};

OSA.connectedProviderListHtml = function(providers, connectedMap) {
    const merged = Object.keys(connectedMap).map(function(id) {
        const catalogProvider = providers.find(function(p) { return p.id === id; });
        return Object.assign({ id: id, name: connectedMap[id].name || id, models: [] }, catalogProvider || {}, { connected: true });
    });
    return OSA.sortProvidersForDropdown(merged).map(function(provider) {
        return OSA.providerRowHtml(provider, connectedMap[provider.id]);
    }).join('');
};

OSA.renderConnectedProviders = function() {
    const list = document.getElementById('connected-provider-list');
    if (!list) return;
    const connectedMap = OSA.modelsConnectedMap || {};
    if (!Object.keys(connectedMap).length) {
        list.innerHTML = '<div class="model-empty">No providers connected yet. Add one to start chatting.</div>';
        return;
    }
    const providers = (OSA.providerCatalog && OSA.providerCatalog.providers) || [];
    list.innerHTML = OSA.connectedProviderListHtml(providers, connectedMap);
};

// ── Add provider list ───────────────────────────────────────

OSA.availableProviderListHtml = function(query) {
    const providers = (OSA.providerCatalog && OSA.providerCatalog.providers) || [];
    const connectedMap = OSA.modelsConnectedMap || {};
    const q = (query || '').trim().toLowerCase();
    const rows = OSA.sortProvidersForDropdown(providers.filter(function(p) { return !connectedMap[p.id]; }))
        .filter(function(p) {
            return !q || [p.id, p.name, p.description || ''].join(' ').toLowerCase().indexOf(q) !== -1;
        });
    if (!rows.length) {
        return '<div class="model-empty">' + (q
            ? 'No providers match "' + OSA.escapeHtml(query.trim()) + '"'
            : 'Every available provider is connected') + '</div>';
    }
    return rows.map(function(p) {
        return '<div class="provider-catalog-item provider-available-row">'
            + '<div class="provider-catalog-header">'
            +   '<div class="provider-catalog-title"><span class="provider-catalog-name">' + OSA.escapeHtml(p.name) + '</span></div>'
            +   '<button type="button" class="btn-action provider-row-btn" onclick="OSA.openAddProviderModal(' + OSA.jsArg(p.id) + ')">Connect</button>'
            + '</div>'
            + '<div class="provider-catalog-meta">' + OSA.escapeHtml(p.description || '') + '</div>'
            + '</div>';
    }).join('');
};

OSA.filterAvailableProviders = function(query) {
    const list = document.getElementById('available-provider-list');
    if (list) list.innerHTML = OSA.availableProviderListHtml(query);
};

OSA.refreshAvailableProviders = function() {
    const panel = document.getElementById('add-provider-panel');
    if (!panel || panel.classList.contains('hidden')) return;
    const search = document.getElementById('add-provider-search');
    OSA.filterAvailableProviders(search ? search.value : '');
};

OSA.toggleAddProviderList = function() {
    const panel = document.getElementById('add-provider-panel');
    const toggle = document.getElementById('add-provider-toggle');
    if (!panel) return;
    const open = panel.classList.contains('hidden');
    panel.classList.toggle('hidden', !open);
    if (toggle) toggle.setAttribute('aria-expanded', open ? 'true' : 'false');
    if (open) {
        const search = document.getElementById('add-provider-search');
        OSA.filterAvailableProviders(search ? search.value : '');
        if (search) search.focus();
    }
};

// ── Current route and pane load ─────────────────────────────

OSA.renderRoutingOverview = function(catalog, providersData) {
    const summaryEl = document.getElementById('provider-routing-summary');
    if (!summaryEl) return;

    const activeId = providersData.default_provider || '';
    const activeModel = providersData.default_model || '';
    const providersById = Object.fromEntries((catalog.providers || []).map(function(provider) {
        return [provider.id, provider];
    }));
    const activeProvider = providersById[activeId] || null;

    if (!activeId || !activeModel) {
        summaryEl.innerHTML = '<div class="provider-route-card"><div><div class="provider-route-title">No active connected route</div><div class="provider-route-meta">Connect a provider below to start routing by provider + model.</div></div></div>';
        return;
    }
    const providerName = activeProvider ? activeProvider.name : activeId;
    summaryEl.innerHTML = '<div class="provider-route-card provider-route-card-active">' +
        '<div>' +
            '<div class="provider-route-kicker">Active provider route</div>' +
            '<div class="provider-route-title">' + OSA.escapeHtml(providerName) + '</div>' +
            '<div class="provider-route-meta">Model: <strong>' + OSA.escapeHtml(activeModel) + '</strong></div>' +
        '</div>' +
        '<div class="provider-route-actions">' +
            '<button class="btn-action" onclick="OSA.openAddProviderModal(\'' + OSA.escapeHtml(activeId) + '\', \'' + OSA.escapeHtml(activeModel) + '\')">Manage</button>' +
        '</div>' +
    '</div>';
};

// Reflects the saved age threshold. A value outside the offered presets gets
// its own option, so the select never shows the wrong choice.
OSA.syncModelMenuAgeSelect = function() {
    const select = document.getElementById('model-menu-age');
    if (!select) return;
    const days = OSA.modelMenu().hide_older_than_days;
    const value = days ? String(days) : '';
    if (value && !Array.from(select.options).some(function(option) { return option.value === value; })) {
        const option = document.createElement('option');
        option.value = value;
        option.textContent = days + ' days';
        select.appendChild(option);
    }
    select.value = value;
};

OSA.renderSettingsProviders = async function() {
    const list = document.getElementById('connected-provider-list');
    if (!list) return;

    list.innerHTML = '<div class="model-empty">Loading...</div>';

    try {
        // The catalog is cached process-wide after the first fetch; only the
        // small connected-providers payload is re-read here.
        const [catalog, providersData] = await Promise.all([
            OSA.loadProviderCatalog(),
            OSA.getJson('/api/providers')
        ]);
        OSA.renderRoutingOverview(catalog, providersData);
        OSA.syncModelMenuAgeSelect();

        const connectedMap = {};
        for (const p of (providersData.providers || [])) {
            connectedMap[p.id] = p;
        }
        OSA.modelsConnectedMap = connectedMap;
        OSA.renderConnectedProviders();
        OSA.refreshAvailableProviders();
    } catch (e) {
        list.innerHTML = '<div class="model-empty">Failed to load providers</div>';
    }
};

// ── Local Server Status (Ollama / Unsloth) ──────────────────

OSA.renderLocalServers = async function() {
    const container = document.getElementById('local-servers-status');
    if (!container) return;
    container.innerHTML = '<div class="model-empty">Checking...</div>';

    let servers;
    try {
        const data = await OSA.getJson('/api/local-servers/status');
        servers = data.servers || [];
    } catch (e) {
        container.innerHTML = '<div class="model-empty">Failed to check local servers</div>';
        return;
    }
    if (!servers.length) {
        container.innerHTML = '<div class="model-empty">No local servers configured</div>';
        return;
    }

    container.innerHTML = servers.map(function(server) {
        const stateClass = server.state === 'online' ? 'ok'
            : (server.state === 'needs_auth' ? 'warn' : 'error');
        const badge = server.state === 'online'
            ? 'Online · ' + (server.model_count || 0) + ' model' + (server.model_count !== 1 ? 's' : '')
            : (server.state === 'needs_auth' ? 'Needs API key' : 'Offline');
        const sampleModels = (server.models || []).slice(0, 4).join(', ');
        const detail = OSA.escapeHtml(server.base_url || '')
            + (sampleModels ? ' · ' + OSA.escapeHtml(sampleModels) : '');

        let keyRow = '';
        if (server.id === 'unsloth' && server.state !== 'online') {
            keyRow = '<div class="local-server-key-row">' +
                '<input type="password" id="local-key-' + OSA.escapeHtml(server.id) + '" placeholder="' +
                (server.has_api_key ? 'Replace API key...' : 'Paste API key...') + '" autocomplete="off" />' +
                '<button type="button" class="btn-action" onclick="OSA.saveLocalServerKey(\'' + OSA.escapeHtml(server.id) + '\')">Save Key</button>' +
                '</div>';
        }

        return '<div class="doctor-status-card ' + stateClass + '">' +
            '<div class="doctor-status-header">' +
                '<div class="doctor-status-title">' + OSA.escapeHtml(server.name || server.id) + '</div>' +
                '<span class="doctor-status-badge ' + stateClass + '">' + OSA.escapeHtml(badge) + '</span>' +
            '</div>' +
            '<div class="doctor-status-detail">' + detail + '</div>' +
            keyRow +
        '</div>';
    }).join('');
};

OSA.refreshLocalServers = function() {
    return OSA.renderLocalServers();
};

OSA.saveLocalServerKey = async function(providerId) {
    const input = document.getElementById('local-key-' + providerId);
    if (!input) return;
    const apiKey = input.value.trim();
    if (!apiKey) { alert('Paste an API key first'); return; }

    // Preserve any existing route/model selection when re-saving the key.
    let existingModel = '';
    try {
        const providersData = await OSA.getJson('/api/providers');
        const entry = (providersData.providers || []).find(function(p) { return p.id === providerId; });
        existingModel = (entry && entry.model) || '';
    } catch (e) { /* fall through with provider default */ }

    let saved;
    try {
        saved = await OSA.postJson('/api/providers', {
            provider_id: providerId,
            api_key: apiKey,
            model: existingModel || undefined
        });
    } catch (e) {
        alert('Failed to save key: ' + (e.message || e));
        return;
    }
    if (saved && saved.error) {
        alert('Failed to save key: ' + saved.error);
        return;
    }

    input.value = '';
    // Auto-route to the first discovered local model so the server is usable
    // immediately instead of pointing at a preset ID that may not exist.
    try {
        const status = await OSA.getJson('/api/local-servers/status');
        const server = (status.servers || []).find(function(s) { return s.id === providerId; });
        const models = (server && server.models) || [];
        if (server && server.state === 'online' && models.length) {
            const known = existingModel && models.indexOf(existingModel) !== -1;
            if (!known) {
                await OSA.postJson('/api/providers/switch', { provider_id: providerId, model: models[0] });
            }
        }
    } catch (e) { /* non-fatal */ }

    OSA.loadProviderCatalog(true).then(function() {
        OSA.renderSettingsProviders();
    });
    OSA.renderLocalServers();
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
    OSA.currentProviderConnected = !!connectedEntry;

    OSA.currentProviderOAuthSupported = provider.oauth_supported;
    OSA.currentProviderApiKeyUrl = provider.api_key_url || null;

    // Set header icon
    const iconMap = { openai: 'OAI', anthropic: 'ANT', google: 'GAI', openrouter: 'ORN', ollama: 'OLL', unsloth: 'UNS', groq: 'GRQ', deepseek: 'DSK', xai: 'XAI' };
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
        addBtn.textContent = OSA.providerSubmitLabel(!!connectedEntry);
        await OSA.updateOAuthUI(providerId);
    } else {
        oauthSection.classList.add('hidden');
        apiKeySection.classList.remove('hidden');
        divider.classList.add('hidden');
        apiKeyTitle.classList.add('hidden');
        addBtn.textContent = OSA.providerSubmitLabel(!!connectedEntry);
    }

    // Pre-fill base URL. The endpoint stays folded for presets and opens for
    // local servers, or when the saved endpoint differs from the preset's.
    document.getElementById('provider-base-url').value = (connectedEntry && connectedEntry.base_url) || provider.base_url || '';
    const localServer = provider.id === 'ollama' || provider.id === 'unsloth';
    const customEndpoint = !provider.base_url
        || (!!connectedEntry && !!connectedEntry.base_url && connectedEntry.base_url !== provider.base_url);
    document.getElementById('provider-advanced').open = localServer || customEndpoint;
    const keyLabel = document.querySelector('label[for="provider-api-key"]');
    if (keyLabel) keyLabel.textContent = localServer ? 'API Key (optional)' : 'API Key';
    // OAuth providers already offer Disconnect in their OAuth view, so the
    // footer Remove is reserved for key-based entries.
    document.getElementById('provider-remove-btn').classList.toggle('hidden', !connectedEntry || !!provider.oauth_supported);

    // API key link
    const apiKeyLink = document.getElementById('api-key-link');
    const apiKeyLinkName = document.getElementById('api-key-link-name');
    if (provider.api_key_url) {
        apiKeyLink.classList.remove('hidden');
        apiKeyLinkName.textContent = provider.name;
        apiKeyLink.onclick = function() {
            const url = OSA.safeUrl(provider.api_key_url);
            if (url) window.open(url, '_blank', 'noopener,noreferrer');
            return false;
        };
    } else {
        apiKeyLink.classList.add('hidden');
    }

    // Reset fields
    document.getElementById('provider-api-key').value = '';
    document.getElementById('provider-api-key').placeholder = connectedEntry && connectedEntry.key_count
        ? 'Leave blank to keep the saved keys, or add another'
        : 'sk-...';
    const apiKeyLabel = document.querySelector('label[for="provider-api-key"]');
    if (apiKeyLabel) {
        apiKeyLabel.textContent = connectedEntry && connectedEntry.key_count
            ? 'Add another API key'
            : 'API Key';
    }
    document.getElementById('provider-default').checked = !!(connectedEntry && connectedEntry.is_default);
    document.getElementById('validation-badge').style.display = 'none';
    document.getElementById('modal-model-select-text').textContent = 'Use provider default (choose later)';
    document.getElementById('provider-model').value = '';
    OSA.closeModalModelDropdown();

    // Populate modal model dropdown
    await OSA.refreshModalProviderModels(provider, connectedEntry);
    OSA.loadProviderKeys(providerId, connectedEntry);
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

    // Set up auto-validation and dynamic Ollama/Unsloth discovery
    document.getElementById('provider-api-key').oninput = function() { OSA.scheduleValidation(); };
    document.getElementById('provider-base-url').oninput = function() {
        OSA.scheduleValidation();
        if (provider.id === 'ollama' || provider.id === 'unsloth') {
            if (OSA.ollamaModelDebounce) clearTimeout(OSA.ollamaModelDebounce);
            OSA.ollamaModelDebounce = setTimeout(function() {
                OSA.refreshModalProviderModels(provider, connectedEntry);
            }, 350);
        }
    };

    modal.classList.remove('hidden');
};

// ── Multiple keys ───────────────────────────────────────────

OSA.loadProviderKeys = async function(providerId, connectedEntry) {
    const section = document.getElementById('provider-keys-section');
    const list = document.getElementById('provider-keys-list');
    const count = document.getElementById('provider-keys-count');
    if (!section || !list) return;
    section.classList.add('hidden');
    list.innerHTML = '';
    if (count) count.textContent = '';
    if (!connectedEntry) return;
    try {
        const res = await OSA.fetchWithAuth('/api/providers/' + encodeURIComponent(providerId) + '/keys');
        const data = await res.json().catch(function() { return {}; });
        if (!res.ok) throw new Error(data.error || 'HTTP ' + res.status);
        const keys = data.keys || [];
        if (!keys.length) return;
        section.classList.remove('hidden');
        if (count) count.textContent = keys.length + (keys.length === 1 ? ' key' : ' keys');
        list.innerHTML = keys.map(function(key) {
            const remove = keys.length > 1
                ? '<button type="button" class="btn-ghost" onclick="OSA.removeProviderKey(' + Number(key.index) + ')">Remove</button>'
                : '';
            return '<div class="provider-key-row"><span>'
                + OSA.escapeHtml(key.preview || '••••')
                + '</span>' + remove + '</div>';
        }).join('');
    } catch (error) {
        console.warn('Could not load provider keys:', error);
    }
};

OSA.removeProviderKey = async function(index) {
    if (!OSA.currentProviderId) return;
    if (!window.confirm('Remove this API key from the rotation?')) return;
    try {
        const res = await OSA.fetchWithAuth(
            '/api/providers/' + encodeURIComponent(OSA.currentProviderId) + '/keys/' + Number(index),
            { method: 'DELETE' }
        );
        const data = await res.json().catch(function() { return {}; });
        if (!res.ok) throw new Error(data.error || 'HTTP ' + res.status);
        await OSA.loadProviderKeys(OSA.currentProviderId, true);
        OSA.renderSettingsProviders();
        if (OSA.showToast) OSA.showToast('API key removed', 'info');
    } catch (error) {
        window.alert(error.message || 'Could not remove that key');
    }
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
        // This route sits behind the /api auth middleware, so it has to carry
        // the same bearer token as every other request. A bare fetch() would
        // always come back 401 and read as "invalid key" even for a good one.
        const res = await OSA.fetchWithAuth('/api/providers/validate', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ provider_id: OSA.currentProviderId, api_key: apiKey, base_url: baseUrl || undefined })
        });
        const data = await res.json().catch(function() { return {}; });

        if (res.ok && data.valid) {
            badge.className = 'validation-badge valid';
            badge.innerHTML = OSA.icons.check + ' Valid';
        } else {
            badge.className = 'validation-badge invalid';
            badge.innerHTML = OSA.icons.xmark + ' ' + OSA.escapeHtml(data.error || ('Check failed (' + res.status + ')'));
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

// True inside the Tauri desktop shell, where window.open popups and
// window.opener callbacks do not function (WebKitGTK/WebView2 webview).
OSA.isTauriWebview = function() {
    try {
        return typeof window.__TAURI__ !== 'undefined' && !!window.__TAURI__;
    } catch (err) {
        return false;
    }
};

// Authenticated JSON helper for the OAuth endpoints, which sit behind the
// login gate. A bare fetch() without the Bearer token gets a 401 with an
// empty body, and res.json() then throws a confusing parse error — surface a
// readable message instead.
OSA.fetchOAuthJson = async function(url, body) {
    const res = await OSA.fetchWithAuth(url, {
        method: 'POST',
        body: JSON.stringify(body || {})
    });
    let data = null;
    try {
        data = await res.json();
    } catch (err) {
        data = null;
    }
    if (!res.ok) {
        const message = (data && data.error)
            || (res.status === 401 ? 'Request was rejected (401). Please sign in again.' : null)
            || ('Request failed (HTTP ' + res.status + ')');
        throw new Error(message);
    }
    if (data && data.error) throw new Error(data.error);
    return data || {};
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
    // Inside the Tauri shell there is no functioning popup at all, so skip it
    // there and use the system-browser flow below instead.
    const useExternalBrowser = OSA.isTauriWebview();
    const oauthWindow = useExternalBrowser ? null : window.open('', '_blank', 'width=600,height=700');

    try {
        const data = await OSA.fetchOAuthJson('/api/oauth/' + OSA.currentProviderId + '/start', {});

        if (!data.success) {
            if (oauthWindow) oauthWindow.close();
            throw new Error('Failed to start OAuth');
        }

        if (data.flow_type === 'pkce') {
            OSA.currentOAuthFlow = {
                type: 'pkce',
                codeVerifier: data.code_verifier,
                state: data.state
            };

            // Tauri shell (or any environment where the popup never opened):
            // open the sign-in page in the OS default browser and wait for the
            // redirect back to the local callback, polling status meanwhile.
            if (useExternalBrowser || !oauthWindow || oauthWindow.closed) {
                if (oauthWindow && !oauthWindow.closed) oauthWindow.close();
                await OSA.startExternalBrowserSignIn(data.auth_url);
                return;
            }

            // Navigate the already-open window to the auth URL
            if (oauthWindow) {
                const authUrl = OSA.safeUrl(data.auth_url);
                if (!authUrl) {
                    oauthWindow.close();
                    throw new Error('OAuth provider returned an unsupported authorization URL');
                }
                oauthWindow.location.href = authUrl;
                oauthWindow.focus();

                const pollTimer = setInterval(function() {
                    try {
                        if (oauthWindow.closed) {
                            clearInterval(pollTimer);
                            OSA.pkcePollTimer = null;
                            OSA.checkOAuthCallback(OSA.currentProviderId);
                        }
                    } catch (e) {}
                }, 500);
                OSA.pkcePollTimer = pollTimer;

                window.oauthCallback = function(params) {
                    clearInterval(pollTimer);
                    OSA.pkcePollTimer = null;
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
            const deviceLink = document.getElementById('oauth-device-link');
            const verifyUrl = OSA.safeUrl(data.verification_uri);
            if (verifyUrl) {
                deviceLink.href = verifyUrl;
                deviceLink.textContent = 'Open verification page';
                deviceLink.classList.remove('hidden');
            } else {
                deviceLink.removeAttribute('href');
                deviceLink.classList.add('hidden');
            }
            document.getElementById('oauth-device-status-text').textContent = 'Waiting for authorization...';
            OSA.deviceCodePollAttempts = 0;
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

// System-browser sign-in: the backend opens the provider page in the OS
// default browser (which, unlike an embedded webview, can actually render the
// provider's login), then we poll our own status until the redirect back to
// the local callback completes the flow.
OSA.oauthStatusPollTimer = null;
OSA.oauthStatusPollAttempts = 0;

OSA.startExternalBrowserSignIn = async function(authUrl) {
    const providerId = OSA.currentProviderId;
    const errorEl = document.getElementById('oauth-error-pkce');
    const safeUrl = OSA.safeUrl(authUrl);
    if (!safeUrl) throw new Error('OAuth provider returned an unsupported authorization URL');

    document.getElementById('oauth-loading-text').textContent = 'Opening sign-in page in your browser...';
    await OSA.fetchOAuthJson('/api/oauth/' + providerId + '/open-browser', { auth_url: safeUrl });

    document.getElementById('oauth-loading-text').textContent = 'Waiting for sign-in in your browser...';
    OSA.oauthStatusPollAttempts = 0;
    OSA.currentOAuthFlow = { type: 'external_browser' };
    OSA.pollOAuthStatus();
};

OSA.pollOAuthStatus = function() {
    if (!OSA.currentOAuthFlow || OSA.currentOAuthFlow.type !== 'external_browser') return;
    const providerId = OSA.currentProviderId;
    OSA.oauthStatusPollAttempts = (OSA.oauthStatusPollAttempts || 0) + 1;
    if (OSA.oauthStatusPollAttempts > 120) {
        OSA.currentOAuthFlow = null;
        OSA.showOAuthView('pkce');
        const errorEl = document.getElementById('oauth-error-pkce');
        errorEl.textContent = 'Timed out waiting for sign-in.';
        errorEl.classList.remove('hidden');
        return;
    }
    OSA.oauthStatusPollTimer = setTimeout(async function() {
        try {
            const status = await OSA.getJson('/api/oauth/' + providerId + '/status');
            if (status.status === 'active' || status.configured) {
                OSA.onOAuthSuccess(providerId);
                return;
            }
        } catch (error) {}
        OSA.pollOAuthStatus();
    }, 2500);
};

OSA.cancelOAuthStatusPoll = function() {
    if (OSA.oauthStatusPollTimer) {
        clearTimeout(OSA.oauthStatusPollTimer);
        OSA.oauthStatusPollTimer = null;
    }
    if (OSA.currentOAuthFlow && OSA.currentOAuthFlow.type === 'external_browser') {
        OSA.currentOAuthFlow = null;
    }
};

OSA.deviceCodeNextDelay = null;

OSA.pollDeviceCode = function() {
    if (!OSA.currentOAuthFlow || OSA.currentOAuthFlow.type !== 'device_code') return;

    const deviceCode = OSA.currentOAuthFlow.deviceCode;
    const baseInterval = OSA.currentOAuthFlow.interval;
    const delay = OSA.deviceCodeNextDelay || baseInterval;
    OSA.deviceCodeNextDelay = null;

    // Cap the poll so a flow that never completes cannot spin forever.
    OSA.deviceCodePollAttempts = (OSA.deviceCodePollAttempts || 0) + 1;
    if (OSA.deviceCodePollAttempts > 200) {
        const statusEl = document.getElementById('oauth-device-status-text');
        if (statusEl) statusEl.textContent = 'Timed out waiting for authorization.';
        return;
    }

    OSA.deviceCodePollTimer = setTimeout(async function() {
        try {
            const data = await OSA.fetchOAuthJson('/api/oauth/' + OSA.currentProviderId + '/device', { device_code: deviceCode });

            if (data.success && data.connected) {
                OSA.onOAuthSuccess(OSA.currentProviderId);
                return;
            }

            if (data.pending) {
                // `slow_down` asks for a gentler cadence; everything else
                // pending just means the user has not approved yet.
                OSA.deviceCodeNextDelay = data.slow_down
                    ? Math.max(baseInterval + 5000, Math.round(baseInterval * 1.5))
                    : baseInterval;
                const statusEl = document.getElementById('oauth-device-status-text');
                if (statusEl) statusEl.textContent = 'Waiting for authorization...';
                OSA.pollDeviceCode();
            } else if (data.error) {
                const statusEl = document.getElementById('oauth-device-status-text');
                if (statusEl) statusEl.textContent = 'Error: ' + data.error;
            }
        } catch (error) {
            // A transient failure (the webview being suspended while the user
            // authorizes in the system browser aborts in-flight requests) must
            // not abandon a sign-in that may already be approved — keep polling.
            OSA.deviceCodeNextDelay = baseInterval;
            const statusEl = document.getElementById('oauth-device-status-text');
            if (statusEl) statusEl.textContent = 'Waiting for authorization... (retrying)';
            OSA.pollDeviceCode();
        }
    }, delay);
};

// Polling a device code depends on a timer, but Chromium (and WebView2) freeze
// timers while the page is hidden — exactly the state of the window while the
// user authorizes in the system browser. Re-poll the moment the page is visible
// or focused again so an approved sign-in registers immediately.
OSA.bindDeviceCodeVisibilityPolling = function() {
    if (OSA.deviceCodeVisibilityBound) return;
    OSA.deviceCodeVisibilityBound = true;
    const wake = (event) => {
        if (!OSA.currentOAuthFlow || OSA.currentOAuthFlow.type !== 'device_code') return;
        // Only the visibilitychange event is gated on visibilityState: a real
        // focus/pageshow means the window is back in front even if WebView2
        // reports the page as hidden.
        if (event && event.type === 'visibilitychange'
            && typeof document.visibilityState === 'string'
            && document.visibilityState === 'hidden') return;
        if (OSA.deviceCodePollTimer) {
            clearTimeout(OSA.deviceCodePollTimer);
            OSA.deviceCodePollTimer = null;
        }
        OSA.deviceCodeNextDelay = null;
        OSA.pollDeviceCode();
    };
    document.addEventListener('visibilitychange', wake);
    window.addEventListener('focus', wake);
    window.addEventListener('pageshow', wake);
};
OSA.bindDeviceCodeVisibilityPolling();

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
    if (typeof OSA.cancelOAuthStatusPoll === 'function') OSA.cancelOAuthStatusPoll();
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
                addBtn.textContent = OSA.providerSubmitLabel(true);
            }
            return;
        }
    }

    // Editing a connected provider keeps its saved key, so the field may be left blank.
    if (!apiKey && !OSA.currentProviderConnected && OSA.currentProviderId !== 'ollama' && OSA.currentProviderId !== 'unsloth') {
        alert('Please enter an API key');
        return;
    }

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
        addBtn.textContent = OSA.providerSubmitLabel(OSA.currentProviderConnected);
    }
};

OSA.providerSubmitLabel = function(isConnected) {
    return isConnected ? 'Save' : 'Connect';
};

OSA.removeProvider = async function() {
    const providerId = OSA.currentProviderId;
    if (!providerId) return;
    const provider = OSA.findCatalogProvider(providerId);
    const name = provider ? provider.name : providerId;
    if (!confirm('Remove ' + name + ' from OSAgent? Its saved API keys are deleted from this machine.')) return;

    const removeBtn = document.getElementById('provider-remove-btn');
    if (removeBtn) removeBtn.disabled = true;
    try {
        const res = await OSA.fetchWithAuth('/api/providers/' + encodeURIComponent(providerId), { method: 'DELETE' });
        const data = await res.json().catch(function() { return {}; });
        if (!res.ok) throw new Error(data.error || 'Failed to remove provider');
        OSA.closeAddProviderModal();
        await OSA.loadProviderCatalog(true);
        await OSA.renderSettingsProviders();
        // Removing the active provider moves the route to another one, so the
        // composer trigger has to be re-read.
        if (typeof OSA.loadModel === 'function') await OSA.loadModel().catch(function() {});
    } catch (error) {
        alert('Failed to remove provider: ' + error.message);
    } finally {
        if (removeBtn) removeBtn.disabled = false;
    }
};

// ── Modal Close ─────────────────────────────────────────────

OSA.closeAddProviderModal = function() {
    const modal = document.getElementById('add-provider-modal');
    if (modal) modal.classList.add('hidden');
    // Stop any in-flight OAuth polling; otherwise a closed modal keeps hitting
    // /oauth/.../device (or the PKCE popup timer) for the rest of the session.
    if (typeof OSA.cancelDeviceCode === 'function' && OSA.currentOAuthFlow?.type === 'device_code') {
        OSA.cancelDeviceCode();
    }
    if (OSA.pkcePollTimer) {
        clearInterval(OSA.pkcePollTimer);
        OSA.pkcePollTimer = null;
    }
    if (typeof OSA.cancelOAuthStatusPoll === 'function') OSA.cancelOAuthStatusPoll();
    OSA.oauthStatusPollAttempts = 0;
    OSA.deviceCodePollAttempts = 0;
    OSA.currentOAuthFlow = null;
    OSA.currentProviderId = null;
    OSA.currentProviderOAuthSupported = false;
    OSA.currentProviderConnected = false;
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

// composedPath() is captured when the event is dispatched, so it still names
// the dropdown after a click handler has re-rendered (and detached) its target.
// A bare target.closest() would miss that and close the menu.
OSA.eventIsInside = function(event, element) {
    if (!element) return false;
    if (event.composedPath && event.composedPath().includes(element)) return true;
    return element.contains(event.target);
};

document.addEventListener('click', function(event) {
    if (OSA.modelDropdownOpen
        && !OSA.eventIsInside(event, document.getElementById('model-dropdown'))
        && !event.target.closest('.model-selector')) {
        OSA.closeModelDropdown(false);
    }
    if (OSA.modalModelDropdownOpen && !event.target.closest('#modal-model-select-wrapper')) {
        OSA.closeModalModelDropdown();
    }
});

window.OSA = window.OSA || {};

// ── Model dropdown ──────────────────────────────────────────
//
// One dropdown, used by the chat bar and by the per-surface model fields in
// Settings. The component owns the chrome: the trigger, the popover, the
// search box, the keyboard navigation and the positioning.
//
// Each surface supplies the list renderer and the click handler, because the
// chat bar switches the live route when a model is picked while a Settings
// field only records a value.
//
// A surface that keeps its state in globals (the composer) passes an adapter
// object in, so `OSA.modelDropdownOpen` and friends stay the single source
// of truth for it.

OSA.ModelDropdown = function(config) {
    const cfg = config || {};
    this.ids = {
        trigger: cfg.triggerId || '',
        dropdown: cfg.dropdownId || '',
        search: cfg.searchId || '',
        list: cfg.listId || '',
    };
    // Elements can be passed directly. A dropdown that is built in JavaScript
    // is not in the document until it is opened, so it cannot be found by id.
    this.elements = cfg.elements || {};
    // A list without an id (the composer's) is found by selector instead.
    this.listSelector = cfg.listSelector || '';
    this.renderList = typeof cfg.renderList === 'function' ? cfg.renderList : function() {};
    // The surface's own handlers. Kept apart from the DOM listeners bound
    // below, which would otherwise clobber them.
    this.listClickHandler = typeof cfg.onListClick === 'function' ? cfg.onListClick : null;
    this.openHandler = typeof cfg.onOpen === 'function' ? cfg.onOpen : null;
    this.closeHandler = typeof cfg.onClose === 'function' ? cfg.onClose : null;
    // The composer keeps its own active-row bookkeeping, so it turns this off
    // and keeps control of the highlighted row.
    this.manageActiveIndex = cfg.manageActiveIndex !== false;
    // The composer closes itself on an outside click already.
    this.closeOnOutsideClick = cfg.closeOnOutsideClick !== false;
    // The composer wires its own handlers through the inline attributes in
    // index.html, so the component must not attach them a second time.
    this.autoBind = cfg.autoBind !== false;
    this.positionOptions = cfg.positionOptions || {};
    this.state = cfg.state || {
        open: false,
        searchQuery: '',
        searchDebounce: null,
        searchRequestId: 0,
        optionNodes: [],
        activeIndex: -1,
    };
};

// ── Elements ────────────────────────────────────────────────
//
// Looked up on every use: the composer dropdown is moved onto the body while
// it is open, and the Settings pane is rebuilt when it re-renders.

OSA.ModelDropdown.prototype.el = function(name) {
    if (this.elements[name]) return this.elements[name];
    const id = this.ids[name];
    if (id) return document.getElementById(id);
    if (name === 'list' && this.listSelector) return document.querySelector(this.listSelector);
    return null;
};

OSA.ModelDropdown.prototype.trigger = function() {
    return this.el('trigger');
};

OSA.ModelDropdown.prototype.dropdown = function() {
    return this.el('dropdown');
};

OSA.ModelDropdown.prototype.searchInput = function() {
    return this.el('search');
};

OSA.ModelDropdown.prototype.list = function() {
    return this.el('list');
};

// ── Wiring ──────────────────────────────────────────────────

OSA.ModelDropdown.prototype.bind = function() {
    if (this.bound) return;
    const self = this;

    this.onTriggerClick = function() { self.toggle(); };
    this.onTriggerKeydown = function(event) { self.handleTriggerKeydown(event); };
    this.onSearchInput = function() { self.handleSearch(); };
    this.onSearchKeydown = function(event) { self.handleKeydown(event); };
    this.onListClick = function(event) { self.handleListClick(event); };
    this.onDocumentClick = function(event) {
        if (!self.state.open || !self.closeOnOutsideClick) return;
        const dropdown = self.dropdown();
        const trigger = self.trigger();
        if (dropdown && dropdown.contains(event.target)) return;
        if (trigger && trigger.contains(event.target)) return;
        self.close();
    };
    this.onViewportChange = function() { self.position(); };

    document.addEventListener('click', this.onDocumentClick);
    window.addEventListener('resize', this.onViewportChange);
    window.addEventListener('scroll', this.onViewportChange, true);
    window.visualViewport?.addEventListener('resize', this.onViewportChange);

    this.bindElements();
    this.bound = true;
};

// Attaches the element listeners. Called again on every open, because the
// dropdown may have been re-parented onto the body in the meantime.
OSA.ModelDropdown.prototype.bindElements = function() {
    if (!this.autoBind) return;
    const trigger = this.trigger();
    if (trigger) {
        trigger.removeEventListener('click', this.onTriggerClick);
        trigger.removeEventListener('keydown', this.onTriggerKeydown);
        trigger.addEventListener('click', this.onTriggerClick);
        trigger.addEventListener('keydown', this.onTriggerKeydown);
    }
    const search = this.searchInput();
    if (search) {
        search.removeEventListener('input', this.onSearchInput);
        search.removeEventListener('keydown', this.onSearchKeydown);
        search.addEventListener('input', this.onSearchInput);
        search.addEventListener('keydown', this.onSearchKeydown);
    }
    const list = this.list();
    if (list && this.listClickHandler) {
        list.removeEventListener('click', this.onListClick);
        list.addEventListener('click', this.onListClick);
    }
};

// Removes everything a surface built for this dropdown. Settings calls it
// when it re-renders the pane the trigger lives in.
OSA.ModelDropdown.prototype.destroy = function() {
    this.close(false);
    if (this.bound) {
        document.removeEventListener('click', this.onDocumentClick);
        window.removeEventListener('resize', this.onViewportChange);
        window.removeEventListener('scroll', this.onViewportChange, true);
        window.visualViewport?.removeEventListener('resize', this.onViewportChange);
        this.bound = false;
    }
    const dropdown = this.dropdown();
    if (dropdown && dropdown.parentElement) dropdown.parentElement.removeChild(dropdown);
    const trigger = this.trigger();
    if (trigger && trigger.parentElement) trigger.parentElement.removeChild(trigger);
    const input = this.input;
    if (input && input.parentElement) input.parentElement.removeChild(input);
};

// ── Open / close ─────────────────────────────────────────────

OSA.ModelDropdown.prototype.toggle = function() {
    if (this.state.open) {
        this.close();
    } else {
        this.open();
    }
};

OSA.ModelDropdown.prototype.open = function(initialQuery) {
    if (this.state.open) return;
    const dropdown = this.dropdown();
    if (!dropdown) return;

    this.state.open = true;
    this.bindElements();

    const trigger = this.trigger();
    if (trigger) trigger.setAttribute('aria-expanded', 'true');
    // The popover lives on the body so a scrolling panel cannot clip it.
    if (dropdown.parentElement !== document.body) document.body.appendChild(dropdown);
    dropdown.classList.remove('hidden');

    const search = this.searchInput();
    this.state.searchQuery = initialQuery || '';
    if (search) {
        search.value = initialQuery || '';
        search.focus();
    }

    if (this.openHandler) this.openHandler();
    this.render();
};

OSA.ModelDropdown.prototype.close = function(restoreFocus) {
    const wasOpen = this.state.open;
    this.state.open = false;
    this.state.searchRequestId += 1;
    if (this.state.searchDebounce) {
        clearTimeout(this.state.searchDebounce);
        this.state.searchDebounce = null;
    }
    if (!wasOpen) return;
    const dropdown = this.dropdown();
    if (dropdown) dropdown.classList.add('hidden');
    const trigger = this.trigger();
    if (trigger) trigger.setAttribute('aria-expanded', 'false');
    if (this.closeHandler) this.closeHandler();
    if (restoreFocus && trigger) trigger.focus();
};

OSA.ModelDropdown.prototype.position = function() {
    if (!this.state.open) return;
    const dropdown = this.dropdown();
    const trigger = this.trigger();
    if (!dropdown || !trigger) return;
    const options = Object.assign({}, this.positionOptions);
    if (options.width === 'trigger') {
        const width = trigger.getBoundingClientRect().width;
        // minWidth/maxWidth let a surface (Settings) ask for a wider popover
        // than the composer's compact menu.
        const min = options.minWidth || 260;
        const max = options.maxWidth || 420;
        options.width = Math.max(min, Math.min(max, width || 320));
        delete options.minWidth;
        delete options.maxWidth;
    }
    OSA.positionModelPopover(dropdown, trigger, options);
};

// ── Rendering ────────────────────────────────────────────────

OSA.ModelDropdown.prototype.render = function() {
    const list = this.list();
    if (!list) return;
    const result = this.renderList(list, this);
    if (result && typeof result.then === 'function') {
        result.then(() => this.afterRender()).catch(error => {
            console.error('Model dropdown render failed:', error);
        });
    } else {
        this.afterRender();
    }
};

// Collects the option rows for the keyboard navigation and re-anchors the
// popover, since a longer list changes its height.
OSA.ModelDropdown.prototype.afterRender = function() {
    const list = this.list();
    if (list) {
        this.state.optionNodes = Array.from(list.querySelectorAll('.model-option'));
        if (this.manageActiveIndex) this.state.activeIndex = -1;
    }
    this.position();
};

// ── Interaction ──────────────────────────────────────────────

OSA.ModelDropdown.prototype.handleSearch = function() {
    const search = this.searchInput();
    this.state.searchQuery = search ? search.value : '';
    if (this.state.searchDebounce) clearTimeout(this.state.searchDebounce);
    const self = this;
    this.state.searchDebounce = setTimeout(function() { self.render(); }, 150);
};

OSA.ModelDropdown.prototype.handleListClick = function(event) {
    if (this.listClickHandler) this.listClickHandler(event, this);
};

OSA.ModelDropdown.prototype.setActiveIndex = function(index) {
    const options = this.state.optionNodes;
    if (!options.length) {
        this.state.activeIndex = -1;
        return;
    }
    const nextIndex = Math.max(0, Math.min(index, options.length - 1));
    const previous = options[this.state.activeIndex];
    if (previous) previous.classList.remove('keyboard-active');
    this.state.activeIndex = nextIndex;
    options[nextIndex].classList.add('keyboard-active');
    options[nextIndex].scrollIntoView({ block: 'nearest' });
};

OSA.ModelDropdown.prototype.handleKeydown = function(event) {
    const key = event.key || '';
    if (key === 'Escape') {
        event.preventDefault();
        // Close the dropdown only: Escape must not also close the settings
        // modal that hosts it.
        event.stopPropagation();
        this.close(true);
        return;
    }
    if (key === 'ArrowDown') {
        event.preventDefault();
        this.setActiveIndex(this.state.activeIndex + 1);
        return;
    }
    if (key === 'ArrowUp') {
        event.preventDefault();
        this.setActiveIndex(this.state.activeIndex < 0 ? this.state.optionNodes.length - 1 : this.state.activeIndex - 1);
        return;
    }
    if (key === 'Home') {
        event.preventDefault();
        this.setActiveIndex(0);
        return;
    }
    if (key === 'End') {
        event.preventDefault();
        this.setActiveIndex(this.state.optionNodes.length - 1);
        return;
    }
    if (key === 'Enter' && this.state.activeIndex >= 0 && this.state.optionNodes[this.state.activeIndex]) {
        event.preventDefault();
        this.state.optionNodes[this.state.activeIndex].click();
    }
};

OSA.ModelDropdown.prototype.handleTriggerKeydown = function(event) {
    const key = event.key || '';
    if (key === 'Escape') {
        if (!this.state.open) return;
        event.preventDefault();
        event.stopPropagation();
        this.close(true);
        return;
    }
    if (key === 'ArrowDown' || key === 'Enter' || key === ' ') {
        event.preventDefault();
        if (!this.state.open) this.open('');
        return;
    }
    if (key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) {
        event.preventDefault();
        this.open(key);
    }
};

// ── Settings model pickers ──────────────────────────────────
//
// The per-surface model fields in Settings. Each one gets its own dropdown
// with the search box and the provider groups of the composer, so picking a
// model reads the same everywhere. The value is "provider_id:model", or
// empty for "same as current model".

OSA.settingsModelPickers = OSA.settingsModelPickers || {};

OSA.initSettingsModelPickers = function(root) {
    const scope = root || document;
    scope.querySelectorAll('[data-model-picker]').forEach(function(mount) {
        const id = mount.getAttribute('data-model-picker');
        if (!id) return;
        const existing = OSA.settingsModelPickers[id];
        // The pane can be re-rendered, which drops the DOM the picker built.
        if (existing && existing.mount === mount && existing.trigger && existing.trigger.isConnected) return;
        if (existing) existing.destroy();
        OSA.settingsModelPickers[id] = OSA.createSettingsModelPicker(id, mount);
    });
};

OSA.createSettingsModelPicker = function(id, mount) {
    const trigger = document.createElement('button');
    trigger.type = 'button';
    trigger.className = 'model-trigger';
    trigger.id = id + '-trigger';
    trigger.setAttribute('aria-haspopup', 'listbox');
    trigger.setAttribute('aria-expanded', 'false');

    const label = document.createElement('span');
    label.className = 'model-trigger-label';
    trigger.appendChild(label);

    const chevron = document.createElement('span');
    chevron.className = 'model-trigger-chevron';
    chevron.setAttribute('aria-hidden', 'true');
    chevron.innerHTML = '&#9662;';
    trigger.appendChild(chevron);

    // The value lives in a hidden input so the settings form reads it the
    // same way it read the old <select>.
    const input = document.createElement('input');
    input.type = 'hidden';
    input.id = id;

    const dropdown = document.createElement('div');
    dropdown.className = 'model-picker-popover hidden';
    dropdown.id = id + '-dropdown';
    dropdown.setAttribute('role', 'dialog');
    dropdown.setAttribute('aria-label', 'Choose a model');

    const searchWrap = document.createElement('div');
    searchWrap.className = 'model-dropdown-search';
    const search = document.createElement('input');
    search.type = 'text';
    search.id = id + '-search';
    search.placeholder = 'Search models...';
    search.autocomplete = 'off';
    search.setAttribute('aria-label', 'Search models');
    searchWrap.appendChild(search);
    dropdown.appendChild(searchWrap);

    const list = document.createElement('div');
    list.className = 'model-dropdown-list';
    list.id = id + '-list';
    list.setAttribute('role', 'listbox');
    dropdown.appendChild(list);

    mount.appendChild(trigger);
    mount.appendChild(input);

    const picker = new OSA.ModelDropdown({
        elements: {
            trigger: trigger,
            dropdown: dropdown,
            search: search,
            list: list,
        },
        // Wider than the composer menu: these fields are picked rarely and
        // model ids are long, so use the room the settings pane has.
        positionOptions: { width: 'trigger', minWidth: 520, maxWidth: 640 },
        renderList: function(listEl, instance) {
            return OSA.renderSettingsModelList(listEl, instance);
        },
        onListClick: function(event, instance) {
            OSA.handleSettingsModelPick(event, instance);
        },
    });
    picker.id = id;
    picker.mount = mount;
    picker.input = input;
    picker.label = label;
    picker.value = '';
    picker.emptyLabel = mount.getAttribute('data-model-empty-label') || 'Same as current model';
    // A freshly built picker reads as "same as current model" until the
    // settings load fills in the stored value.
    label.textContent = picker.emptyLabel;
    picker.bind();
    return picker;
};

// The value of a picker is "provider_id:model"; an empty value means "same
// as the current model".
OSA.setSettingsModelValue = function(id, value) {
    const picker = OSA.settingsModelPickers[id];
    if (!picker) return;
    const next = value || '';
    picker.input.value = next;
    picker.value = next;
    picker.label.textContent = OSA.settingsModelLabel(picker, next);
};

OSA.getSettingsModelValue = function(id) {
    const picker = OSA.settingsModelPickers[id];
    return picker ? (picker.input.value || '') : '';
};

// A stored value is "provider_id:model". A value whose prefix is not a
// configured provider is a bare model name, which the runtime also accepts.
OSA.resolveModelRef = function(value) {
    const text = (value || '').trim();
    if (!text) return { providerId: '', modelId: '' };
    const index = text.indexOf(':');
    if (index <= 0) return { providerId: '', modelId: text };
    const providerId = text.slice(0, index);
    const modelId = text.slice(index + 1);
    const catalog = OSA.providerCatalog || {};
    const known = (catalog.providers || []).some(function(provider) {
        return provider.id === providerId;
    });
    if (known) return { providerId: providerId, modelId: modelId };
    // Not a provider, so the whole value is the model name.
    const matchesModel = (catalog.all_models || []).some(function(model) {
        return model.id === text;
    });
    if (matchesModel) return { providerId: '', modelId: text };
    return { providerId: providerId, modelId: modelId };
};

OSA.settingsModelLabel = function(picker, value) {
    if (!value) return picker.emptyLabel;
    const ref = OSA.resolveModelRef(value);
    const catalog = OSA.providerCatalog || {};
    const model = (catalog.all_models || []).find(function(item) {
        return item.id === ref.modelId && (!ref.providerId || item.provider_id === ref.providerId);
    });
    // A bare model name still belongs to whichever provider lists it.
    const providerId = ref.providerId || (model ? model.provider_id : '');
    const provider = (catalog.providers || []).find(function(item) { return item.id === providerId; });
    const providerName = provider ? provider.name : providerId;
    const modelName = model ? (model.name || model.id) : ref.modelId;
    if (!providerName) return modelName;
    return providerName + ' · ' + modelName;
};

// Only the models of a connected provider can actually run, so those are the
// only ones offered.
OSA.settingsConnectedProviderIds = function() {
    const catalog = OSA.providerCatalog || {};
    const connected = {};
    (catalog.providers || []).forEach(function(provider) {
        if (provider.connected) connected[provider.id] = true;
    });
    return connected;
};

OSA.renderSettingsModelList = async function(list, picker) {
    const query = picker.state.searchQuery.trim();
    // The catalog carries the connected flags, so it has to be there before
    // the list can be built.
    if (!(OSA.providerCatalog && OSA.providerCatalog.providers && OSA.providerCatalog.providers.length)) {
        try {
            await OSA.loadProviderCatalog();
        } catch (error) {
            console.error('Failed to load the provider catalog:', error);
        }
    }
    const connected = OSA.settingsConnectedProviderIds();
    const catalog = OSA.providerCatalog || {};
    const ref = OSA.resolveModelRef(picker.value || '');

    // The same "show in model menu" filters as the composer dropdown, except
    // that the picker's own stored value always stays visible — hiding a model
    // from the menu must never make a saved setting look lost.
    const keepCurrent = function(model) {
        if (!ref.modelId || model.id !== ref.modelId) return false;
        return !ref.providerId || model.provider_id === ref.providerId;
    };
    const visible = function(models) {
        return OSA.modelMenuVisibleModels(models, {
            connected: connected,
            always: keepCurrent,
        });
    };

    let models;
    if (query) {
        const requestId = ++picker.state.searchRequestId;
        try {
            const found = await OSA.getJson('/api/providers/search?q=' + encodeURIComponent(query));
            if (requestId !== picker.state.searchRequestId) return;
            models = visible(found);
        } catch (error) {
            if (requestId !== picker.state.searchRequestId) return;
            list.innerHTML = '<div class="model-empty">Search failed</div>';
            return;
        }
    } else {
        models = visible(catalog.all_models);
    }

    const groups = OSA.groupModelsByProvider(models);

    let html = '<div class="model-option' + (picker.value ? '' : ' active') + '" role="option" data-model-empty="true">'
        + '<div class="model-option-info">'
        + '<span class="model-option-name">' + OSA.escapeHtml(picker.emptyLabel) + '</span>'
        + '</div>'
        + '</div>';

    let currentFound = false;
    for (let i = 0; i < groups.length; i++) {
        const group = groups[i];
        html += '<div class="model-group-title">' + OSA.escapeHtml(group.name) + '</div>';
        for (let j = 0; j < group.models.length; j++) {
            const model = group.models[j];
            if (model.id === ref.modelId && (!ref.providerId || model.provider_id === ref.providerId)) {
                currentFound = true;
            }
            html += OSA.buildModelOptionHtml(model, model.provider_id, ref.modelId, {
                currentProviderId: ref.providerId,
            });
        }
    }

    // A stored value that is no longer in the catalog (a model that was
    // removed, or a provider that is gone) stays visible and pickable, so
    // opening the picker never silently drops the setting.
    if (picker.value && !currentFound) {
        html += '<div class="model-group-title">Current</div>';
        html += '<div class="model-option active" role="option" data-model-id="' + OSA.escapeAttr(ref.modelId)
            + '" data-provider-id="' + OSA.escapeAttr(ref.providerId) + '">'
            + '<div class="model-option-info">'
            + '<span class="model-option-name">' + OSA.escapeHtml(ref.modelId) + '</span>'
            + '<span class="model-option-id">' + OSA.escapeHtml(picker.value) + '</span>'
            + '</div>'
            + '</div>';
    }

    list.innerHTML = html || '<div class="model-empty">No models available</div>';
};

OSA.handleSettingsModelPick = function(event, picker) {
    // The favourite star is a composer-menu concept, but it means the same
    // thing here: it toggles the model for the whole app.
    const star = event.target.closest ? event.target.closest('[data-model-action="favourite"]') : null;
    if (star) {
        event.preventDefault();
        event.stopPropagation();
        const option = star.closest('.model-option[data-model-id]');
        if (option) {
            OSA.toggleFavourite(
                option.dataset.modelId || '',
                option.dataset.providerId || '',
                option.dataset.modelName || option.dataset.modelId || '',
                option.dataset.providerName || ''
            );
            picker.render();
        }
        return;
    }

    const empty = event.target.closest ? event.target.closest('[data-model-empty]') : null;
    if (empty) {
        OSA.setSettingsModelValue(picker.id, '');
        picker.close();
        return;
    }
    const option = event.target.closest ? event.target.closest('.model-option[data-model-id]') : null;
    if (!option) return;
    const modelId = option.dataset.modelId || '';
    const providerId = option.dataset.providerId || '';
    // A model without a provider keeps its bare name, the way the old
    // <select> stored it.
    const value = providerId ? providerId + ':' + modelId : modelId;
    OSA.setSettingsModelValue(picker.id, value);
    picker.close();
};

// Closes every open picker, e.g. when the settings modal shuts.
OSA.closeSettingsModelPickers = function() {
    for (const id in OSA.settingsModelPickers) {
        OSA.settingsModelPickers[id].close();
    }
};

const assert = require('node:assert/strict');
const test = require('node:test');

let Window;

test.before(async () => {
    ({ Window } = await import('happy-dom'));
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.localStorage = window.localStorage;
    global.Node = window.Node;
    global.HTMLElement = window.HTMLElement;
    global.requestAnimationFrame = () => 1;
    global.cancelAnimationFrame = () => {};
    global.OSA = window.OSA = {};

    require('../js/state.js');
    require('../js/utils.js');
    require('../js/model-dropdown.js');
    require('../js/providers.js');

    OSA.escapeHtml = value => String(value || '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
});

function groupTitles() {
    return Array.from(document.querySelectorAll('#model-dropdown .model-group-title'))
        .map(node => node.textContent);
}

test('composer search lists connected providers first when the catalog has not loaded', () => {
    document.body.innerHTML = '<div id="model-dropdown"><div class="model-dropdown-list"></div></div>';
    OSA.modelDropdownOpen = true;
    OSA.providerCatalog = { providers: [], all_models: [] };

    OSA.renderModelSearchResults([
        { id: 'gpt-4o', name: 'GPT-4o', provider_id: 'openai', provider_name: 'OpenAI', available: false, category: '' },
        { id: 'qwen3', name: 'Qwen3', provider_id: 'ollama', provider_name: 'Ollama (Local)', available: true, category: '' },
    ], '');

    assert.deepEqual(groupTitles(), ['Ollama (Local)', 'OpenAI']);
});

test('composer search lists connected providers first from the catalog flag', () => {
    document.body.innerHTML = '<div id="model-dropdown"><div class="model-dropdown-list"></div></div>';
    OSA.modelDropdownOpen = true;
    OSA.providerCatalog = {
        providers: [
            { id: 'openai', name: 'OpenAI', connected: false },
            { id: 'anthropic', name: 'Anthropic', connected: true },
        ],
        all_models: [],
    };

    OSA.renderModelSearchResults([
        { id: 'gpt-4o', name: 'GPT-4o', provider_id: 'openai', provider_name: 'OpenAI', available: false, category: '' },
        { id: 'claude', name: 'Claude', provider_id: 'anthropic', provider_name: 'Anthropic', available: false, category: '' },
    ], '');

    assert.deepEqual(groupTitles(), ['Anthropic', 'OpenAI']);
});

test('settings connected rows follow the dropdown provider order', () => {
    const providers = [
        { id: 'anthropic', name: 'Anthropic', models: [] },
        { id: 'openai', name: 'OpenAI', models: [] },
    ];
    const connectedMap = {
        anthropic: { id: 'anthropic', name: 'Anthropic', model: 'claude', key_count: 1, api_key_preview: '(configured)' },
        openai: { id: 'openai', name: 'OpenAI', model: 'gpt-4o', key_count: 1, api_key_preview: '(configured)' },
    };
    OSA.providerCatalog = { providers, all_models: [], model_menu: {} };

    const host = document.createElement('div');
    host.innerHTML = OSA.connectedProviderListHtml(providers, connectedMap);
    const order = Array.from(host.querySelectorAll('.provider-row')).map(node => node.dataset.providerId);

    assert.deepEqual(order, ['openai', 'anthropic']);
});

test('connected row shows the menu switch state and counts models in the menu', () => {
    const models = [
        { id: 'gpt-4o', name: 'GPT-4o', provider_id: 'openai' },
        { id: 'gpt-4o-mini', name: 'GPT-4o mini', provider_id: 'openai' },
    ];
    OSA.providerCatalog = {
        providers: [{ id: 'openai', name: 'OpenAI', models }],
        all_models: [],
        model_menu: { hidden_providers: [], hidden_models: ['openai:gpt-4o'] },
    };

    const host = document.createElement('div');
    host.innerHTML = OSA.providerRowHtml(
        { id: 'openai', name: 'OpenAI', models },
        { id: 'openai', name: 'OpenAI', model: 'gpt-4o-mini', key_count: 1, api_key_preview: '(configured)', is_default: true },
    );

    assert.equal(host.querySelector('.provider-row-models-count').textContent, '2 models · 1 in menu');
    assert.equal(host.querySelector('.provider-row-controls input').checked, true);

    OSA.providerCatalog.model_menu.hidden_providers = ['openai'];
    host.innerHTML = OSA.providerRowHtml(
        { id: 'openai', name: 'OpenAI', models },
        { id: 'openai', name: 'OpenAI', model: 'gpt-4o-mini', key_count: 1, api_key_preview: '(configured)' },
    );
    assert.equal(host.querySelector('.provider-row-controls input').checked, false);
    assert.equal(host.querySelector('.provider-row-models-count').textContent, '2 models · 0 in menu');
});

test('model menu hides providers and models from the dropdown but keeps the active model', async () => {
    document.body.innerHTML = '<div id="model-dropdown"><div class="model-dropdown-list"></div></div>';
    OSA.modelDropdownOpen = true;
    OSA.modelSearchQuery = '';
    OSA.favourites = [];
    OSA.currentModelId = 'gpt-4o-mini';
    OSA.currentModelProviderId = 'openai';
    const model = (id, providerId, providerName) => ({
        id, name: id, provider_id: providerId, provider_name: providerName,
        context_window: 128000, supports_tools: true, supports_vision: false, category: '',
    });
    OSA.providerCatalog = {
        providers: [
            { id: 'openai', name: 'OpenAI', connected: true, models: [] },
            { id: 'openrouter', name: 'OpenRouter', connected: true, models: [] },
        ],
        all_models: [
            model('gpt-4o', 'openai', 'OpenAI'),
            model('gpt-4o-mini', 'openai', 'OpenAI'),
            model('or-model', 'openrouter', 'OpenRouter'),
        ],
        model_menu: { hidden_providers: ['openrouter'], hidden_models: ['openai:gpt-4o'] },
    };
    OSA.clearModelDropdownRenderCache();

    await OSA.renderModelDropdown();

    const ids = Array.from(document.querySelectorAll('#model-dropdown .model-option[data-model-id]'))
        .map(node => node.dataset.modelId);
    assert.deepEqual(ids, ['gpt-4o-mini']);
});

test('a favourite shows in its provider group as well as the favourites block, once per block', async () => {
    document.body.innerHTML = '<div id="model-dropdown"><div class="model-dropdown-list"></div></div>';
    OSA.modelDropdownOpen = true;
    OSA.modelSearchQuery = '';
    OSA.currentModelId = '';
    OSA.currentModelProviderId = '';
    OSA.favourites = [{ id: 'gpt-4o-mini', provider_id: 'openai', name: 'GPT-4o mini', provider_name: 'OpenAI' }];
    const model = (id, providerId, providerName, extra) => Object.assign({
        id, name: id, provider_id: providerId, provider_name: providerName,
        context_window: 128000, supports_tools: true, supports_vision: false, category: '',
    }, extra || {});
    OSA.providerCatalog = {
        providers: [{ id: 'openai', name: 'OpenAI', connected: true, models: [] }],
        all_models: [model('gpt-4o', 'openai', 'OpenAI'), model('gpt-4o-mini', 'openai', 'OpenAI')],
        model_menu: {},
    };
    OSA.clearModelDropdownRenderCache();

    await OSA.renderModelDropdown();

    const ids = Array.from(document.querySelectorAll('#model-dropdown .model-option[data-model-id]'))
        .map(node => node.dataset.modelId);
    assert.deepEqual(ids, ['gpt-4o-mini', 'gpt-4o', 'gpt-4o-mini']);
    OSA.favourites = [];
});

test('an age threshold hides older models but keeps undated models and favourites', async () => {
    document.body.innerHTML = '<div id="model-dropdown"><div class="model-dropdown-list"></div></div>';
    OSA.modelDropdownOpen = true;
    OSA.modelSearchQuery = '';
    OSA.currentModelId = '';
    OSA.favourites = [{ id: 'ancient', provider_id: 'openai', name: 'Ancient', provider_name: 'OpenAI' }];
    const model = (id, releaseDate) => ({
        id, name: id, provider_id: 'openai', provider_name: 'OpenAI',
        context_window: 128000, supports_tools: true, supports_vision: false, category: '',
        release_date: releaseDate,
    });
    const today = new Date();
    const recent = new Date(today.getTime() - 30 * 86400000).toISOString().slice(0, 10);
    OSA.providerCatalog = {
        providers: [{ id: 'openai', name: 'OpenAI', connected: true, models: [] }],
        all_models: [model('ancient', '2019-01-01'), model('recent', recent), model('undated', null), model('old', '2019-06-01')],
        model_menu: { hide_older_than_days: 365 },
    };
    OSA.clearModelDropdownRenderCache();

    await OSA.renderModelDropdown();

    const ids = Array.from(document.querySelectorAll('#model-dropdown .model-option[data-model-id]'))
        .map(node => node.dataset.modelId);
    // 'ancient' is old but a favourite, so it shows in the favourites block and its provider group.
    assert.deepEqual(ids, ['ancient', 'ancient', 'recent', 'undated']);
    OSA.favourites = [];
});

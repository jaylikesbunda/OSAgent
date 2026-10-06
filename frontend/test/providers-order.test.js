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

test('settings model search lists connected providers first', () => {
    OSA.modelsConnectedMap = { anthropic: { id: 'anthropic' } };
    const providers = [
        { id: 'openai', name: 'OpenAI', models: [{ id: 'gpt-4o', name: 'GPT-4o', provider_id: 'openai', provider_name: 'OpenAI' }] },
        { id: 'anthropic', name: 'Anthropic', models: [{ id: 'claude', name: 'Claude', provider_id: 'anthropic', provider_name: 'Anthropic' }] },
    ];

    const html = OSA.modelsSearchHtml(providers, 'o', 'o');
    const host = document.createElement('div');
    host.innerHTML = html;
    const providerOrder = Array.from(host.querySelectorAll('.model-option[data-provider-id]'))
        .map(node => node.dataset.providerId);

    assert.deepEqual(providerOrder, ['anthropic', 'openai']);
});

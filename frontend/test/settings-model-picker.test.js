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
});

const model = (id, name, providerId, providerName) => ({
    id,
    name,
    provider_id: providerId,
    provider_name: providerName,
    context_window: 128000,
    supports_tools: true,
    supports_vision: false,
    category: '',
});

function setupMount(id) {
    document.body.innerHTML = `<div id="mount" class="model-picker-mount" data-model-picker="${id}"`
        + ' data-model-empty-label="Same as current model"></div>';
    OSA.settingsModelPickers = {};
    OSA.favourites = [];
    OSA.providerCatalog = {
        providers: [
            { id: 'openai', name: 'OpenAI', connected: true, models: [] },
            { id: 'anthropic', name: 'Anthropic', connected: true, models: [] },
            { id: 'mystery', name: 'Mystery', connected: false, models: [] },
        ],
        all_models: [
            model('gpt-4o', 'GPT-4o', 'openai', 'OpenAI'),
            model('gpt-4o-mini', 'GPT-4o mini', 'openai', 'OpenAI'),
            model('claude-3', 'Claude 3', 'anthropic', 'Anthropic'),
            model('hidden-model', 'Hidden', 'mystery', 'Mystery'),
        ],
        model_menu: {},
    };
    OSA.initSettingsModelPickers();
    return OSA.settingsModelPickers[id];
}

function tick() {
    return new Promise(resolve => setTimeout(resolve, 0));
}

async function openPicker(picker) {
    picker.open();
    await tick();
    await tick();
}

test('a settings mount builds a trigger and the hidden input the form saves from', () => {
    const picker = setupMount('setting-discord-model');

    const trigger = document.getElementById('setting-discord-model-trigger');
    const input = document.getElementById('setting-discord-model');
    assert.ok(trigger, 'the trigger keeps the label target in the markup');
    assert.equal(trigger.tagName, 'BUTTON');
    assert.ok(input, 'the hidden input keeps the id saveSettings reads');
    assert.equal(input.type, 'hidden');
    assert.equal(input.value, '');
    assert.equal(picker.label.textContent, 'Same as current model');
});

test('a stored provider:model value becomes the label and the input value', () => {
    const picker = setupMount('setting-discord-model');

    OSA.setSettingsModelValue('setting-discord-model', 'openai:gpt-4o');

    assert.equal(picker.input.value, 'openai:gpt-4o');
    assert.equal(picker.label.textContent, 'OpenAI · GPT-4o');
});

test('the list offers the empty row and only connected providers', async () => {
    const picker = setupMount('setting-small-model');

    await OSA.renderSettingsModelList(picker.list(), picker);

    const groups = Array.from(picker.list().querySelectorAll('.model-group-title')).map(node => node.textContent);
    assert.deepEqual(groups, ['OpenAI', 'Anthropic']);
    const empty = picker.list().querySelector('[data-model-empty]');
    assert.ok(empty);
    assert.ok(empty.classList.contains('active'), 'an empty value marks "same as current"');
    const ids = Array.from(picker.list().querySelectorAll('.model-option[data-model-id]')).map(node => node.dataset.modelId);
    assert.deepEqual(ids, ['gpt-4o', 'gpt-4o-mini', 'claude-3']);
});

test('the stored value is marked active and picking a row writes provider:model', async () => {
    const picker = setupMount('setting-discord-model');
    OSA.setSettingsModelValue('setting-discord-model', 'anthropic:claude-3');
    await openPicker(picker);

    const active = picker.list().querySelector('.model-option.active');
    assert.equal(active.dataset.modelId, 'claude-3');

    const target = picker.list().querySelector('.model-option[data-model-id="gpt-4o"]');
    target.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));

    assert.equal(picker.input.value, 'openai:gpt-4o');
    assert.equal(picker.label.textContent, 'OpenAI · GPT-4o');
    assert.ok(picker.dropdown().classList.contains('hidden'), 'picking closes the popover');
});

test('the empty row clears the field back to "same as current model"', async () => {
    const picker = setupMount('setting-small-model');
    OSA.setSettingsModelValue('setting-small-model', 'openai:gpt-4o');
    await openPicker(picker);

    picker.list().querySelector('[data-model-empty]')
        .dispatchEvent(new window.MouseEvent('click', { bubbles: true }));

    assert.equal(picker.input.value, '');
    assert.equal(picker.label.textContent, 'Same as current model');
});

test('a value that is no longer in the catalog stays visible under Current', async () => {
    const picker = setupMount('setting-subagent-model-general');
    OSA.setSettingsModelValue('setting-subagent-model-general', 'openai:gpt-5-imaginary');

    await OSA.renderSettingsModelList(picker.list(), picker);

    const groups = Array.from(picker.list().querySelectorAll('.model-group-title')).map(node => node.textContent);
    assert.equal(groups[groups.length - 1], 'Current');
    const current = picker.list().querySelector('.model-option.active');
    assert.equal(current.dataset.modelId, 'gpt-5-imaginary');
    assert.equal(current.dataset.providerId, 'openai');
});

test('the bare model name form survives a round trip', async () => {
    const picker = setupMount('setting-discord-model');
    OSA.setSettingsModelValue('setting-discord-model', 'gpt-4o');

    assert.equal(picker.input.value, 'gpt-4o');
    assert.equal(picker.label.textContent, 'OpenAI · GPT-4o');

    await OSA.renderSettingsModelList(picker.list(), picker);
    assert.equal(picker.list().querySelector('.model-option.active').dataset.modelId, 'gpt-4o');
});

test('search keeps only connected providers and is debounced through the state', async () => {
    const picker = setupMount('setting-small-model');
    OSA.getJson = async url => {
        assert.match(url, /\/api\/providers\/search\?q=gpt/);
        return [
            model('gpt-4o', 'GPT-4o', 'openai', 'OpenAI'),
            model('hidden-model', 'Hidden', 'mystery', 'Mystery'),
        ];
    };

    picker.state.searchQuery = 'gpt';
    await OSA.renderSettingsModelList(picker.list(), picker);

    const groups = Array.from(picker.list().querySelectorAll('.model-group-title')).map(node => node.textContent);
    assert.deepEqual(groups, ['OpenAI']);
});

test('arrow keys walk the rows and Enter picks the highlighted one', async () => {
    const picker = setupMount('setting-discord-model');
    OSA.setSettingsModelValue('setting-discord-model', 'openai:gpt-4o');
    await openPicker(picker);

    const search = picker.searchInput();
    search.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
    search.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true, cancelable: true }));
    search.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true, cancelable: true }));

    // Row 0 is the empty row, row 1 is gpt-4o and row 2 is gpt-4o mini.
    assert.equal(picker.input.value, 'openai:gpt-4o');
});

test('Escape closes the popover without bubbling to the settings modal', async () => {
    const picker = setupMount('setting-small-model');
    await openPicker(picker);

    let bubbled = false;
    document.addEventListener('keydown', () => { bubbled = true; });

    const event = new window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true });
    picker.searchInput().dispatchEvent(event);

    assert.equal(event.defaultPrevented, true);
    assert.equal(bubbled, false);
    assert.ok(picker.dropdown().classList.contains('hidden'));
});

test('closing the settings modal hides an open picker', async () => {
    const picker = setupMount('setting-small-model');
    await openPicker(picker);
    assert.ok(!picker.dropdown().classList.contains('hidden'));

    OSA.closeSettingsModelPickers();

    assert.ok(picker.dropdown().classList.contains('hidden'));
    assert.equal(picker.trigger().getAttribute('aria-expanded'), 'false');
});

test('initSettingsModelPickers rebuilds a picker whose mount was replaced', () => {
    const first = setupMount('setting-discord-model');
    document.body.innerHTML = '<div id="mount" class="model-picker-mount" data-model-picker="setting-discord-model"></div>';

    OSA.initSettingsModelPickers();

    const second = OSA.settingsModelPickers['setting-discord-model'];
    assert.notEqual(second, first);
    assert.ok(second.trigger().isConnected);
    assert.equal(second.emptyLabel, 'Same as current model');
});

test('the list applies the same show-in-model-menu filters as the composer dropdown', async () => {
    const picker = setupMount('setting-small-model');
    OSA.providerCatalog.model_menu = { hidden_models: ['openai:gpt-4o-mini'] };

    await OSA.renderSettingsModelList(picker.list(), picker);

    const ids = Array.from(picker.list().querySelectorAll('.model-option[data-model-id]')).map(node => node.dataset.modelId);
    assert.deepEqual(ids, ['gpt-4o', 'claude-3']);
});

test('a model hidden from the menu is gone from the picker but the stored value stays visible', async () => {
    const picker = setupMount('setting-discord-model');
    OSA.setSettingsModelValue('setting-discord-model', 'openai:gpt-4o');
    // gpt-4o is hidden from the model menu: it must still be visible (and
    // active) here, otherwise the saved setting would look lost.
    OSA.providerCatalog.model_menu = { hidden_models: ['openai:gpt-4o'] };

    await OSA.renderSettingsModelList(picker.list(), picker);

    const active = picker.list().querySelector('.model-option.active');
    assert.ok(active);
    assert.equal(active.dataset.modelId, 'gpt-4o');

    const ids = Array.from(picker.list().querySelectorAll('.model-option[data-model-id]')).map(node => node.dataset.modelId);
    assert.deepEqual(ids, ['gpt-4o', 'gpt-4o-mini', 'claude-3']);
});

test('a hidden provider is filtered out unless the stored value lives there', async () => {
    const picker = setupMount('setting-discord-model');
    OSA.setSettingsModelValue('setting-discord-model', 'anthropic:claude-3');
    OSA.providerCatalog.model_menu = { hidden_providers: ['anthropic'] };

    await OSA.renderSettingsModelList(picker.list(), picker);

    const groups = Array.from(picker.list().querySelectorAll('.model-group-title')).map(node => node.textContent);
    assert.deepEqual(groups, ['OpenAI', 'Anthropic']);
    const ids = Array.from(picker.list().querySelectorAll('.model-option[data-model-id]')).map(node => node.dataset.modelId);
    assert.deepEqual(ids, ['gpt-4o', 'gpt-4o-mini', 'claude-3']);
});

test('search results respect the model-menu filters too', async () => {
    const picker = setupMount('setting-small-model');
    OSA.providerCatalog.model_menu = { hidden_models: ['openai:gpt-4o-mini'] };
    OSA.getJson = async () => [
        model('gpt-4o', 'GPT-4o', 'openai', 'OpenAI'),
        model('gpt-4o-mini', 'GPT-4o mini', 'openai', 'OpenAI'),
    ];

    picker.state.searchQuery = 'gpt';
    await OSA.renderSettingsModelList(picker.list(), picker);

    const ids = Array.from(picker.list().querySelectorAll('.model-option[data-model-id]')).map(node => node.dataset.modelId);
    assert.deepEqual(ids, ['gpt-4o']);
});

test('the settings popover is wider than the composer menu', async () => {
    const picker = setupMount('setting-discord-model');
    // The picker asks for a wider minimum than the composer's compact menu.
    assert.equal(picker.positionOptions.minWidth, 520);

    await openPicker(picker);

    assert.equal(picker.dropdown().style.width, '520px');
});

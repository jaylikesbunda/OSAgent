const assert = require('node:assert/strict');
const test = require('node:test');
const { performance } = require('node:perf_hooks');

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
    global.DOMException = window.DOMException || DOMException;
    global.requestAnimationFrame = callback => callback(performance.now());
    global.cancelAnimationFrame = () => {};
    global.OSA = window.OSA = {};

    // app.js performs its normal auth bootstrap when loaded. Keep that
    // bootstrap harmless; the tests install their own fetch router below.
    document.body.innerHTML = '<div id="login-view"></div><div id="app-view"></div>';
    OSA.initTheme = () => {};
    global.fetch = window.fetch = async () => jsonResponse({ required: true });

    require('../js/state.js');
    require('../js/utils.js');
    require('../js/api.js');
    require('../js/messages.js');
    require('../js/transcript.js');
    require('../js/tools.js');
    require('../js/app.js');
    require('../js/providers.js');
});

test.beforeEach(() => {
    document.body.replaceChildren();
    OSA.SessionStore = {};
    OSA.sessionSelectionRequestId = 0;
    OSA.sessionSelectionAbortController = null;
    OSA.currentSession = null;
    OSA.currentSessionId = null;
    OSA.currentModelId = null;
    OSA.currentModelProviderId = null;
    OSA._contentMatchIds = null;
    OSA._collapsedGroups = new Set();
    OSA.unreadSessions = {};
    OSA.tmodelReset();
});

function jsonResponse(body, status = 200) {
    return {
        ok: status >= 200 && status < 300,
        status,
        json: async () => JSON.parse(JSON.stringify(body)),
    };
}

function stats(samples) {
    const sorted = [...samples].sort((a, b) => a - b);
    const percentile = fraction => sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * fraction))];
    return {
        min: sorted[0],
        p50: percentile(0.5),
        p95: percentile(0.95),
        max: sorted[sorted.length - 1],
        average: samples.reduce((sum, value) => sum + value, 0) / samples.length,
    };
}

async function benchmark(name, operation, iterations = 7) {
    // Warm up module-level caches and JIT code before recording samples.
    await operation();
    const samples = [];
    for (let i = 0; i < iterations; i++) {
        const start = performance.now();
        await operation();
        samples.push(performance.now() - start);
    }
    const result = stats(samples);
    console.log(`[webui benchmark] ${name}`, {
        samples: samples.length,
        minMs: Number(result.min.toFixed(2)),
        p50Ms: Number(result.p50.toFixed(2)),
        p95Ms: Number(result.p95.toFixed(2)),
        maxMs: Number(result.max.toFixed(2)),
    });
    return result;
}

function collectGarbage() {
    if (typeof global.gc !== 'function') return false;
    // A second collection helps clear objects finalized by the first pass.
    global.gc();
    global.gc();
    return true;
}

function memorySnapshot() {
    const usage = process.memoryUsage();
    return {
        heapUsedMb: usage.heapUsed / 1024 / 1024,
        rssMb: usage.rss / 1024 / 1024,
    };
}

async function memoryBenchmark(name, operation, iterations = 25) {
    await operation();
    const gcAvailable = collectGarbage();
    const before = memorySnapshot();
    for (let i = 0; i < iterations; i++) await operation();
    const peak = memorySnapshot();
    collectGarbage();
    const after = memorySnapshot();
    const result = {
        gcAvailable,
        heapRetainedMb: after.heapUsedMb - before.heapUsedMb,
        heapPeakMb: peak.heapUsedMb - before.heapUsedMb,
        rssDeltaMb: after.rssMb - before.rssMb,
    };
    console.log(`[webui memory] ${name}`, {
        iterations,
        gcAvailable,
        heapRetainedMb: Number(result.heapRetainedMb.toFixed(2)),
        heapPeakMb: Number(result.heapPeakMb.toFixed(2)),
        rssDeltaMb: Number(result.rssDeltaMb.toFixed(2)),
    });
    return result;
}

function makeMessageFixture(sessionId, cycles = 12) {
    const messages = [];
    const tools = [];

    for (let cycle = 0; cycle < cycles; cycle++) {
        messages.push({
            role: 'user',
            content: `Inspect module ${cycle}`,
            timestamp: `2026-09-19T00:${String(cycle).padStart(2, '0')}:00Z`,
            metadata: { client_message_id: `${sessionId}-client-${cycle}` },
        });

        const assistantIndex = messages.length;
        const callId = `${sessionId}-call-${cycle}`;
        messages.push({
            role: 'assistant',
            content: `I will inspect module ${cycle}.`,
            thinking: `Reasoning about module ${cycle}.`,
            timestamp: `2026-09-19T00:${String(cycle).padStart(2, '0')}:01Z`,
            tool_calls: [{ id: callId }],
            metadata: { kind: 'tool_prelude' },
        });
        messages.push({
            role: 'tool',
            content: `result-${cycle}`,
            tool_call_id: callId,
            metadata: {},
        });
        messages.push({
            role: 'assistant',
            content: `Module ${cycle} is ready.`,
            timestamp: `2026-09-19T00:${String(cycle).padStart(2, '0')}:02Z`,
            metadata: {},
        });

        tools.push({
            tool_call_id: callId,
            tool_name: 'read_file',
            arguments: { path: `src/module-${cycle}.js` },
            output: `result-${cycle}`,
            message_index: assistantIndex,
            completed: true,
            success: true,
            timestamp: `2026-09-19T00:${String(cycle).padStart(2, '0')}:02Z`,
        });
    }

    return {
        id: sessionId,
        task_status: 'idle',
        metadata: { name: `Benchmark ${sessionId}` },
        messages,
        tools,
    };
}

function installSessionSelectionStubs() {
    document.body.innerHTML = '<div id="header-title"></div><div id="messages"></div>';
    OSA.resetTranscriptView = () => {};
    OSA.resetStreamingMessage = () => {};
    OSA.hideThinkingIndicator = () => {};
    OSA.showThinkingIndicator = () => {};
    OSA.renderEmptyTranscript = () => {};
    OSA.renderQueuedMessages = () => {};
    OSA.fetchAndRenderTodos = async () => {};
    OSA.loadSessionWorkspace = async () => {};
    OSA.loadSessionPersona = async () => {};
    OSA.loadSessionBreadcrumb = async () => {};
    OSA.connectEventSource = () => {};
    OSA.disconnectSessionChannel = () => {};
    OSA.startToolSync = () => {};
    OSA.stopToolSync = () => {};
    OSA.setSendButtonStopMode = () => {};
    OSA.resetSendButton = () => {};
    OSA.setHeaderTitleRenameable = () => {};
    OSA.setTurnStartTime = () => {};
    OSA.getShowThinkingBlocks = () => true;
    // DOM reconciliation has dedicated coverage in transcript-dom.test.js.
    // Keep this benchmark focused on selection, data merging, and model rebuild
    // rather than happy-dom's layout/rendering implementation.
    OSA.tmodelMarkDirty = () => {};
    OSA.showErrorCard = message => { throw new Error(message); };
}

function installSessionApi(sessions) {
    global.fetch = window.fetch = async (url, options = {}) => {
        if (options.signal?.aborted) {
            throw new DOMException('Aborted', 'AbortError');
        }

        const path = new URL(url, window.location.origin).pathname;
        const parts = path.split('/').filter(Boolean);
        const sessionId = parts[parts.indexOf('sessions') + 1];
        const session = sessions[sessionId];

        if (path.match(/^\/api\/sessions\/[^/]+$/)) return jsonResponse(session);
        if (path.endsWith('/tools')) return jsonResponse(session?.tools || []);
        if (path.endsWith('/subagents')) return jsonResponse({ subagents: [], has_running: false });
        if (path.endsWith('/history')) return jsonResponse([{ sequence: 100 }]);
        if (path.endsWith('/queue')) return jsonResponse([]);
        if (path.endsWith('/checkpoints')) return jsonResponse([]);
        if (path.endsWith('/todos')) return jsonResponse([]);
        if (path.endsWith('/workspace')) return jsonResponse({ id: 'default', path: '' });
        if (path.endsWith('/parent')) return jsonResponse({ session: null });
        return jsonResponse({});
    };
}

test('session switching reconstructs messages and tool cards without duplication', async () => {
    installSessionSelectionStubs();
    const sessionA = makeMessageFixture('session-a', 3);
    const sessionB = makeMessageFixture('session-b', 2);
    installSessionApi({ 'session-a': sessionA, 'session-b': sessionB });

    await OSA.selectSession('session-a');
    await OSA.selectSession('session-b');
    await OSA.selectSession('session-a');

    const messages = OSA.TModel.items.filter(item => item.kind === 'message');
    const tools = OSA.TModel.items.filter(item => item.kind === 'tool');
    assert.equal(OSA.getCurrentSession().id, 'session-a');
    assert.equal(messages.length, 3 * 3);
    assert.equal(tools.length, 3);
    assert.deepEqual(messages.filter(item => item.role === 'user').map(item => item.content), [
        'Inspect module 0',
        'Inspect module 1',
        'Inspect module 2',
    ]);
    assert.deepEqual(tools.map(item => item.prelude), [
        'I will inspect module 0.',
        'I will inspect module 1.',
        'I will inspect module 2.',
    ]);
    assert.equal(new Set(OSA.TModel.items.map(item => item.key)).size, OSA.TModel.items.length);
});

test('session switching benchmark reports end-to-end selection latency', async () => {
    installSessionSelectionStubs();
    const sessions = {};
    for (let i = 0; i < 4; i++) sessions[`session-${i}`] = makeMessageFixture(`session-${i}`, 8);
    installSessionApi(sessions);

    let next = 0;
    const result = await benchmark('session switch (API mocked)', async () => {
        await OSA.selectSession(`session-${next % 4}`);
        next++;
    }, 5);

    assert.ok(Number.isFinite(result.p50) && result.p50 >= 0);
    assert.equal(OSA.getCurrentSession().id, 'session-1');
    assert.equal(OSA.TModel.items.filter(item => item.kind === 'message').length, 8 * 3);
});

test('transcript reconstruction benchmark preserves ordering at realistic history size', async () => {
    const session = makeMessageFixture('large-session', 60);
    const rebuild = () => OSA.rebuildTranscriptFromSession(session, session.tools, [], { reason: 'benchmark' });

    rebuild();
    const result = await benchmark('transcript rebuild (240 persisted messages)', rebuild, 8);

    const items = OSA.TModel.items;
    assert.ok(Number.isFinite(result.p50) && result.p50 >= 0);
    assert.equal(items.filter(item => item.kind === 'tool').length, 60);
    assert.equal(items.filter(item => item.role === 'user').length, 60);
    assert.equal(items.filter(item => item.role === 'assistant').length, 120);
    assert.equal(items[0].content, 'Inspect module 0');
    assert.equal(items.at(-1).content, 'Module 59 is ready.');
    assert.equal(new Set(items.map(item => item.key)).size, items.length);
});

function makeModel(id, provider, category = 'popular') {
    return {
        id: `${provider.id}/model-${id}`,
        name: `Model ${id}`,
        provider_id: provider.id,
        provider_name: provider.name,
        category,
        context_window: 128000,
        supports_tools: true,
        supports_vision: id % 3 === 0,
    };
}

function makeCatalog() {
    const providers = [];
    const allModels = [];
    for (let providerIndex = 0; providerIndex < 8; providerIndex++) {
        const provider = {
            id: `provider-${providerIndex}`,
            name: `Provider ${providerIndex}`,
            connected: providerIndex < 3,
            models: [],
        };
        for (let modelIndex = 0; modelIndex < 80; modelIndex++) {
            const model = makeModel(providerIndex * 80 + modelIndex, provider);
            provider.models.push(model);
            allModels.push(model);
        }
        providers.push(provider);
    }
    return { providers, all_models: allModels };
}

function installModelPickerDom(catalog) {
    document.body.innerHTML = `
        <button id="model-input"><span id="model-input-label"></span></button>
        <div id="model-dropdown" class="hidden">
            <input id="model-search" />
            <div class="model-dropdown-list"></div>
        </div>
        <div id="model-catalog-list"></div>
    `;
    OSA.providerCatalog = catalog;
    OSA.modelsConnectedMap = {};
    OSA.providerCatalogPromise = null;
    OSA.providerCatalogFetchedAt = Date.now();
    OSA.clearModelDropdownRenderCache();
    OSA.modelDropdownOpen = true;
    OSA.modelSearchQuery = '';
    OSA.modelOptionDelegationReady = false;
    OSA.favourites = [];
    OSA.afterModelDropdownRender();
}

test('model dropdown and remote search benchmarks keep results complete', async () => {
    const catalog = makeCatalog();
    installModelPickerDom(catalog);
    const matchingModels = catalog.all_models.filter(model => model.id.includes('model-1')).slice(0, 40);
    OSA.getJson = async url => {
        const query = new URL(url, window.location.origin).searchParams.get('q') || '';
        return matchingModels.filter(model => model.id.includes(query));
    };

    OSA.clearModelDropdownRenderCache();
    const coldStart = performance.now();
    await OSA.renderModelDropdown();
    const coldMs = performance.now() - coldStart;
    console.log('[webui benchmark] model dropdown cold render', { ms: Number(coldMs.toFixed(2)) });

    const firstOption = document.querySelector('.model-option[data-model-id]');
    const openResult = await benchmark('model dropdown cached render (240 connected models)', async () => {
        OSA.modelSearchQuery = '';
        await OSA.renderModelDropdown();
    }, 8);
    assert.ok(document.querySelectorAll('.model-option[data-model-id]').length >= 240);
    assert.equal(document.querySelector('.model-option[data-model-id]'), firstOption);

    const searchResult = await benchmark('model dropdown remote search render', async () => {
        OSA.modelSearchQuery = 'model-1';
        await OSA.renderModelDropdown();
    }, 8);
    assert.ok(document.querySelectorAll('.model-option[data-model-id]').length > 0);
    assert.match(document.querySelector('.model-option[data-model-id]').dataset.modelId, /model-1/);
    assert.ok(openResult.p50 >= 0 && searchResult.p50 >= 0);
});

test('settings model and session sidebar searches benchmark local filtering', async () => {
    const catalog = makeCatalog();
    installModelPickerDom(catalog);
    const modelResult = await benchmark('settings model catalog filter', () => {
        OSA.filterSettingsModels('model-1');
    }, 10);
    assert.ok(document.querySelectorAll('#model-catalog-list .model-option').length > 0);

    const sessions = [];
    for (let i = 0; i < 1200; i++) {
        sessions.push(`<div class="session-item" data-session-id="session-${i}" data-session-source="web">${i % 100 === 0 ? 'needle' : 'session'} ${i}</div>`);
    }
    document.body.innerHTML = sessions.join('');
    OSA._contentMatchIds = new Set(['session-1199']);
    const sessionResult = await benchmark('sidebar session filter (1200 rows)', () => {
        OSA.filterSessions('needle');
    }, 10);

    const visible = [...document.querySelectorAll('.session-item')]
        .filter(item => item.style.display !== 'none');
    assert.ok(visible.some(item => item.dataset.sessionId === 'session-1199'));
    assert.ok(modelResult.p50 >= 0 && sessionResult.p50 >= 0);
});

test('memory benchmark reports retained heap after repeated UI workflows', async () => {
    const session = makeMessageFixture('memory-session', 60);
    const transcriptResult = await memoryBenchmark(
        'repeated transcript rebuild (240 persisted messages)',
        () => OSA.rebuildTranscriptFromSession(session, session.tools, [], { reason: 'memory-benchmark' }),
    );

    const catalog = makeCatalog();
    installModelPickerDom(catalog);
    const models = catalog.all_models;
    const dropdownResult = await memoryBenchmark(
        'repeated model option generation (240 connected models)',
        () => {
            let html = '';
            for (const model of models) {
                html += OSA.buildModelOptionHtml(model, model.provider_id, '', {});
            }
            return html;
        },
    );

    for (const result of [transcriptResult, dropdownResult]) {
        assert.equal(result.gcAvailable, true);
        assert.ok(Number.isFinite(result.heapRetainedMb));
        assert.ok(Number.isFinite(result.rssDeltaMb));
    }

    // Memory budgets are environment-specific. CI can opt into a hard budget
    // without making local runs flaky: WEBUI_MEMORY_BUDGET_MB=32.
    const budgetMb = Number(process.env.WEBUI_MEMORY_BUDGET_MB || 0);
    if (budgetMb > 0) {
        assert.ok(
            transcriptResult.heapRetainedMb <= budgetMb,
            `transcript retained ${transcriptResult.heapRetainedMb.toFixed(2)} MB (budget ${budgetMb} MB)`,
        );
        assert.ok(
            dropdownResult.heapRetainedMb <= budgetMb,
            `dropdown retained ${dropdownResult.heapRetainedMb.toFixed(2)} MB (budget ${budgetMb} MB)`,
        );
    }
});

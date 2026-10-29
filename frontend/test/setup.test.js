const assert = require('node:assert/strict');
const test = require('node:test');

let Window;

test.before(async () => {
    ({ Window } = await import('happy-dom'));
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.OSA = window.OSA = {};

    require('../js/setup.js');
});

function mockResponse(status, body) {
    return {
        ok: status >= 200 && status < 300,
        status: status,
        json: async () => body
    };
}

test('setupNeeded trusts the server readiness flag', async () => {
    OSA.fetchWithAuth = async () => mockResponse(200, { needs_setup: true });
    assert.equal(await OSA.setupNeeded(), true);

    OSA.fetchWithAuth = async () => mockResponse(200, { needs_setup: false });
    assert.equal(await OSA.setupNeeded(), false);
});

test('setupNeeded falls back to config when the endpoint is missing', async () => {
    OSA.fetchWithAuth = async (url) => {
        if (url === '/api/setup/state') return mockResponse(404, {});
        return mockResponse(200, {
            providers: [{ id: 'openrouter' }],
            default_model: 'anthropic/claude-sonnet-4',
            agent: { workspace: '~/.osagent/workspace' }
        });
    };

    assert.equal(await OSA.setupNeeded(), false);
});

test('a fresh config with no provider entries still needs setup', async () => {
    OSA.fetchWithAuth = async (url) => {
        if (url === '/api/setup/state') return mockResponse(404, {});
        return mockResponse(200, {
            providers: [],
            default_model: 'gpt-4.1',
            agent: { workspace: '~/.osagent/workspace' }
        });
    };

    assert.equal(await OSA.setupNeeded(), true);
});

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
    global.requestAnimationFrame = () => 1;
    global.cancelAnimationFrame = () => {};
    global.OSA = window.OSA = {};

    require('../js/utils.js');
    require('../js/api.js');
    require('../js/providers.js');

    OSA.safeUrl = (url, schemes) => {
        try {
            const parsed = new URL(String(url), 'http://localhost/');
            const allowed = schemes || ['http:', 'https:'];
            return allowed.includes(parsed.protocol) ? parsed.href : '';
        } catch (err) {
            return '';
        }
    };
    OSA.getToken = () => 'test-token';
});

function mountOAuthDom() {
    document.body.innerHTML = `
        <div id="oauth-connected-view" class="hidden"><span id="oauth-connected-account"></span></div>
        <div id="oauth-pkce-view" class="hidden">
            <span id="oauth-provider-name-pkce"></span>
            <span id="oauth-connect-text"></span>
            <div id="oauth-error-pkce" class="hidden"></div>
        </div>
        <div id="oauth-device-view" class="hidden">
            <span id="oauth-device-code-display"></span>
            <a id="oauth-device-link"></a>
            <span id="oauth-device-status-text"></span>
        </div>
        <div id="oauth-loading-view" class="hidden"><p id="oauth-loading-text"></p></div>
        <input type="checkbox" id="provider-default" />
    `;
}

test.beforeEach(() => {
    mountOAuthDom();
    OSA.currentProviderId = 'openai';
    OSA.currentOAuthFlow = null;
    OSA.oauthProviders = [{ id: 'openai', name: 'OpenAI', flow_type: 'pkce' }];
    OSA.deviceCodePollTimer = null;
    OSA.oauthStatusPollTimer = null;
    OSA.pkcePollTimer = null;
    delete window.__TAURI__;
    delete window.oauthCallback;
    // Any raw fetch() in the OAuth flow is a bug: these endpoints sit behind
    // the login gate, so every call must go through the token-injecting helper.
    global.fetch = () => { throw new Error('raw fetch used; expected OSA.fetchWithAuth'); };
});

// Behaves like the real helper: injects the Bearer token, then delegates to
// the per-test responder.
function stubAuthedFetch(responder) {
    OSA.fetchWithAuth = async (url, options) => {
        const headers = Object.assign(
            { Authorization: 'Bearer test-token' },
            (options && options.headers) || {}
        );
        return responder(url, Object.assign({}, options, { headers }));
    };
}

test('OAuth start goes through the authenticated fetch helper', async () => {
    const seen = [];
    stubAuthedFetch(async (url, options) => {
        seen.push({ url, options });
        return { ok: true, status: 200, json: async () => ({ success: false }) };
    });
    window.open = () => null;

    await OSA.initiateOAuth();

    const start = seen.find((call) => call.url === '/api/oauth/openai/start');
    assert.ok(start, 'start request was made');
    assert.equal(start.options.headers.Authorization, 'Bearer test-token');
    // Popup blocked in this harness (window.open -> null), so the external
    // browser flow must NOT start when /start itself failed.
    assert.equal(OSA.currentOAuthFlow, null);
    assert.match(document.getElementById('oauth-error-pkce').textContent, /Failed to start OAuth/);
});

test('a 401 with an empty body surfaces a readable error, not a JSON parse crash', async () => {
    stubAuthedFetch(async () => ({
        ok: false,
        status: 401,
        json: async () => { throw new SyntaxError('Unexpected end of JSON input'); },
    }));
    window.open = () => ({ closed: false, close() {}, focus() {} });

    await OSA.initiateOAuth();

    assert.match(document.getElementById('oauth-error-pkce').textContent, /401/);
});

test('Tauri shell uses the system browser and polls status to success', async () => {
    window.__TAURI__ = {};
    const calls = [];
    stubAuthedFetch(async (url, options) => {
        calls.push({ url, body: options && options.body });
        if (url.endsWith('/start')) {
            return {
                ok: true,
                status: 200,
                json: async () => ({
                    success: true,
                    flow_type: 'pkce',
                    auth_url: 'https://auth.openai.com/oauth/authorize?x=1',
                    state: 's',
                    code_verifier: 'v',
                }),
            };
        }
        if (url.endsWith('/open-browser')) {
            return { ok: true, status: 200, json: async () => ({ success: true }) };
        }
        throw new Error('unexpected ' + url);
    });
    let successes = 0;
    OSA.getJson = async (url) => {
        assert.match(url, /\/api\/oauth\/openai\/status/);
        return { status: 'active', configured: true };
    };
    // Mirror the real handler's cleanup so the assertion below is meaningful.
    OSA.onOAuthSuccess = async () => { successes += 1; OSA.cancelOAuthStatusPoll(); OSA.currentOAuthFlow = null; };
    OSA.updateOAuthUI = async () => {};

    // Run the poll timer inline so the test stays deterministic.
    const realSetTimeout = global.setTimeout;
    global.setTimeout = (fn) => { fn(); return 1; };
    try {
        await OSA.initiateOAuth();
    } finally {
        global.setTimeout = realSetTimeout;
    }

    const opened = calls.find((call) => call.url === '/api/oauth/openai/open-browser');
    assert.ok(opened, 'system browser was asked to open the sign-in page');
    assert.match(opened.body, /auth\.openai\.com/);
    assert.equal(successes, 1);
    assert.equal(OSA.currentOAuthFlow, null);
});

test('a blocked popup in a normal browser also falls back to the system browser', async () => {
    window.open = () => null;
    const calls = [];
    stubAuthedFetch(async (url) => {
        calls.push(url);
        if (url.endsWith('/start')) {
            return {
                ok: true,
                status: 200,
                json: async () => ({
                    success: true,
                    flow_type: 'pkce',
                    auth_url: 'https://auth.openai.com/oauth/authorize?x=1',
                    state: 's',
                    code_verifier: 'v',
                }),
            };
        }
        return { ok: true, status: 200, json: async () => ({ success: true }) };
    });
    OSA.getJson = async () => ({ status: 'not_configured', configured: false });
    const realSetTimeout = global.setTimeout;
    let scheduled = 0;
    global.setTimeout = () => { scheduled += 1; return scheduled; };
    try {
        await OSA.initiateOAuth();
    } finally {
        global.setTimeout = realSetTimeout;
    }

    assert.ok(calls.includes('/api/oauth/openai/open-browser'));
    assert.equal(OSA.currentOAuthFlow.type, 'external_browser');
    assert.match(document.getElementById('oauth-loading-text').textContent, /browser/);
    OSA.cancelOAuthStatusPoll();
});

test('device-code polling sends the auth token', async () => {
    const seen = [];
    stubAuthedFetch(async (url, options) => {
        seen.push({ url, options });
        return { ok: true, status: 200, json: async () => ({ success: true, connected: true }) };
    });
    OSA.currentOAuthFlow = { type: 'device_code', deviceCode: 'dc', interval: 0 };
    OSA.onOAuthSuccess = async () => {};
    const realSetTimeout = global.setTimeout;
    global.setTimeout = (fn) => { fn(); return 1; };
    try {
        OSA.pollDeviceCode();
        await new Promise((resolve) => setImmediate(resolve));
    } finally {
        global.setTimeout = realSetTimeout;
    }
    const poll = seen.find((call) => call.url === '/api/oauth/openai/device');
    assert.ok(poll, 'device poll request was made');
    assert.equal(poll.options.headers.Authorization, 'Bearer test-token');
});

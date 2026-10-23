const assert = require('node:assert/strict');
const test = require('node:test');

let Window;

test.before(async () => {
    ({ Window } = await import('happy-dom'));
    const window = new Window({ url: 'http://127.0.0.1:8765/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.localStorage = window.localStorage;
    global.requestAnimationFrame = () => 1;
    global.cancelAnimationFrame = () => {};
    global.confirm = () => true;
    global.OSA = window.OSA = {};

    require('../js/utils.js');
    require('../js/api.js');
    require('../js/github.js');
});

test.beforeEach(() => {
    document.body.innerHTML = OSA.GithubUI.buildPaneHTML();
    OSA.GithubUI.deviceFlowActive = false;
    OSA.GithubUI.devicePollTimer = null;
    OSA.GithubUI.devicePollInFlight = false;
    OSA.GithubUI.devicePollIntervalMs = 5000;
    OSA.GithubUI.devicePollFailures = 0;
    // Capture scheduled polls instead of running real timers.
    OSA.GithubUI.__scheduled = null;
    OSA.GithubUI.scheduleDevicePoll = function (delay) {
        this.__scheduled = { delay, fn: () => this.pollDeviceFlow() };
    };
});

test('a transient poll failure keeps the sign-in alive instead of stranding it', async () => {
    OSA.GithubUI.deviceFlowActive = true;
    let calls = 0;
    OSA.GithubUI.fetchJson = async (url) => {
        assert.equal(url, '/api/github/oauth/poll');
        calls += 1;
        if (calls === 1) throw new TypeError('Failed to fetch');
        return { status: 'pending' };
    };

    await OSA.GithubUI.pollDeviceFlow();

    // The flow must survive the aborted request (the webview was suspended
    // while the user authorized in the system browser).
    assert.equal(OSA.GithubUI.deviceFlowActive, true);
    assert.equal(calls, 1);
    assert.ok(OSA.GithubUI.__scheduled, 'the next poll was scheduled');
    assert.match(document.getElementById('github-device-status').textContent, /retrying/i);
});

test('a completed poll registers the sign-in and stops polling', async () => {
    OSA.GithubUI.deviceFlowActive = true;
    let loaded = false;
    OSA.GithubUI.fetchJson = async () => ({ status: 'complete', login: 'octocat' });
    OSA.GithubUI.load = async () => { loaded = true; };

    await OSA.GithubUI.pollDeviceFlow();

    assert.equal(loaded, true, 'the pane reloaded to show the connected account');
    assert.equal(OSA.GithubUI.deviceFlowActive, false, 'the flow stopped');
    assert.equal(OSA.GithubUI.devicePollTimer, null, 'no timer is left armed');
    assert.match(document.querySelector('.github-message-slot').textContent, /octocat/);
});

test('the server reporting no sign-in stops polling and refreshes quietly', async () => {
    OSA.GithubUI.deviceFlowActive = true;
    let refreshed = false;
    OSA.GithubUI.fetchJson = async () => {
        throw new Error('No GitHub sign-in in progress. Start it from Settings > GitHub.');
    };
    OSA.GithubUI.refreshStatus = async () => { refreshed = true; };

    await OSA.GithubUI.pollDeviceFlow();

    assert.equal(OSA.GithubUI.deviceFlowActive, false);
    assert.equal(refreshed, true);
});

test('returning to the page re-polls immediately instead of waiting on a frozen timer', () => {
    OSA.GithubUI.deviceFlowActive = true;
    OSA.GithubUI.bindVisibilityPolling();
    OSA.GithubUI.__scheduled = null;

    window.dispatchEvent(new window.Event('focus'));

    assert.ok(OSA.GithubUI.__scheduled, 'a poll was scheduled on focus');
    assert.equal(OSA.GithubUI.__scheduled.delay, 0, 'the wake poll runs immediately');
});

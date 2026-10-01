const assert = require('node:assert/strict');
const test = require('node:test');
global.window = global;
global.OSA = {};
global.document = { getElementById: () => null };
require('../js/stop.js');
require('../js/api.js');
const originalTimeout = global.setTimeout;
const originalClearTimeout = global.clearTimeout;
let session, processing, stopping, timer, notices, resets;
test.beforeEach(() => {
    session = { id: 'one' };
    processing = true;
    stopping = false;
    timer = null;
    notices = [];
    resets = 0;
    OSA._stopTimeout = null;
    OSA.getCurrentSession = () => session;
    OSA.isAgentStopping = () => stopping;
    OSA.setStopping = value => { stopping = value; };
    OSA.setProcessing = value => { processing = value; };
    OSA.setSendButtonStopMode = () => {};
    OSA.resetSendButton = () => { resets++; };
    OSA.showToast = text => notices.push(text);
    OSA.cancelSession = async () => ({ success: true });
    global.setTimeout = callback => { timer = callback; return 1; };
    global.clearTimeout = () => {};
});
test.after(() => { global.setTimeout = originalTimeout; global.clearTimeout = originalClearTimeout; });

test('failed Stop keeps the run visible and allows retry', async () => {
    OSA.cancelSession = async () => { throw new Error('Offline'); };
    await OSA.stopGeneration();
    assert.equal(processing, true);
    assert.equal(stopping, false);
    assert.equal(resets, 0);
    assert.deepEqual(notices, ['Offline']);
});

test('a delayed Stop never resets a different chat', async () => {
    await OSA.stopGeneration();
    session = { id: 'two' };
    await timer();
    assert.equal(processing, true);
    assert.equal(resets, 0);
});

test('timeout confirms server status instead of pretending a running task stopped', async () => {
    await OSA.stopGeneration();
    OSA.fetchWithAuth = async () => ({ ok: true, json: async () => ({ task_status: 'running' }) });
    await timer();
    assert.equal(processing, true);
    assert.equal(stopping, false);
    assert.equal(resets, 0);
    assert.match(notices[0], /Still stopping/);
});

test('missed terminal event is recovered only after server confirms idle', async () => {
    await OSA.stopGeneration();
    OSA.fetchWithAuth = async () => ({ ok: true, json: async () => ({ task_status: 'active' }) });
    let cancelledSession;
    OSA.handleEventCancelled = event => { cancelledSession = event.session_id; };
    await timer();
    assert.equal(cancelledSession, 'one');
});

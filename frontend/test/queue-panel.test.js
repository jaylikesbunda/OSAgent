const assert = require('node:assert/strict');
const test = require('node:test');

let Window;

test.before(async () => {
    ({ Window } = await import('happy-dom'));
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.Node = window.Node;
    global.HTMLElement = window.HTMLElement;
    global.requestAnimationFrame = () => 1;
    global.cancelAnimationFrame = () => {};
    global.localStorage = window.localStorage;
    global.OSA = window.OSA = {};

    require('../js/state.js');
    require('../js/utils.js');
    require('../js/messages.js');

    OSA.tmodelMarkDirty = () => {};
    OSA.resizeMessageInput = () => {};
    OSA.isAgentProcessing = () => false;
});

test.beforeEach(() => {
    document.body.replaceChildren();
    const panel = document.createElement('div');
    panel.id = 'queue-panel';
    panel.className = 'queue-panel hidden';
    document.body.appendChild(panel);
    const input = document.createElement('textarea');
    input.id = 'message-input';
    document.body.appendChild(input);
    const toggle = document.createElement('button');
    toggle.id = 'followup-toggle';
    toggle.className = 'followup-toggle hidden';
    document.body.appendChild(toggle);
    OSA.queueBusy = false;
    OSA.queueEditingId = null;
    OSA.queueEditStash = null;
    OSA.SessionStore = {};
});

function seedSession(id, queue) {
    OSA.setCurrentSession({ id, task_status: 'active', messages: [], metadata: {} });
    OSA.setSessionQueue(queue);
}

test('queue panel renders one row per item with escaped text and actions', () => {
    seedSession('q1', [
        { id: 'a', content: 'first <b>thing</b>', status: 'pending', position: 1 },
        { id: 'b', content: 'second thing', status: 'pending', position: 2 },
    ]);
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    const panel = document.getElementById('queue-panel');
    assert.equal(panel.classList.contains('hidden'), false);
    const rows = panel.querySelectorAll('.queue-row');
    assert.equal(rows.length, 2);
    assert.equal(rows[0].querySelector('.queue-order').textContent, '1');
    assert.equal(rows[0].querySelector('.queue-text').textContent, 'first <b>thing</b>');
    assert.ok(rows[0].querySelector('[data-action="steer"]'));
    assert.ok(rows[0].querySelector('[data-action="remove"]'));
    assert.ok(rows[0].querySelector('[data-action="down"]'));
    assert.ok(!rows[0].querySelector('[data-action="up"]'));
    assert.ok(rows[1].querySelector('[data-action="up"]'));
});

test('empty queue hides the panel', () => {
    seedSession('q1', []);
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    assert.equal(document.getElementById('queue-panel').classList.contains('hidden'), true);
});

test('dispatching rows hide their actions', () => {
    seedSession('q1', [{ id: 'a', content: 'going', status: 'dispatching', position: 1 }]);
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    const row = document.querySelector('.queue-row');
    assert.equal(row.classList.contains('dispatching'), true);
    assert.equal(row.querySelector('.queue-actions').textContent.trim(), '');
});

test('editing loads the text into the composer and Escape restores the draft', () => {
    seedSession('q1', [{ id: 'a', content: 'queued text', status: 'pending', position: 1 }]);
    const input = document.getElementById('message-input');
    input.value = 'my draft';
    OSA.editQueuedMessage('a');
    assert.equal(OSA.queueEditingId, 'a');
    assert.equal(input.value, 'queued text');
    assert.ok(document.querySelector('.queue-row.editing'));
    OSA.cancelQueueEdit();
    assert.equal(OSA.queueEditingId, null);
    assert.equal(input.value, 'my draft');
});

test('remove issues DELETE and refreshes the queue', async () => {
    seedSession('q1', [{ id: 'a', content: 'bye', status: 'pending', position: 1 }]);
    const calls = [];
    OSA.fetchWithAuth = async (url, opts) => {
        calls.push([url, opts && opts.method]);
        return { ok: true, json: async () => ({}) };
    };
    let refreshed = false;
    OSA.refreshCurrentSessionQueue = async () => { refreshed = true; return []; };
    OSA.showErrorCard = () => {};
    await OSA.removeQueuedMessage('a');
    assert.deepEqual(calls, [['/api/sessions/q1/queue/a', 'DELETE']]);
    assert.equal(refreshed, true);
});

test('follow-up preference toggles and labels the composer button', () => {
    seedSession('q1', [{ id: 'a', content: 'x', status: 'pending', position: 1 }]);
    OSA.setFollowUpBehavior('queue');
    assert.equal(OSA.getFollowUpBehavior(), 'queue');
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    const toggle = document.getElementById('followup-toggle');
    assert.equal(toggle.classList.contains('hidden'), false);
    assert.equal(toggle.textContent, 'Queue');
    OSA.toggleFollowUpBehavior();
    assert.equal(OSA.getFollowUpBehavior(), 'steer');
    assert.equal(toggle.textContent, 'Steer');
});

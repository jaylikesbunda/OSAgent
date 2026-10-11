const assert = require('node:assert/strict');
const test = require('node:test');

test.before(async () => {
    const { Window } = await import('happy-dom');
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.localStorage = window.localStorage;
    global.OSA = window.OSA = {};

    require('../js/state.js');
    require('../js/utils.js');
    require('../js/messages.js');

    OSA.resizeMessageInput = () => {};
    OSA.updateFollowUpToggle = () => {};
    OSA.showErrorCard = () => {};
    OSA.tmodelMarkDirty = () => {};
    OSA.setThinkingStatus = () => {};
    OSA.refreshCurrentSessionQueue = () => Promise.resolve([]);
    OSA.getCurrentSession = () => ({ id: 's1' });
    OSA.escapeHtml = value => String(value || '');
});

function mountComposer() {
    document.body.innerHTML = '<div id="messages"></div><div id="queue-panel" class="hidden"></div><textarea id="message-input"></textarea>';
    return document.getElementById('message-input');
}

test('an edited queued message that leaves the queue ends edit mode and restores the draft', () => {
    const input = mountComposer();
    OSA.queueEditingId = 'q1';
    OSA.queueEditStash = { text: 'my draft', cursor: 8 };
    input.value = 'edited text';

    // The edited message was dispatched, so it is no longer in the queue.
    OSA.renderQueuedMessages([{ id: 'q2', content: 'other', status: 'pending', position: 1 }]);

    assert.equal(OSA.queueEditingId, null);
    assert.equal(OSA.queueEditStash, null);
    assert.equal(input.value, 'my draft');
});

test('edit mode stays on while the edited message is still queued', () => {
    const input = mountComposer();
    OSA.queueEditingId = 'q1';
    OSA.queueEditStash = { text: '', cursor: 0 };
    input.value = 'still editing';

    OSA.renderQueuedMessages([{ id: 'q1', content: 'queued', status: 'pending', position: 1 }]);

    assert.equal(OSA.queueEditingId, 'q1');
    assert.equal(input.value, 'still editing');
});

test('a message sent while busy shows as a local row with no actions', () => {
    mountComposer();
    OSA.setSessionQueue([]);

    OSA.addLocalQueueItem('client-1', 'hello there');

    const queue = OSA.getSessionQueue();
    assert.equal(queue.length, 1);
    assert.equal(queue[0].local, true);
    const row = document.querySelector('.queue-row.local');
    assert.ok(row, 'local row rendered');
    assert.match(row.textContent, /Sending/);
    assert.equal(row.querySelector('[data-action="remove"]'), null);
    assert.equal(row.querySelector('[data-action="steer"]'), null);

    OSA.removeLocalQueueItem('client-1');
    assert.equal(OSA.getSessionQueue().length, 0);
});

test('removing a queued message takes its row away before the server answers', () => {
    mountComposer();
    OSA.setSessionQueue([
        { id: 'q1', content: 'first', status: 'pending', position: 1 },
        { id: 'q2', content: 'second', status: 'pending', position: 2 },
    ]);
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    let resolveRequest;
    OSA.fetchWithAuth = () => new Promise(resolve => { resolveRequest = resolve; });

    OSA.removeQueuedMessage('q1');

    assert.deepEqual(OSA.getSessionQueue().map(q => q.id), ['q2']);
    assert.equal(document.querySelectorAll('.queue-row').length, 1);
    resolveRequest({ ok: true, status: 200, json: async () => ({}) });
});

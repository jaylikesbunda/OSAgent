const assert = require('node:assert/strict');
const test = require('node:test');

global.window = global;
global.requestAnimationFrame = () => 1;
global.cancelAnimationFrame = () => {};
require('../js/state.js');
require('../js/utils.js');
require('../js/messages.js');
require('../js/transcript.js');
require('../js/tools.js');

function makeSession(id) {
    return { id, task_status: 'active', messages: [], metadata: {} };
}

function setup() {
    OSA.SessionStore = {};
    OSA.tmodelReset();
    const sessionA = makeSession('session-a');
    const sessionB = makeSession('session-b');
    OSA.setCurrentSession(sessionA);
    OSA.resetMessageChain(sessionA.id);
    OSA.resetMessageChain(sessionB.id);
    OSA.getShowThinkingBlocks = () => true;
    OSA.stripSpeakBlock = text => text || '';
    OSA.showThinkingIndicator = () => {};
    OSA.hideThinkingIndicator = () => {};
    OSA.setSendButtonStopMode = () => {};
    OSA.renderQueuedMessages = () => {};
    OSA.startToolSync = () => {};
    OSA.stopToolSync = () => {};
    OSA.setSessionSidebarRunning = () => {};
    OSA.loadSessions = () => {};
    OSA.refreshCurrentSessionQueue = () => Promise.resolve([]);
    return { sessionA, sessionB };
}

function emit(event) {
    OSA.handleAgentEvent(event);
}

test('background session events accumulate without touching the viewed transcript', () => {
    const { sessionA } = setup();
    const before = OSA.TModel.items.length;

    emit({ session_id: 'session-b', sequence: 1, type: 'thinking' });
    emit({ session_id: 'session-b', sequence: 2, type: 'response_start' });
    emit({ session_id: 'session-b', sequence: 3, type: 'response_chunk', content: 'Hello from B' });

    // Viewed transcript untouched.
    assert.equal(OSA.TModel.items.length, before);
    assert.equal(OSA.getCurrentSession().id, 'session-a');
    assert.equal(sessionA.messages.length, 0);

    // Background entry accumulated the stream into its stored messages.
    const entryB = OSA.getSessionEntry('session-b');
    const mirror = entryB.session
        ? entryB.session.messages.filter(m => m.role === 'assistant').pop()
        : null;
    // entry.session is null until the session is visited; the chain still advanced.
    assert.equal(entryB.chain.eventSeqNumber, 3);
    assert.equal(entryB.processing, true);
    if (mirror) assert.match(mirror.content, /Hello from B/);
});

test('background tool events merge into the entry tool list', () => {
    setup();
    emit({ session_id: 'session-b', sequence: 1, type: 'thinking' });
    emit({ session_id: 'session-b', sequence: 2, type: 'tool_start', tool_call_id: 'call-1', tool_name: 'read_file', arguments: { path: 'a.txt' } });
    emit({ session_id: 'session-b', sequence: 3, type: 'tool_complete', tool_call_id: 'call-1', tool_name: 'read_file', success: true, output: 'ok' });

    const entryB = OSA.getSessionEntry('session-b');
    assert.equal(entryB.tools.length, 1);
    assert.equal(entryB.tools[0].completed, true);
    assert.equal(entryB.tools[0].output, 'ok');
    // No tool card leaked into the viewed transcript.
    assert.equal(OSA.tmodelGet('tool:call-1'), undefined);
});

test('background queue dispatch removes the item and appends the user message', () => {
    const { sessionB } = setup();
    const entryB = OSA.getSessionEntry('session-b');
    entryB.session = sessionB;
    entryB.queue = [{ id: 'q1', client_message_id: 'c1', content: 'queued hello', status: 'pending', position: 1 }];

    emit({ session_id: 'session-b', sequence: 1, type: 'queued_message_dispatched', queue_entry_id: 'q1', client_message_id: 'c1', content: 'queued hello' });

    assert.equal(entryB.queue.length, 0);
    const userMsg = sessionB.messages.filter(m => m.role === 'user').pop();
    assert.equal(userMsg && userMsg.content, 'queued hello');
});

test('background turn completion clears processing without touching the viewed session', () => {
    setup();
    emit({ session_id: 'session-b', sequence: 1, type: 'thinking' });
    assert.equal(OSA.getSessionEntry('session-b').processing, true);
    emit({ session_id: 'session-b', sequence: 2, type: 'response_complete', usage: null });
    const entryB = OSA.getSessionEntry('session-b');
    assert.equal(entryB.processing, false);
    // Viewed session state untouched.
    assert.equal(OSA.getCurrentSession().id, 'session-a');
});

test('event sequence dedup is per-session', () => {
    setup();
    emit({ session_id: 'session-a', sequence: 1, type: 'thinking' });
    emit({ session_id: 'session-b', sequence: 1, type: 'thinking' });
    assert.equal(OSA.getSessionEntry('session-a').chain.eventSeqNumber, 1);
    assert.equal(OSA.getSessionEntry('session-b').chain.eventSeqNumber, 1);
    // Replaying session A's seq 1 is dropped; session B is unaffected.
    emit({ session_id: 'session-a', sequence: 1, type: 'thinking' });
    assert.equal(OSA.getSessionEntry('session-a').chain.eventSeqNumber, 1);
});

test('background completion flags the session unread; viewing clears it', () => {
    setup();
    OSA.unreadSessions = {};
    emit({ session_id: 'session-b', sequence: 1, type: 'thinking' });
    assert.equal(OSA.isSessionUnread('session-b'), false);
    emit({ session_id: 'session-b', sequence: 2, type: 'response_complete', usage: null });
    assert.equal(OSA.isSessionUnread('session-b'), true);
    assert.equal(OSA.isSessionUnread('session-a'), false);
    global.document = { querySelector: () => null };
    OSA.markSessionSeen('session-b');
    delete global.document;
    assert.equal(OSA.isSessionUnread('session-b'), false);
});

test('session letter avatar falls back to # only for blank names', () => {
    setup();
    assert.equal(OSA.sessionInitialFor('  hello world'), 'H');
    assert.equal(OSA.sessionInitialFor('123 abc'), '1');
    assert.equal(OSA.sessionInitialFor('   '), '#');
    assert.equal(OSA.sessionInitialFor(''), '#');
    const hue = OSA.sessionHueFor('some-id');
    assert.ok(Number.isInteger(hue) && hue >= 0 && hue < 360);
    assert.equal(OSA.sessionHueFor('some-id'), hue);
});

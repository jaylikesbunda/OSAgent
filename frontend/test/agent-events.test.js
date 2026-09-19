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

function setup() {
    const session = {
        id: 'event-session',
        task_status: 'running',
        messages: [{ role: 'user', content: 'Run the checks', metadata: {} }],
    };
    OSA.SessionStore = {};
    OSA.setCurrentSession(session);
    OSA.tmodelReset();
    OSA.resetMessageChain(session.id);
    OSA.getShowThinkingBlocks = () => true;
    OSA.setHasReceivedResponse = () => {};
    OSA.setProcessing = () => {};
    OSA.setStopping = () => {};
    OSA.showThinkingIndicator = () => {};
    OSA.hideThinkingIndicator = () => {};
    OSA.setSendButtonStopMode = () => {};
    OSA.getStreamingAssistantDomId = () => null;
    OSA.setStreamingAssistantDomId = () => {};
    OSA.startToolSync = () => {};
    OSA.getSessionQueue = () => [];
    OSA.renderQueuedMessages = () => {};
    OSA.clearRetryNotice = () => {};
    OSA.resetSpeechStream = () => {};
    OSA.feedSpeechStream = () => {};
    OSA.persistToolStart = () => {};
    OSA.persistToolComplete = () => {};
    OSA.previewReadToolOutput = () => {};
    OSA.isContextTool = () => false;
    OSA.stripSpeakBlock = text => text || '';
    OSA.stripToolCallMarkup = text => text || '';
    return session;
}

function emit(sequence, type, extra = {}) {
    OSA.handleAgentEvent({
        session_id: 'event-session',
        sequence,
        type,
        ...extra,
    });
}

test('dispatcher deduplicates overlapping transports and preserves tool-boundary reasoning', () => {
    setup();
    emit(1, 'thinking');
    emit(2, 'thinking_start');
    emit(3, 'thinking_delta', { content: 'Trace the renderer.' });
    emit(3, 'thinking_delta', { content: 'Trace the renderer.' });
    emit(4, 'thinking_end');
    emit(5, 'response_start');
    emit(6, 'response_chunk', { content: 'Inspecting now.' });
    emit(6, 'response_chunk', { content: 'Inspecting now.' });
    emit(7, 'tool_start', { tool_call_id: 'call-a', tool_name: 'read_file', message_index: 1 });
    emit(8, 'tool_complete', { tool_call_id: 'call-a', tool_name: 'read_file', success: true, output: 'ok' });

    const reasoning = OSA.TModel.items.filter(item => item.kind === 'message');
    const tool = OSA.tmodelGet('tool:call-a');
    assert.equal(reasoning.length, 1);
    assert.equal(reasoning[0].thinking, 'Trace the renderer.');
    assert.equal(tool.prelude, 'Inspecting now.');
    assert.equal(tool.completed, true);
    assert.equal(OSA.getMessageChain().eventSeqNumber, 8);
});

test('dispatcher creates a fresh stable segment after each completed tool', () => {
    setup();
    emit(1, 'thinking_start');
    emit(2, 'thinking_delta', { content: 'First phase.' });
    emit(3, 'tool_start', { tool_call_id: 'call-a', tool_name: 'read_file', message_index: 1 });
    emit(4, 'tool_complete', { tool_call_id: 'call-a', tool_name: 'read_file', success: true, output: 'ok' });
    emit(5, 'thinking_start');
    emit(6, 'thinking_delta', { content: 'Second phase.' });
    emit(7, 'tool_start', { tool_call_id: 'call-b', tool_name: 'read_file', message_index: 3 });

    assert.deepEqual(
        OSA.TModel.items.filter(item => item.kind === 'message').map(item => item.thinking),
        ['First phase.', 'Second phase.'],
    );
    assert.deepEqual(
        OSA.TModel.items.map(item => item.kind),
        ['message', 'tool', 'message', 'tool'],
    );
});

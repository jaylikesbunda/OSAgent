const assert = require('node:assert/strict');
const test = require('node:test');

global.window = global;
global.requestAnimationFrame = () => 1;
global.cancelAnimationFrame = () => {};
require('../js/utils.js');
require('../js/messages.js');
require('../js/transcript.js');

function resetModel() {
    OSA.tmodelReset();
    OSA.stripSpeakBlock = text => text;
    OSA.stripToolCallMarkup = text => text;
    OSA.getShowThinkingBlocks = () => true;
    OSA.getCurrentSession = () => null;
    OSA.isAgentProcessing = () => false;
    OSA.isHiddenSyntheticMessage = () => false;
    OSA.isContextTool = () => false;
    OSA.completedDurationMs = () => 0;
    OSA.hideThinkingIndicator = () => {};
    OSA.feedSpeechStream = () => {};
}

test('tool boundary preserves thinking and returns response text for the tool card', () => {
    resetModel();
    const segment = OSA.tmodelMessageItem('assistant:live', {
        role: 'assistant',
        content: 'I will inspect the renderer.',
        thinking: 'Need to trace the event flow.',
    }, 1, { live: true, streaming: true, thinkingStreaming: true });
    OSA.tmodelAppend(segment);

    const prelude = OSA.tmodelFinalizeSegmentForToolCall();

    assert.equal(prelude, 'I will inspect the renderer.');
    assert.equal(OSA.tmodelGet(segment.key), segment);
    assert.equal(segment.content, '');
    assert.equal(segment.thinking, 'Need to trace the event flow.');
    assert.equal(segment.streaming, false);
    assert.equal(segment.thinkingStreaming, false);
});

test('tool boundary removes a prelude-only segment', () => {
    resetModel();
    const segment = OSA.tmodelMessageItem('assistant:live', {
        role: 'assistant',
        content: 'Checking now.',
        thinking: '',
    }, 1, { live: true, streaming: true });
    OSA.tmodelAppend(segment);

    assert.equal(OSA.tmodelFinalizeSegmentForToolCall(), 'Checking now.');
    assert.equal(OSA.tmodelGet(segment.key), undefined);
});

test('history rebuild keeps tool-prelude reasoning but folds its content into the tool card', () => {
    resetModel();
    const session = {
        task_status: 'active',
        messages: [{
            role: 'assistant',
            content: 'I will inspect the renderer.',
            thinking: 'Need to trace the event flow.',
            timestamp: '2026-09-13T00:00:00Z',
            tool_calls: [{ id: 'call-1' }],
            metadata: { kind: 'tool_prelude' },
        }],
    };

    OSA.rebuildTranscriptFromSession(session, [{
        tool_call_id: 'call-1',
        tool_name: 'read_file',
        completed: true,
        success: true,
        message_index: 0,
    }], [], { adoptStreaming: false });

    assert.equal(OSA.TModel.items.length, 2);
    assert.equal(OSA.TModel.items[0].kind, 'message');
    assert.equal(OSA.TModel.items[0].content, '');
    assert.equal(OSA.TModel.items[0].thinking, 'Need to trace the event flow.');
    assert.equal(OSA.TModel.items[1].kind, 'tool');
    assert.equal(OSA.TModel.items[1].prelude, 'I will inspect the renderer.');
});

test('multiple think-response-tool cycles retain every reasoning segment in order', () => {
    resetModel();
    const session = {
        id: 'session-1',
        task_status: 'running',
        messages: [{ role: 'user', content: 'Investigate it', metadata: {} }],
    };
    OSA.getCurrentSession = () => session;

    for (const [index, values] of [
        ['1', ['First thought.', 'Checking the first path.', 'call-1']],
        ['2', ['Second thought.', 'Checking the second path.', 'call-2']],
    ]) {
        OSA.beginThinkingDisplay();
        OSA.appendThinkingChunk(values[0]);
        OSA.completeThinkingDisplay();
        OSA.beginAssistantResponse();
        OSA.appendAssistantChunk(values[1]);
        const prelude = OSA.tmodelFinalizeSegmentForToolCall();
        OSA.tmodelToolStart({
            tool_call_id: values[2],
            tool_name: 'read_file',
            message_index: Number(index),
            prelude,
        });
        OSA.tmodelToolComplete({
            tool_call_id: values[2],
            tool_name: 'read_file',
            success: true,
            output: 'ok',
        });
    }

    const messages = OSA.TModel.items.filter(item => item.kind === 'message');
    const tools = OSA.TModel.items.filter(item => item.kind === 'tool');
    assert.deepEqual(messages.map(item => item.thinking), ['First thought.', 'Second thought.']);
    assert.deepEqual(tools.map(item => item.prelude), ['Checking the first path.', 'Checking the second path.']);
    assert.deepEqual(OSA.TModel.items.map(item => item.kind), ['message', 'tool', 'message', 'tool']);
});

test('snapshot ownership ignores optimistic users and expires completed agent activity', () => {
    resetModel();
    OSA.tmodelAppend(OSA.tmodelMessageItem('client:1', {
        role: 'user',
        content: 'Hello',
        metadata: { client_message_id: '1' },
    }, 0, { live: true }));
    assert.equal(OSA.tmodelHasLiveAgentActivity(), false);

    OSA.tmodelAppend(OSA.tmodelMessageItem('assistant:1', {
        role: 'assistant',
        content: 'Working',
    }, 1, { live: true, streaming: true }));
    assert.equal(OSA.tmodelHasLiveAgentActivity(), true);

    OSA.tmodelSettleLiveItems();
    assert.equal(OSA.tmodelHasLiveAgentActivity(), false);
    assert.equal(OSA.TModel.items.every(item => item.live === false), true);
});

test('terminal snapshot restores missed text while preserving live keys and tool cards', () => {
    resetModel();
    const live = OSA.tmodelMessageItem('assistant:live-key', {
        role: 'assistant',
        content: 'Partial prelude',
        thinking: 'Reasoning',
    }, 1, { live: true, streaming: true });
    OSA.tmodelAppend(live);
    OSA.tmodelAppend(OSA.tmodelToolItem({
        tool_call_id: 'call-live',
        tool_name: 'read_file',
        message_index: 1,
    }, { completed: true, success: true, live: true }));

    const session = {
        task_status: 'active',
        messages: [
            { role: 'user', content: 'Check it', metadata: {} },
            {
                role: 'assistant',
                content: 'Complete prelude',
                thinking: 'Reasoning',
                tool_calls: [{ id: 'call-live' }],
                metadata: { kind: 'tool_prelude' },
            },
            { role: 'tool', content: 'ok', tool_call_id: 'call-live', metadata: {} },
            { role: 'assistant', content: 'Complete final answer', thinking: '', metadata: {} },
        ],
    };

    OSA.rebuildTranscriptFromSession(session, [], [], {
        preserveKeys: true,
        keepCurrentArtifacts: true,
        adoptStreaming: false,
    });

    const messages = OSA.TModel.items.filter(item => item.kind === 'message');
    const reasoning = messages.find(item => item.role === 'assistant' && item.thinking === 'Reasoning');
    assert.equal(reasoning.key, 'assistant:live-key');
    assert.equal(reasoning.thinking, 'Reasoning');
    assert.equal(OSA.tmodelGet('tool:call-live').prelude, 'Complete prelude');
    assert.equal(messages.at(-1).content, 'Complete final answer');
});

test('late tool-start delivery restores a prelude without reopening a backend-synced card', () => {
    resetModel();
    const completed = OSA.tmodelToolItem({
        tool_call_id: 'late-call',
        tool_name: 'read_file',
    }, { completed: true, success: true, live: false });
    OSA.tmodelAppend(completed);

    OSA.tmodelToolStart({
        tool_call_id: 'late-call',
        tool_name: 'read_file',
        prelude: 'Recovered prelude',
    });

    assert.equal(completed.completed, true);
    assert.equal(completed.status, 'done');
    assert.equal(completed.prelude, 'Recovered prelude');
});

test('subagent progress updates one stable row and counts genuine reruns', () => {
    resetModel();
    const subagent = OSA.tmodelSubagentCreated({
        subagent_session_id: 'child-1',
        description: 'Audit renderer',
    });

    OSA.tmodelSubagentProgress({ subagent_session_id: 'child-1', status: 'executing', tool_name: 'grep', tool_count: 1 });
    OSA.tmodelSubagentProgress({ subagent_session_id: 'child-1', status: 'executing', tool_name: 'grep', tool_count: 1 });
    OSA.tmodelSubagentProgress({ subagent_session_id: 'child-1', status: 'completed', tool_name: 'grep', tool_count: 1 });
    assert.deepEqual(subagent.tools, [{ name: 'grep', status: 'completed', count: 1 }]);

    OSA.tmodelSubagentProgress({ subagent_session_id: 'child-1', status: 'executing', tool_name: 'grep', tool_count: 2 });
    assert.deepEqual(subagent.tools, [{ name: 'grep', status: 'running', count: 2 }]);
});

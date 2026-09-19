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
    global.OSA = window.OSA = {};

    require('../js/state.js');
require('../js/utils.js');
    require('../js/messages.js');
    require('../js/transcript.js');

    OSA.escapeHtml = value => String(value || '')
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
    OSA.stripSpeakBlock = text => text || '';
    OSA.stripToolCallMarkup = text => text || '';
    OSA.getShowThinkingBlocks = () => true;
    OSA.getAttachmentImageSrc = () => '';
    OSA.updateAssistantMessageActions = () => {};
    OSA.updateAssistantRestoreButton = () => {};
    OSA.setStreamingAssistantDomId = () => {};
    OSA.toolLabel = name => name;
    OSA.toolIcon = () => 'T';
    OSA.summarizeToolArgs = (name, args) => args && args.path ? args.path : '';
    OSA.setToolCardPreviewData = () => {};
    OSA.setContextToolPreviewData = () => {};
    OSA.formatToolOutput = () => '';
    OSA.isContextTool = () => false;
});

test.beforeEach(() => {
    document.body.replaceChildren();
    OSA.transcriptView = {
        toolNodesByCallId: new Map(),
        ctxNodesByCallId: new Map(),
        wrapperNodesByKey: new Map(),
    };
    OSA.getTranscriptView = () => OSA.transcriptView;
});

test('incremental markdown intentionally withholds incomplete lines until newline or flush', () => {
    const el = document.createElement('div');
    document.body.appendChild(el);
    el._md = OSA.createIncrementalMd();

    OSA.renderIncrementalMarkdown(el, 'Incomplete line');
    assert.equal(el.textContent, '');

    OSA.renderIncrementalMarkdown(el, 'Incomplete line\nNext partial');
    assert.equal(el.textContent, 'Incomplete line');

    OSA.flushIncrementalMarkdown(el, 'Incomplete line\nNext partial');
    assert.equal(el.textContent, 'Incomplete lineNext partial');
});

test('repeated thinking signals reuse one indicator and one animation', () => {
    const messages = document.createElement('div');
    messages.id = 'messages';
    document.body.appendChild(messages);
    OSA.mountFloatingNode = node => messages.appendChild(node);
    OSA.setTurnStartTime = () => {};
    OSA.tmodelMarkDirty = () => {};
    let animationStarts = 0;
    OSA._initThinkingCanvas = () => {
        animationStarts += 1;
        return () => {};
    };

    const first = OSA.showThinkingIndicator();
    const second = OSA.showThinkingIndicator();

    assert.equal(second, first);
    assert.equal(document.querySelectorAll('#thinking-indicator').length, 1);
    assert.equal(animationStarts, 1);
    OSA.hideThinkingIndicator();
});

test('thinking patches preserve the existing DOM node and expanded state', () => {
    const wrapper = document.createElement('div');
    document.body.appendChild(wrapper);
    const item = OSA.tmodelMessageItem('assistant:1', {
        role: 'assistant',
        content: '',
        thinking: 'First line\n',
    }, 1, { live: true, streaming: true, thinkingStreaming: true });
    const unit = { type: 'message', key: item.key, item, items: [item] };

    OSA.patchMessageUnit(wrapper, unit);
    const message = wrapper.firstElementChild;
    const thinking = message.querySelector('.message-thinking');
    thinking.classList.add('expanded');

    item.thinking += 'Second line\n';
    OSA.patchMessageUnit(wrapper, unit);

    assert.equal(wrapper.firstElementChild, message);
    assert.equal(message.querySelector('.message-thinking'), thinking);
    assert.equal(thinking.classList.contains('expanded'), true);
    assert.match(thinking.textContent, /Second line/);
});

test('thinking end flushes its final incomplete line without replacing the block', () => {
    const wrapper = document.createElement('div');
    document.body.appendChild(wrapper);
    const item = OSA.tmodelMessageItem('assistant:phase', {
        role: 'assistant',
        content: '',
        thinking: 'Final thought without a newline',
    }, 1, { live: true, streaming: true, thinkingStreaming: true });
    const unit = { type: 'message', key: item.key, item, items: [item] };

    OSA.patchMessageUnit(wrapper, unit);
    const thinking = wrapper.querySelector('.message-thinking');
    const body = thinking.querySelector('.thinking-body');
    assert.equal(body.textContent, '');

    item.thinkingStreaming = false;
    OSA.patchMessageUnit(wrapper, unit);

    assert.equal(wrapper.querySelector('.message-thinking'), thinking);
    assert.equal(thinking.querySelector('.thinking-body'), body);
    assert.equal(body.textContent, 'Final thought without a newline');
});

test('parallel tool status updates never detach or recreate cards', () => {
    const wrapper = document.createElement('div');
    document.body.appendChild(wrapper);
    const first = OSA.tmodelToolItem({ tool_call_id: 'a', tool_name: 'read_file', arguments: { path: 'a' } });
    const second = OSA.tmodelToolItem({ tool_call_id: 'b', tool_name: 'read_file', arguments: { path: 'b' } });
    const unit = { type: 'parallel-group', key: 'par:tool:a', items: [first, second] };

    OSA.patchParallelGroupUnit(wrapper, unit);
    const group = wrapper.firstElementChild;
    const header = group.querySelector('.parallel-group-header');
    const firstCard = document.getElementById('tool-a');
    const secondCard = document.getElementById('tool-b');

    first.completed = true;
    first.success = true;
    OSA.patchParallelGroupUnit(wrapper, unit);
    second.completed = true;
    second.success = true;
    OSA.patchParallelGroupUnit(wrapper, unit);

    assert.equal(wrapper.firstElementChild, group);
    assert.equal(group.querySelector('.parallel-group-header'), header);
    assert.equal(document.getElementById('tool-a'), firstCard);
    assert.equal(document.getElementById('tool-b'), secondCard);
    assert.match(header.textContent, /2 tools/);
});

test('forming a parallel group moves the first card without recreating it', () => {
    const singleWrapper = document.createElement('div');
    document.body.appendChild(singleWrapper);
    const first = OSA.tmodelToolItem({ tool_call_id: 'move-a', tool_name: 'read_file', arguments: {} });
    OSA.patchToolUnit(singleWrapper, { type: 'tool', key: first.key, items: [first] });
    const firstCard = document.getElementById('tool-move-a');

    const groupWrapper = document.createElement('div');
    document.body.appendChild(groupWrapper);
    const second = OSA.tmodelToolItem({ tool_call_id: 'move-b', tool_name: 'read_file', arguments: {} });
    OSA.patchParallelGroupUnit(groupWrapper, {
        type: 'parallel-group',
        key: 'par:tool:move-a',
        items: [first, second],
    });

    assert.equal(document.getElementById('tool-move-a'), firstCard);
    assert.equal(firstCard.parentElement, groupWrapper.querySelector('.parallel-group'));
});

test('context tool progress patches fields without rebuilding the row', () => {
    const item = OSA.tmodelToolItem({
        tool_call_id: 'ctx-a',
        tool_name: 'grep',
        arguments: { path: 'frontend' },
    });
    const row = OSA.buildContextToolRow(item);
    document.body.appendChild(row);
    const action = row.querySelector('.context-inline-action');
    const detail = row.querySelector('.context-inline-detail');
    const preview = row.querySelector('.context-inline-preview-btn');
    const status = row.querySelector('.context-inline-status');

    item.completed = true;
    item.success = true;
    OSA.patchContextToolRow(row, item);

    assert.equal(row.querySelector('.context-inline-action'), action);
    assert.equal(row.querySelector('.context-inline-detail'), detail);
    assert.equal(row.querySelector('.context-inline-preview-btn'), preview);
    assert.equal(row.querySelector('.context-inline-status'), status);
    assert.equal(status.textContent, 'done');
});

test('same-count attachment changes are rendered instead of being skipped', () => {
    OSA.renderAttachmentMarkup = attachments => attachments.map(item => item.filename).join(',');
    const message = document.createElement('div');
    document.body.appendChild(message);
    const item = {
        images: [],
        attachments: [{ filename: 'first.txt', mime: 'text/plain' }],
    };

    OSA.patchMessageAttachments(message, item);
    const wrap = message.querySelector('.message-attachments');
    assert.equal(wrap.textContent, 'first.txt');

    item.attachments = [{ filename: 'replacement.txt', mime: 'text/plain' }];
    OSA.patchMessageAttachments(message, item);
    assert.equal(message.querySelector('.message-attachments'), wrap);
    assert.equal(wrap.textContent, 'replacement.txt');
});

test('keyed list reconciliation preserves existing node identity while inserting and removing', () => {
    const list = document.createElement('div');
    const first = document.createElement('div');
    const second = document.createElement('div');
    const middle = document.createElement('div');
    list.append(first, second);
    document.body.appendChild(list);

    assert.equal(OSA.reconcileTranscriptList(list, [first, second]), false);
    assert.equal(OSA.reconcileTranscriptList(list, [first, middle, second]), true);
    assert.deepEqual(Array.from(list.children), [first, middle, second]);
    assert.equal(OSA.reconcileTranscriptList(list, [first, second]), true);
    assert.deepEqual(Array.from(list.children), [first, second]);
});

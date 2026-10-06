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
    global.Node = window.Node;
    global.HTMLElement = window.HTMLElement;
    global.requestAnimationFrame = () => 1;
    global.cancelAnimationFrame = () => {};
    global.OSA = window.OSA = {};

    require('../js/state.js');
require('../js/utils.js');
    require('../js/messages.js');
    require('../js/transcript.js');
    require('../js/settings.js');

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
    localStorage.clear();
    OSA.transcriptView = {
        toolNodesByCallId: new Map(),
        ctxNodesByCallId: new Map(),
        wrapperNodesByKey: new Map(),
    };
    OSA.getTranscriptView = () => OSA.transcriptView;
});

test('scrolling upward pauses transcript auto-scroll even near the bottom', () => {
    const view = {
        lastScrollTop: 1000,
        userPinnedToBottom: true,
        autoScrollPaused: false,
        forceStickBottom: true,
    };
    const messages = { scrollTop: 990, scrollHeight: 1100, clientHeight: 100 };

    OSA.updateTranscriptScrollState(view, messages);

    assert.equal(view.autoScrollPaused, true);
    assert.equal(view.userPinnedToBottom, false);
    assert.equal(view.forceStickBottom, false);
});

test('stream growth does not resume auto-scroll after the user scrolls up', () => {
    const view = {
        lastScrollTop: 1000,
        userPinnedToBottom: true,
        autoScrollPaused: false,
        forceStickBottom: true,
    };
    const messages = { scrollTop: 990, scrollHeight: 1100, clientHeight: 100 };

    OSA.updateTranscriptScrollState(view, messages);
    messages.scrollHeight = 1200;
    OSA.updateTranscriptScrollState(view, messages);

    assert.equal(view.autoScrollPaused, true);
    assert.equal(view.userPinnedToBottom, false);
});

test('auto-scroll resumes after the user returns to the bottom', () => {
    const view = {
        lastScrollTop: 990,
        userPinnedToBottom: false,
        autoScrollPaused: true,
        forceStickBottom: false,
    };
    const messages = { scrollTop: 1100, scrollHeight: 1200, clientHeight: 100 };

    OSA.updateTranscriptScrollState(view, messages);

    assert.equal(view.autoScrollPaused, false);
    assert.equal(view.userPinnedToBottom, true);
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

test('thinking cards render between tools and collapse with the group', () => {
    const wrapper = document.createElement('div');
    document.body.appendChild(wrapper);
    const first = OSA.tmodelToolItem({ tool_call_id: 'r-a', tool_name: 'bash', arguments: {} });
    const second = OSA.tmodelToolItem({ tool_call_id: 'r-b', tool_name: 'bash', arguments: {} });
    const unit = {
        type: 'parallel-group',
        items: [first, second],
        entries: [
            { kind: 'tool', item: first },
            { kind: 'reasoning', key: 'reason-1', text: 'Think before acting.' },
            { kind: 'tool', item: second },
        ],
        reasoning: [{ key: 'reason-1', text: 'Think before acting.' }],
    };
    OSA.patchParallelGroupUnit(wrapper, unit);
    const group = wrapper.firstElementChild;
    const header = group.querySelector('.parallel-group-header');
    const card = group.querySelector('.tool-group-thinking-card');
    assert.ok(card, 'thinking card exists');
    assert.equal(card.hidden, true);
    // The old header badge is gone.
    assert.equal(header.querySelector('.tool-group-thinking'), null);
    assert.doesNotMatch(header.textContent, /Thinking/);
    assert.match(card.textContent, /Think before acting/);

    // Sits between the two tool cards, in order.
    const ordered = Array.from(group.children).filter(child => child !== header);
    assert.equal(ordered[0], document.getElementById('tool-r-a'));
    assert.equal(ordered[1], card);
    assert.equal(ordered[2], document.getElementById('tool-r-b'));

    header.click();
    assert.equal(card.hidden, false);
    assert.equal(group.querySelector('.tool-group-thinking-card'), card);
    assert.equal(document.getElementById('tool-r-a'), group.querySelector('#tool-r-a'));
});

test('collapsed tool groups retain expansion and expose failures during live updates', () => {
    const wrapper = document.createElement('div');
    document.body.appendChild(wrapper);
    const items = ['fold-a', 'fold-b'].map(id => OSA.tmodelToolItem({
        tool_call_id: id, tool_name: 'bash', arguments: {},
    }));
    const unit = { type: 'parallel-group', items };
    OSA.patchParallelGroupUnit(wrapper, unit);
    const group = wrapper.firstElementChild;
    const header = group.querySelector('button');
    const card = document.getElementById('tool-fold-a');
    assert.equal(card.hidden, true);
    assert.equal(header.getAttribute('aria-expanded'), 'false');
    header.click();
    assert.equal(card.hidden, false);
    items[0].completed = true;
    items[0].success = false;
    OSA.patchParallelGroupUnit(wrapper, unit);
    assert.equal(header.getAttribute('aria-expanded'), 'true');
    assert.match(header.textContent, /1 failed/);
    assert.equal(document.getElementById('tool-fold-a'), card);
    header.click();
    assert.equal(card.hidden, true);
    assert.match(header.textContent, /1 failed/);
});

test('tool preview preference persists and immediately updates existing context groups', () => {
    const wrapper = document.createElement('div');
    document.body.appendChild(wrapper);
    const items = ['read_file', 'grep', 'glob'].map((name, index) => OSA.tmodelToolItem({
        tool_call_id: 'preview-' + index, tool_name: name, arguments: {},
    }));
    OSA.patchContextGroupUnit(wrapper, { items });
    const group = wrapper.firstElementChild;
    const rows = Array.from(group.querySelectorAll('.context-inline-item'));
    assert.equal(rows.every(row => row.hidden), true);
    assert.match(group.textContent, /1 read, 2 searches/);
    OSA.setToolGroupPreview('2');
    assert.equal(localStorage.getItem('osagent-tool-group-preview'), '2');
    assert.deepEqual(rows.map(row => row.hidden), [false, false, true]);
    group.querySelector('button').click();
    assert.equal(rows.every(row => !row.hidden), true);
    OSA.setToolGroupPreview('all');
    assert.equal(OSA.getToolGroupPreview(), 'all');
    assert.equal(rows.every(row => !row.hidden), true);
    group.querySelector('button').click();
    assert.equal(rows.every(row => row.hidden), true);
    OSA.setToolGroupPreview('invalid');
    assert.equal(OSA.getToolGroupPreview(), 0);
});

test('diagrams remain standalone beside grouped tools', () => {
    OSA.tmodelReset();
    ['bash', 'draw_diagram', 'bash', 'bash'].forEach((name, index) => {
        OSA.tmodelAppend(OSA.tmodelToolItem({ tool_call_id: 'diagram-group-' + index, tool_name: name, arguments: {} }));
    });
    const units = OSA.buildTranscriptUnits();
    assert.deepEqual(units.map(unit => unit.type), ['tool', 'tool', 'parallel-group']);
    assert.equal(units[1].items[0].toolName, 'draw_diagram');
});

test('one context read is collapsed and internal message indices do not split adjacent tools', () => {
    const wrapper = document.createElement('div'); document.body.appendChild(wrapper);
    const read = OSA.tmodelToolItem({tool_call_id: 'single-context', tool_name: 'read_file'});
    OSA.patchContextGroupUnit(wrapper, {items: [read]});
    assert.equal(wrapper.querySelector('.tool-group-toggle').hidden, false);
    assert.equal(wrapper.querySelector('.context-inline-item').hidden, true);
    OSA.tmodelReset();
    [{id:'late-a', index:4, time:1000}, {id:'late-b', index:4, time:10000}, {id:'later-round', index:8, time:10001}].forEach(call => {
        const item = OSA.tmodelToolItem({tool_call_id:call.id, tool_name:'bash', message_index:call.index});
        item.ts = call.time; OSA.tmodelAppend(item);
    });
    assert.deepEqual(OSA.buildTranscriptUnits().map(unit => unit.items.length), [3]);
});

test('mixed tool categories share a group until a visible message separates them', () => {
    OSA.tmodelReset();
    const tools = ['read_file', 'bash', 'search_files', 'edit_file', 'read_file'].map((name, index) => {
        const item = OSA.tmodelToolItem({tool_call_id: 'mixed-' + index, tool_name: name, message_index: index});
        item.context = name === 'read_file' || name === 'search_files';
        return item;
    });
    tools.forEach(item => OSA.tmodelAppend(item));
    OSA.tmodelAppend({kind: 'message', key: 'visible-update', role: 'assistant', content: 'Checking the result.'});
    OSA.tmodelAppend(OSA.tmodelToolItem({tool_call_id: 'after-update', tool_name: 'bash'}));
    const units = OSA.buildTranscriptUnits();
    assert.deepEqual(units.map(unit => unit.items.length), [5, 1, 1]);
    assert.deepEqual(units.map(unit => unit.type), ['parallel-group', 'message', 'tool']);
});

test('a live context group retains expansion when other tool categories join it', () => {
    OSA.tmodelReset();
    const read = OSA.tmodelToolItem({tool_call_id: 'growing-read', tool_name: 'read_file'});
    read.context = true;
    OSA.tmodelAppend(read);
    const wrapper = document.createElement('div');
    document.body.appendChild(wrapper);
    const initial = OSA.buildTranscriptUnits()[0];
    OSA.patchUnit(wrapper, initial);
    const group = wrapper.firstElementChild;
    const header = group.querySelector('.tool-group-toggle');
    const row = group.querySelector('.context-inline-item');
    header.click();
    const shell = OSA.tmodelToolItem({tool_call_id: 'growing-shell', tool_name: 'bash', message_index: 99});
    OSA.tmodelAppend(shell);
    const grown = OSA.buildTranscriptUnits()[0];
    assert.equal(grown.key, initial.key);
    OSA.patchUnit(wrapper, grown);
    assert.equal(wrapper.firstElementChild, group);
    assert.equal(group.querySelector('.tool-group-toggle'), header);
    assert.equal(group.querySelector('.context-inline-item'), row);
    assert.equal(header.getAttribute('aria-expanded'), 'true');
    assert.equal(row.hidden, false);
    assert.equal(group.querySelector('.tool-container').hidden, false);
    assert.match(header.textContent, /2 tools/);
});

test('shell detail defaults apply immediately and preserve an explicit fold during progress', () => {
    const wrapper = document.createElement('div'); document.body.appendChild(wrapper);
    const item = OSA.tmodelToolItem({tool_call_id:'detail-default', tool_name:'bash'});
    OSA.patchToolUnit(wrapper, {items:[item]});
    const container = document.getElementById('tool-detail-default');
    OSA.setToolDetailDefault('shell', true);
    assert.equal(container.querySelector('.tool-body').classList.contains('visible'), true);
    container._toolExpanded = false;
    item.completed = true; item.success = true;
    OSA.patchToolCardElement(container, item);
    assert.equal(container.querySelector('.tool-body').classList.contains('visible'), false);
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

test('jump-to-latest button follows scroll position and re-pins on click', () => {
    const messages = document.createElement('div');
    messages.id = 'messages';
    document.body.appendChild(messages);
    const button = document.createElement('button');
    button.id = 'scroll-to-bottom';
    button.className = 'scroll-to-bottom hidden';
    document.body.appendChild(button);

    OSA.TModel.items = [{}];

    // Detached from the tail: the affordance is visible.
    OSA.updateScrollToBottomButton({ scrollTop: 200, scrollHeight: 1000, clientHeight: 100 });
    assert.equal(button.classList.contains('hidden'), false);

    // Back at the bottom: it hides again.
    OSA.updateScrollToBottomButton({ scrollTop: 900, scrollHeight: 1000, clientHeight: 100 });
    assert.equal(button.classList.contains('hidden'), true);

    const view = { autoScrollPaused: true, userPinnedToBottom: false, forceStickBottom: false };
    OSA.getTranscriptView = () => view;
    OSA.scrollMessagesToBottom = () => {};
    OSA.jumpToLatest();
    assert.equal(view.autoScrollPaused, false);
    assert.equal(view.userPinnedToBottom, true);
    assert.equal(button.classList.contains('hidden'), true);
});

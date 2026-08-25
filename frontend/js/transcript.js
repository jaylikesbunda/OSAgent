window.OSA = window.OSA || {};

OSA.TModel = {
    items: [],
    byKey: new Map(),
    dirty: false,
    frame: null,
    pendingReason: '',
    liveSeq: 0,
};

OSA.tmodelReset = function() {
    OSA.TModel.items = [];
    OSA.TModel.byKey = new Map();
    OSA.TModel.dirty = false;
    if (OSA.TModel.frame != null) {
        cancelAnimationFrame(OSA.TModel.frame);
        OSA.TModel.frame = null;
    }
    OSA.TModel.pendingReason = '';
};

OSA.tmodelAppend = function(item) {
    if (!item || !item.key) return null;
    const existing = OSA.TModel.byKey.get(item.key);
    if (existing) {
        const idx = OSA.TModel.items.indexOf(existing);
        if (idx >= 0) OSA.TModel.items[idx] = item;
        else OSA.TModel.items.push(item);
    } else {
        OSA.TModel.items.push(item);
    }
    OSA.TModel.byKey.set(item.key, item);
    return item;
};

OSA.tmodelGet = function(key) {
    return OSA.TModel.byKey.get(key);
};

OSA.tmodelRemove = function(key) {
    const item = OSA.TModel.byKey.get(key);
    if (!item) return false;
    OSA.TModel.byKey.delete(key);
    const idx = OSA.TModel.items.indexOf(item);
    if (idx >= 0) OSA.TModel.items.splice(idx, 1);
    return true;
};

OSA.tmodelLast = function() {
    return OSA.TModel.items[OSA.TModel.items.length - 1] || null;
};

OSA.tmodelStreamingItem = function() {
    const last = OSA.tmodelLast();
    return (last && last.kind === 'message' && last.role === 'assistant' && last.streaming) ? last : null;
};

OSA.tmodelLiveKey = function(prefix) {
    OSA.TModel.liveSeq += 1;
    return prefix + ':' + Date.now().toString(36) + ':' + OSA.TModel.liveSeq;
};

OSA.tmodelMarkDirty = function(reason) {
    OSA.TModel.dirty = true;
    if (reason) OSA.TModel.pendingReason = reason;
    OSA.scheduleTranscriptRender();
};

OSA.eventTimestampMs = function(value) {
    if (!value) return null;
    if (typeof value === 'number') {
        return value > 1e12 ? value : value * 1000;
    }
    const parsed = new Date(value).getTime();
    return Number.isFinite(parsed) ? parsed : null;
};

OSA.messageIndexValue = function(value) {
    const parsed = Number.parseInt(value, 10);
    return Number.isInteger(parsed) ? parsed : null;
};

OSA.tmodelMessageItem = function(key, message, messageIndex, opts = {}) {
    return {
        kind: 'message',
        key,
        role: message.role || 'user',
        content: message.content || '',
        thinking: message.thinking || '',
        timestamp: message.timestamp || '',
        toolCalls: message.tool_calls || null,
        images: Array.isArray(message.images) ? message.images : [],
        attachments: message.metadata && Array.isArray(message.metadata.attachments)
            ? message.metadata.attachments
            : [],
        clientMessageId: (message.metadata && message.metadata.client_message_id) || '',
        messageIndex: Number.isInteger(messageIndex) ? messageIndex : null,
        live: !!opts.live,
        streaming: !!opts.streaming,
        thinkingStreaming: !!opts.thinkingStreaming,
        durationMs: null,
        tps: null,
        totalTokens: null,
    };
};

OSA.tmodelToolItem = function(event, opts = {}) {
    const callId = event.tool_call_id || OSA.tmodelLiveKey('call');
    const ts = OSA.eventTimestampMs(event.timestamp) || Date.now();
    const completed = opts.completed === true;
    const success = opts.success === true;
    return {
        kind: 'tool',
        key: 'tool:' + callId,
        callId,
        toolName: event.tool_name || '',
        args: event.arguments || {},
        output: typeof event.output === 'string' ? event.output : '',
        title: typeof event.title === 'string' ? event.title : '',
        status: completed ? (success ? 'done' : 'failed') : 'running',
        success,
        completed,
        metadata: (event.metadata && typeof event.metadata === 'object') ? event.metadata : null,
        ts,
        anchorIndex: OSA.messageIndexValue(event.message_index),
        context: !!OSA.isContextTool(event.tool_name),
        live: opts.live !== false,
    };
};

OSA.tmodelToolEventView = function(item) {
    return {
        tool_call_id: item.callId,
        tool_name: item.toolName,
        arguments: item.args,
        output: item.output,
        success: item.success,
        metadata: item.metadata,
        title: item.title,
    };
};

OSA.tmodelToolStart = function(event) {
    if (!event || !event.tool_call_id) return null;
    OSA.insertCurrentSessionToolBoundary(event);
    if (event.tool_name === 'subagent') return null;
    const key = 'tool:' + event.tool_call_id;
    let item = OSA.tmodelGet(key);
    if (item) {
        if (item.completed) return item;
        item.toolName = event.tool_name || item.toolName;
        item.args = event.arguments || item.args;
        item.completed = false;
        item.success = false;
        item.status = 'running';
        item.output = '';
        item.ts = OSA.eventTimestampMs(event.timestamp) || Date.now();
    } else {
        item = OSA.tmodelToolItem(event);
        OSA.tmodelAppend(item);
    }
    OSA.tmodelMarkDirty('tool-start');
    return item;
};

OSA.tmodelToolProgress = function(event) {
    if (!event || !event.tool_call_id) return;
    const item = OSA.tmodelGet('tool:' + event.tool_call_id);
    if (!item) return;
    item.status = (event.status || 'running').toLowerCase();
    OSA.tmodelMarkDirty('tool-progress');
};

OSA.tmodelToolComplete = function(event) {
    if (!event || !event.tool_call_id || event.tool_name === 'subagent') return null;
    const key = 'tool:' + event.tool_call_id;
    let item = OSA.tmodelGet(key);
    if (item) {
        item.output = typeof event.output === 'string' ? event.output : item.output;
        item.title = typeof event.title === 'string' ? event.title : item.title;
        item.metadata = (event.metadata && typeof event.metadata === 'object') ? event.metadata : item.metadata;
        item.completed = true;
        item.success = event.success === true;
        item.status = item.success ? 'done' : 'failed';
    } else {
        item = OSA.tmodelToolItem(event, { completed: true, success: event.success === true });
        OSA.tmodelAppend(item);
    }
    if (item.toolName === 'task') {
        OSA.tmodelAddTaskMessage(item.output || '');
    }
    OSA.tmodelMarkDirty('tool-complete');
    return item;
};

OSA.tmodelAddTaskMessage = function(content) {
    const cleaned = String(content || '').replace(/\s{2,}/g, ' ').trim();
    if (!cleaned) return;
    OSA.tmodelAppend({ kind: 'task', key: OSA.tmodelLiveKey('task'), content: cleaned });
};

OSA.tmodelAddError = function(error) {
    const msg = String(error || 'Unknown error');
    // Dedup: provider errors can be delivered twice (ws + sse, or retry) — ignore if last error identical within 2s
    const last = OSA.TModel.items[OSA.TModel.items.length - 1];
    if (last && last.kind === 'error' && last.error === msg) return;
    // also check second-last in case an interleaving cancelled/error
    const prev = OSA.TModel.items[OSA.TModel.items.length - 2];
    if (prev && prev.kind === 'error' && prev.error === msg) return;
    OSA.tmodelAppend({ kind: 'error', key: OSA.tmodelLiveKey('error'), error: msg });
};

OSA.tmodelAddCancelled = function() {
    OSA.tmodelAppend({ kind: 'cancelled', key: OSA.tmodelLiveKey('cancelled') });
};

OSA.tmodelSubagentItem = function(data, live) {
    const subagentId = data.subagent_session_id || data.session_id;
    const isRunning = data.is_running === true
        || (live && data.status !== 'completed' && data.status !== 'failed' && data.status !== 'cancelled');
    return {
        kind: 'subagent',
        key: 'subagent:' + subagentId,
        subagentId,
        description: data.description || 'Subagent task',
        agentType: data.agent_type || 'general',
        prompt: data.prompt || '',
        status: data.status || 'running',
        isRunning,
        toolCount: data.tool_count || 0,
        result: data.result || '',
        currentTool: '',
        retryText: '',
        tools: [],
        durationMs: data.duration_ms || OSA.completedDurationMs(data.created_at, data.completed_at) || null,
        contextState: data.context_state || null,
        anchorIndex: null,
        live: live !== false,
    };
};

OSA.tmodelSubagentCreated = function(event) {
    if (!event || !event.subagent_session_id) return null;
    const key = 'subagent:' + event.subagent_session_id;
    let item = OSA.tmodelGet(key);
    if (!item) {
        item = OSA.tmodelSubagentItem(event, true);
        OSA.tmodelAppend(item);
    }
    OSA.tmodelMarkDirty('subagent-created');
    return item;
};

OSA.tmodelSubagentProgress = function(event) {
    if (!event || !event.subagent_session_id) return;
    const item = OSA.tmodelGet('subagent:' + event.subagent_session_id);
    if (!item) return;
    item.toolCount = event.tool_count || item.toolCount;
    const status = event.status || 'running';
    if (status === 'executing') {
        item.isRunning = true;
        item.currentTool = event.tool_name || item.currentTool;
        item.retryText = '';
    } else if (status === 'completed' || status === 'failed') {
        if (!item.currentTool || item.currentTool === (event.tool_name || '')) item.currentTool = '';
        if (event.tool_name) item.tools.push({ name: event.tool_name, status });
        item.retryText = '';
    } else if (event.tool_name) {
        item.tools.push({ name: event.tool_name, status: 'running' });
    }
    OSA.tmodelMarkDirty('subagent-progress');
};

OSA.tmodelSubagentCompleted = function(event) {
    if (!event || !event.subagent_session_id) return;
    const item = OSA.tmodelGet('subagent:' + event.subagent_session_id);
    if (!item) return;
    item.status = event.status || 'completed';
    item.isRunning = false;
    item.result = event.result || item.result;
    item.toolCount = event.tool_count || item.toolCount;
    item.durationMs = event.duration_ms || item.durationMs;
    item.currentTool = '';
    item.retryText = '';
    OSA.tmodelMarkDirty('subagent-completed');
};

OSA.tmodelSubagentRetry = function(event) {
    if (!event || !event.subagent_session_id) return;
    const item = OSA.tmodelGet('subagent:' + event.subagent_session_id);
    if (!item) return;
    const delay = event.next_retry_in_ms ? Math.max(1, Math.round(event.next_retry_in_ms / 1000)) : null;
    const attempt = event.attempt_count || 0;
    const max = event.max_attempts || 0;
    let text = delay ? `retrying in ~${delay}s` : 'retrying';
    if (attempt && max) text += ` (attempt ${attempt}/${max})`;
    item.retryText = text;
    item.isRunning = true;
    OSA.tmodelMarkDirty('subagent-retry');
};

OSA.tmodelSubagentContextUpdate = function(event) {
    if (!event || !event.subagent_session_id) return;
    const item = OSA.tmodelGet('subagent:' + event.subagent_session_id);
    if (!item) return;
    item.contextState = event;
    OSA.tmodelMarkDirty('subagent-context');
};

OSA.tmodelEnsureAssistantSegment = function() {
    const existing = OSA.tmodelStreamingItem();
    if (existing) return existing;

    const session = OSA.getCurrentSession();
    const msgs = (session && Array.isArray(session.messages)) ? session.messages : [];
    const lastMsg = msgs[msgs.length - 1];
    let mirror = null;
    if (lastMsg && lastMsg.role === 'assistant' && !(lastMsg.content || '').trim() && !(lastMsg.thinking || '').trim()) {
        mirror = lastMsg;
    }
    if (!mirror) {
        mirror = OSA.ensureCurrentSessionAssistantMessage(true);
    }
    if (!mirror) return null;

    let idx = null;
    if (session && Array.isArray(session.messages)) {
        const found = session.messages.indexOf(mirror);
        if (found >= 0) idx = found;
    }

    return OSA.tmodelAppend(OSA.tmodelMessageItem(
        OSA.tmodelLiveKey('assistant'),
        {
            role: 'assistant',
            content: '',
            thinking: null,
            timestamp: mirror.timestamp || new Date().toISOString(),
            metadata: {},
        },
        idx,
        { live: true, streaming: true },
    ));
};

OSA.tmodelFinalizeSegmentForToolCall = function() {
    const item = OSA.tmodelStreamingItem();
    if (!item) return;
    item.streaming = false;
    item.thinkingStreaming = false;
    const display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
    if (!display.trim() && !(item.thinking || '').trim()) {
        OSA.tmodelRemove(item.key);
    }
    OSA.tmodelMarkDirty('segment-boundary');
};

OSA.tmodelPruneEmptyStreamingSegment = function() {
    const item = OSA.tmodelStreamingItem();
    if (!item) return;
    const display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
    if (!display.trim() && !(item.thinking || '').trim()) {
        OSA.tmodelRemove(item.key);
        OSA.tmodelMarkDirty('prune-empty');
    }
};

OSA.tmodelReleaseStreamingSegment = function() {
    const item = OSA.tmodelStreamingItem();
    if (!item) return;
    item.streaming = false;
    item.thinkingStreaming = false;
    OSA.tmodelMarkDirty('release-stream');
};

OSA.tmodelFinalizeStreamingSegment = function(usage) {
    const item = OSA.tmodelStreamingItem();
    if (!item) return null;
    item.streaming = false;
    item.thinkingStreaming = false;

    const startTime = OSA.getTurnStartTime();
    if (startTime) {
        item.durationMs = Date.now() - startTime;
        const elapsedSec = item.durationMs / 1000;
        if (usage && usage.output > 0 && elapsedSec > 0) {
            item.tps = (usage.output / elapsedSec).toFixed(1);
            item.totalTokens = usage.total || null;
        }
    }

    const display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
    if (!display.trim() && !(item.thinking || '').trim()) {
        OSA.tmodelRemove(item.key);
    }
    OSA.tmodelMarkDirty('segment-final');
    return item;
};

OSA.getMessageRenderKey = function(message, originalIndex) {
    const clientId = message && message.metadata && message.metadata.client_message_id;
    if (clientId) return 'client:' + clientId;
    const ts = message && message.timestamp ? String(message.timestamp) : '';
    const role = message && message.role ? String(message.role) : 'unknown';
    const toolId = message && message.tool_call_id ? String(message.tool_call_id) : '';
    return `idx:${originalIndex}|${role}|${ts}|${toolId}`;
};

OSA.getMessageRenderSignature = function(message) {
    const attachments = message && message.metadata && Array.isArray(message.metadata.attachments)
        ? message.metadata.attachments.length
        : 0;
    const images = message && Array.isArray(message.images) ? message.images.length : 0;
    return [
        message?.role || '',
        message?.content || '',
        message?.thinking || '',
        message?.timestamp || '',
        String(attachments),
        String(images),
        OSA.getShowThinkingBlocks() ? '1' : '0',
    ].join('\u0001');
};

OSA.rebuildTranscriptFromSession = function(session, toolEvents = [], subagentTasks = [], options = {}) {
    const messages = (session && Array.isArray(session.messages)) ? session.messages : [];
    const items = [];

    messages.forEach(function(message, idx) {
        if (!message || message.role === 'tool') return;
        if (OSA.isHiddenSyntheticMessage(message)) return;
        if (message.role === 'assistant') {
            const hasContent = !!(message.content || '').trim();
            const hasVisibleThinking = OSA.getShowThinkingBlocks() && !!(message.thinking || '').trim();
            const hasToolCalls = Array.isArray(message.tool_calls) && message.tool_calls.length > 0;
            if (!hasContent && !hasVisibleThinking && !hasToolCalls) return;
        }
        items.push(OSA.tmodelMessageItem(OSA.getMessageRenderKey(message, idx), message, idx));
    });

    const anchorOf = function(entry) {
        if (entry.kind === 'message') return entry.messageIndex;
        return entry.anchorIndex;
    };

    const tools = (Array.isArray(toolEvents) ? toolEvents : [])
        .filter(function(t) { return t && t.tool_call_id && t.tool_name !== 'subagent'; })
        .sort(function(a, b) {
            const delta = (a.message_index || 0) - (b.message_index || 0);
            if (delta !== 0) return delta;
            return (OSA.eventTimestampMs(a.timestamp) || 0) - (OSA.eventTimestampMs(b.timestamp) || 0);
        });

    tools.forEach(function(t) {
        const item = OSA.tmodelToolItem({
            tool_call_id: t.tool_call_id,
            tool_name: t.tool_name,
            arguments: t.arguments || {},
            output: typeof t.output === 'string' ? t.output : '',
            title: typeof t.title === 'string' ? t.title : '',
            metadata: t.metadata,
            message_index: t.message_index,
            timestamp: t.timestamp,
        }, { completed: t.completed === true, success: t.success === true, live: false });
        let pos = items.length;
        for (let i = items.length - 1; i >= 0; i--) {
            const anchor = anchorOf(items[i]);
            if (anchor !== null && anchor <= (item.anchorIndex === null ? -1 : item.anchorIndex)) {
                pos = i + 1;
                break;
            }
        }
        items.splice(pos, 0, item);
    });

    const subagents = (Array.isArray(subagentTasks) ? subagentTasks : [])
        .slice()
        .sort(function(a, b) {
            return (OSA.eventTimestampMs(a.created_at) || 0) - (OSA.eventTimestampMs(b.created_at) || 0);
        });

    subagents.forEach(function(task) {
        if (!task || !task.session_id) return;
        const item = OSA.tmodelSubagentItem(task, false);
        const createdMs = OSA.eventTimestampMs(task.created_at);
        let pos = items.length;
        if (createdMs !== null) {
            for (let i = items.length - 1; i >= 0; i--) {
                const entry = items[i];
                if (entry.kind !== 'message') continue;
                const entryMs = OSA.eventTimestampMs(entry.timestamp);
                if (entryMs !== null && entryMs <= createdMs) {
                    pos = i + 1;
                    break;
                }
            }
        }
        items.splice(pos, 0, item);
    });

    const running = (session && session.task_status === 'running')
        || (typeof OSA.isAgentProcessing === 'function' && OSA.isAgentProcessing());
    if (running && options.adoptStreaming !== false) {
        for (let i = items.length - 1; i >= 0; i--) {
            if (items[i].kind !== 'message') continue;
            if (items[i].role === 'assistant') {
                items[i].streaming = true;
            }
            break;
        }
    }

    OSA.tmodelReset();
    items.forEach(function(item) { OSA.tmodelAppend(item); });
    OSA.tmodelMarkDirty(options.reason || 'rebuild');
};

OSA.rebuildAfterTruncate = function(fromIndex) {
    const session = OSA.getCurrentSession();
    if (!session) return;
    const tools = (OSA.getSessionToolEvents() || []).filter(function(t) {
        const messageIndex = OSA.messageIndexValue(t.message_index);
        return messageIndex === null || messageIndex < fromIndex;
    });
    OSA.setSessionToolEvents(tools);
    OSA.rebuildTranscriptFromSession(session, tools, OSA.getSessionSubagentTasks() || []);
};

OSA.buildTranscriptUnits = function() {
    const items = OSA.TModel.items;
    const units = [];
    const PARALLEL_WINDOW_MS = 2000;
    let i = 0;

    while (i < items.length) {
        const item = items[i];
        if (item.kind === 'tool' && item.context) {
            const groupItems = [];
            while (i < items.length && items[i].kind === 'tool' && items[i].context) {
                groupItems.push(items[i]);
                i += 1;
            }
            units.push({ type: 'context-group', key: 'ctxgrp:' + groupItems[0].key, items: groupItems });
            continue;
        }
        if (item.kind === 'tool') {
            const run = [item];
            let j = i + 1;
            while (j < items.length
                && items[j].kind === 'tool'
                && !items[j].context
                && Math.abs((items[j].ts || 0) - (item.ts || 0)) <= PARALLEL_WINDOW_MS) {
                run.push(items[j]);
                j += 1;
            }
            if (run.length >= 2) {
                units.push({ type: 'parallel-group', key: 'par:' + run[0].key, items: run });
                i = j;
            } else {
                units.push({ type: 'tool', key: item.key, items: [item] });
                i += 1;
            }
            continue;
        }
        units.push({ type: item.kind, key: item.key, item, items: [item] });
        i += 1;
    }
    return units;
};

OSA.unitSignature = function(unit) {
    return unit.items.map(function(item) {
        if (item.kind === 'message') {
            return [
                item.role,
                OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || ''),
                item.thinking || '',
                item.timestamp || '',
                item.images.length + '/' + item.attachments.length,
                item.streaming ? 'S' : '',
                item.thinkingStreaming ? 'T' : '',
                item.durationMs || '',
                item.tps || '',
                OSA.getShowThinkingBlocks() ? '1' : '0',
            ].join('\u0002');
        }
        if (item.kind === 'tool') {
            return [item.toolName, item.status, item.completed ? '1' : '0', item.output || '', item.title || ''].join('\u0002');
        }
        if (item.kind === 'subagent') {
            return [
                item.status, item.isRunning ? 'R' : '', item.toolCount,
                item.result, item.currentTool, item.retryText,
                item.tools.map(function(t) { return t.name + ':' + t.status; }).join(','),
                item.durationMs || '', item.contextState ? JSON.stringify(item.contextState) : '',
            ].join('\u0002');
        }
        return JSON.stringify(item);
    }).join('\u0001');
};

OSA.unitHasLiveStream = function(unit) {
    return unit.items.some(function(item) {
        return item.kind === 'message' && (item.streaming || item.thinkingStreaming);
    });
};

OSA.ensureMessageLayers = function() {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

    const view = OSA.getTranscriptView();
    if (view.initialized && view.transcriptRoot?.isConnected && view.floatingRoot?.isConnected) {
        return view;
    }

    const transcriptRoot = document.createElement('div');
    transcriptRoot.className = 'messages-transcript-root';

    const topSpacer = document.createElement('div');
    topSpacer.className = 'messages-virtual-spacer top';

    const topSentinel = document.createElement('div');
    topSentinel.className = 'messages-virtual-sentinel top';
    topSentinel.setAttribute('aria-hidden', 'true');

    const listRoot = document.createElement('div');
    listRoot.className = 'messages-transcript-list';

    const bottomSentinel = document.createElement('div');
    bottomSentinel.className = 'messages-virtual-sentinel bottom';
    bottomSentinel.setAttribute('aria-hidden', 'true');

    const bottomSpacer = document.createElement('div');
    bottomSpacer.className = 'messages-virtual-spacer bottom';

    transcriptRoot.append(topSpacer, topSentinel, listRoot, bottomSentinel, bottomSpacer);

    const floatingRoot = document.createElement('div');
    floatingRoot.className = 'messages-floating-root';

    messagesDiv.replaceChildren(transcriptRoot, floatingRoot);

    view.transcriptRoot = transcriptRoot;
    view.topSpacer = topSpacer;
    view.topSentinel = topSentinel;
    view.listRoot = listRoot;
    view.bottomSentinel = bottomSentinel;
    view.bottomSpacer = bottomSpacer;
    view.floatingRoot = floatingRoot;

    if (!view.scrollHandlerAttached) {
        messagesDiv.addEventListener('scroll', function() {
            const distance = messagesDiv.scrollHeight - messagesDiv.scrollTop - messagesDiv.clientHeight;
            view.userPinnedToBottom = distance < 120;
        }, { passive: true });
        view.scrollHandlerAttached = true;
    }

    if (!view.ioTop) {
        view.ioTop = new IntersectionObserver(function(entries) {
            if (view.isRendering || view.shiftInProgress) return;
            if (!view.units || view.units.length <= view.maxWindowSize) return;
            if ((Date.now() - view.lastShiftAt) < 80) return;
            if (entries.some(function(entry) { return entry.isIntersecting; })) {
                OSA.shiftTranscriptWindow(-1);
            }
        }, { root: messagesDiv, threshold: 0.01, rootMargin: '220px 0px 0px 0px' });
    }

    if (!view.ioBottom) {
        view.ioBottom = new IntersectionObserver(function(entries) {
            if (view.isRendering || view.shiftInProgress) return;
            if (!view.units || view.units.length <= view.maxWindowSize) return;
            if ((Date.now() - view.lastShiftAt) < 80) return;
            entries.forEach(function(entry) {
                if (entry.target === view.bottomSentinel) {
                    view.userPinnedToBottom = entry.isIntersecting;
                }
            });
            if (entries.some(function(entry) { return entry.isIntersecting; })) {
                OSA.shiftTranscriptWindow(1);
            }
        }, { root: messagesDiv, threshold: 0.01, rootMargin: '0px 0px 220px 0px' });
    }

    view.ioTop.disconnect();
    view.ioBottom.disconnect();
    view.ioTop.observe(topSentinel);
    view.ioBottom.observe(bottomSentinel);
    view.initialized = true;
    return view;
};

OSA.getFloatingRoot = function() {
    const view = OSA.ensureMessageLayers();
    return view ? view.floatingRoot : null;
};

OSA.mountFloatingNode = function(node, insertBefore = null) {
    const floatingRoot = OSA.getFloatingRoot();
    if (!floatingRoot || !node) return node;
    if (insertBefore && insertBefore.parentNode === floatingRoot) {
        floatingRoot.insertBefore(node, insertBefore);
    } else {
        floatingRoot.appendChild(node);
    }
    return node;
};

OSA.renderStreamingText = function(el, text) {
    if (el.dataset.rawText === text && el.dataset.renderedText === text) return;
    if (el.dataset.renderedText === undefined || el.dataset.renderedText === '') {
        el.innerHTML = '';
        el._md = OSA.createIncrementalMd();
    }
    if (el._md) {
        OSA.renderIncrementalMarkdown(el, text);
    } else {
        el.innerHTML = OSA.formatMessage(text);
        el.dataset.renderedText = text;
    }
    el.dataset.rawText = text;
};

OSA.setStaticMessageHtml = function(el, text) {
    el._md = null;
    el.innerHTML = text.trim() ? OSA.formatMessage(text) : '';
    el.dataset.rawText = text;
    el.dataset.renderedText = text;
};

OSA.estimateUnitRangeHeight = function(view, units, start, end) {
    let total = 0;
    for (let i = start; i < end; i++) {
        const unit = units[i];
        if (!unit) continue;
        total += view.messageHeights.get(unit.key) || view.avgMessageHeight;
    }
    return total;
};

OSA.shiftTranscriptWindow = function(direction) {
    const view = OSA.getTranscriptView();
    const total = view.units ? view.units.length : 0;
    if (!total || view.shiftInProgress) return;

    let nextStart = view.windowStart;
    let nextEnd = view.windowEnd;

    if (direction < 0 && view.windowStart > 0) {
        nextStart = Math.max(0, view.windowStart - view.windowShiftSize);
        nextEnd = Math.min(total, nextStart + view.maxWindowSize);
    } else if (direction > 0 && view.windowEnd < total) {
        nextEnd = Math.min(total, view.windowEnd + view.windowShiftSize);
        nextStart = Math.max(0, nextEnd - view.maxWindowSize);
    } else {
        return;
    }

    view.windowStart = nextStart;
    view.windowEnd = nextEnd;
    view.shiftInProgress = true;
    OSA.renderTranscript({
        reason: 'window-shift',
        keepWindow: true,
        preserveScroll: true,
        stickToBottom: false,
    });
    requestAnimationFrame(function() {
        view.lastShiftAt = Date.now();
        view.shiftInProgress = false;
    });
};

OSA.scheduleTranscriptRender = function() {
    if (OSA.TModel.frame != null) return;
    OSA.TModel.frame = requestAnimationFrame(function() {
        OSA.TModel.frame = null;
        if (!OSA.TModel.dirty) return;
        OSA.TModel.dirty = false;
        const reason = OSA.TModel.pendingReason;
        OSA.TModel.pendingReason = '';
        OSA.renderTranscript({ reason: reason });
    });
};

OSA.renderTranscript = function(options = {}) {
    const perfStart = OSA.perfNow ? OSA.perfNow() : Date.now();
    const view = OSA.ensureMessageLayers();
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv || !view || !view.listRoot) return;
    if (view.isRendering) {
        OSA.TModel.dirty = true;
        OSA.scheduleTranscriptRender();
        return;
    }
    view.isRendering = true;

    try {
        const units = OSA.buildTranscriptUnits();
        view.units = units;
        view.descriptors = units;
        const total = units.length;
        const shouldStickBottom = !!(options.stickToBottom || view.userPinnedToBottom);

        if (!options.keepWindow || view.windowEnd <= view.windowStart) {
            if (total <= view.maxWindowSize) {
                view.windowStart = 0;
                view.windowEnd = total;
            } else if (shouldStickBottom || options.preferTail) {
                view.windowEnd = total;
                view.windowStart = Math.max(0, total - view.maxWindowSize);
            } else {
                view.windowStart = Math.max(0, Math.min(view.windowStart, total - view.maxWindowSize));
                view.windowEnd = Math.min(total, view.windowStart + view.maxWindowSize);
            }
        } else {
            view.windowStart = Math.max(0, Math.min(view.windowStart, total));
            view.windowEnd = Math.max(view.windowStart, Math.min(view.windowEnd, total));
            if ((view.windowEnd - view.windowStart) > view.maxWindowSize) {
                view.windowEnd = view.windowStart + view.maxWindowSize;
            }
        }

        const windowed = units.slice(view.windowStart, view.windowEnd);

        const anchorWrapper = view.listRoot.querySelector('.transcript-entry');
        const anchorKey = anchorWrapper ? anchorWrapper.dataset.unitKey : '';
        const anchorTop = anchorWrapper ? anchorWrapper.getBoundingClientRect().top : 0;

        const desired = [];
        windowed.forEach(function(unit) {
            desired.push(OSA.ensureUnitNode(view, unit));
        });

        const structureChanged = OSA.reconcileTranscriptList(view.listRoot, desired);

        const allUnitKeys = new Set(units.map(function(u) { return u.key; }));
        Array.from(view.wrapperNodesByKey.keys()).forEach(function(key) {
            if (!allUnitKeys.has(key)) view.wrapperNodesByKey.delete(key);
        });
        Array.from(view.toolNodesByCallId.keys()).forEach(function(callId) {
            if (!OSA.tmodelGet('tool:' + callId)) view.toolNodesByCallId.delete(callId);
        });
        Array.from(view.ctxNodesByCallId.keys()).forEach(function(callId) {
            if (!OSA.tmodelGet('tool:' + callId)) view.ctxNodesByCallId.delete(callId);
        });

        const heightBefore = OSA.estimateUnitRangeHeight(view, units, 0, view.windowStart);
        const heightAfter = OSA.estimateUnitRangeHeight(view, units, view.windowEnd, total);
        view.topSpacer.style.height = Math.max(0, Math.round(heightBefore)) + 'px';
        view.bottomSpacer.style.height = Math.max(0, Math.round(heightAfter)) + 'px';

        if (structureChanged) {
            let measuredTotal = 0;
            let measuredCount = 0;
            desired.forEach(function(wrapper) {
                const height = wrapper.getBoundingClientRect().height;
                if (height > 0) {
                    view.messageHeights.set(wrapper.dataset.unitKey, height);
                    measuredTotal += height;
                    measuredCount += 1;
                }
            });
            if (measuredCount > 0) {
                view.avgMessageHeight = measuredTotal / measuredCount;
            }
        }

        if (shouldStickBottom) {
            messagesDiv.scrollTop = messagesDiv.scrollHeight;
        } else if (options.preserveScroll !== false && anchorKey) {
            const nextAnchor = desired.find(function(w) { return w.dataset.unitKey === anchorKey; }) || null;
            if (nextAnchor) {
                const nextTop = nextAnchor.getBoundingClientRect().top;
                messagesDiv.scrollTop += (nextTop - anchorTop);
            }
        }

        if (!OSA.tmodelStreamingItem()) {
            OSA.setStreamingAssistantDomId(null);
        }

        view.lastDescriptorCount = total;

        const elapsedMs = Math.round((OSA.perfNow ? OSA.perfNow() : Date.now()) - perfStart);
        if ((options.reason === 'rebuild' || options.reason === 'session-switch' || elapsedMs > 24) && OSA.perfLog) {
            OSA.perfLog('renderTranscript', {
                reason: options.reason || '',
                totalUnits: total,
                renderedUnits: windowed.length,
                elapsedMs,
            });
        }
        if (OSA.debug) {
            OSA.debug.log('render.transcript', {
                reason: options.reason || '',
                total,
                windowStart: view.windowStart,
                windowEnd: view.windowEnd,
                structureChanged,
            });
        }
    } finally {
        view.isRendering = false;
    }
};

OSA.reconcileTranscriptList = function(listRoot, desiredNodes) {
    let cursor = listRoot.firstChild;
    let same = true;
    for (let i = 0; i < desiredNodes.length; i++) {
        if (cursor !== desiredNodes[i]) { same = false; break; }
        cursor = cursor.nextSibling;
    }
    if (same && !cursor) return false;

    cursor = listRoot.firstChild;
    let changed = false;
    for (const node of desiredNodes) {
        if (cursor === node) {
            cursor = cursor.nextSibling;
            continue;
        }
        listRoot.insertBefore(node, cursor);
        changed = true;
    }
    while (cursor) {
        const next = cursor.nextSibling;
        cursor.remove();
        changed = true;
        cursor = next;
    }
    return changed;
};

OSA.ensureUnitNode = function(view, unit) {
    let wrapper = view.wrapperNodesByKey.get(unit.key);
    if (wrapper && !wrapper.isConnected) wrapper = null;
    if (!wrapper) {
        wrapper = document.createElement('div');
        wrapper.className = 'transcript-entry';
        wrapper.dataset.unitKey = unit.key;
        view.wrapperNodesByKey.set(unit.key, wrapper);
    }
    wrapper.dataset.unitKey = unit.key;
    OSA.patchUnit(wrapper, unit);
    return wrapper;
};

OSA.patchUnit = function(wrapper, unit) {
    if (!OSA.unitHasLiveStream(unit)) {
        const sig = OSA.unitSignature(unit);
        if (wrapper.dataset.sig === sig) return;
        wrapper.dataset.sig = sig;
    } else {
        wrapper.dataset.sig = OSA.unitSignature(unit);
    }

    switch (unit.type) {
        case 'message':
            OSA.patchMessageUnit(wrapper, unit);
            break;
        case 'tool':
            OSA.patchToolUnit(wrapper, unit);
            break;
        case 'context-group':
            OSA.patchContextGroupUnit(wrapper, unit);
            break;
        case 'parallel-group':
            OSA.patchParallelGroupUnit(wrapper, unit);
            break;
        case 'subagent':
            OSA.patchSubagentUnit(wrapper, unit);
            break;
        case 'task':
            OSA.patchSimpleMessageUnit(wrapper, unit, 'task', 'Tasks', function(item) {
                const uuidRegex = /\b([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\b/gi;
                const content = String(item.content || '').replace(uuidRegex, function(match, uuid) {
                    return `<a class="subagent-link" href="#session=${uuid}" onclick="event.preventDefault(); event.stopPropagation(); OSA.openSubagentSession('${uuid}')">${uuid}</a>`;
                });
                return OSA.formatMessage(content);
            });
            break;
        case 'error':
            OSA.patchSimpleMessageUnit(wrapper, unit, 'error', 'Error', function(item) {
                return OSA.escapeHtml(item.error || '');
            });
            break;
        case 'cancelled':
            OSA.patchSimpleMessageUnit(wrapper, unit, 'cancelled', 'Cancelled', function() {
                return 'Operation stopped by user';
            });
            break;
        default:
            break;
    }
};

OSA.ensureMessageContentEl = function(msgEl) {
    let contentEl = msgEl.querySelector(':scope > .message-content');
    if (!contentEl) {
        contentEl = document.createElement('div');
        contentEl.className = 'message-content';
        msgEl.appendChild(contentEl);
    }
    return contentEl;
};

OSA.patchMessageUnit = function(wrapper, unit) {
    const item = unit.item;
    let msgEl = wrapper.firstElementChild;
    if (!msgEl || !msgEl.classList.contains('message')) {
        msgEl = document.createElement('div');
        wrapper.replaceChildren(msgEl);
    }

    const wasStreaming = msgEl.classList.contains('streaming');
    const nextClass = 'message ' + item.role + (item.streaming ? ' streaming' : '');
    if (msgEl.className !== nextClass) msgEl.className = nextClass;

    if (item.messageIndex !== null) msgEl.dataset.messageIndex = String(item.messageIndex);
    if (item.timestamp) msgEl.dataset.messageTimestamp = item.timestamp;
    if (item.clientMessageId) msgEl.dataset.clientMessageId = item.clientMessageId;
    else delete msgEl.dataset.clientMessageId;

    if (item.streaming) {
        if (!msgEl.id) {
            msgEl.id = 'assistant-stream-' + Date.now().toString(36) + '-' + Math.random().toString(36).slice(2, 7);
        }
        OSA.setStreamingAssistantDomId(msgEl.id);
    }

    let roleEl = msgEl.querySelector(':scope > .message-role');
    if (!roleEl) {
        roleEl = document.createElement('div');
        roleEl.className = 'message-role';
        msgEl.prepend(roleEl);
    }
    const roleText = item.role === 'user' ? 'You' : 'OSA';
    if (roleEl.textContent !== roleText) roleEl.textContent = roleText;

    const contentEl = OSA.ensureMessageContentEl(msgEl);

    const showThinking = item.role === 'assistant'
        && OSA.getShowThinkingBlocks()
        && !!(item.thinking || '').trim();
    let thinkingWrap = msgEl.querySelector(':scope > .message-thinking');
    if (showThinking) {
        if (!thinkingWrap) {
            thinkingWrap = document.createElement('div');
            thinkingWrap.className = 'message-thinking';
            thinkingWrap.innerHTML = '<button type="button" class="thinking-toggle" onclick="OSA.toggleThinkingBlock(this)">'
                + '<span class="thinking-toggle-label">Thinking</span>'
                + '<span class="thinking-preview"></span>'
                + '</button>'
                + '<div class="thinking-body"></div>';
            msgEl.insertBefore(thinkingWrap, contentEl);
        }
        const body = thinkingWrap.querySelector('.thinking-body');
        const thinkingText = item.thinking || '';
        if ((body.dataset.rawText || '') !== thinkingText) {
            if (item.thinkingStreaming || item.streaming) {
                OSA.renderStreamingText(body, thinkingText);
            } else {
                OSA.setStaticMessageHtml(body, thinkingText);
            }
        }
        thinkingWrap.classList.toggle('streaming', !!item.thinkingStreaming);
        OSA.setThinkingPreview(thinkingWrap, thinkingText);
    } else if (thinkingWrap) {
        thinkingWrap.remove();
    }

    if (item.role === 'assistant') {
        const display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
        if ((contentEl.dataset.rawText || '') !== display) {
            if (item.streaming) {
                OSA.renderStreamingText(contentEl, display);
            } else {
                OSA.setStaticMessageHtml(contentEl, display);
            }
        }
    } else if (contentEl.textContent !== (item.content || '')) {
        contentEl.textContent = item.content || '';
    }

    OSA.patchMessageAttachments(msgEl, item);

    if (item.role === 'assistant') {
        let actionsEl = msgEl.querySelector(':scope > .message-actions');
        if (!actionsEl) {
            actionsEl = document.createElement('div');
            actionsEl.className = 'message-actions';
            msgEl.appendChild(actionsEl);
        }
        OSA.patchAssistantMetrics(msgEl, item);
        if (!item.streaming) {
            OSA.updateAssistantMessageActions(msgEl, null);
        } else {
            actionsEl.style.display = 'none';
        }
    }

    if (wasStreaming && !item.streaming) {
        OSA.finalizeIncrementalRenders(msgEl);
    }
};

OSA.patchMessageAttachments = function(msgEl, item) {
    const sig = item.images.length + '|' + item.attachments.length;
    if (msgEl.dataset.attachmentsSig === sig) return;
    msgEl.dataset.attachmentsSig = sig;

    let wrap = msgEl.querySelector(':scope > .message-attachments');
    if (!sig || sig === '0|0') {
        if (wrap) wrap.remove();
        return;
    }
    if (!wrap) {
        wrap = document.createElement('div');
        wrap.className = 'message-attachments';
        const actionsEl = msgEl.querySelector(':scope > .message-actions');
        if (actionsEl) msgEl.insertBefore(wrap, actionsEl);
        else msgEl.appendChild(wrap);
    }
    wrap.innerHTML = OSA.renderAttachmentMarkup([].concat(
        item.images.map(function(img) {
            return {
                kind: 'image',
                mime: img.mime || '',
                filename: img.filename || '',
                previewUrl: OSA.getAttachmentImageSrc(img),
            };
        }),
        item.attachments,
    ));
};

OSA.patchAssistantMetrics = function(msgEl, item) {
    const actionsEl = msgEl.querySelector(':scope > .message-actions');
    if (!actionsEl) return;

    if (item.durationMs !== null && item.durationMs !== undefined) {
        let durationEl = actionsEl.querySelector('.turn-duration');
        if (!durationEl) {
            durationEl = document.createElement('span');
            durationEl.className = 'turn-duration';
            actionsEl.appendChild(durationEl);
        }
        const elapsed = Math.round(item.durationMs / 1000);
        durationEl.textContent = elapsed < 60
            ? elapsed + 's'
            : Math.floor(elapsed / 60) + 'm ' + (elapsed % 60) + 's';
    }

    if (item.tps) {
        let tpsEl = actionsEl.querySelector('.turn-tokens');
        if (!tpsEl) {
            tpsEl = document.createElement('span');
            tpsEl.className = 'turn-tokens';
            actionsEl.appendChild(tpsEl);
        }
        tpsEl.textContent = item.tps + ' tok/s';
        if (item.totalTokens) tpsEl.title = item.totalTokens + ' total tokens';
    }
};

OSA.buildToolCardElement = function(item) {
    const domId = 'tool-' + item.callId;
    const label = OSA.toolLabel(item.toolName);
    const icon = OSA.toolIcon(item.toolName);
    const subtitle = OSA.summarizeToolArgs(item.toolName, item.args);
    const isCompleted = item.completed === true;
    const isSuccess = item.success === true;
    const statusText = isCompleted ? (isSuccess ? 'done' : 'failed') : 'running';
    const statusClass = isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending';
    const titleClass = isCompleted ? '' : 'tool-title-pending';
    const chevronOpacity = isCompleted ? '' : 'opacity:0';

    const container = document.createElement('div');
    container.id = domId;
    container.className = 'tool-container';
    container.dataset.callId = item.callId;
    container.innerHTML = `
        <div class="tool-card tool-inline" id="card-${domId}" data-tool="${OSA.escapeHtml(item.toolName)}">
            <div class="tool-trigger tool-trigger-inline" onclick="OSA.handleToolCardClick('${domId}')">
                <span class="tool-icon">${icon}</span>
                <span class="tool-title ${titleClass}" id="title-${domId}">${OSA.escapeHtml(label)}</span>
                ${subtitle ? `<span class="tool-subtitle" id="subtitle-${domId}">${OSA.escapeHtml(subtitle)}</span>` : ''}
                <button type="button" class="tool-preview-btn hidden" onclick="OSA.openPreviewFromButton('${domId}', event)" title="Open in preview" aria-label="Open in preview">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M3 5h18"></path>
                        <path d="M3 12h7"></path>
                        <path d="M3 19h7"></path>
                        <rect x="12" y="8" width="9" height="11" rx="1"></rect>
                    </svg>
                </button>
                <span class="tool-status-badge ${statusClass}" id="status-${domId}">${statusText}</span>
                <span class="tool-chevron" id="chevron-${domId}" style="${chevronOpacity}">&#x25B6;</span>
            </div>
            <div class="tool-body" id="body-${domId}">
                <div class="tool-body-inner">
                    <div class="tool-args" id="args-${domId}">${OSA.escapeHtml(JSON.stringify(item.args, null, 2))}</div>
                    <div class="tool-output" id="output-${domId}" style="display:none"></div>
                </div>
            </div>
        </div>`;
    return container;
};

OSA.patchToolCardElement = function(container, item) {
    const domId = 'tool-' + item.callId;
    const isCompleted = item.completed === true;
    const isSuccess = item.success === true;

    const statusEl = container.querySelector('#status-' + OSA.cssEscape(domId));
    if (statusEl) {
        const statusText = isCompleted ? (isSuccess ? 'done' : 'failed') : (item.status || 'running').toLowerCase();
        const statusClass = isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending';
        if (statusEl.textContent !== statusText) statusEl.textContent = statusText;
        statusEl.className = 'tool-status-badge ' + statusClass;
    }

    const titleEl = container.querySelector('#title-' + OSA.cssEscape(domId));
    if (titleEl) titleEl.classList.toggle('tool-title-pending', !isCompleted);

    const subtitleEl = container.querySelector('#subtitle-' + OSA.cssEscape(domId));
    if (item.title && subtitleEl && !subtitleEl.textContent) {
        subtitleEl.textContent = String(item.title);
    }

    if (isCompleted && container.dataset.doneFlag !== '1') {
        container.dataset.doneFlag = '1';
        const card = container.querySelector(':scope > .tool-card');
        if (card) {
            card.classList.add('tool-complete');
            setTimeout(function() { card.classList.remove('tool-complete'); }, 400);
        }
        const chevron = container.querySelector('#chevron-' + OSA.cssEscape(domId));
        if (chevron) chevron.style.opacity = '';
    }

    const outputChanged = container._toolOutput !== item.output;
    if (outputChanged) {
        container._toolOutput = item.output;
        delete container.dataset.badgesDone;
        delete container.dataset.cmdLineDone;
    }

    if (isCompleted && item.output && outputChanged) {
        const outputEl = container.querySelector('#output-' + OSA.cssEscape(domId));
        if (outputEl) {
            const eventView = OSA.tmodelToolEventView(item);
            const renderedDiff = ['write_file', 'edit_file', 'apply_patch'].includes(item.toolName)
                ? OSA.renderToolDiff(outputEl, eventView)
                : false;
            const formatted = OSA.formatToolOutput(item.toolName, item.output);
            if (!renderedDiff && formatted) {
                outputEl.textContent = formatted;
                outputEl.style.display = '';
            } else if (renderedDiff) {
                outputEl.style.display = '';
            }
        }
        OSA.setToolCardPreviewData(domId, OSA.tmodelToolEventView(item));

        if (isSuccess && ['write_file', 'edit_file', 'apply_patch'].includes(item.toolName)) {
            const diff = OSA.parseDiffChanges(item.output);
            if ((diff.additions > 0 || diff.deletions > 0) && subtitleEl && !container.dataset.badgesDone) {
                container.dataset.badgesDone = '1';
                subtitleEl.innerHTML = subtitleEl.textContent
                    + ` <span class="diff-add">+${diff.additions}</span><span class="diff-del">-${diff.deletions}</span>`;
            }
        }

        if (item.toolName === 'bash' && isSuccess && !container.dataset.cmdLineDone) {
            container.dataset.cmdLineDone = '1';
            const argsEl = container.querySelector('#args-' + OSA.cssEscape(domId));
            const body = container.querySelector('#body-' + OSA.cssEscape(domId));
            if (argsEl) argsEl.style.display = 'none';
            const cmd = ((item.args && item.args.command) || '').trim();
            if (cmd && body) {
                const cmdLine = document.createElement('div');
                cmdLine.className = 'shell-command-line';
                cmdLine.innerHTML = '<span class="shell-prompt">$</span> <span class="shell-cmd">' + OSA.escapeHtml(cmd) + '</span>';
                const bodyInner = body.querySelector('.tool-body-inner');
                if (bodyInner) bodyInner.insertBefore(cmdLine, bodyInner.firstChild);
            }
        }
    } else if (!isCompleted) {
        OSA.setToolCardPreviewData(domId, OSA.tmodelToolEventView(item));
    }
};

OSA.cssEscape = function(value) {
    return (window.CSS && window.CSS.escape)
        ? window.CSS.escape(value)
        : String(value).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
};

OSA.ensureToolContainerNode = function(item) {
    const view = OSA.getTranscriptView();
    let el = view.toolNodesByCallId.get(item.callId);
    if (!el) {
        el = OSA.buildToolCardElement(item);
        view.toolNodesByCallId.set(item.callId, el);
    }
    OSA.patchToolCardElement(el, item);
    return el;
};

OSA.patchToolUnit = function(wrapper, unit) {
    const item = unit.items[0];
    let container = wrapper.firstElementChild;
    if (!container || !container.classList.contains('tool-container') || container.dataset.callId !== item.callId) {
        container = OSA.ensureToolContainerNode(item);
        wrapper.replaceChildren(container);
        return;
    }
    OSA.patchToolCardElement(container, item);
};

OSA.buildContextToolRow = function(item) {
    const row = document.createElement('div');
    row.className = 'context-inline-item';
    row.id = 'ctx-' + item.callId;
    row.setAttribute('onclick', "OSA.handleContextToolClick('ctx-" + item.callId + "')");
    OSA.patchContextToolRow(row, item);
    return row;
};

OSA.patchContextToolRow = function(row, item) {
    const isCompleted = item.completed === true;
    const isSuccess = item.success === true;
    const statusText = isCompleted ? (isSuccess ? 'done' : 'failed') : (item.status || 'running').toLowerCase();
    row.innerHTML = `
        <span class="context-inline-action">${OSA.escapeHtml(OSA.toolLabel(item.toolName))}</span>
        <span class="context-inline-detail">${OSA.escapeHtml(OSA.summarizeToolArgs(item.toolName, item.args))}</span>
        <button type="button" class="context-inline-preview-btn hidden" onclick="OSA.openPreviewFromContextButton('${row.id}', event)" title="Open in preview" aria-label="Open in preview">
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 5h18"></path>
                <path d="M3 12h7"></path>
                <path d="M3 19h7"></path>
                <rect x="12" y="8" width="9" height="11" rx="1"></rect>
            </svg>
        </button>
        <span class="context-inline-status ${isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending'}">${statusText}</span>
    `;
    OSA.setContextToolPreviewData(row, OSA.tmodelToolEventView(item));
};

OSA.patchContextGroupUnit = function(wrapper, unit) {
    const view = OSA.getTranscriptView();
    let group = wrapper.firstElementChild;
    if (!group || !group.classList.contains('context-inline-group')) {
        group = document.createElement('div');
        group.className = 'tool-container context-inline-group';
        group.id = 'context-tool-group-' + unit.items[0].callId;
        wrapper.replaceChildren(group);
    }

    unit.items.forEach(function(item) {
        let row = view.ctxNodesByCallId.get(item.callId);
        if (!row || !row.isConnected || row.parentNode !== group) {
            if (!row) {
                row = OSA.buildContextToolRow(item);
                view.ctxNodesByCallId.set(item.callId, row);
            }
            group.appendChild(row);
        }
        OSA.patchContextToolRow(row, item);
    });
};

OSA.patchParallelGroupUnit = function(wrapper, unit) {
    let group = wrapper.firstElementChild;
    if (!group || !group.classList.contains('parallel-group')) {
        group = document.createElement('div');
        group.className = 'parallel-group';
        wrapper.replaceChildren(group);
    }

    const anyRunning = unit.items.some(function(item) { return !item.completed; });
    const headerLabel = unit.items.length + ' tools ' + (anyRunning ? 'running' : 'executed') + ' concurrently';
    const headerSig = unit.items.length + (anyRunning ? '|r' : '|d');
    if (group.dataset.headerSig !== headerSig) {
        group.dataset.headerSig = headerSig;
        group.innerHTML = '<div class="parallel-group-header"><span class="parallel-count">' + OSA.escapeHtml(headerLabel) + '</span></div>';
    }

    unit.items.forEach(function(item) {
        const card = OSA.ensureToolContainerNode(item);
        if (card.parentNode !== group) group.appendChild(card);
    });
};

OSA.buildSubagentCardElement = function(item) {
    const subagentId = item.subagentId;
    const card = document.createElement('div');
    card.id = 'subagent-' + subagentId;
    card.className = 'subagent-card';
    const contextRingHtml = OSA.buildContextRingHtml(item.contextState, subagentId);
    const durationText = OSA.formatSubagentDuration(item.durationMs);
    card.innerHTML = `
        <div class="subagent-header" onclick="OSA.toggleSubagentCard('${subagentId}')">
            <div class="subagent-info">
                <span class="subagent-icon">A</span>
                <span class="subagent-title">${OSA.escapeHtml(item.description)}</span>
                <span class="subagent-type">${OSA.escapeHtml(item.agentType)}</span>
            </div>
            <div class="subagent-status">
                ${contextRingHtml}
                <span class="subagent-status-badge" id="subagent-status-${subagentId}"></span>
                <span class="subagent-tool-count" id="subagent-count-${subagentId}"></span>
                <span class="subagent-chevron" id="subagent-chevron-${subagentId}">&#x25B6;</span>
            </div>
        </div>
        <div class="subagent-live" id="subagent-live-${subagentId}" style="display:none">
            <span class="subagent-current-tool" id="subagent-current-${subagentId}"></span>
        </div>
        <div class="subagent-body" id="subagent-body-${subagentId}" style="display:none">
            <div class="subagent-body-inner">
                <div class="subagent-prompt" id="subagent-prompt-${subagentId}"></div>
                <div class="subagent-tools" id="subagent-tools-${subagentId}"></div>
                <div class="subagent-result" id="subagent-result-${subagentId}" style="display:none"></div>
                <div class="subagent-actions">
                    <button class="subagent-btn" onclick="OSA.openSubagentSession('${subagentId}')">Open Session</button>
                </div>
            </div>
        </div>
    `;
    return card;
};

OSA.patchSubagentUnit = function(wrapper, unit) {
    const item = unit.item;
    let card = wrapper.firstElementChild;
    if (!card || !card.classList.contains('subagent-card')) {
        card = OSA.buildSubagentCardElement(item);
        wrapper.replaceChildren(card);
    }

    const subagentId = item.subagentId;
    const statusWrap = card.querySelector('.subagent-status');
    if (statusWrap && item.contextState && !statusWrap.querySelector('.subagent-context-ring')) {
        statusWrap.insertAdjacentHTML('afterbegin', OSA.buildContextRingHtml(item.contextState, subagentId));
    }
    const badgeStatus = item.isRunning ? 'running' : (item.retryText ? 'retrying' : (item.status || 'running'));
    const statusBadge = card.querySelector('#subagent-status-' + OSA.cssEscape(subagentId));
    if (statusBadge) {
        if (statusBadge.textContent !== badgeStatus) statusBadge.textContent = badgeStatus;
        statusBadge.className = 'subagent-status-badge ' + badgeStatus;
    }

    const countEl = card.querySelector('#subagent-count-' + OSA.cssEscape(subagentId));
    if (countEl) {
        const durationText = OSA.formatSubagentDuration(item.durationMs);
        const label = item.toolCount + ' tool' + (item.toolCount !== 1 ? 's' : '') + (durationText ? ' · ' + durationText : '');
        if (countEl.textContent !== label) countEl.textContent = label;
    }

    const promptEl = card.querySelector('#subagent-prompt-' + OSA.cssEscape(subagentId));
    if (promptEl && item.prompt && promptEl.textContent !== item.prompt) {
        promptEl.textContent = item.prompt;
    }

    const liveStrip = card.querySelector('#subagent-live-' + OSA.cssEscape(subagentId));
    const currentEl = card.querySelector('#subagent-current-' + OSA.cssEscape(subagentId));
    const liveText = item.retryText
        ? '↳ ' + item.retryText
        : (item.currentTool ? '↳ ' + item.currentTool : '');
    if (currentEl && currentEl.textContent !== liveText) currentEl.textContent = liveText;
    if (liveStrip) liveStrip.style.display = liveText ? '' : 'none';

    const toolsEl = card.querySelector('#subagent-tools-' + OSA.cssEscape(subagentId));
    if (toolsEl) {
        const toolsSig = item.tools.map(function(t) { return t.name + ':' + t.status; }).join(',');
        if (toolsEl.dataset.sig !== toolsSig) {
            toolsEl.dataset.sig = toolsSig;
            toolsEl.innerHTML = item.tools.map(function(t) {
                return '<div class="subagent-tool-item ' + OSA.escapeHtml(t.status) + '">' + OSA.escapeHtml(t.name) + '</div>';
            }).join('');
        }
    }

    const resultEl = card.querySelector('#subagent-result-' + OSA.cssEscape(subagentId));
    if (resultEl) {
        if (item.result) {
            resultEl.style.display = 'block';
            resultEl.innerHTML = '<div class="subagent-result-label">Result:</div><div class="subagent-result-text">'
                + OSA.escapeHtml(item.result.slice(0, 500))
                + (item.result.length > 500 ? '…' : '')
                + '</div>';
        } else if (!item.isRunning) {
            resultEl.style.display = 'none';
            resultEl.innerHTML = '';
        }
    }

    const cancelBtnId = 'subagent-cancel-' + subagentId;
    let cancelBtn = card.querySelector('#' + OSA.cssEscape(cancelBtnId));
    if (item.isRunning && !cancelBtn) {
        const actions = card.querySelector('.subagent-actions');
        if (actions) {
            cancelBtn = document.createElement('button');
            cancelBtn.id = cancelBtnId;
            cancelBtn.className = 'subagent-btn subagent-btn-cancel';
            cancelBtn.textContent = 'Cancel';
            cancelBtn.onclick = function() { OSA.cancelSubagent(subagentId); };
            actions.appendChild(cancelBtn);
        }
    } else if (!item.isRunning && cancelBtn) {
        cancelBtn.remove();
    }

    OSA.updateSubagentContextRing(subagentId, item.contextState);
};

OSA.patchSimpleMessageUnit = function(wrapper, unit, variant, roleLabel, renderContent) {
    const item = unit.item;
    let msgEl = wrapper.firstElementChild;
    if (!msgEl || !msgEl.classList.contains('message') || msgEl.dataset.simpleVariant !== variant) {
        msgEl = document.createElement('div');
        msgEl.className = 'message ' + variant;
        msgEl.dataset.simpleVariant = variant;
        wrapper.replaceChildren(msgEl);
    }
    const html = renderContent(item);
    if (msgEl.dataset.contentSig !== html) {
        msgEl.dataset.contentSig = html;
        msgEl.innerHTML = '<div class="message-role">' + roleLabel + '</div><div class="message-content">' + html + '</div>';
    }
};

OSA.getStreamingAssistantMessage = function() {
    const item = OSA.tmodelStreamingItem();
    if (!item) return null;
    if (OSA.TModel.dirty) {
        OSA.TModel.dirty = false;
        const reason = OSA.TModel.pendingReason;
        OSA.TModel.pendingReason = '';
        OSA.renderTranscript({ reason: reason });
    }
    const view = OSA.getTranscriptView();
    const wrapper = view.wrapperNodesByKey.get(item.key);
    return wrapper ? wrapper.querySelector(':scope > .message') : null;
};

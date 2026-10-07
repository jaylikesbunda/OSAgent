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

OSA.tmodelHasLiveAgentActivity = function() {
    return OSA.TModel.items.some(function(item) {
        if (!item || !item.live) return false;
        if (item.kind === 'message') return item.role === 'assistant';
        return item.kind === 'tool' || item.kind === 'subagent';
    });
};

OSA.tmodelSettleLiveItems = function() {
    OSA.TModel.items.forEach(function(item) {
        if (item) item.live = false;
    });
};

// Terminal events (Stop, error) do not emit tool_complete/subagent_completed
// for work that was still in flight, so any running card would keep its pulsing
// badge forever (and reload as "running" from the persisted completed=false
// event). Finish every running item when the turn ends abnormally.
OSA.tmodelSettleRunningItems = function(status) {
    const finalStatus = status || 'cancelled';
    const success = finalStatus === 'completed';
    let changed = false;
    OSA.TModel.items.forEach(function(item) {
        if (!item) return;
        item.live = false;
        if (item.kind === 'tool' && !item.completed) {
            item.completed = true;
            item.success = success;
            item.status = finalStatus;
            changed = true;
        } else if (item.kind === 'subagent' && item.isRunning) {
            item.isRunning = false;
            item.status = finalStatus;
            item.currentTool = '';
            item.retryText = '';
            changed = true;
        }
    });
    return changed;
};

// Copies text to the clipboard and flashes a check on the button that asked for
// it. No-op when the Clipboard API is unavailable.
OSA.copyTextWithFeedback = function(text, button) {
    if (!text) return;
    if (!(navigator.clipboard && navigator.clipboard.writeText)) return;
    navigator.clipboard.writeText(text).then(function() {
        if (!button) return;
        button.classList.add('copied');
        setTimeout(function() { button.classList.remove('copied'); }, 1500);
    }).catch(function(err) {
        console.warn('Copy failed:', err);
    });
};

OSA.copyToolCard = function(domId, event) {
    if (event) event.stopPropagation();
    const container = document.getElementById(domId);
    if (!container) return;
    let args = '';
    try {
        args = container._toolArgs ? JSON.stringify(container._toolArgs, null, 2) : '';
    } catch (err) {
        args = '';
    }
    const output = container._toolOutput || '';
    const text = [args, output].filter(Boolean).join('\n\n');
    OSA.copyTextWithFeedback(text, container.querySelector('.tool-copy-btn'));
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

OSA.tmodelCompactionItem = function(key, message, messageIndex) {
    const raw = message.content || '';
    let content = OSA.stripCompactedSummary
        ? OSA.stripCompactedSummary(raw)
        : String(raw || '').trim();
    content = OSA.stripToolCallMarkup ? OSA.stripToolCallMarkup(content) : content;
    return {
        kind: 'compaction',
        key,
        content,
        timestamp: message.timestamp || '',
        messageIndex: Number.isInteger(messageIndex) ? messageIndex : null,
    };
};

OSA.tmodelMessageItem = function(key, message, messageIndex, opts = {}) {
    const tokens = message.tokens || null;
    const cachedRead = tokens && Number.isFinite(tokens.cached_read) ? tokens.cached_read : null;
    const cachedWrite = tokens && Number.isFinite(tokens.cached_write) ? tokens.cached_write : null;
    const inputTokens = tokens && Number.isFinite(tokens.input) ? tokens.input : 0;
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
        totalTokens: tokens && tokens.total ? tokens.total : null,
        cachedRead,
        cachedWrite,
        cacheReason: tokens && typeof tokens.cache_reason === 'string' ? tokens.cache_reason : null,
        cacheReported: !!tokens,
        turnUsage: null,
        turnCacheHitRate: null,
        cacheHitRate: cachedRead !== null && inputTokens > 0
            ? ((cachedRead / inputTokens) * 100).toFixed(0)
            : null,
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
        prelude: typeof event.prelude === 'string' ? event.prelude : '',
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
    // The narration the model streamed just before this call is the tool's
    // prelude: fold it into the card instead of leaving a separate bubble.
    const streaming = OSA.tmodelStreamingItem();
    let prelude = typeof event.prelude === 'string' ? event.prelude.trim() : '';
    if (streaming) {
        const streamedPrelude = OSA.stripSpeakBlock
            ? OSA.stripSpeakBlock(streaming.content || '')
            : (streaming.content || '');
        if (streamedPrelude.trim()) prelude = streamedPrelude.trim();
    }
    let item = OSA.tmodelGet(key);
    if (item) {
        if (item.completed) {
            if (prelude && !item.prelude) {
                item.prelude = prelude;
                OSA.tmodelMarkDirty('tool-prelude-recovered');
            }
            return item;
        }
        item.toolName = event.tool_name || item.toolName;
        item.args = event.arguments || item.args;
        item.completed = false;
        item.success = false;
        item.status = 'running';
        item.output = '';
        item.ts = OSA.eventTimestampMs(event.timestamp) || Date.now();
        if (prelude && !item.prelude) item.prelude = prelude;
    } else {
        item = OSA.tmodelToolItem(Object.assign({}, event, prelude ? { prelude } : null));
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
    if (OSA._previewState?.open) OSA.renderFileTree?.();
    return item;
};

OSA.tmodelAddTaskMessage = function(content) {
    const cleaned = String(content || '').replace(/\s{2,}/g, ' ').trim();
    if (!cleaned) return;
    OSA.tmodelAppend({ kind: 'task', key: OSA.tmodelLiveKey('task'), content: cleaned });
};

// One Stop click can surface as several terminal events (the streaming layer,
// the run wrapper and a reconnect replay each emit `cancelled`/`error`). Track
// the most recent terminal banner so a burst collapses to a single card.
OSA._lastTerminalBannerAt = 0;
OSA._lastTerminalBannerKind = '';
OSA._TERMINAL_BANNER_COALESCE_MS = 1500;

OSA._isRecentTerminalBanner = function(kind) {
    if (!OSA._lastTerminalBannerAt) return false;
    if (Date.now() - OSA._lastTerminalBannerAt > OSA._TERMINAL_BANNER_COALESCE_MS) return false;
    return OSA._lastTerminalBannerKind === kind;
};

OSA._noteTerminalBanner = function(kind) {
    OSA._lastTerminalBannerAt = Date.now();
    OSA._lastTerminalBannerKind = kind;
};

OSA._resetTerminalBannerWindow = function() {
    OSA._lastTerminalBannerAt = 0;
    OSA._lastTerminalBannerKind = '';
};

OSA.tmodelAddError = function(error) {
    const msg = String(error || 'Unknown error');
    // A Stop is reported as `cancelled`, never as an error card. Some backend
    // paths used to re-classify cancellation ("Session error: Operation
    // cancelled") — swallow those so a cancel shows exactly one clean banner.
    if (/operation cancelled/i.test(msg)) {
        return;
    }
    // Dedup: provider errors can be delivered twice (ws + sse, or retry) —
    // ignore if the last terminal card was already this error moments ago.
    const last = OSA.TModel.items[OSA.TModel.items.length - 1];
    if (last && last.kind === 'error' && last.error === msg) return;
    // also check second-last in case an interleaving cancelled/error
    const prev = OSA.TModel.items[OSA.TModel.items.length - 2];
    if (prev && prev.kind === 'error' && prev.error === msg) return;
    OSA._noteTerminalBanner('error');
    OSA.tmodelAppend({ kind: 'error', key: OSA.tmodelLiveKey('error'), error: msg });
};

OSA.tmodelAddCancelled = function() {
    // Multiple run wrappers can each report the same stop; keep one card.
    if (OSA._isRecentTerminalBanner('cancelled')) return;
    OSA._noteTerminalBanner('cancelled');
    OSA.tmodelAppend({ kind: 'cancelled', key: OSA.tmodelLiveKey('cancelled') });
};

OSA.tmodelSubagentItem = function(data, live) {
    const subagentId = data.subagent_session_id || data.session_id;
    const terminalStatuses = ['completed', 'partial', 'failed', 'cancelled', 'timeout'];
    const isRunning = data.is_running === true
        || (live && terminalStatuses.indexOf(data.status) === -1);
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
        createdAt: data.created_at || data.timestamp || '',
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
    } else {
        // A manual resume reuses the child session, so the lifecycle event
        // updates the existing card instead of creating a second one.
        item.description = event.description || item.description;
        item.agentType = event.agent_type || item.agentType;
        item.prompt = event.prompt || item.prompt;
        item.status = 'running';
        item.isRunning = true;
        item.result = '';
        item.currentTool = '';
        item.retryText = '';
    }
    OSA.tmodelMarkDirty('subagent-created');
    return item;
};

OSA._MAX_SUBAGENT_TOOL_ROWS = 50;

OSA._subagentIsIterationMarker = function(name) {
    return /^iteration_\d+$/i.test(String(name || ''));
};

// Merge a tool row in place (first-seen order, status flips in place) instead
// of appending a new row per event. Repeat completions of the same tool bump
// a counter. Returns true when the visible card content actually changed.
OSA._subagentUpsertToolRow = function(item, name, status) {
    if (!name || OSA._subagentIsIterationMarker(name)) return false;
    const rows = item.tools;
    for (let i = rows.length - 1; i >= 0; i--) {
        if (rows[i].name === name) {
            const row = rows[i];
            let changed = false;
            // A fresh execution after a settled state is a re-run of the
            // same tool: count it instead of adding a duplicate row.
            if (status === 'running' && (row.status === 'completed' || row.status === 'failed')) {
                row.count = (row.count || 1) + 1;
                changed = true;
            }
            if (row.status !== status) { row.status = status; changed = true; }
            return changed;
        }
    }
    rows.push({ name, status, count: 1 });
    if (rows.length > OSA._MAX_SUBAGENT_TOOL_ROWS) {
        rows.splice(0, rows.length - OSA._MAX_SUBAGENT_TOOL_ROWS);
    }
    return true;
};

OSA.tmodelSubagentProgress = function(event) {
    if (!event || !event.subagent_session_id) return false;
    const item = OSA.tmodelGet('subagent:' + event.subagent_session_id);
    if (!item) return false;
    let changed = false;
    const count = event.tool_count || 0;
    if (count && count !== item.toolCount) { item.toolCount = count; changed = true; }
    const status = event.status || 'running';
    if (status === 'executing') {
        if (!item.isRunning) { item.isRunning = true; changed = true; }
        if (item.retryText) { item.retryText = ''; changed = true; }
        const tool = event.tool_name || '';
        if (tool && tool !== item.currentTool) { item.currentTool = tool; changed = true; }
        if (tool && OSA._subagentUpsertToolRow(item, tool, 'running')) changed = true;
    } else if (status === 'completed' || status === 'failed') {
        // Per-iteration heartbeats reuse tool_name for the loop counter
        // ("iteration_N") — they carry no tool info and must not add rows or
        // clobber the live strip.
        const tool = event.tool_name || '';
        if (tool && !OSA._subagentIsIterationMarker(tool)) {
            if (OSA._subagentUpsertToolRow(item, tool, status)) changed = true;
            if (item.currentTool === tool) { item.currentTool = ''; changed = true; }
        }
        if (item.retryText) { item.retryText = ''; changed = true; }
    } else if (event.tool_name) {
        if (OSA._subagentUpsertToolRow(item, event.tool_name, 'running')) changed = true;
    }
    if (changed) OSA.tmodelMarkDirty('subagent-progress');
    return changed;
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

// Task-level retry: the whole run is being relaunched after a transient
// failure, resuming the same subagent session.
OSA.tmodelSubagentTaskRetry = function(event) {
    if (!event || !event.subagent_session_id) return;
    const item = OSA.tmodelGet('subagent:' + event.subagent_session_id);
    if (!item) return;
    const delay = event.next_retry_in_ms ? Math.max(1, Math.round(event.next_retry_in_ms / 1000)) : null;
    const attempt = event.attempt_count || 0;
    const max = event.max_attempts || 0;
    let text = delay ? `resuming in ~${delay}s` : 'resuming';
    if (attempt && max) text += ` (attempt ${attempt}/${max})`;
    item.retryText = 'provider error — ' + text;
    item.status = 'running';
    item.isRunning = true;
    OSA.tmodelMarkDirty('subagent-task-retry');
};

OSA.tmodelSubagentContextUpdate = function(event) {
    const subagentId = event && (event.subagent_session_id || event.session_id);
    if (!subagentId) return;
    const item = OSA.tmodelGet('subagent:' + subagentId);
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

OSA.tmodelClearAllStreamingFlags = function() {
    // Every streaming transition keys off the *last* item, so an assistant
    // segment that stops being last (tool/task/subagent/question item
    // appended after it) would keep streaming=true forever: stuck typing
    // cursor, and a phantom streaming item that suppresses the thinking
    // indicator. Terminal transitions must clear every segment, not just
    // the tail one.
    let cleared = false;
    OSA.TModel.items.forEach(function(item) {
        if (!item || item.kind !== 'message' || item.role !== 'assistant') return;
        if (item.streaming || item.thinkingStreaming) {
            item.streaming = false;
            item.thinkingStreaming = false;
            cleared = true;
        }
    });
    return cleared;
};

OSA.tmodelFinalizeSegmentForToolCall = function() {
    const item = OSA.tmodelStreamingItem();
    if (!item) {
        OSA.tmodelClearAllStreamingFlags();
        return '';
    }

    // The visible response text immediately before a tool call is rendered as
    // that card's prelude. Reasoning is a separate part of the transcript and
    // must survive the boundary: removing the whole segment made its thinking
    // block disappear between consecutive tool calls.
    const display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
    item.streaming = false;
    item.thinkingStreaming = false;
    if ((item.thinking || '').trim()) {
        item.content = '';
    } else {
        OSA.tmodelRemove(item.key);
    }
    OSA.tmodelClearAllStreamingFlags();
    OSA.tmodelMarkDirty('segment-boundary');
    return display.trim();
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
    OSA.tmodelClearAllStreamingFlags();
    OSA.tmodelMarkDirty('release-stream');
};

OSA.tmodelFinalizeStreamingSegment = function(usage) {
    const item = OSA.tmodelStreamingItem();
    if (!item) {
        // The tail item is not a streaming segment, but an older segment may
        // still carry the flag (see tmodelClearAllStreamingFlags): clear it
        // so no typing cursor survives the turn.
        if (OSA.tmodelClearAllStreamingFlags()) OSA.tmodelMarkDirty('segment-final');
        return null;
    }
    item.streaming = false;
    item.thinkingStreaming = false;

    const startTime = OSA.getTurnStartTime();
    if (usage) {
        item.totalTokens = usage.total || null;
        item.cacheReported = true;
        const cachedRead = Number.isFinite(usage.cached_read) ? usage.cached_read : null;
        item.cachedRead = cachedRead;
        item.cachedWrite = Number.isFinite(usage.cached_write) ? usage.cached_write : null;
        item.cacheReason = typeof usage.cache_reason === 'string' ? usage.cache_reason : null;
        item.cacheHitRate = cachedRead !== null && usage.input > 0
            ? ((cachedRead / usage.input) * 100).toFixed(0)
            : null;
        item.turnUsage = usage.turn_usage || null;
        const turnCachedRead = item.turnUsage && Number.isFinite(item.turnUsage.cached_read)
            ? item.turnUsage.cached_read
            : null;
        item.turnCacheHitRate = turnCachedRead !== null && item.turnUsage.input > 0
            ? ((turnCachedRead / item.turnUsage.input) * 100).toFixed(0)
            : null;
    }

    if (startTime) {
        item.durationMs = Date.now() - startTime;
        const elapsedSec = item.durationMs / 1000;
        if (usage && usage.output > 0 && elapsedSec > 0) {
            item.tps = (usage.output / elapsedSec).toFixed(1);
        }
    }

    const display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
    if (!display.trim() && !(item.thinking || '').trim()) {
        OSA.tmodelRemove(item.key);
    }
    OSA.tmodelClearAllStreamingFlags();
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

// Pre-compaction history fetched from the backend, kept per session so any
// rebuild (session switch, snapshot adopt, truncate) can splice it back in.
OSA.sessionArchives = OSA.sessionArchives || new Map();

OSA.getSessionArchive = function(sessionId) {
    if (!sessionId) return [];
    return OSA.sessionArchives.get(sessionId) || [];
};

OSA.setSessionArchive = function(sessionId, messages) {
    if (!sessionId) return;
    if (Array.isArray(messages) && messages.length) {
        OSA.sessionArchives.set(sessionId, messages);
    } else {
        OSA.sessionArchives.delete(sessionId);
    }
};

// A compaction just archived more history, so the cached prefix is stale.
// Refetch it before the next rebuild or the compacted messages would vanish.
OSA.refreshSessionArchive = async function(sessionId) {
    if (!sessionId) return;
    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(sessionId)}/archive`);
        if (!res.ok) return;
        const data = await res.json().catch(() => null);
        if (data && Array.isArray(data.messages)) {
            OSA.setSessionArchive(sessionId, data.messages);
        }
    } catch (_) {
        // Keep whatever was cached; the transcript still renders.
    }
};

// Insert anchor-ordered entries (tool cards) into the message/compaction list
// exactly the way the old scan-and-splice did: each entry lands immediately
// after the last item whose integer anchor is <= the entry's anchor. Chat
// messages are the only boundaries; a compaction card carries a messageIndex
// but no anchor, so entries can land before it.
//
// The naive implementation rescans backwards and splices per entry, which is
// O(entries x base) and dominates load time on sessions with thousands of
// blocks. Because entries arrive with non-decreasing anchors, the boundary
// only ever moves forward, so one pass reproduces the same order. One quirk of
// the original survives: if an entry has no preceding message boundary it is
// appended past everything, and every later entry chains after it.
//
// When entries are not anchor-ordered (rare compaction/archive splices), it
// falls back to the incremental insertion so placement is unchanged.
OSA.mergeAnchorOrderedItems = function(base, entries) {
    if (!entries.length) return base;
    for (let i = 1; i < entries.length; i++) {
        if (!(entries[i - 1].anchorIndex <= entries[i].anchorIndex)) {
            return OSA.insertItemsByAnchorFallback(base, entries);
        }
    }

    const messagePositions = [];
    const messageAnchors = [];
    for (let i = 0; i < base.length; i++) {
        if (base[i].kind === 'message' && Number.isInteger(base[i].messageIndex)) {
            messagePositions.push(i);
            messageAnchors.push(base[i].messageIndex);
        }
    }

    const groups = new Map();
    let mi = 0;
    for (let e = 0; e < entries.length; e++) {
        const anchor = entries[e].anchorIndex;
        while (mi < messageAnchors.length && messageAnchors[mi] <= anchor) mi++;
        const boundary = mi - 1;
        if (boundary < 0) {
            // No preceding message: the original appended this entry past
            // everything, and each later entry chained after it.
            return base.concat(entries);
        }
        const position = messagePositions[boundary];
        let list = groups.get(position);
        if (!list) {
            list = [];
            groups.set(position, list);
        }
        list.push(entries[e]);
    }

    const merged = [];
    for (let i = 0; i < base.length; i++) {
        merged.push(base[i]);
        const list = groups.get(i);
        if (list) {
            for (let k = 0; k < list.length; k++) merged.push(list[k]);
        }
    }
    return merged;
};

// Exact-but-quadratic placement used only when entries are not anchor-ordered.
OSA.insertItemsByAnchorFallback = function(base, entries) {
    const result = base.slice();
    entries.forEach(function(entry) {
        const target = Number.isInteger(entry.anchorIndex) ? entry.anchorIndex : -1;
        let pos = result.length;
        for (let i = result.length - 1; i >= 0; i--) {
            const item = result[i];
            const anchor = item.kind === 'message' ? item.messageIndex : item.anchorIndex;
            if (anchor !== null && anchor !== undefined && anchor <= target) {
                pos = i + 1;
                break;
            }
        }
        result.splice(pos, 0, entry);
    });
    return result;
};

OSA.rebuildTranscriptFromSession = function(session, toolEvents = [], subagentTasks = [], options = {}) {
    let messages = (session && Array.isArray(session.messages)) ? session.messages : [];
    // Pre-compaction history the backend archived. Splice it back in just
    // before the compaction summary that replaced it, so the chat keeps its
    // earlier messages and tool groups, and the summary card lands at the
    // compaction boundary rather than at the top.
    const archived = Array.isArray(options.archivedMessages)
        ? options.archivedMessages
        : (session ? OSA.getSessionArchive(session.id) : []);
    if (archived.length) {
        let insertAt = messages.findIndex(function(message) {
            return OSA.isCompactionSummaryMessage && OSA.isCompactionSummaryMessage(message);
        });
        if (insertAt < 0) insertAt = 0;
        messages = messages.slice(0, insertAt).concat(archived, messages.slice(insertAt));
    }
    const priorItems = OSA.TModel.items.slice();
    let items = [];

    messages.forEach(function(message, idx) {
        if (!message || message.role === 'tool') return;
        // Compaction summaries are synthetic but render as a collapsed
        // system card (not a chat bubble); every other synthetic control
        // message stays hidden from the transcript.
        if (OSA.isCompactionSummaryMessage && OSA.isCompactionSummaryMessage(message)) {
            const text = OSA.stripCompactedSummary
                ? OSA.stripCompactedSummary(message.content || '')
                : String(message.content || '').trim();
            if (!text) return;
            items.push(OSA.tmodelCompactionItem(OSA.getMessageRenderKey(message, idx), message, idx));
            return;
        }
        if (OSA.isHiddenSyntheticMessage(message)) return;
        if (message.role === 'assistant') {
            const kind = message.metadata && message.metadata.kind;
            // Tool-prelude narration renders inside its tool card; skip the
            // response text from the separate bubble. Keep any reasoning as a
            // thinking-only transcript item so reload matches the live view.
            if (kind === 'tool_prelude'
                && Array.isArray(message.tool_calls)
                && message.tool_calls.length > 0) {
                const hasVisibleThinking = OSA.getShowThinkingBlocks() && !!(message.thinking || '').trim();
                if (!hasVisibleThinking) return;
                const thinkingOnly = Object.assign({}, message, { content: '' });
                items.push(OSA.tmodelMessageItem(OSA.getMessageRenderKey(message, idx), thinkingOnly, idx));
                return;
            }
            const hasContent = !!(message.content || '').trim();
            const hasVisibleThinking = OSA.getShowThinkingBlocks() && !!(message.thinking || '').trim();
            const hasToolCalls = Array.isArray(message.tool_calls) && message.tool_calls.length > 0;
            if (!hasContent && !hasVisibleThinking && !hasToolCalls) return;
        }
        items.push(OSA.tmodelMessageItem(OSA.getMessageRenderKey(message, idx), message, idx));
    });

    // Tool events created before message_index was persisted (and events
    // written by older clients with the default value 0) can still be placed
    // exactly by matching their call id to the assistant tool-call message.
    const toolCallAnchors = new Map();
    messages.forEach(function(message, index) {
        if (!message || !Array.isArray(message.tool_calls)) return;
        message.tool_calls.forEach(function(call) {
            if (call && call.id) toolCallAnchors.set(call.id, index);
        });
    });

    const toolSource = (Array.isArray(toolEvents) ? toolEvents : []).slice();
    if (options.keepCurrentArtifacts) {
        const seenToolIds = new Set(toolSource.map(function(tool) { return tool && tool.tool_call_id; }).filter(Boolean));
        priorItems.forEach(function(item) {
            if (!item || item.kind !== 'tool' || seenToolIds.has(item.callId)) return;
            seenToolIds.add(item.callId);
            toolSource.push({
                tool_call_id: item.callId,
                tool_name: item.toolName,
                arguments: item.args,
                output: item.output,
                title: item.title,
                metadata: item.metadata,
                message_index: item.anchorIndex,
                timestamp: item.ts,
                completed: item.completed,
                success: item.success,
            });
        });
    }
    const filteredToolSource = toolSource
        .filter(function(t) { return t && t.tool_call_id && t.tool_name !== 'subagent'; });
    // Parse each timestamp once instead of inside the comparator, which runs
    // O(tools log tools) times and otherwise rebuilds a Date per comparison.
    const toolSortMs = new Map();
    filteredToolSource.forEach(function(t) {
        toolSortMs.set(t, OSA.eventTimestampMs(t.timestamp) || 0);
    });
    const tools = filteredToolSource.sort(function(a, b) {
        const delta = (a.message_index || 0) - (b.message_index || 0);
        if (delta !== 0) return delta;
        return toolSortMs.get(a) - toolSortMs.get(b);
    });

    // Fold persisted tool-prelude narration into the card it introduced so a
    // history reload renders the same single card as the live stream did.
    // The message loop above skips these bubbles from `items` but the raw
    // `messages` array still carries them for this fold.
    const preludeByAnchor = new Map();
    messages.forEach(function(message, index) {
        const kind = message && message.metadata && message.metadata.kind;
        if (!message || message.role !== 'assistant' || kind !== 'tool_prelude') return;
        if (!Array.isArray(message.tool_calls) || message.tool_calls.length === 0) return;
        const text = OSA.stripSpeakBlock
            ? OSA.stripSpeakBlock(message.content || '')
            : (message.content || '');
        if (!text.trim()) return;
        if (!preludeByAnchor.has(index)) preludeByAnchor.set(index, []);
        preludeByAnchor.get(index).push(text.trim());
    });

    const toolItems = [];
    tools.forEach(function(t) {
        const inferredAnchor = toolCallAnchors.has(t.tool_call_id)
            ? toolCallAnchors.get(t.tool_call_id)
            : null;
        // No anchor means the owning assistant message is no longer in the
        // transcript (e.g. compacted into the summary and archived). The
        // recorded message_index is pre-compaction and would splice the card
        // at a wrong position, so drop it instead of misplacing it.
        if (inferredAnchor === null) return;
        const anchorPrelude = preludeByAnchor.has(inferredAnchor)
            ? preludeByAnchor.get(inferredAnchor).join('\n\n')
            : '';
        toolItems.push(OSA.tmodelToolItem({
            tool_call_id: t.tool_call_id,
            tool_name: t.tool_name,
            arguments: t.arguments || {},
            output: typeof t.output === 'string' ? t.output : '',
            title: typeof t.title === 'string' ? t.title : '',
            prelude: anchorPrelude,
            metadata: t.metadata,
            message_index: inferredAnchor,
            timestamp: t.timestamp,
        }, { completed: t.completed === true, success: t.success === true, live: false }));
    });

    // Place every tool card in one pass. The previous approach scanned back
    // through the growing items array and spliced for each tool, which is
    // O(tools x items) and dominates load time once a session has thousands
    // of blocks.
    items = OSA.mergeAnchorOrderedItems(items, toolItems);

    const subagentSource = (Array.isArray(subagentTasks) ? subagentTasks : []).slice();
    if (options.keepCurrentArtifacts) {
        const seenSubagentIds = new Set(subagentSource.map(function(task) { return task && task.session_id; }).filter(Boolean));
        priorItems.forEach(function(item) {
            if (!item || item.kind !== 'subagent' || seenSubagentIds.has(item.subagentId)) return;
            seenSubagentIds.add(item.subagentId);
            subagentSource.push({
                session_id: item.subagentId,
                description: item.description,
                agent_type: item.agentType,
                prompt: item.prompt,
                status: item.status,
                is_running: item.isRunning,
                tool_count: item.toolCount,
                result: item.result,
                duration_ms: item.durationMs,
                context_state: item.contextState,
                created_at: item.createdAt,
            });
        });
    }
    const subagents = subagentSource
        .slice()
        .sort(function(a, b) {
            return (OSA.eventTimestampMs(a.created_at) || 0) - (OSA.eventTimestampMs(b.created_at) || 0);
        });

    const oldestVisibleMs = (function() {
        let oldest = null;
        for (const entry of items) {
            if (entry.kind !== 'message') continue;
            const ms = OSA.eventTimestampMs(entry.timestamp);
            if (ms === null) continue;
            if (oldest === null || ms < oldest) oldest = ms;
        }
        return oldest;
    })();

    subagents.forEach(function(task) {
        if (!task || !task.session_id) return;
        const createdMs = OSA.eventTimestampMs(task.created_at);
        // Tasks from compacted-away history have no in-transcript anchor
        // (they are timestamp-ordered). Anything older than the oldest
        // visible message belongs to the summarized prefix, so drop it
        // rather than appending it at the end out of order.
        if (createdMs !== null && oldestVisibleMs !== null && createdMs < oldestVisibleMs) return;
        const item = OSA.tmodelSubagentItem(task, false);
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

    if (options.preserveKeys) {
        // Index the previous model once. Matching each item against every
        // prior item was O(items x priorItems), which stalled the periodic
        // running-snapshot rebuild on long sessions.
        const priorMessageByClientId = new Map();
        const priorMessageByIndex = new Map();
        const priorMessageByIndexNoClientId = new Map();
        const priorSubagentById = new Map();
        priorItems.forEach(function(candidate) {
            if (!candidate) return;
            if (candidate.kind === 'message') {
                if (candidate.clientMessageId) {
                    const clientKey = candidate.role + '\u0000' + candidate.clientMessageId;
                    if (!priorMessageByClientId.has(clientKey)) {
                        priorMessageByClientId.set(clientKey, candidate);
                    }
                } else if (candidate.messageIndex !== null) {
                    const indexKey = candidate.role + '\u0000' + candidate.messageIndex;
                    if (!priorMessageByIndexNoClientId.has(indexKey)) {
                        priorMessageByIndexNoClientId.set(indexKey, candidate);
                    }
                }
                if (candidate.messageIndex !== null) {
                    const indexKey = candidate.role + '\u0000' + candidate.messageIndex;
                    if (!priorMessageByIndex.has(indexKey)) {
                        priorMessageByIndex.set(indexKey, candidate);
                    }
                }
            } else if (candidate.kind === 'subagent' && !priorSubagentById.has(candidate.subagentId)) {
                priorSubagentById.set(candidate.subagentId, candidate);
            }
        });
        items.forEach(function(item) {
            if (item.kind === 'message') {
                let prior = null;
                if (item.clientMessageId) {
                    prior = priorMessageByClientId.get(item.role + '\u0000' + item.clientMessageId) || null;
                }
                if (!prior && item.messageIndex !== null) {
                    // The old predicate only matched by index when the item
                    // carried no client id or the candidate lacked one.
                    const indexKey = item.role + '\u0000' + item.messageIndex;
                    prior = item.clientMessageId
                        ? (priorMessageByIndexNoClientId.get(indexKey) || null)
                        : (priorMessageByIndex.get(indexKey) || null);
                }
                if (prior) item.key = prior.key;
            } else if (item.kind === 'subagent') {
                const prior = priorSubagentById.get(item.subagentId);
                if (prior) {
                    item.tools = prior.tools;
                    item.currentTool = prior.currentTool;
                    item.retryText = prior.retryText;
                    item.key = prior.key;
                }
            }
        });
    }

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

// A reasoning-only assistant segment: it carries thinking but no visible
// text/images/attachments. These are the segments the stream finalizes before
// a tool call, so they normally sit right next to a run of tools.
OSA.isThinkingOnlyMessage = function(item) {
    if (!item || item.kind !== 'message' || item.role !== 'assistant') return false;
    if (!OSA.getShowThinkingBlocks() || !(item.thinking || '').trim()) return false;
    if (item.images && item.images.length) return false;
    if (item.attachments && item.attachments.length) return false;
    let display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
    display = OSA.stripToolCallMarkup ? OSA.stripToolCallMarkup(display) : display;
    return !display.trim();
};

OSA.buildTranscriptUnits = function() {
    // Keep hidden reasoning in the model, but do not render its empty role
    // label or let it break an otherwise continuous run of tool calls.
    const items = OSA.TModel.items.filter(function(item) {
        if (item.kind !== 'message' || item.role !== 'assistant') return true;
        let display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
        display = OSA.stripToolCallMarkup ? OSA.stripToolCallMarkup(display) : display;
        return !!display.trim()
            || (OSA.getShowThinkingBlocks() && !!(item.thinking || '').trim())
            || !!(item.images && item.images.length)
            || !!(item.attachments && item.attachments.length);
    });
    const units = [];
    // Reasoning immediately followed by a run of tool calls folds into that
    // group instead of standing as its own "Thinking" card.
    const pendingReasoning = [];
    let i = 0;

    while (i < items.length) {
        const item = items[i];

        if (OSA.isThinkingOnlyMessage(item)) {
            let k = i;
            while (k < items.length && OSA.isThinkingOnlyMessage(items[k])) k += 1;
            if (k < items.length && items[k].kind === 'tool' && items[k].toolName !== 'draw_diagram') {
                for (let m = i; m < k; m++) {
                    pendingReasoning.push({ key: items[m].key, text: items[m].thinking || '', item: items[m] });
                }
                i = k;
                continue;
            }
        }

        if (item.kind === 'tool' && item.toolName !== 'draw_diagram') {
            // Visible transcript entries delimit runs. Provider message indices
            // and tool categories can change during uninterrupted tool work.
            const run = [item];
            const reasoning = pendingReasoning.splice(0, pendingReasoning.length);
            // Ordered mix of tools and reasoning so thinking can render between
            // the tool cards it actually preceded, not lumped at the top.
            const entries = reasoning.map(function(r) {
                return { kind: 'reasoning', key: r.key, text: r.text, item: r.item };
            });
            entries.push({ kind: 'tool', item: item });
            let j = i + 1;
            while (j < items.length) {
                const next = items[j];
                if (next.kind === 'tool' && next.toolName !== 'draw_diagram') {
                    run.push(next);
                    entries.push({ kind: 'tool', item: next });
                    j += 1;
                    continue;
                }
                // Reasoning between two tool calls belongs to the same group
                // rather than starting a new one, so a turn with several
                // think/tool cycles collapses into a single tool group with
                // every reasoning block inside it.
                if (OSA.isThinkingOnlyMessage(next)) {
                    let k = j;
                    const folded = [];
                    while (k < items.length && OSA.isThinkingOnlyMessage(items[k])) {
                        folded.push({ key: items[k].key, text: items[k].thinking || '', item: items[k] });
                        k += 1;
                    }
                    if (k < items.length && items[k].kind === 'tool' && items[k].toolName !== 'draw_diagram') {
                        folded.forEach(function(r) {
                            reasoning.push(r);
                            entries.push({ kind: 'reasoning', key: r.key, text: r.text, item: r.item });
                        });
                        j = k;
                        continue;
                    }
                }
                break;
            }
            // A single tool with no reasoning stays a standalone card; once
            // reasoning is attached it needs a group to collapse into.
            if (run.length >= 2 || item.context || reasoning.length) {
                units.push({
                    type: 'parallel-group',
                    key: 'par:' + run[0].key,
                    items: run,
                    reasoning: reasoning,
                    entries: entries,
                });
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

    // Safety net: reasoning is only stashed when a tool run follows, but if
    // that ever stops being true, render it rather than dropping it.
    pendingReasoning.forEach(function(r) {
        units.push({ type: 'message', key: r.item.key, item: r.item, items: [r.item] });
    });

    return units;
};

OSA.unitSignature = function(unit) {
    const base = unit.items.map(function(item) {
        if (item.kind === 'message') {
            let sigContent = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
            sigContent = OSA.stripToolCallMarkup ? OSA.stripToolCallMarkup(sigContent) : sigContent;
            return [
                item.role,
                sigContent,
                item.thinking || '',
                item.timestamp || '',
                item.images.length + '/' + item.attachments.length,
                item.streaming ? 'S' : '',
                item.thinkingStreaming ? 'T' : '',
                item.durationMs || '',
                item.tps || '',
                item.cacheReported ? '1' : '',
                item.cacheHitRate || '',
                item.turnCacheHitRate || '',
                OSA.getShowThinkingBlocks() ? '1' : '0',
            ].join('\u0002');
        }
        if (item.kind === 'tool') {
            // Tool metadata is load-bearing: the diagram viewer, the diff
            // renderer, and the read_file preview payload all read from it,
            // and it only arrives with the completion event. Folding it into
            // the signature is what makes the card re-patch when a completion
            // lands after the start event.
            return [
                item.toolName,
                item.status,
                item.completed ? '1' : '0',
                item.output || '',
                item.title || '',
                item.prelude || '',
                item.metadata ? JSON.stringify(item.metadata) : '',
            ].join('\u0002');
        }
        if (item.kind === 'subagent') {
            return [
                item.status, item.isRunning ? 'R' : '', item.toolCount,
                item.result, item.currentTool, item.retryText,
                item.tools.map(function(t) { return t.name + ':' + t.status + 'x' + (t.count || 1); }).join(','),
                item.durationMs || '', item.contextState ? JSON.stringify(item.contextState) : '',
            ].join('\u0002');
        }
        if (item.kind === 'compaction') {
            return ['compaction', item.content || '', item.timestamp || ''].join('\u0002');
        }
        return JSON.stringify(item);
    }).join('\u0001');

    if (unit.entries && unit.entries.length) {
        return base + '\u0001E:' + unit.entries.map(function(entry) {
            return entry.kind === 'reasoning'
                ? 'R:' + (entry.text || '')
                : 'T:' + ((entry.item && (entry.item.callId || entry.item.key)) || '');
        }).join('\u0002');
    }
    if (unit.reasoning && unit.reasoning.length) {
        return base + '\u0001R:' + unit.reasoning.map(function(r) { return r.text || ''; }).join('\u0002');
    }
    return base;
};

OSA.unitHasLiveStream = function(unit) {
    return unit.items.some(function(item) {
        return item.kind === 'message' && (item.streaming || item.thinkingStreaming);
    });
};

OSA.pauseTranscriptAutoScroll = function(view) {
    if (!view) return;
    view.autoScrollPaused = true;
    view.userPinnedToBottom = false;
    view.forceStickBottom = false;
};

OSA.updateTranscriptScrollState = function(view, messagesDiv) {
    if (!view || !messagesDiv) return;
    const scrollTop = messagesDiv.scrollTop;
    const previousScrollTop = Number.isFinite(view.lastScrollTop) ? view.lastScrollTop : scrollTop;
    const distance = Math.max(0, messagesDiv.scrollHeight - scrollTop - messagesDiv.clientHeight);
    const scrolledUp = scrollTop < previousScrollTop - 1;
    view.lastScrollTop = scrollTop;

    if (scrolledUp) OSA.pauseTranscriptAutoScroll(view);

    // Once the user scrolls upward, keep streaming renders detached from the
    // tail until they explicitly return all the way to the bottom.
    if (view.autoScrollPaused) {
        if (!scrolledUp && distance <= 2) {
            view.autoScrollPaused = false;
            view.userPinnedToBottom = true;
        } else {
            view.userPinnedToBottom = false;
        }
        OSA.updateScrollToBottomButton(messagesDiv);
        return;
    }

    view.userPinnedToBottom = distance < 120;
    if (distance >= 120) view.forceStickBottom = false;
    OSA.updateScrollToBottomButton(messagesDiv);
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
    view.lastScrollTop = messagesDiv.scrollTop;

    if (!view.scrollHandlerAttached) {
        messagesDiv.addEventListener('scroll', function() {
            OSA.updateTranscriptScrollState(view, messagesDiv);
        }, { passive: true });
        messagesDiv.addEventListener('wheel', function(event) {
            if (event.deltaY < 0) OSA.pauseTranscriptAutoScroll(view);
        }, { passive: true });
        view.scrollHandlerAttached = true;
    }

    if (!view.resizeHandlerAttached) {
        window.addEventListener('resize', function() {
            OSA.positionScrollToBottomButton();
        });
        view.resizeHandlerAttached = true;
    }

    if (!view.inputResizeObserver && typeof ResizeObserver !== 'undefined') {
        const inputArea = document.querySelector('.chat-area > .input-area');
        if (inputArea) {
            view.inputResizeObserver = new ResizeObserver(function() {
                OSA.positionScrollToBottomButton();
            });
            view.inputResizeObserver.observe(inputArea);
        }
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
            // Window shifting uses the expanded observer margin. Pinning is
            // governed by actual scroll position and user intent above.
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

OSA.scrollMessagesToBottom = function() {
    // Programmatic sticks must bypass the container's smooth scrolling:
    // retargeting an in-flight smooth animation mid-load is what left the
    // viewport stranded halfway. Instant jump, then restore.
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;
    const prev = messagesDiv.style.scrollBehavior;
    messagesDiv.style.scrollBehavior = 'auto';
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    const view = OSA.getTranscriptView && OSA.getTranscriptView();
    if (view) view.lastScrollTop = messagesDiv.scrollTop;
    void messagesDiv.offsetHeight;
    messagesDiv.style.scrollBehavior = prev;
};

// Show the jump-to-latest affordance only when the viewport is detached from
// the tail. Being at (or near) the bottom hides it again.
OSA.updateScrollToBottomButton = function(messagesDiv) {
    const button = document.getElementById('scroll-to-bottom');
    if (!button) return;
    const div = messagesDiv || document.getElementById('messages');
    if (!div) return;
    const distance = Math.max(0, div.scrollHeight - div.scrollTop - div.clientHeight);
    const hasContent = !!(OSA.TModel && OSA.TModel.items && OSA.TModel.items.length);
    const shouldShow = hasContent && distance > 80;
    if (button.classList.contains('hidden') === shouldShow) {
        button.classList.toggle('hidden', !shouldShow);
        if (shouldShow) OSA.positionScrollToBottomButton();
    }
};

// Keep the button just above the composer, whose height changes with the
// number of lines and with the todo/permission docks above it.
OSA.positionScrollToBottomButton = function() {
    const button = document.getElementById('scroll-to-bottom');
    if (!button) return;
    const inputArea = document.querySelector('.chat-area > .input-area');
    if (!inputArea) return;
    const height = inputArea.getBoundingClientRect().height;
    if (height > 0) button.style.bottom = Math.round(height + 12) + 'px';
};

// Explicit user intent: re-attach to the tail and resume auto-scroll.
OSA.jumpToLatest = function() {
    const view = OSA.getTranscriptView && OSA.getTranscriptView();
    if (view) {
        view.autoScrollPaused = false;
        view.userPinnedToBottom = true;
        view.forceStickBottom = true;
    }
    OSA.scrollMessagesToBottom();
    OSA.updateScrollToBottomButton();
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

    // Entry animations are only for nodes that appear mid-turn. Bulk renders
    // (session switch, rebuild, window shift) mount a whole screen at once and
    // must not slide every card in.
    view.animateEnter = !(
        options.reason === 'session-switch'
        || options.reason === 'rebuild'
        || options.reason === 'window-shift'
        || options.keepWindow
    );

    try {
        const units = OSA.buildTranscriptUnits();
        view.units = units;
        view.descriptors = units;
        const total = units.length;
        // Opening a chat and sending a message always land at the bottom,
        // even if the user had scrolled up before: the scroll listener
        // re-arms pinning afterwards, so this never fights manual scrolling.
        const shouldStickBottom = !!(
            options.stickToBottom
            || options.reason === 'session-switch'
            || options.reason === 'user-message'
            || view.forceStickBottom
            || view.userPinnedToBottom
        );

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

        // Calculate virtual spacers after measuring the newly mounted window.
        // Doing this before measurement leaves the first session render using
        // the fallback average for every off-screen unit, which shows up as
        // blank gaps between otherwise correctly ordered messages.
        const heightBefore = OSA.estimateUnitRangeHeight(view, units, 0, view.windowStart);
        const heightAfter = OSA.estimateUnitRangeHeight(view, units, view.windowEnd, total);
        view.topSpacer.style.height = Math.max(0, Math.round(heightBefore)) + 'px';
        view.bottomSpacer.style.height = Math.max(0, Math.round(heightAfter)) + 'px';

        if (shouldStickBottom) {
            OSA.scrollMessagesToBottom();
            view.userPinnedToBottom = true;
            if (options.reason === 'session-switch' || options.reason === 'user-message') {
                // Content below the fold (images, late markdown layout) can
                // expand after this frame and push the tail out of view. One
                // deferred instant re-pin covers it without fighting later
                // scrolling.
                requestAnimationFrame(function() {
                    OSA.scrollMessagesToBottom();
                });
            }
        } else if (options.preserveScroll !== false && anchorKey) {
            const nextAnchor = desired.find(function(w) { return w.dataset.unitKey === anchorKey; }) || null;
            if (nextAnchor) {
                const nextTop = nextAnchor.getBoundingClientRect().top;
                messagesDiv.scrollTop += (nextTop - anchorTop);
                view.lastScrollTop = messagesDiv.scrollTop;
            }
        }

        OSA.updateScrollToBottomButton(messagesDiv);

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
        if (view.animateEnter) {
            wrapper.classList.add('unit-enter');
            wrapper.addEventListener('animationend', function(event) {
                if (event.target !== wrapper) return;
                wrapper.classList.remove('unit-enter');
            });
        }
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
    } else if (wrapper.dataset.sig !== 'live') {
        wrapper.dataset.sig = 'live';
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
                    return `<a class="subagent-link" href="#session=${OSA.escapeAttr(uuid)}" onclick="event.preventDefault(); event.stopPropagation(); OSA.openSubagentSession(${OSA.jsArg(uuid)})">${OSA.escapeHtml(uuid)}</a>`;
                });
                return OSA.formatMessage(content);
            });
            break;
        case 'compaction':
            OSA.patchSimpleMessageUnit(wrapper, unit, 'compaction', 'Context compacted', function(item) {
                const body = OSA.formatMessage(String(item.content || ''));
                return '<details><summary>Earlier history summarized — expand to review</summary>'
                    + '<div class="compaction-body">' + body + '</div></details>';
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
            if (item.thinkingStreaming) {
                OSA.renderStreamingText(body, thinkingText);
            } else {
                OSA.setStaticMessageHtml(body, thinkingText);
            }
        } else if (!item.thinkingStreaming && body._md) {
            // A thinking phase can end without a trailing newline. Flush its
            // intentionally buffered final line at the phase boundary instead
            // of leaving it invisible until the whole response completes.
            OSA.flushIncrementalMarkdown(body, thinkingText);
        }
        thinkingWrap.classList.toggle('streaming', !!item.thinkingStreaming);
        OSA.setThinkingPreview(thinkingWrap, thinkingText);
    } else if (thinkingWrap) {
        thinkingWrap.remove();
    }

    if (item.role === 'assistant') {
        let display = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(item.content || '') : (item.content || '');
        display = OSA.stripToolCallMarkup ? OSA.stripToolCallMarkup(display) : display;
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
    const sig = JSON.stringify([
        item.images.map(function(image) {
            return [image.filename || '', image.mime || '', image.preview_url || image.previewUrl || ''];
        }),
        item.attachments.map(function(attachment) {
            return [
                attachment.filename || '',
                attachment.mime || '',
                attachment.kind || '',
                attachment.previewUrl || attachment.preview_url || '',
                attachment.size_bytes || attachment.sizeBytes || 0,
                attachment.truncated ? 1 : 0,
            ];
        }),
    ]);
    if (msgEl.dataset.attachmentsSig === sig) return;
    msgEl.dataset.attachmentsSig = sig;

    let wrap = msgEl.querySelector(':scope > .message-attachments');
    if (item.images.length === 0 && item.attachments.length === 0) {
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

    if (item.tps || item.cacheReported) {
        let tpsEl = actionsEl.querySelector('.turn-tokens');
        if (!tpsEl) {
            tpsEl = document.createElement('span');
            tpsEl.className = 'turn-tokens';
            actionsEl.appendChild(tpsEl);
        }
        const speed = item.tps ? item.tps + ' tok/s' : '';
        const cache = item.cacheHitRate !== null && item.cacheHitRate !== undefined
            ? 'request cache ' + item.cacheHitRate + '%'
            : 'request cache n/a';
        const turnCache = item.turnCacheHitRate !== null && item.turnCacheHitRate !== undefined
            ? 'turn cache ' + item.turnCacheHitRate + '%'
            : '';
        const cacheReason = item.cacheReason ? ' (' + item.cacheReason + ')' : '';
        tpsEl.textContent = [speed, cache + cacheReason, turnCache].filter(Boolean).join(' · ');
        const details = [];
        if (item.totalTokens) details.push(item.totalTokens + ' total tokens');
        if (item.cachedRead !== null && item.cachedRead !== undefined) {
            details.push(item.cachedRead + ' cached input tokens');
        }
        if (item.cachedWrite !== null && item.cachedWrite !== undefined) {
            details.push(item.cachedWrite + ' cache-write tokens');
        }
        if (item.cacheReason) details.push('cache reason: ' + item.cacheReason);
        if (item.turnUsage) {
            details.push(
                'full-turn cache: ' + (item.turnCacheHitRate !== null ? item.turnCacheHitRate + '%' : 'n/a')
                + ' across ' + item.turnUsage.input + ' input tokens'
            );
        }
        if (item.cacheReported && item.cacheHitRate === null) {
            details.push('provider did not report cache reads');
        }
        if (details.length) tpsEl.title = details.join(' · ');
    }
};

OSA.buildToolCardElement = function(item) {
    const domId = 'tool-' + item.callId;
    const label = OSA.toolLabel(item.toolName);
    const icon = OSA.toolIcon(item.toolName);
    const subtitle = OSA.summarizeToolArgs(item.toolName, item.args);
    const isDiagram = item.toolName === 'draw_diagram';
    const isCompleted = item.completed === true;
    const isSuccess = item.success === true;
    const statusText = isCompleted
        ? (item.status === 'cancelled' ? 'cancelled' : (isSuccess ? 'done' : 'failed'))
        : 'running';
    const statusClass = isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending';
    const titleClass = isCompleted ? '' : 'tool-title-pending';
    const chevronOpacity = isCompleted ? '' : 'opacity:0.35';

    const container = document.createElement('div');
    container.id = domId;
    container.className = 'tool-container';
    container.dataset.callId = item.callId;
    container._toolArgs = item.args;
    const view = OSA.getTranscriptView();
    if (view && view.animateEnter) {
        container.classList.add('tool-enter');
        container.addEventListener('animationend', function(event) {
            if (event.target !== container) return;
            container.classList.remove('tool-enter');
        });
    }

    // A diagram is the answer, not a footnote: it renders as its own card in
    // the message column instead of behind a tool row that has to be opened.
    if (isDiagram) {
        container.classList.add('diagram-container');
        container.innerHTML = `
            <div class="tool-card tool-diagram" id="card-${domId}" data-tool="${OSA.escapeHtml(item.toolName)}">
                <div class="diagram-host" id="diagram-${domId}"></div>
            </div>`;
        return container;
    }

    container.innerHTML = `
        <div class="tool-card tool-inline${isDiagram ? ' tool-diagram' : ''}" id="card-${domId}" data-tool="${OSA.escapeHtml(item.toolName)}">
            <div class="tool-trigger tool-trigger-inline" onclick="OSA.handleToolCardClick(${OSA.jsArg(domId)})">
                <span class="tool-icon">${icon}</span>
                <span class="tool-title ${titleClass}" id="title-${domId}">${OSA.escapeHtml(label)}</span>
                ${subtitle ? `<span class="tool-subtitle" id="subtitle-${domId}">${OSA.escapeHtml(subtitle)}</span>` : ''}
                <button type="button" class="tool-preview-btn hidden" onclick="OSA.openPreviewFromButton(${OSA.jsArg(domId)}, event)" title="Open in preview" aria-label="Open in preview">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                        <path d="M3 5h18"></path>
                        <path d="M3 12h7"></path>
                        <path d="M3 19h7"></path>
                        <rect x="12" y="8" width="9" height="11" rx="1"></rect>
                    </svg>
                </button>
                <button type="button" class="tool-copy-btn" onclick="OSA.copyToolCard(${OSA.jsArg(domId)}, event)" title="Copy tool call" aria-label="Copy tool call">
                    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2"></rect><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path></svg>
                </button>
                <span class="tool-status-badge ${statusClass}" id="status-${domId}">${statusText}</span>
                <span class="tool-chevron" id="chevron-${domId}" style="${chevronOpacity}">&#x25B6;</span>
            </div>
            <div class="tool-body" id="body-${domId}">
                <div class="tool-body-inner">
                    ${item.prelude ? `<div class="tool-prelude" id="prelude-${domId}">${OSA.escapeHtml(item.prelude)}</div>` : ''}
                    <div class="tool-args" id="args-${domId}"${isDiagram ? ' style="display:none"' : ''}>${OSA.escapeHtml(JSON.stringify(item.args, null, 2))}</div>
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
    container._toolArgs = item.args;
    container.dataset.toolName = item.toolName;
    OSA.applyToolDisclosure?.(container, item.toolName);

    const statusEl = container.querySelector('#status-' + OSA.cssEscape(domId));
    if (statusEl) {
        const statusText = isCompleted
            ? (item.status === 'cancelled' ? 'cancelled' : (isSuccess ? 'done' : 'failed'))
            : (item.status || 'running').toLowerCase();
        const statusClass = isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending';
        if (statusEl.textContent !== statusText) statusEl.textContent = statusText;
        statusEl.className = 'tool-status-badge ' + statusClass;
    }

    const titleEl = container.querySelector('#title-' + OSA.cssEscape(domId));
    if (titleEl) titleEl.classList.toggle('tool-title-pending', !isCompleted);

    let preludeEl = container.querySelector('#prelude-' + OSA.cssEscape(domId));
    if (item.prelude && !preludeEl) {
        const bodyInner = container.querySelector('.tool-body-inner');
        if (bodyInner) {
            preludeEl = document.createElement('div');
            preludeEl.className = 'tool-prelude';
            preludeEl.id = 'prelude-' + domId;
            bodyInner.insertBefore(preludeEl, bodyInner.firstChild);
        }
    }
    if (preludeEl && (preludeEl.textContent || '') !== (item.prelude || '')) {
        preludeEl.textContent = item.prelude || '';
    }

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

    if (item.toolName === 'draw_diagram') {
        OSA.patchDiagramCardElement(container, domId, item);
        return;
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

// `draw_diagram` owns its whole card: the transcript renders the diagram as a
// first-class element in the message column, with no tool row, no collapse
// chevron, and no args/output pane. The diagram metadata rides along with the
// tool event, so a reloaded session rebuilds the same card through this path.
OSA.patchDiagramCardElement = function(container, domId, item) {
    const card = container.querySelector(':scope > .tool-card');
    if (!card) return;
    card.classList.add('tool-diagram');
    container.classList.add('diagram-container');

    const host = card.querySelector('.diagram-host');
    if (!host) return;

    const showPending = function(message) {
        // Never on top of a mounted diagram: a stale running event must not
        // resurrect a placeholder under/over the real card.
        if (host.querySelector('.diagram-card') || host.querySelector('.diagram-pending')) return;
        const pending = document.createElement('div');
        pending.className = 'diagram-pending';
        const spinner = document.createElement('span');
        spinner.className = 'diagram-pending-spinner';
        const label = document.createElement('span');
        label.className = 'diagram-pending-label';
        label.textContent = message;
        pending.appendChild(spinner);
        pending.appendChild(label);
        host.appendChild(pending);
    };

    if (!item.completed) {
        showPending('Drawing diagram\u2026');
        return;
    }

    const metadata = item.metadata && item.metadata.kind === 'diagram' ? item.metadata : null;
    if (!metadata) {
        showPending('Diagram unavailable');
        return;
    }
    const sig = JSON.stringify([
        metadata.title || '',
        metadata.source || '',
        metadata.theme || '',
        metadata.spec || null,
        metadata.svg || '',
    ]);
    if (host.dataset.diagramSig === sig) return;
    if (typeof OSA.Diagram === 'undefined' || typeof OSA.Diagram.mount !== 'function') return;
    OSA.Diagram.mount(host, metadata);
    host.dataset.diagramSig = sig;
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
    if (!row.dataset.initialized) {
        row.dataset.initialized = '1';
        row.innerHTML = `
            <span class="context-inline-action"></span>
            <span class="context-inline-detail"></span>
            <button type="button" class="context-inline-preview-btn hidden" onclick="OSA.openPreviewFromContextButton(${OSA.jsArg(row.id)}, event)" title="Open in preview" aria-label="Open in preview">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M3 5h18"></path>
                    <path d="M3 12h7"></path>
                    <path d="M3 19h7"></path>
                    <rect x="12" y="8" width="9" height="11" rx="1"></rect>
                </svg>
            </button>
            <span class="context-inline-status"></span>
        `;
    }
    const actionEl = row.querySelector('.context-inline-action');
    const detailEl = row.querySelector('.context-inline-detail');
    const statusEl = row.querySelector('.context-inline-status');
    const action = OSA.toolLabel(item.toolName);
    const detail = OSA.summarizeToolArgs(item.toolName, item.args);
    if (actionEl && actionEl.textContent !== action) actionEl.textContent = action;
    if (detailEl && detailEl.textContent !== detail) detailEl.textContent = detail;
    if (statusEl) {
        if (statusEl.textContent !== statusText) statusEl.textContent = statusText;
        statusEl.className = 'context-inline-status ' + (isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending');
    }
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

    OSA.renderToolGroupEntries(group, unit, view, true);
    OSA.patchToolGroupDisclosure(group, unit.items, true);
};

// Tool groups render an ordered mix of tool cards and thinking cards, so
// reasoning sits between the calls it introduced. The header stays first;
// everything after it is reconciled by key to preserve node identity.
OSA.ensureToolGroupHeader = function(group) {
    let header = group.querySelector(':scope > .tool-group-toggle');
    if (header) return header;
    header = document.createElement('button');
    header.type = 'button';
    header.className = 'parallel-group-header tool-group-toggle';
    header.innerHTML = '<span class="tool-group-title"></span>'
        + '<span class="parallel-count"></span>'
        + '<span class="tool-group-chevron" aria-hidden="true"></span>';
    header.addEventListener('click', function() {
        group._groupExpanded = header.getAttribute('aria-expanded') !== 'true';
        OSA.patchToolGroupDisclosure(group, group._groupItems, group._contextGroup);
    });
    group.prepend(header);
    return header;
};

// A collapsible thinking card using the same markup/style as message
// reasoning, mounted inline in the tool group.
OSA.ensureGroupThinkingNode = function(group, entry) {
    if (!group._reasoningNodes) group._reasoningNodes = new Map();
    const key = entry.key || ('reasoning:' + group._reasoningNodes.size);
    let node = group._reasoningNodes.get(key);
    if (!node || !node.isConnected) {
        node = document.createElement('div');
        node.className = 'message-thinking tool-group-thinking-card';
        node.dataset.groupRole = 'reasoning';
        node.innerHTML = '<button type="button" class="thinking-toggle" onclick="OSA.toggleThinkingBlock(this)">'
            + '<span class="thinking-toggle-label">Thinking</span>'
            + '<span class="thinking-preview"></span>'
            + '</button>'
            + '<div class="thinking-body"></div>';
        group._reasoningNodes.set(key, node);
    }
    const text = entry.text || '';
    const body = node.querySelector('.thinking-body');
    if (body && body.dataset.rawText !== text) {
        body.innerHTML = OSA.formatMessage(text);
        body.dataset.rawText = text;
    }
    OSA.setThinkingPreview(node, text);
    return node;
};

OSA.renderToolGroupEntries = function(group, unit, view, forceContext) {
    const header = OSA.ensureToolGroupHeader(group);
    const entries = unit.entries && unit.entries.length
        ? unit.entries
        : (unit.reasoning || []).map(function(r) {
            return { kind: 'reasoning', key: r.key, text: r.text, item: r.item };
        }).concat(unit.items.map(function(item) {
            return { kind: 'tool', item: item };
        }));

    const desired = entries.map(function(entry) {
        if (entry.kind === 'reasoning') {
            return OSA.ensureGroupThinkingNode(group, entry);
        }
        const item = entry.item;
        let card;
        if (forceContext || item.context) {
            card = view.ctxNodesByCallId.get(item.callId);
            if (!card) {
                card = OSA.buildContextToolRow(item);
                view.ctxNodesByCallId.set(item.callId, card);
            }
            OSA.patchContextToolRow(card, item);
        } else {
            card = OSA.ensureToolContainerNode(item);
        }
        card.dataset.groupRole = 'tool';
        return card;
    });

    const desiredSet = new Set(desired);
    let cursor = header.nextSibling;
    desired.forEach(function(node) {
        if (node === cursor) {
            cursor = cursor.nextSibling;
            return;
        }
        group.insertBefore(node, cursor);
    });
    Array.from(group.children).forEach(function(child) {
        if (child !== header && !desiredSet.has(child)) child.remove();
    });
};

OSA.patchToolGroupDisclosure = function(group, items, context) {
    group._groupItems = items;
    group._contextGroup = context;
    const header = OSA.ensureToolGroupHeader(group);
    const limit = typeof OSA.getToolGroupPreview === 'function' ? OSA.getToolGroupPreview() : 0;
    const expanded = group._groupExpanded === undefined ? limit === 'all' : group._groupExpanded;
    const visibleCount = expanded ? items.length : (limit === 'all' ? 0 : limit);
    const running = items.filter(function(item) { return !item.completed; }).length;
    const failed = items.filter(function(item) { return item.completed && !item.success && item.status !== 'cancelled'; }).length;
    const cancelled = items.filter(function(item) { return item.status === 'cancelled'; }).length;
    const counts = [];
    if (context) {
        const reads = items.filter(function(item) { return item.toolName === 'read_file'; }).length;
        const lists = items.filter(function(item) { return item.toolName === 'list_files'; }).length;
        const searches = items.length - reads - lists;
        if (reads) counts.push(reads + ' read' + (reads === 1 ? '' : 's'));
        if (searches) counts.push(searches + ' search' + (searches === 1 ? '' : 'es'));
        if (lists) counts.push(lists + ' listing' + (lists === 1 ? '' : 's'));
    }
    const title = context
        ? (running ? 'Gathering context' : 'Gathered context')
        : items.length + ' tool' + (items.length === 1 ? '' : 's');
    const statuses = [];
    if (counts.length) statuses.push(counts.join(', '));
    if (running) statuses.push(running + ' running');
    if (failed) statuses.push(failed + ' failed');
    if (cancelled) statuses.push(cancelled + ' cancelled');
    const hiddenCount = Math.max(0, items.length - visibleCount);
    if (hiddenCount && visibleCount) statuses.push(hiddenCount + ' more');
    const titleEl = header.querySelector('.tool-group-title');
    if (titleEl && titleEl.textContent !== title) titleEl.textContent = title;
    const countEl = header.querySelector('.parallel-count');
    const countText = statuses.join(' · ');
    if (countEl && countEl.textContent !== countText) countEl.textContent = countText;
    const chevronEl = header.querySelector('.tool-group-chevron');
    const chevronText = expanded ? '▾' : '▸';
    if (chevronEl && chevronEl.textContent !== chevronText) chevronEl.textContent = chevronText;
    const aria = String(expanded);
    if (header.getAttribute('aria-expanded') !== aria) header.setAttribute('aria-expanded', aria);
    if (header.hidden) header.hidden = false;
    const runningAttr = String(running > 0);
    if (header.dataset.running !== runningAttr) header.dataset.running = runningAttr;

    // Thinking cards collapse with the tools; the tool preview limit applies
    // to tool cards only, in order.
    let toolIndex = 0;
    Array.from(group.children).forEach(function(child) {
        if (child === header) return;
        const role = child.dataset.groupRole
            || (child.classList.contains('tool-group-thinking-card') ? 'reasoning' : 'tool');
        let shouldHide;
        if (role === 'reasoning') {
            shouldHide = !expanded;
        } else {
            shouldHide = toolIndex >= visibleCount;
            toolIndex += 1;
        }
        if (child.hidden !== shouldHide) child.hidden = shouldHide;
    });
};

OSA.patchParallelGroupUnit = function(wrapper, unit) {
    const view = OSA.getTranscriptView();
    let group = wrapper.firstElementChild;
    if (!group || !group.classList.contains('parallel-group')) {
        group = document.createElement('div');
        group.className = 'parallel-group';
        wrapper.replaceChildren(group);
    }

    OSA.renderToolGroupEntries(group, unit, view);
    OSA.patchToolGroupDisclosure(
        group,
        unit.items,
        unit.items.every(function(item) { return item.context; })
    );
};

OSA.subagentStatusLabel = function(status, isRunning) {
    if (isRunning) return 'Running';
    switch (status) {
        case 'completed': return 'Done';
        case 'partial': return 'Partial';
        case 'failed': return 'Failed';
        case 'cancelled': return 'Cancelled';
        case 'timeout': return 'Timed out';
        case 'retrying': return 'Retrying';
        default:
            return status ? String(status).charAt(0).toUpperCase() + String(status).slice(1) : '';
    }
};

OSA.buildSubagentCardElement = function(item) {
    const subagentId = item.subagentId;
    const sid = OSA.escapeAttr(subagentId);
    const card = document.createElement('div');
    card.id = 'subagent-' + subagentId;
    card.className = 'subagent-card';
    card.dataset.status = item.isRunning ? 'running' : (item.status || 'completed');
    card.dataset.expanded = 'false';
    card.innerHTML = `
        <div class="subagent-header" role="button" tabindex="0" aria-expanded="false"
             onclick="OSA.toggleSubagentCard(${OSA.jsArg(subagentId)})"
             onkeydown="if(event.key==='Enter'||event.key===' '){event.preventDefault();OSA.toggleSubagentCard(${OSA.jsArg(subagentId)});}">
            <span class="subagent-dot" aria-hidden="true"></span>
            <span class="subagent-title">${OSA.escapeHtml(item.description)}</span>
            <span class="subagent-type">${OSA.escapeHtml(item.agentType)}</span>
            <span class="subagent-spacer"></span>
            <span class="subagent-tool-count" id="subagent-count-${sid}"></span>
            <span class="subagent-status">
                <span class="subagent-status-badge" id="subagent-status-${sid}"></span>
            </span>
            <span class="subagent-chevron" id="subagent-chevron-${sid}" aria-hidden="true">&#x25B6;</span>
        </div>
        <div class="subagent-live" id="subagent-live-${sid}" style="display:none">
            <span class="subagent-current-tool" id="subagent-current-${sid}"></span>
        </div>
        <div class="subagent-body" id="subagent-body-${sid}">
            <div class="subagent-body-inner">
                <section class="subagent-section">
                    <div class="subagent-section-label">Task</div>
                    <div class="subagent-prompt" id="subagent-prompt-${sid}"></div>
                </section>
                <section class="subagent-section">
                    <div class="subagent-section-label">Activity</div>
                    <div class="subagent-tools" id="subagent-tools-${sid}"></div>
                </section>
                <section class="subagent-section subagent-result" id="subagent-result-${sid}" style="display:none"></section>
                <div class="subagent-actions">
                    <button type="button" class="subagent-btn subagent-btn-primary" onclick="OSA.openSubagentSession(${OSA.jsArg(subagentId)})">Open session</button>
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
    const badgeStatus = item.retryText ? 'retrying' : (item.isRunning ? 'running' : (item.status || 'running'));
    card.dataset.status = badgeStatus;
    const statusBadge = card.querySelector('#subagent-status-' + OSA.cssEscape(subagentId));
    if (statusBadge) {
        const label = OSA.subagentStatusLabel(badgeStatus, item.isRunning);
        if (statusBadge.textContent !== label) statusBadge.textContent = label;
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
        const toolsSig = item.tools.map(function(t) { return t.name + ':' + t.status + 'x' + (t.count || 1); }).join(',');
        if (toolsEl.dataset.sig !== toolsSig) {
            toolsEl.dataset.sig = toolsSig;
            toolsEl.innerHTML = item.tools.map(function(t) {
                const repeat = (t.count || 1) > 1 ? ' <span class="subagent-tool-repeat">×' + t.count + '</span>' : '';
                return '<div class="subagent-tool-item ' + OSA.escapeHtml(t.status) + '">' + OSA.escapeHtml(t.name) + repeat + '</div>';
            }).join('');
        }
    }

    const resultEl = card.querySelector('#subagent-result-' + OSA.cssEscape(subagentId));
    if (resultEl) {
        if (item.result) {
            const resultSig = String(item.result.length) + ':' + String(item.result.slice(0, 64));
            if (resultEl.dataset.sig !== resultSig) {
                resultEl.dataset.sig = resultSig;
                resultEl.style.display = 'block';
                resultEl.innerHTML = '<div class="subagent-result-head"><span class="subagent-result-label">Result</span>'
                    + '<button type="button" class="subagent-result-copy" aria-label="Copy result">Copy</button></div>'
                    + '<div class="subagent-result-text"></div>';
                const textEl = resultEl.querySelector('.subagent-result-text');
                if (textEl) {
                    const escaped = OSA.escapeHtml(item.result);
                    textEl.innerHTML = typeof OSA.linkifySessionIds === 'function'
                        ? OSA.linkifySessionIds(escaped)
                        : escaped;
                }
                const copyBtn = resultEl.querySelector('.subagent-result-copy');
                if (copyBtn) {
                    copyBtn.addEventListener('click', function() {
                        OSA.copyTextWithFeedback(item.result, copyBtn);
                    });
                }
            } else {
                resultEl.style.display = 'block';
            }
        } else if (!item.isRunning) {
            if (resultEl.dataset.sig !== '') {
                resultEl.dataset.sig = '';
                resultEl.style.display = 'none';
                resultEl.innerHTML = '';
            }
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

    const resumableStatuses = ['timeout', 'partial', 'failed', 'cancelled'];
    const resumeBtnId = 'subagent-resume-' + subagentId;
    let resumeBtn = card.querySelector('#' + OSA.cssEscape(resumeBtnId));
    const canResume = !item.isRunning && resumableStatuses.includes(item.status);
    if (canResume && !resumeBtn) {
        const actions = card.querySelector('.subagent-actions');
        if (actions) {
            resumeBtn = document.createElement('button');
            resumeBtn.id = resumeBtnId;
            resumeBtn.className = 'subagent-btn subagent-btn-resume';
            resumeBtn.textContent = 'Resume';
            resumeBtn.onclick = function(event) {
                event.stopPropagation();
                OSA.resumeSubagent(subagentId);
            };
            actions.insertBefore(resumeBtn, actions.firstChild);
        }
    } else if (!canResume && resumeBtn) {
        resumeBtn.remove();
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

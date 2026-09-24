window.OSA = window.OSA || {};

OSA.isCompactionSummaryMessage = function(message) {
    return !!(message && message.metadata
        && message.metadata.synthetic
        && (message.metadata.kind || '') === 'compaction_summary');
};

// Display text for a compaction summary: unwrap the
// `<compacted-summary>...</compacted-summary>` frame the backend stores and
// drop trailing model instructions ("Do not acknowledge...", "Keep the
// working notes...") so the card shows only the human-readable handoff.
// Unframed legacy bodies ("Compaction summary:\n...") pass through as-is.
OSA.stripCompactedSummary = function(text) {
    let out = String(text || '');
    const framed = out.match(/<compacted-summary>([\s\S]*?)<\/compacted-summary>/i);
    if (framed) out = framed[1];
    out = out.replace(/\n*Do not acknowledge the compacted summary explicitly in your reply\.[^\n]*/i, '');
    out = out.replace(/\n*Keep the working notes current with `update_notes`\.[^\n]*/i, '');
    return out.trim();
};

// Models sometimes emit Qwen/GLM-style `<tool_call>...</tool_call>` text
// blocks that this client never parses into structured calls (when the call
// succeeds the real tool card renders separately). Strip them from displayed
// assistant text so raw call markup never leaks into the chat; surrounding
// narration is preserved. A dangling unclosed block (partial stream) is cut
// too so markup never flashes mid-turn.
OSA.stripToolCallMarkup = function(text) {
    if (!text) return text;
    let out = String(text).replace(/<tool_call>[\s\S]*?<\/tool_call>/gi, '');
    const open = out.match(/<tool_call>/i);
    if (open) out = out.slice(0, open.index);
    return out.replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n');
};

OSA.isHiddenSyntheticMessage = function(message) {
    if (!message || !message.metadata) return false;
    // Tool preludes are real narration folded into their tool card: never
    // hidden from the transcript, and always kept model-visible in history.
    if (message.role === 'assistant' && (message.metadata.kind || '') === 'tool_prelude') {
        return false;
    }
    if (!message.metadata.synthetic) return false;

    return true;
};

OSA.showThinkingIndicator = function() {
    const existing = document.getElementById('thinking-indicator');
    if (existing) return existing;

    OSA.setTurnStartTime(Date.now());

    const indicator = document.createElement('div');
    indicator.id = 'thinking-indicator';
    indicator.className = 'thinking-indicator';
    indicator.innerHTML = `
        <canvas class="thinking-canvas" id="thinking-canvas"></canvas>
        <div class="thinking-info">
            <span class="thinking-label">Thinking</span>
            <span class="thinking-sublabel" id="thinking-sublabel">Sending request</span>
        </div>
    `;

    OSA.mountFloatingNode(indicator);
    OSA.tmodelMarkDirty('thinking-indicator');

    const canvas = document.getElementById('thinking-canvas');
    if (canvas) {
        OSA._thinkingCanvasAnim = OSA._initThinkingCanvas(canvas);
    }

    const sublabels = [
        'Sending request',
        'Waiting for response',
        'Processing response',
    ];
    let labelIdx = 0;
    OSA._thinkingSublabelTimer = setInterval(() => {
        const el = document.getElementById('thinking-sublabel');
        if (!el) { clearInterval(OSA._thinkingSublabelTimer); return; }
        labelIdx = (labelIdx + 1) % sublabels.length;
        el.textContent = sublabels[labelIdx];
    }, 3000);
    return indicator;
};

OSA._initThinkingCanvas = function(canvas) {
    const dpr = window.devicePixelRatio || 1;
    const size = 28;
    canvas.width = size * dpr;
    canvas.height = size * dpr;
    canvas.style.width = size + 'px';
    canvas.style.height = size + 'px';

    const ctx = canvas.getContext('2d');
    ctx.scale(dpr, dpr);

    let frame;
    const center = size / 2;

    const orbits = [
        { rx: 10, ry: 4.5, tilt: -0.4, speed: 2.2, phase: 0, dotSize: 1.4, trailLen: 6 },
        { rx: 10, ry: 4.5, tilt: 0.9, speed: 1.6, phase: 2.1, dotSize: 1.2, trailLen: 5 },
        { rx: 10, ry: 4.5, tilt: -1.7, speed: 2.8, phase: 4.2, dotSize: 1.0, trailLen: 7 },
    ];

    const trailBuf = orbits.map(o => []);

    function draw(t) {
        ctx.clearRect(0, 0, size, size);
        const time = t * 0.001;

        const grad = ctx.createRadialGradient(center, center, 0, center, center, 5);
        grad.addColorStop(0, 'rgba(255,255,255,0.35)');
        grad.addColorStop(1, 'rgba(255,255,255,0)');
        ctx.beginPath();
        ctx.arc(center, center, 5, 0, Math.PI * 2);
        ctx.fillStyle = grad;
        ctx.fill();

        ctx.beginPath();
        ctx.arc(center, center, 1.5, 0, Math.PI * 2);
        ctx.fillStyle = 'rgba(255,255,255,0.7)';
        ctx.fill();

        orbits.forEach((orbit, idx) => {
            const cosT = Math.cos(orbit.tilt);
            const sinT = Math.sin(orbit.tilt);
            const angle = time * orbit.speed + orbit.phase;

            ctx.beginPath();
            ctx.strokeStyle = 'rgba(255,255,255,0.06)';
            ctx.lineWidth = 0.5;
            for (let a = 0; a <= Math.PI * 2; a += 0.08) {
                const ex = center + Math.cos(a) * orbit.rx;
                const ey = center + Math.sin(a) * orbit.ry;
                const px = center + (ex - center) * cosT - (ey - center) * sinT;
                const py = center + (ex - center) * sinT + (ey - center) * cosT;
                if (a === 0) ctx.moveTo(px, py);
                else ctx.lineTo(px, py);
            }
            ctx.closePath();
            ctx.stroke();

            const ex = center + Math.cos(angle) * orbit.rx;
            const ey = center + Math.sin(angle) * orbit.ry;
            const px = center + (ex - center) * cosT - (ey - center) * sinT;
            const py = center + (ex - center) * sinT + (ey - center) * cosT;

            trailBuf[idx].push({ x: px, y: py });
            if (trailBuf[idx].length > orbit.trailLen) trailBuf[idx].shift();

            for (let i = 0; i < trailBuf[idx].length; i++) {
                const tp = trailBuf[idx][i];
                const a = ((i + 1) / trailBuf[idx].length) * 0.25;
                const s = orbit.dotSize * (0.3 + 0.7 * (i / trailBuf[idx].length));
                ctx.beginPath();
                ctx.arc(tp.x, tp.y, s, 0, Math.PI * 2);
                ctx.fillStyle = `rgba(255,255,255,${a})`;
                ctx.fill();
            }

            ctx.beginPath();
            ctx.arc(px, py, orbit.dotSize, 0, Math.PI * 2);
            ctx.fillStyle = 'rgba(255,255,255,0.8)';
            ctx.fill();
        });

        frame = requestAnimationFrame(draw);
    }

    frame = requestAnimationFrame(draw);
    return function cancel() {
        cancelAnimationFrame(frame);
    };
};

OSA.hideThinkingIndicator = function() {
    const indicator = document.getElementById('thinking-indicator');
    if (indicator) indicator.remove();
    if (OSA._thinkingSublabelTimer) {
        clearInterval(OSA._thinkingSublabelTimer);
        OSA._thinkingSublabelTimer = null;
    }
    if (OSA._thinkingCanvasAnim) {
        OSA._thinkingCanvasAnim();
        OSA._thinkingCanvasAnim = null;
    }
};

OSA.clearPendingFormattedRenders = function() {
    const frame = OSA.getPendingFormattedFrame();
    if (frame) {
        cancelAnimationFrame(frame);
        OSA.setPendingFormattedFrame(null);
    }
    OSA.getPendingFormattedElements().forEach(el => { if (el) delete el._onRendered; });
    OSA.getPendingFormattedElements().clear();
};

OSA.scheduleFormattedRender = function(element, rawText, onRendered) {
    if (!element) return;
    rawText = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(rawText) : rawText;
    element.dataset.rawText = rawText;
    if (onRendered) element._onRendered = onRendered;
    OSA.getPendingFormattedElements().add(element);

    if (OSA.getPendingFormattedFrame()) {
        return;
    }

    OSA.setPendingFormattedFrame(requestAnimationFrame(() => {
        OSA.setPendingFormattedFrame(null);
        const pending = Array.from(OSA.getPendingFormattedElements());
        OSA.getPendingFormattedElements().clear();
        pending.forEach(el => {
            if (!el || !el.isConnected) return;
            const rawText = el.dataset.rawText || '';
            if (el.dataset.renderedText === rawText) return;
            if (el.dataset.renderedText === undefined || el.dataset.renderedText === '') {
                el.innerHTML = '';
                el._md = OSA.createIncrementalMd();
            }
            if (el._md) {
                OSA.renderIncrementalMarkdown(el, rawText);
            } else {
                el.innerHTML = OSA.formatMessage(rawText);
                el.dataset.renderedText = rawText;
            }
            if (el._onRendered) {
                el._onRendered();
                delete el._onRendered;
            }
        });
    }));
};

OSA.getThinkingPreview = function(text) {
    if (!text) return '';
    const line = text.split('\n').map(part => part.trim()).find(Boolean) || '';
    if (line.length <= 88) return line;
    return `${line.slice(0, 85)}...`;
};

OSA.toggleThinkingBlock = function(toggle) {
    const container = toggle && toggle.closest ? toggle.closest('.message-thinking') : null;
    if (!container) return;
    container.classList.toggle('expanded');
    container.dataset.userToggled = 'true';
};

OSA.renderThinkingSection = function(thinking, expanded = false) {
    if (!OSA.getShowThinkingBlocks()) return '';
    if (!thinking || !thinking.trim()) return '';
    const preview = OSA.getThinkingPreview(thinking);
    return `
        <div class="message-thinking${expanded ? ' expanded' : ''}">
            <button type="button" class="thinking-toggle" onclick="OSA.toggleThinkingBlock(this)">
                <span class="thinking-toggle-label">Thinking</span>
                <span class="thinking-preview">${OSA.escapeHtml(preview)}</span>
            </button>
            <div class="thinking-body">${OSA.formatMessage(thinking)}</div>
        </div>
    `;
};

OSA.setThinkingPreview = function(container, text) {
    if (!container) return;
    const previewEl = container.querySelector('.thinking-preview');
    if (!previewEl) return;
    const preview = OSA.getThinkingPreview(text);
    previewEl.textContent = preview;
    previewEl.style.display = preview ? '' : 'none';
};

OSA.resetStreamingMessage = function() {
    OSA.clearPendingFormattedRenders();
    OSA.setStreamingAssistantDomId(null);
};

OSA.resetMessageChain = function(sessionId) {
    const targetId = sessionId || OSA.getCurrentSessionId();
    const chain = targetId ? OSA.getMessageChainFor(targetId) : OSA.messageChain;
    chain.lastEventType = null;
    chain.lastAssistantDomId = null;
    chain.pendingToolCallIds = [];
    chain.eventSessionId = targetId || chain.eventSessionId || null;
    chain.eventSeqNumber = 0;
    chain.lastThinkingEndSeq = 0;
    chain.lastToolStartSeq = 0;
};

// Session-scoped mirror writes. Streaming events for background sessions must
// update that session's stored messages, never the currently viewed session.
OSA.getSessionObjectFor = function(sessionId) {
    if (!sessionId) return OSA.getCurrentSession();
    const entry = OSA.getSessionEntry(sessionId);
    if (entry.session) return entry.session;
    const current = OSA.getCurrentSession();
    if (current && current.id === sessionId) return current;
    return null;
};

// Appends a user message to a specific entry's stored session without touching
// the transcript DOM or TModel (used for background queue dispatches).
OSA.appendUserMessageToEntry = function(sessionId, content, opts = {}) {
    const session = OSA.getSessionObjectFor(sessionId);
    if (!session) return null;
    if (!Array.isArray(session.messages)) session.messages = [];
    const clientMessageId = opts.clientMessageId || '';
    const exists = session.messages.some(function(message) {
        if (message.role !== 'user') return false;
        const existingClientId = message.metadata && message.metadata.client_message_id;
        if (clientMessageId) return existingClientId === clientMessageId;
        return message.content === content;
    });
    if (exists) return null;
    const entry = {
        role: 'user',
        content,
        thinking: null,
        timestamp: opts.timestamp || new Date().toISOString(),
        tool_calls: null,
        tool_call_id: null,
        metadata: clientMessageId ? { client_message_id: clientMessageId } : {},
        tokens: null,
        images: [],
    };
    session.messages.push(entry);
    return entry;
};

// Merges a live tool event into a background entry's tool list so returning to
// the session shows the work even before the server snapshot is refetched.
OSA.upsertEntryToolEvent = function(entry, event, completed) {
    if (!entry || !event || !event.tool_call_id || event.tool_name === 'subagent') return;
    const tools = entry.tools;
    const existing = tools.find(function(t) { return t && t.tool_call_id === event.tool_call_id; });
    if (existing) {
        if (completed) {
            existing.completed = true;
            existing.success = event.success === true;
            if (typeof event.output === 'string') existing.output = event.output;
            if (typeof event.title === 'string') existing.title = event.title;
        }
        return;
    }
    tools.push({
        tool_call_id: event.tool_call_id,
        tool_name: event.tool_name,
        arguments: event.arguments || {},
        output: (completed && typeof event.output === 'string') ? event.output : '',
        title: typeof event.title === 'string' ? event.title : '',
        metadata: event.metadata,
        message_index: event.message_index,
        timestamp: event.timestamp,
        completed: !!completed,
        success: event.success === true,
    });
};

OSA.ensureCurrentSessionAssistantMessage = function(forceNew = false, sessionId) {
    const session = sessionId ? OSA.getSessionObjectFor(sessionId) : OSA.getCurrentSession();
    if (!session) return null;
    if (!Array.isArray(session.messages)) session.messages = [];
    const last = session.messages[session.messages.length - 1];
    if (!forceNew && last && last.role === 'assistant' && !OSA.isHiddenSyntheticMessage(last)) return last;

    const next = {
        role: 'assistant',
        content: '',
        thinking: null,
        timestamp: new Date().toISOString(),
        tool_calls: null,
        tool_call_id: null,
        metadata: {},
        tokens: null,
    };
    session.messages.push(next);
    const targetId = sessionId || session.id;
    const entry = targetId && OSA.getSessionEntry ? OSA.getSessionEntry(targetId) : null;
    if (entry) entry.messagesDirty = true;
    return next;
};

OSA.appendCurrentSessionAssistantThinking = function(content, sessionId) {
    if (!content) return;
    const message = OSA.ensureCurrentSessionAssistantMessage(false, sessionId);
    if (!message) return;
    const current = message.thinking || '';
    if (content.length >= 4 && current.endsWith(content)) return;
    message.thinking = current + content;
    const targetId = sessionId || OSA.getCurrentSessionId?.();
    const entry = targetId && OSA.getSessionEntry ? OSA.getSessionEntry(targetId) : null;
    if (entry) entry.messagesDirty = true;
};

OSA.appendCurrentSessionAssistantContent = function(content, sessionId) {
    if (!content) return;
    const message = OSA.ensureCurrentSessionAssistantMessage(false, sessionId);
    if (!message) return;
    const current = message.content || '';
    if (content.length >= 4 && current.endsWith(content)) return;
    message.content = current + content;
    const targetId = sessionId || OSA.getCurrentSessionId?.();
    const entry = targetId && OSA.getSessionEntry ? OSA.getSessionEntry(targetId) : null;
    if (entry) entry.messagesDirty = true;
};

OSA.resetCurrentSessionAssistantContent = function() {
    const session = OSA.getCurrentSession();
    if (!session || !Array.isArray(session.messages) || session.messages.length === 0) return;
    const last = session.messages[session.messages.length - 1];
    if (last && last.role === 'assistant') {
        last.content = '';
        last.thinking = null;
    }
};

OSA.insertCurrentSessionToolBoundary = function(event) {
    const sessionId = event && event.session_id ? event.session_id : null;
    const session = sessionId ? OSA.getSessionObjectFor(sessionId) : OSA.getCurrentSession();
    if (!session) return null;
    if (!Array.isArray(session.messages)) session.messages = [];

    const callId = event && event.tool_call_id ? event.tool_call_id : null;
    if (callId) {
        const existing = session.messages.find(message => message.role === 'tool' && message.tool_call_id === callId);
        if (existing) return existing;
    }

    const parsedTimestamp = event && event.timestamp ? new Date(event.timestamp) : new Date();
    const timestamp = Number.isNaN(parsedTimestamp.getTime())
        ? new Date().toISOString()
        : parsedTimestamp.toISOString();

    const toolMessage = {
        role: 'tool',
        content: '',
        thinking: null,
        timestamp,
        tool_calls: null,
        tool_call_id: callId,
        metadata: {},
        tokens: null,
    };

    session.messages.push(toolMessage);
    const targetId = sessionId || session.id;
    const entry = targetId && OSA.getSessionEntry ? OSA.getSessionEntry(targetId) : null;
    if (entry) entry.messagesDirty = true;
    return toolMessage;
};

OSA.getActiveTurnAssistantIndex = function(session) {
    const list = session && Array.isArray(session.messages) ? session.messages : [];
    for (let i = list.length - 1; i >= 0; i--) {
        const message = list[i];
        if (!message || message.role === 'tool') continue;
        return message.role === 'assistant' ? i : -1;
    }
    return -1;
};

OSA.getActiveTurnAssistantMessage = function(session) {
    if (!session || !Array.isArray(session.messages) || session.messages.length === 0) {
        return null;
    }

    const visible = session.messages.filter(message => {
        if (message.role === 'tool') return false;
        if (OSA.isHiddenSyntheticMessage(message)) return false;
        // Mid-turn tool preludes are UI-hidden checkpoints, never the turn's
        // spoken or final reply — even the closing summary lives in a later
        // non-synthetic assistant segment.
        if (message.role === 'assistant'
            && Array.isArray(message.tool_calls)
            && message.tool_calls.length > 0) return false;
        return true;
    });
    if (!visible.length) {
        return null;
    }

    const last = visible[visible.length - 1];
    if (!last || last.role !== 'assistant') {
        return null;
    }

    const hasContent = !!(last.content || '').trim();
    const hasVisibleThinking = OSA.getShowThinkingBlocks() && !!(last.thinking || '').trim();
    if (!hasContent && !hasVisibleThinking) {
        return null;
    }

    return last;
};

OSA.releaseStreamingAssistantMessage = function() {
    OSA.tmodelReleaseStreamingSegment();
};

OSA.beginThinkingDisplay = function() {
    if (!OSA.getShowThinkingBlocks()) return null;
    OSA.hideThinkingIndicator();

    let item = OSA.tmodelStreamingItem();
    if (item && (item.content || '').trim()) {
        item.streaming = false;
        item.thinkingStreaming = false;
        item = null;
    }
    if (!item) {
        item = OSA.tmodelEnsureAssistantSegment();
    }
    if (!item) return null;
    item.thinkingStreaming = true;
    OSA.tmodelMarkDirty('thinking-start');
    return item;
};

OSA.appendThinkingChunk = function(content) {
    if (!content) return;
    OSA.appendCurrentSessionAssistantThinking(content);
    if (!OSA.getShowThinkingBlocks()) return;

    const item = OSA.tmodelEnsureAssistantSegment();
    if (!item) return;
    const session = OSA.getCurrentSession();
    const msgs = session && Array.isArray(session.messages) ? session.messages : [];
    const mirror = msgs[msgs.length - 1];
    if (mirror && mirror.role === 'assistant' && (mirror.thinking || '')) {
        item.thinking = mirror.thinking;
    } else {
        item.thinking = (item.thinking || '') + content;
    }
    item.thinkingStreaming = true;
    OSA.tmodelMarkDirty('thinking-delta');
};

OSA.completeThinkingDisplay = function() {
    const item = OSA.tmodelStreamingItem();
    if (item && item.thinkingStreaming) {
        item.thinkingStreaming = false;
        OSA.tmodelMarkDirty('thinking-end');
    }
};

OSA.beginAssistantResponse = function() {
    OSA.hideThinkingIndicator();

    let item = OSA.tmodelStreamingItem();
    if (item && (item.content || '').trim()) {
        item.streaming = false;
        item.thinkingStreaming = false;
        item = null;
    }
    if (item) {
        item.content = '';
        const session = OSA.getCurrentSession();
        const msgs = session && Array.isArray(session.messages) ? session.messages : [];
        const mirror = msgs[msgs.length - 1];
        if (mirror && mirror.role === 'assistant') mirror.content = '';
    } else {
        item = OSA.tmodelEnsureAssistantSegment();
    }
    OSA.tmodelMarkDirty('response-start');
    return item;
};

OSA.appendAssistantChunk = function(content) {
    if (!content) return;
    OSA.feedSpeechStream?.(content);
    OSA.appendCurrentSessionAssistantContent(content);

    const item = OSA.tmodelEnsureAssistantSegment();
    if (!item) return;
    const session = OSA.getCurrentSession();
    const msgs = session && Array.isArray(session.messages) ? session.messages : [];
    const mirror = msgs[msgs.length - 1];
    if (mirror && mirror.role === 'assistant' && (mirror.content || '')) {
        item.content = mirror.content;
    } else {
        item.content = (item.content || '') + content;
    }
    OSA.tmodelMarkDirty('response-chunk');
};

OSA.pruneEmptyStreamingMessage = function() {
    OSA.tmodelPruneEmptyStreamingSegment();
};

OSA.completeAssistantResponse = function(usage) {
    const session = OSA.getCurrentSession();
    const sourceMessage = OSA.getActiveTurnAssistantMessage(session);
    const item = OSA.tmodelFinalizeStreamingSegment(usage);

    const rawText = item ? (item.content || '') : (sourceMessage?.content || '');
    const thinkingText = item ? (item.thinking || '') : (sourceMessage?.thinking || '');

    if (rawText && OSA.getTurnStartTime() && OSA.getTtsEnabled() && OSA.getVoiceConfig()?.enabled) {
        const activePersona = OSA.getActivePersona();
        const isRoleplay = activePersona?.id === 'custom';

        const rawFull = OSA._speechStreamBuffer || sourceMessage?.content || rawText;
        const speakBlock = OSA.extractSpeakBlock?.(rawFull);

        if (speakBlock && !isRoleplay) {
            if (!OSA.speechStreamHandledTurn?.()) {
                OSA.speakText(speakBlock, { interrupt: false });
            } else {
                const tail = OSA.sanitizeSpeechText(OSA.unspokenSpeechTail?.() || '');
                if (tail) {
                    OSA.speakText(tail, { interrupt: false });
                }
            }
        } else if (OSA.speechStreamHandledTurn?.() && !isRoleplay) {
            const tail = OSA.sanitizeSpeechText(OSA.unspokenSpeechTail?.() || '');
            if (tail) {
                OSA.speakText(tail, { interrupt: false });
            }
        } else {
            const speechText = OSA.prepareSpeechText(OSA.stripSpeakBlock(rawFull), isRoleplay);
            if (speechText) {
                OSA.speakText(speechText);
            }
        }
    }
    OSA.resetSpeechStream?.();

    OSA.setTurnStartTime(null);
    OSA.resetStreamingMessage();
    OSA.tmodelSettleLiveItems?.();
    OSA.updateTodoDock();
    const currentSession = OSA.getCurrentSession();
    if (currentSession && currentSession.id && typeof OSA.loadSessionCheckpoints === 'function') {
        OSA.loadSessionCheckpoints(currentSession.id, { silent: true });
    }
    if (!rawText && !thinkingText) {
        OSA.tmodelMarkDirty('turn-empty');
    }
};

OSA.describeCheckpointForUi = function(checkpoint) {
    const timeLabel = checkpoint?.created_at
        ? OSA.formatRelativeDateTime(checkpoint.created_at)
        : 'unknown time';
    const toolLabel = checkpoint?.tool_name ? ` via ${checkpoint.tool_name}` : '';
    return `${timeLabel}${toolLabel}`;
};

OSA.findNearestCheckpointForMessage = function(messageTimestamp, messageIndex = null) {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !currentSession.id || typeof OSA.getSessionCheckpoints !== 'function') return null;

    const messageTsMs = OSA.timestampToMs(messageTimestamp);
    if (messageTsMs === null) return null;

    const checkpoints = OSA.getSessionCheckpoints(currentSession.id);
    if (!Array.isArray(checkpoints) || checkpoints.length === 0) return null;

    let nextAssistantTsMs = null;
    if (Number.isInteger(messageIndex) && Array.isArray(currentSession.messages)) {
        for (let idx = messageIndex + 1; idx < currentSession.messages.length; idx += 1) {
            const candidate = currentSession.messages[idx];
            if (!candidate || candidate.role !== 'assistant' || OSA.isHiddenSyntheticMessage(candidate)) continue;
            nextAssistantTsMs = OSA.timestampToMs(candidate.timestamp);
            if (nextAssistantTsMs !== null) break;
        }
    }

    let fallbackCheckpoint = null;
    for (let idx = checkpoints.length - 1; idx >= 0; idx -= 1) {
        const checkpoint = checkpoints[idx];
        const checkpointTs = OSA.timestampToMs(checkpoint?.created_at);
        if (checkpointTs === null) continue;

        if (checkpointTs < messageTsMs) {
            fallbackCheckpoint = checkpoint;
            continue;
        }

        if (nextAssistantTsMs !== null && checkpointTs >= nextAssistantTsMs) {
            break;
        }

        if (checkpointTs >= messageTsMs) {
            return checkpoint;
        }
    }

    return fallbackCheckpoint;
};

OSA.renderAssistantActionButtons = function(checkpoint) {
    let html = '<button class="msg-action-btn msg-action-copy" onclick="OSA.copyAssistantMessageElement(this)" title="Copy">Copy</button>';

    if (checkpoint && checkpoint.id) {
        const label = OSA.describeCheckpointForUi(checkpoint);
        html += '<button class="msg-action-btn msg-action-restore" data-checkpoint-id="'
            + OSA.escapeHtml(checkpoint.id)
            + '" onclick="OSA.restoreCheckpointFromButton(this)" title="'
            + OSA.escapeHtml('Restore to checkpoint (' + label + ')')
            + '">Restore</button>';
    } else {
        html += '<button class="msg-action-btn msg-action-restore" disabled title="No restore checkpoint available yet">Restore</button>';
    }

    html += '<button class="msg-action-btn msg-action-retry" onclick="OSA.regenerateFromMessage(this)" title="Discard this reply and run the turn again">Retry</button>';
    html += '<button class="msg-action-btn msg-action-speak" onclick="OSA.speakMessageElement(this)" title="Read this message aloud (click again to stop)">Speak</button>';
    html += '<button class="msg-action-btn msg-action-feedback-up" onclick="OSA.toggleMessageFeedback(this, \'positive\')" title="Rate this reply as helpful">Good</button>';
    html += '<button class="msg-action-btn msg-action-feedback-down" onclick="OSA.toggleMessageFeedback(this, \'negative\')" title="Rate this reply as unhelpful">Bad</button>';

    return html;
};

OSA.messageFeedbackCache = {};

OSA.feedbackKey = function(sessionId, seq) {
    return sessionId + ':' + seq;
};

OSA.feedbackSeqForButton = function(button) {
    const messageEl = button.closest('.message') || button.closest('.transcript-entry');
    if (!messageEl) return NaN;
    const raw = messageEl.dataset.messageIndex;
    const parsed = Number.parseInt(raw || '', 10);
    return Number.isInteger(parsed) ? parsed : NaN;
};

OSA.applyMessageFeedbackState = function(actionsEl, feedback) {
    if (!actionsEl) return;
    const up = actionsEl.querySelector('.msg-action-feedback-up');
    const down = actionsEl.querySelector('.msg-action-feedback-down');
    if (up) up.classList.toggle('active', feedback && feedback.rating === 'positive');
    if (down) down.classList.toggle('active', feedback && feedback.rating === 'negative');
};

OSA.applyCachedFeedbackToMessage = function(messageEl) {
    const session = OSA.getCurrentSession();
    if (!session || !session.id) return;
    const seq = OSA.feedbackSeqForButton(messageEl);
    if (!Number.isInteger(seq)) return;
    const actionsEl = messageEl.querySelector('.message-actions');
    if (!actionsEl) return;
    const feedback = OSA.messageFeedbackCache[OSA.feedbackKey(session.id, seq)];
    OSA.applyMessageFeedbackState(actionsEl, feedback);
};

OSA.toggleMessageFeedback = async function(button, rating) {
    const session = OSA.getCurrentSession();
    if (!session || !session.id) return;
    const seq = OSA.feedbackSeqForButton(button);
    if (!Number.isInteger(seq)) return;

    const key = OSA.feedbackKey(session.id, seq);
    const current = OSA.messageFeedbackCache[key];

    if (current && current.rating === rating) {
        try {
            const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(session.id)}/feedback`, {
                method: 'DELETE',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ seq }),
            });
            if (res.ok) {
                delete OSA.messageFeedbackCache[key];
                const messageEl = button.closest('.message');
                if (messageEl) OSA.applyCachedFeedbackToMessage(messageEl);
            }
        } catch (err) {
            OSA.debug.warn('feedback.delete', String(err));
        }
        return;
    }

    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(session.id)}/feedback`, {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                seq,
                rating,
                if_version: current ? current.version : null,
            }),
        });
        const outcome = await res.json().catch(() => ({}));
        const feedback = outcome.feedback || (outcome.status === 'conflict' ? outcome.current : null);
        if (feedback) {
            OSA.messageFeedbackCache[key] = feedback;
        } else if (!res.ok) {
            throw new Error(outcome.error || 'Failed to save feedback');
        }
        const messageEl = button.closest('.message');
        if (messageEl) OSA.applyCachedFeedbackToMessage(messageEl);
    } catch (err) {
        OSA.debug.warn('feedback.put', String(err));
    }
};

OSA.lastFeedbackSessionId = null;

OSA.refreshMessageFeedback = async function() {
    const session = OSA.getCurrentSession();
    if (!session || !session.id || OSA.lastFeedbackSessionId === session.id) return;
    OSA.lastFeedbackSessionId = session.id;
    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(session.id)}/feedback`);
        if (!res.ok) return;
        const rows = await res.json().catch(() => []);
        (rows || []).forEach(function(item) {
            OSA.messageFeedbackCache[OSA.feedbackKey(session.id, item.seq)] = item;
        });
    } catch (err) {
        OSA.debug.warn('feedback.list', String(err));
    }
    document.querySelectorAll('#messages .message.assistant').forEach(function(messageEl) {
        OSA.applyCachedFeedbackToMessage(messageEl);
    });
};

OSA.indexOfRenderedMessage = function(messageEl) {
    const all = Array.from(document.querySelectorAll('#messages .message'));
    return all.indexOf(messageEl);
};

OSA.regenerateFromMessage = async function(button) {
    const messageEl = button.closest('.message');
    const session = OSA.getCurrentSession();
    if (!messageEl || !session?.id) return;

    if (OSA.isAgentProcessing()) {
        OSA.showToast?.('Stop the current turn before retrying.');
        return;
    }

    const index = OSA.indexOfRenderedMessage(messageEl);
    if (index < 1) return;

    const all = Array.from(document.querySelectorAll('#messages .message'));
    let userIndex = index - 1;
    while (userIndex >= 0 && !all[userIndex].classList.contains('user')) {
        userIndex -= 1;
    }
    if (userIndex < 0) return;

    const prompt = all[userIndex].querySelector('.message-content')?.dataset.rawText
        || all[userIndex].innerText
        || '';
    if (!prompt.trim()) return;

    const sessionUserIndex = Number.parseInt(all[userIndex].dataset.messageIndex || '', 10);
    await OSA.truncateSessionMessages(
        session.id,
        Number.isInteger(sessionUserIndex) ? sessionUserIndex : userIndex,
    );

    const input = document.getElementById('message-input');
    if (input) input.value = prompt.trim();
    OSA.sendMessage();
};

OSA.truncateSessionMessages = async function(sessionId, from) {
    const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(sessionId)}/messages/truncate`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ from }),
    });
    if (!res.ok) {
        const err = await res.json().catch(() => ({}));
        throw new Error(err.error || 'Failed to edit history');
    }
    const session = OSA.getCurrentSession();
    if (session && Array.isArray(session.messages)) {
        session.messages.length = Math.min(session.messages.length, from);
    }
    OSA.rebuildAfterTruncate(from);
};

OSA.compactSession = async function(options = {}) {
    const session = OSA.getCurrentSession();
    if (!session?.id) {
        OSA.showToast?.('No active session to compact.');
        return;
    }
    if (OSA.isAgentProcessing && OSA.isAgentProcessing()) {
        OSA.showToast?.('Stop the agent before compacting.');
        return;
    }
    try {
        OSA.showToast?.('Compacting conversation...');
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(session.id)}/compact`, {
            method: 'POST',
            body: JSON.stringify(options && options.focus ? { focus: options.focus } : {}),
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) {
            throw new Error(data.error || 'Compaction failed');
        }
        const parts = [];
        if (data.compacted_messages) parts.push(`${data.compacted_messages} summarized`);
        if (data.pruned_messages) parts.push(`${data.pruned_messages} pruned`);
        OSA.showToast?.(parts.length ? `Compacted: ${parts.join(', ')}.` : 'Nothing to compact yet.');
        if (typeof OSA.selectSession === 'function') {
            await OSA.selectSession(session.id);
        } else if (typeof OSA.loadSessions === 'function') {
            OSA.loadSessions();
        }
        if (typeof OSA.scheduleSessionInspectorRefresh === 'function') {
            OSA.scheduleSessionInspectorRefresh();
        }
    } catch (error) {
        console.error('Compaction failed:', error);
        OSA.showToast?.(error.message || 'Compaction failed.');
    }
};

OSA.updateAssistantMessageActions = function(messageEl, sourceMessage) {
    if (!messageEl) return;
    const actionsEl = messageEl.querySelector('.message-actions');
    if (!actionsEl) return;

    const contentEl = messageEl.querySelector('.message-content');
    const rawText = contentEl ? (contentEl.dataset.rawText || contentEl.textContent || '') : '';
    if (!rawText.trim()) {
        actionsEl.style.display = 'none';
        return;
    }

    const durationEl = actionsEl.querySelector('.turn-duration');
    const tpsEl = actionsEl.querySelector('.turn-tokens');

    const sourceTimestamp = (sourceMessage && sourceMessage.timestamp)
        || messageEl.dataset.messageTimestamp
        || '';
    if (sourceTimestamp) {
        messageEl.dataset.messageTimestamp = sourceTimestamp;
    }

    const messageIndex = Number.parseInt(messageEl.dataset.messageIndex || '', 10);
    const checkpoint = OSA.findNearestCheckpointForMessage(
        sourceTimestamp,
        Number.isInteger(messageIndex) ? messageIndex : null,
    );
    actionsEl.innerHTML = OSA.renderAssistantActionButtons(checkpoint);

    const copyBtn = actionsEl.querySelector('.msg-action-copy');
    const restoreBtn = actionsEl.querySelector('.msg-action-restore');
    if (tpsEl && restoreBtn) {
        restoreBtn.after(tpsEl);
    } else if (tpsEl && copyBtn) {
        copyBtn.after(tpsEl);
    } else if (tpsEl) {
        actionsEl.appendChild(tpsEl);
    }
    if (durationEl) {
        actionsEl.appendChild(durationEl);
    }

    actionsEl.style.display = '';
};

OSA.updateAssistantRestoreButtons = function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !Array.isArray(currentSession.messages)) return;

    document.querySelectorAll('#messages .message.assistant').forEach(function(messageEl) {
        const messageIndex = Number.parseInt(messageEl.dataset.messageIndex || '', 10);
        const sourceMessage = Number.isInteger(messageIndex) ? currentSession.messages[messageIndex] : null;
        OSA.updateAssistantMessageActions(messageEl, sourceMessage);
    });
};

OSA.restoreCheckpointFromButton = function(button) {
    const checkpointId = button?.dataset?.checkpointId || '';
    if (!checkpointId) return;
    OSA.restoreCheckpoint(checkpointId, button);
};

OSA.showCheckpointRestoreDialog = function(checkpointLabel, snapshotCount, diffData) {
    return new Promise(function(resolve) {
        const modal = document.createElement('div');
        modal.className = 'modal';
        modal.style.display = 'flex';

        var TOOL_OUTPUT_PREFIX = '.osa_tool_outputs/';
        var allDiffs = Array.isArray(diffData?.diffs) ? diffData.diffs : [];
        var diffs = allDiffs.filter(function(d) {
            return !(d.path || '').startsWith(TOOL_OUTPUT_PREFIX);
        });
        var allChangedFiles = Array.isArray(diffData?.changed_files) ? diffData.changed_files : [];
        var changedFiles = allChangedFiles.filter(function(p) {
            return !p.startsWith(TOOL_OUTPUT_PREFIX);
        });

        var subtitle = snapshotCount > 0
            ? 'This will restore session state and revert ' + snapshotCount + ' captured file snapshot' + (snapshotCount === 1 ? '' : 's') + '.'
            : 'This will restore session state.';

        var selectedIdx = diffs.length > 0 ? 0 : -1;

        modal.innerHTML = ''
            + '<div class="modal-content" style="max-width:960px; width:92vw; max-height:85vh; display:flex; flex-direction:column;">'
            + '  <div class="modal-header"><h3>Restore checkpoint</h3></div>'
            + '  <div class="modal-body" style="padding:14px 16px; flex:1; overflow:auto;">'
            + '    <p style="margin:0 0 8px 0; color:var(--text-secondary);">' + OSA.escapeHtml(checkpointLabel) + '</p>'
            + '    <p style="margin:0 0 12px 0; color:var(--text-secondary);">' + OSA.escapeHtml(subtitle) + '</p>'
            + '    <p style="margin:0 0 8px 0; color:var(--text-secondary);">Changed files: ' + changedFiles.length + '</p>'
            + '    <div class="checkpoint-restore-layout" style="display:flex; gap:12px; min-height:120px;">'
            + '      <div class="checkpoint-file-list" style="min-width:200px; max-width:280px; overflow-y:auto; max-height:360px; border:1px solid var(--border); border-radius:6px; padding:4px 0;"></div>'
            + '      <div class="checkpoint-diff-preview" style="flex:1; overflow:auto; max-height:360px; border:1px solid var(--border); border-radius:6px; padding:8px;"></div>'
            + '    </div>'
            + '  </div>'
            + '  <div class="modal-actions" style="display:flex; justify-content:flex-end; gap:8px; padding:12px 16px; border-top:1px solid var(--border);">'
            + '    <button class="btn-ghost checkpoint-cancel">Cancel</button>'
            + '    <button class="btn-action checkpoint-restore">Restore</button>'
            + '  </div>'
            + '</div>';

        document.body.appendChild(modal);

        var fileList = modal.querySelector('.checkpoint-file-list');
        var diffHost = modal.querySelector('.checkpoint-diff-preview');

        function renderFileList() {
            fileList.innerHTML = '';
            if (diffs.length === 0) {
                fileList.innerHTML = '<div style="padding:8px 12px; color:var(--text-secondary); font-size:12px;">No file changes</div>';
                return;
            }
            diffs.forEach(function(diff, idx) {
                var item = document.createElement('button');
                item.type = 'button';
                item.style.cssText = 'display:block; width:100%; text-align:left; padding:6px 12px; border:none; background:none; cursor:pointer; font-size:12px; font-family:inherit; color:var(--text-primary);';
                if (idx === selectedIdx) {
                    item.style.background = 'var(--bg-hover, rgba(255,255,255,0.06))';
                }
                item.addEventListener('mouseenter', function() { if (idx !== selectedIdx) item.style.background = 'var(--bg-hover, rgba(255,255,255,0.03))'; });
                item.addEventListener('mouseleave', function() { if (idx !== selectedIdx) item.style.background = 'none'; });
                item.addEventListener('click', function() {
                    selectedIdx = idx;
                    renderFileList();
                    renderDiff();
                });
                var status = diff.status || 'modified';
                var badge = status === 'added' ? '+' : status === 'deleted' ? '-' : '~';
                var badgeColor = status === 'added' ? '#4caf50' : status === 'deleted' ? '#f44336' : 'var(--text-secondary)';
                item.innerHTML = '<span style="color:' + badgeColor + '; margin-right:4px; font-weight:600;">' + OSA.escapeHtml(badge) + '</span> ' + OSA.escapeHtml(diff.path || '');
                fileList.appendChild(item);
            });
        }

        function renderDiff() {
            diffHost.innerHTML = '';
            if (selectedIdx < 0 || selectedIdx >= diffs.length) {
                diffHost.innerHTML = '<div style="color:var(--text-secondary); font-size:12px; padding:8px;">Select a file to preview changes</div>';
                return;
            }
            var diff = diffs[selectedIdx];
            var path = document.createElement('div');
            path.style.cssText = 'font-size:12px; color:var(--text-secondary); margin-bottom:6px; font-weight:600;';
            path.textContent = diff.path || 'Diff preview';
            diffHost.appendChild(path);
            if (typeof OSA.renderDiffView === 'function') {
                var oldContent = OSA.extractOldContentFromUnifiedDiff(diff.diff || '');
                var newContent = OSA.extractNewContentFromUnifiedDiff(diff.diff || '');
                diffHost.appendChild(OSA.renderDiffView(oldContent, newContent));
            }
        }

        renderFileList();
        renderDiff();

        var close = function(value) {
            modal.remove();
            resolve(value);
        };

        modal.addEventListener('click', function(event) {
            if (event.target === modal) close(false);
        });
        modal.querySelector('.checkpoint-cancel')?.addEventListener('click', function() { close(false); });
        modal.querySelector('.checkpoint-restore')?.addEventListener('click', function() { close(true); });
    });
};

OSA.extractOldContentFromUnifiedDiff = function(diffText) {
    const lines = (diffText || '').split('\n');
    const oldLines = [];
    lines.forEach(function(line) {
        if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('@@')) return;
        if (line.startsWith('+')) return;
        if (line.startsWith('-')) {
            oldLines.push(line.slice(1));
            return;
        }
        if (line.startsWith(' ')) oldLines.push(line.slice(1));
    });
    return oldLines.join('\n');
};

OSA.extractNewContentFromUnifiedDiff = function(diffText) {
    const lines = (diffText || '').split('\n');
    const newLines = [];
    lines.forEach(function(line) {
        if (line.startsWith('+++') || line.startsWith('---') || line.startsWith('@@')) return;
        if (line.startsWith('-')) return;
        if (line.startsWith('+')) {
            newLines.push(line.slice(1));
            return;
        }
        if (line.startsWith(' ')) newLines.push(line.slice(1));
    });
    return newLines.join('\n');
};

OSA.shouldSnapshotBeRestoredForCheckpoint = function(snapshot, checkpoint) {
    if (!snapshot || !checkpoint) return false;

    const snapshotMs = OSA.timestampToMs(snapshot.created_at);
    const checkpointMs = OSA.timestampToMs(checkpoint.created_at);
    if (snapshotMs === null || checkpointMs === null) return false;

    if (snapshotMs > checkpointMs) {
        return true;
    }

    if (snapshotMs === checkpointMs) {
        const checkpointTool = checkpoint.tool_name || '';
        return !!checkpointTool && checkpointTool === (snapshot.tool_name || '');
    }

    return false;
};

OSA.fetchRestorePlan = async function(sessionId, checkpoint) {
    if (!sessionId || !checkpoint?.id) {
        return { snapshots: [], count: 0 };
    }

    const res = await OSA.fetchWithAuth(`/api/sessions/${sessionId}/snapshots`);
    const data = await res.json().catch(() => []);
    if (!res.ok) {
        throw new Error(data.error || `HTTP ${res.status}`);
    }

    const snapshots = (Array.isArray(data) ? data : []).filter(function(snapshot) {
        return OSA.shouldSnapshotBeRestoredForCheckpoint(snapshot, checkpoint);
    });

    return {
        snapshots,
        count: snapshots.length,
    };
};

OSA.restoreCheckpoint = async function(checkpointId, button) {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !currentSession.id) return;

    const sessionId = currentSession.id;
    const checkpoints = (typeof OSA.getSessionCheckpoints === 'function')
        ? OSA.getSessionCheckpoints(sessionId)
        : [];
    const checkpoint = checkpoints.find(function(item) { return item.id === checkpointId; });
    const checkpointLabel = checkpoint
        ? OSA.describeCheckpointForUi(checkpoint)
        : 'the selected checkpoint';

    let plan = { snapshots: [], count: 0 };
    let checkpointDiffData = null;
    try {
        if (checkpoint) {
            plan = await OSA.fetchRestorePlan(sessionId, checkpoint);
            const diffRes = await OSA.fetchWithAuth(`/api/sessions/${sessionId}/checkpoints/${checkpointId}/diff`);
            const diffData = await diffRes.json().catch(() => ({}));
            if (diffRes.ok) checkpointDiffData = diffData;
        }
    } catch (error) {
        console.warn('Failed to fetch restore plan:', error);
    }

    const snapshotCount = plan.count || 0;
    const confirmed = await OSA.showCheckpointRestoreDialog(checkpointLabel, snapshotCount, checkpointDiffData);
    if (!confirmed) return;

    const restoreButton = button || null;
    const previousLabel = restoreButton ? restoreButton.textContent : '';
    if (restoreButton) {
        restoreButton.disabled = true;
        restoreButton.textContent = 'Restoring...';
    }

    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${sessionId}/restore`, {
            method: 'POST',
            body: JSON.stringify({ checkpoint_id: checkpointId, restore_files: true }),
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) {
            throw new Error(data.error || `HTTP ${res.status}`);
        }

        if (typeof OSA.loadSessionCheckpoints === 'function') {
            await OSA.loadSessionCheckpoints(sessionId, { silent: true });
        }
        await OSA.selectSession(sessionId);
        const revertedCount = Number.isFinite(data?.reverted_snapshots) ? data.reverted_snapshots : snapshotCount;
        alert(`Session restored to checkpoint. Reverted ${revertedCount} file snapshot${revertedCount === 1 ? '' : 's'}.`);
    } catch (error) {
        alert(`Failed to restore checkpoint: ${error.message || 'Unknown error'}`);
    } finally {
        if (restoreButton) {
            restoreButton.disabled = false;
            restoreButton.textContent = previousLabel || 'Restore';
        }
    }
};

OSA.copyAssistantMessage = function(domId) {
    const message = document.getElementById(domId);
    if (!message) return;
    const contentEl = message.querySelector('.message-content');
    const text = contentEl ? (contentEl.dataset.rawText || contentEl.textContent) : '';
    if (!text) return;
    navigator.clipboard.writeText(text).then(() => {
        const btn = message.querySelector('.msg-action-copy');
        if (btn) { btn.textContent = 'Copied!'; setTimeout(() => btn.textContent = 'Copy', 2000); }
    });
};

OSA.copyAssistantMessageElement = function(button) {
    const message = button && button.closest ? button.closest('.message.assistant') : null;
    if (!message) return;
    const contentEl = message.querySelector('.message-content');
    const text = contentEl ? (contentEl.dataset.rawText || contentEl.textContent || '') : '';
    if (!text) return;
    navigator.clipboard.writeText(text).then(() => {
        const original = button.textContent;
        button.textContent = 'Copied!';
        setTimeout(() => {
            button.textContent = original;
        }, 2000);
    });
};

OSA.showErrorCard = function(errorMsg, options = {}) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) emptyState.remove();

    const truncated = errorMsg.length > 120 ? errorMsg.slice(0, 120) + '...' : errorMsg;
    const card = document.createElement('div');
    card.className = 'error-card';
    // A send failure offers a retry instead of only a dismiss, so the user does
    // not have to retype what they already wrote.
    const retryAction = typeof options.onRetry === 'function' ? 'OSA.retryFailedSend(this)' : '';
    const retryLabel = options.retryLabel || 'Retry';
    card.innerHTML = `
        <div class="error-card-icon">!</div>
        <div class="error-card-body">
            <div class="error-card-title">${OSA.escapeHtml(options.title || 'Something went wrong')}</div>
            <div class="error-card-message" title="${OSA.escapeAttr(errorMsg)}">${OSA.escapeHtml(truncated)}</div>
        </div>
        ${retryAction ? `<button class="error-card-retry error-card-retry-primary" onclick="${retryAction}">${OSA.escapeHtml(retryLabel)}</button>` : ''}
        <button class="error-card-retry" onclick="this.closest('.error-card').remove()">Dismiss</button>
    `;
    if (typeof options.onRetry === 'function') {
        OSA._lastFailedSend = options.onRetry;
        card.querySelector('.error-card-retry-primary').addEventListener('click', function() {
            card.remove();
        });
    }
    OSA.mountFloatingNode(card);
    OSA.tmodelMarkDirty('error-card');
};

OSA.retryFailedSend = function() {
    const retry = OSA._lastFailedSend;
    OSA._lastFailedSend = null;
    if (typeof retry === 'function') retry();
};

OSA.formatInlineMarkdown = function(line) {
    let s = line
        .replace(/\[([^\]]+)\]\(([^)]+)\)/g, function(match, label, url) {
            const href = String(url).trim();
            // Only http(s) links become anchors: a javascript:/data: URL here
            // would execute on click. Quotes are percent-encoded because the
            // surrounding text is already entity-escaped but quotes are not.
            if (!/^https?:\/\//i.test(href)) return match;
            return '<a href="' + href.replace(/"/g, '%22').replace(/'/g, '%27') + '" target="_blank" rel="noopener noreferrer">' + label + '</a>';
        })
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/\*([^*]+)\*/g, '<em>$1</em>')
        .replace(/`([^`]+)`/g, '<code>$1</code>');
    s = s.replace(/(^|[^"=])(https?:\/\/[^\s<>"')\]]+)/g, '$1<a href="$2" target="_blank" rel="noopener noreferrer">$2</a>');
    return s;
};

OSA.formatMessage = function(text) {
    text = OSA.stripSpeakBlock ? OSA.stripSpeakBlock(text) : text;
    const escaped = OSA.escapeHtml((text || '').replace(/\n+$/, ''));
    const lines = escaped.split('\n');
    let html = '';
    let listItems = [];
    let codeBlock = null;
    let codeLines = [];
    let tableRows = [];
    let tableHasHeader = false;

    const formatInlineMarkdown = OSA.formatInlineMarkdown;

    const flushList = () => {
        if (listItems.length) {
            html += `<ul>${listItems.join('')}</ul>`;
            listItems = [];
        }
    };

    const flushTable = () => {
        if (!tableRows.length) return;
        let tableHtml = '<table>';
        tableRows.forEach((row, i) => {
            const tag = (i === 0 && tableHasHeader) ? 'th' : 'td';
            tableHtml += '<tr>' + row.map(c => `<${tag}>${formatInlineMarkdown(c.trim())}</${tag}>`).join('') + '</tr>';
        });
        tableHtml += '</table>';
        html += tableHtml;
        tableRows = [];
        tableHasHeader = false;
    };

    const flushCodeBlock = () => {
        if (codeBlock) {
            const lang = codeBlock.lang ? ` class="language-${codeBlock.lang}"` : '';
            const code = codeLines.join('\n');
            const highlighted = codeBlock.lang ? OSA.highlightCode(code, codeBlock.lang) : OSA.escapeHtml(code);
            html += `<div class="code-block"><div class="code-header"><span class="code-lang">${codeBlock.lang || 'text'}</span><button class="code-copy" onclick="OSA.copyCode(this)">Copy</button></div><pre><code${lang}>${highlighted}</code></pre></div>`;
            codeBlock = null;
            codeLines = [];
        }
    };

    const isTableRow = (line) => {
        const t = line.trim();
        return t.startsWith('|') && t.endsWith('|') && t.length > 2;
    };

    const isTableSeparator = (line) => /^\|[\s\-:|]+\|$/.test(line.trim());

    const parseTableCells = (line) => {
        const t = line.trim();
        return t.slice(1, -1).split('|');
    };

    const isHeader = (line) => /^#+\s/.test(line);
    const headerLevel = (line) => {
        const m = line.match(/^(#+)/);
        return m ? m[1].length : 0;
    };

    for (const line of lines) {
        const trimmed = line.trim();
        if (codeBlock) {
            if (trimmed === '```') { flushCodeBlock(); } else { codeLines.push(line); }
            continue;
        }
        if (isTableRow(trimmed)) {
            if (isTableSeparator(trimmed)) {
                tableHasHeader = true;
                continue;
            }
            flushList();
            tableRows.push(parseTableCells(trimmed));
            continue;
        } else if (tableRows.length) {
            flushTable();
        }
        if (isHeader(trimmed)) {
            flushList();
            const level = headerLevel(trimmed);
            const text = trimmed.replace(/^#+\s/, '');
            html += `<h${level}>${formatInlineMarkdown(text)}</h${level}>`;
            continue;
        }
        const codeBlockMatch = trimmed.match(/^```(\w+)?$/);
        if (codeBlockMatch) { flushList(); codeBlock = { lang: codeBlockMatch[1] || null }; continue; }
        if (trimmed.startsWith('- ')) { listItems.push(`<li>${formatInlineMarkdown(trimmed.slice(2))}</li>`); continue; }
        const numberedMatch = trimmed.match(/^(\d+)\.\s+(.*)/);
        if (numberedMatch) { listItems.push(`<li>${formatInlineMarkdown(numberedMatch[2])}</li>`); continue; }
        flushList();
        if (trimmed.length === 0) { html += '<br>'; } else { html += `<p>${formatInlineMarkdown(line)}</p>`; }
    }

    flushList();
    flushTable();
    flushCodeBlock();
    return html;
};

OSA.createIncrementalMd = function() {
    return {
        renderedLen: 0,
        codeLang: null,
        codeText: null,
        codeFirst: true,
        listEl: null,
        tableRows: null,
        tableHeader: false,
        tableEl: null,
        tableRowCount: 0,
    };
};

OSA.mdIsTableRow = function(t) {
    return t.startsWith('|') && t.endsWith('|') && t.length > 2;
};

OSA.mdIsTableSeparator = function(t) {
    return /^\|[\s\-:|]+\|$/.test(t);
};

OSA.mdIsHeader = function(t) {
    return /^#+\s/.test(t);
};

OSA.mdParseTableCells = function(t) {
    return t.slice(1, -1).split('|');
};

OSA.mdBuildListItem = function(text) {
    const li = document.createElement('li');
    li.innerHTML = OSA.formatInlineMarkdown(OSA.escapeHtml(text));
    return li;
};

OSA.mdBuildTable = function(md) {
    const table = md.tableEl || document.createElement('table');
    if (!md.tableEl) {
        md.tableEl = table;
        md.tableRowCount = 0;
    }
    (md.tableRows || []).forEach((row, i) => {
        const tr = document.createElement('tr');
        const tag = (md.tableRowCount + i === 0 && md.tableHeader) ? 'th' : 'td';
        row.forEach(cell => {
            const cellEl = document.createElement(tag);
            cellEl.innerHTML = OSA.formatInlineMarkdown(OSA.escapeHtml(cell.trim()));
            tr.appendChild(cellEl);
        });
        table.appendChild(tr);
    });
    md.tableRowCount = (md.tableRowCount || 0) + (md.tableRows || []).length;
    md.tableRows = [];
    return table;
};

OSA.mdOpenCodeBlock = function(md, lang, el) {
    const wrap = document.createElement('div');
    wrap.className = 'code-block';
    const header = document.createElement('div');
    header.className = 'code-header';
    const langSpan = document.createElement('span');
    langSpan.className = 'code-lang';
    langSpan.textContent = lang || 'text';
    const copyBtn = document.createElement('button');
    copyBtn.className = 'code-copy';
    copyBtn.textContent = 'Copy';
    copyBtn.onclick = function() { OSA.copyCode(this); };
    header.appendChild(langSpan);
    header.appendChild(copyBtn);
    const pre = document.createElement('pre');
    const code = document.createElement('code');
    if (lang) code.className = 'language-' + lang;
    const textNode = document.createTextNode('');
    code.appendChild(textNode);
    pre.appendChild(code);
    wrap.appendChild(header);
    wrap.appendChild(pre);
    el.appendChild(wrap);
    md.codeLang = lang || '';
    md.codeText = textNode;
    md.codeFirst = true;
};

OSA.mdAppendLine = function(md, line, el) {
    const trimmed = line.trim();

    if (md.codeLang !== null) {
        if (trimmed === '```') {
            const text = md.codeText ? md.codeText.data : '';
            const codeEl = md.codeText ? md.codeText.parentNode : null;
            if (codeEl) {
                codeEl.innerHTML = md.codeLang
                    ? OSA.highlightCode(text, md.codeLang)
                    : OSA.escapeHtml(text);
            }
            md.codeLang = null;
            md.codeText = null;
        } else if (md.codeText) {
            md.codeText.appendData((md.codeFirst ? '' : '\n') + line);
            md.codeFirst = false;
        }
        return;
    }

    if (OSA.mdIsTableRow(trimmed)) {
        if (OSA.mdIsTableSeparator(trimmed)) {
            md.tableHeader = true;
            return;
        }
        if (md.listEl) {
            if (!md.listEl.isConnected) el.appendChild(md.listEl);
            md.listEl = null;
        }
        if (!md.tableRows) md.tableRows = [];
        if (!md.tableEl) {
            md.tableEl = document.createElement('table');
            md.tableRowCount = 0;
            el.appendChild(md.tableEl);
        }
        md.tableRows.push(OSA.mdParseTableCells(trimmed));
        // Render table rows incrementally instead of rebuilding the table on
        // every line: stale header detection flickers mid-stream.
        OSA.mdBuildTable(md);
        return;
    }

    if (md.tableRows && md.tableRows.length) {
        OSA.mdBuildTable(md);
    }
    md.tableEl = null;
    md.tableRowCount = 0;
    md.tableRows = null;

    if (OSA.mdIsHeader(trimmed)) {
        if (md.listEl) {
            if (!md.listEl.isConnected) el.appendChild(md.listEl);
            md.listEl = null;
        }
        const level = (trimmed.match(/^(#+)/) || ['', ''])[1].length;
        const h = document.createElement('h' + level);
        h.innerHTML = OSA.formatInlineMarkdown(OSA.escapeHtml(trimmed.replace(/^#+\s/, '')));
        el.appendChild(h);
        return;
    }

    if (/^```(\w+)?$/.test(trimmed)) {
        if (md.listEl) {
            if (!md.listEl.isConnected) el.appendChild(md.listEl);
            md.listEl = null;
        }
        OSA.mdOpenCodeBlock(md, (trimmed.match(/^```(\w+)?$/) || ['', ''])[1], el);
        return;
    }

    if (trimmed.startsWith('- ')) {
        if (!md.listEl) md.listEl = document.createElement('ul');
        if (!md.listEl.isConnected) el.appendChild(md.listEl);
        md.listEl.appendChild(OSA.mdBuildListItem(trimmed.slice(2)));
        return;
    }

    const numberedMatch = trimmed.match(/^(\d+)\.\s+(.*)/);
    if (numberedMatch) {
        if (!md.listEl) md.listEl = document.createElement('ul');
        if (!md.listEl.isConnected) el.appendChild(md.listEl);
        md.listEl.appendChild(OSA.mdBuildListItem(numberedMatch[2]));
        return;
    }

    if (md.listEl) {
        if (!md.listEl.isConnected) el.appendChild(md.listEl);
        md.listEl = null;
    }

    if (trimmed.length === 0) {
        el.appendChild(document.createElement('br'));
    } else {
        const p = document.createElement('p');
        p.innerHTML = OSA.formatInlineMarkdown(OSA.escapeHtml(line));
        el.appendChild(p);
    }
};

OSA.renderIncrementalMarkdown = function(el, rawText) {
    let md = el._md;
    if (!md || rawText.length < md.renderedLen) {
        el.innerHTML = '';
        el._md = OSA.createIncrementalMd();
        md = el._md;
    }
    const pending = rawText.slice(md.renderedLen);
    if (!pending) {
        el.dataset.renderedText = rawText;
        return;
    }

    // Commit only newline-terminated lines, each styled exactly once as it is
    // appended. The trailing fragment without a terminator is held back
    // silently until more text arrives or the turn flushes, so unfinished
    // text is never shown in a half-styled state.
    // Code fences stream complete lines straight into the fence's text node
    // (no innerHTML restyle), so they commit the same way as other lines.
    const lastNl = pending.lastIndexOf('\n');
    if (lastNl < 0) {
        el.dataset.renderedText = rawText;
        return;
    }
    const lines = pending.slice(0, lastNl + 1).split('\n');
    lines.pop();
    for (const line of lines) {
        OSA.mdAppendLine(md, line, el);
    }
    md.renderedLen += lastNl + 1;

    el.dataset.renderedText = rawText;
};

OSA.flushIncrementalMarkdown = function(el, rawText) {
    let md = el && el._md;
    if (!md) return;
    if (rawText.length < md.renderedLen) {
        el.innerHTML = '';
        el._md = OSA.createIncrementalMd();
        md = el._md;
    }
    const partial = rawText.slice(md.renderedLen);
    if (partial) {
        const lines = partial.split('\n');
        for (const line of lines) {
            OSA.mdAppendLine(md, line, el);
        }
    }
    if (md.listEl) {
        if (!md.listEl.isConnected) el.appendChild(md.listEl);
        md.listEl = null;
    }
    if (md.tableRows && md.tableRows.length) {
        OSA.mdBuildTable(md);
    }
    md.tableEl = null;
    md.tableRowCount = 0;
    md.tableRows = null;
    if (md.codeLang !== null && md.codeText) {
        const codeEl = md.codeText.parentNode;
        if (codeEl) {
            const text = md.codeText.data;
            codeEl.innerHTML = md.codeLang
                ? OSA.highlightCode(text, md.codeLang)
                : OSA.escapeHtml(text);
        }
        md.codeLang = null;
        md.codeText = null;
    }
    while (el.lastChild && el.lastChild.tagName === 'BR') {
        el.lastChild.remove();
    }
    md.renderedLen = rawText.length;
    el.dataset.renderedText = rawText;
};

OSA.finalizeIncrementalRenders = function(messageEl) {
    if (!messageEl) return;
    const contentEl = messageEl.querySelector('.message-content');
    if (contentEl) OSA.flushIncrementalMarkdown(contentEl, contentEl.dataset.rawText || '');
    const thinkingBody = messageEl.querySelector('.thinking-body');
    if (thinkingBody) OSA.flushIncrementalMarkdown(thinkingBody, thinkingBody.dataset.rawText || '');
};

OSA.highlightCode = function(code, lang) {
    const keywords = {
        c: ['int', 'char', 'void', 'return', 'if', 'else', 'for', 'while', 'include', 'define', 'typedef', 'struct', 'const', 'static'],
        cpp: ['int', 'char', 'void', 'return', 'if', 'else', 'for', 'while', 'include', 'define', 'class', 'public', 'private', 'protected', 'const', 'static', 'auto', 'template'],
        python: ['def', 'return', 'if', 'else', 'elif', 'for', 'while', 'import', 'from', 'class', 'try', 'except', 'finally', 'with', 'as', 'lambda', 'yield'],
        javascript: ['function', 'return', 'if', 'else', 'for', 'while', 'const', 'let', 'var', 'class', 'import', 'export', 'async', 'await', 'try', 'catch', 'finally'],
        rust: ['fn', 'let', 'mut', 'pub', 'use', 'mod', 'struct', 'enum', 'impl', 'trait', 'if', 'else', 'match', 'return', 'const', 'static'],
        java: ['public', 'private', 'protected', 'class', 'interface', 'void', 'int', 'String', 'return', 'if', 'else', 'for', 'while', 'import', 'package']
    };
    // Tokenize on the RAW text first, then escape per-token at emit time.
    // The old version escaped first and highlighted with regexes on the HTML,
    // so the string pass matched its own `class="token-keyword"` attributes
    // and wrapped them again (the `>"token-keyword">` cascade in previews).
    const langKeywords = new Set((keywords[(lang || '').toLowerCase()] || []).map(k => k.toLowerCase()));
    const esc = function(s) {
        return String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    };
    const span = function(cls, text) {
        return '<span class="' + cls + '">' + esc(text) + '</span>';
    };
    const isWordChar = function(ch) {
        return /[A-Za-z0-9_]/.test(ch || '');
    };
    let html = '';
    let i = 0;
    const n = code.length;
    while (i < n) {
        const ch = code[i];
        if (ch === '/' && code[i + 1] === '/') {
            let j = code.indexOf('\n', i);
            if (j < 0) j = n;
            html += span('token-comment', code.slice(i, j));
            i = j;
        } else if (ch === '#') {
            const lineStart = i === 0 || code[i - 1] === '\n';
            if (lineStart) {
                let j = code.indexOf('\n', i);
                if (j < 0) j = n;
                html += span('token-comment', code.slice(i, j));
                i = j;
            } else {
                html += esc(ch);
                i += 1;
            }
        } else if (ch === '"' || ch === "'") {
            let j = i + 1;
            while (j < n && code[j] !== ch) {
                if (code[j] === '\\') j += 1;
                j += 1;
            }
            if (j < n) j += 1;
            html += span('token-string', code.slice(i, j));
            i = j;
        } else if (/[0-9]/.test(ch) && !isWordChar(code[i - 1])) {
            let j = i;
            while (j < n && /[0-9]/.test(code[j])) j += 1;
            if (j < n && code[j] === '.' && /[0-9]/.test(code[j + 1])) {
                j += 1;
                while (j < n && /[0-9]/.test(code[j])) j += 1;
            }
            if (!isWordChar(code[j])) {
                html += span('token-number', code.slice(i, j));
                i = j;
            } else {
                html += esc(ch);
                i += 1;
            }
        } else if (/[A-Za-z_]/.test(ch) && !isWordChar(code[i - 1])) {
            let j = i;
            while (j < n && /[A-Za-z0-9_]/.test(code[j])) j += 1;
            const word = code.slice(i, j);
            if (langKeywords.has(word.toLowerCase())) {
                html += span('token-keyword', word);
            } else {
                html += esc(word);
            }
            i = j;
        } else {
            html += esc(ch);
            i += 1;
        }
    }
    return html;
};

OSA.copyCode = function(btn) {
    const code = btn.closest('.code-block').querySelector('code').textContent;
    navigator.clipboard.writeText(code).then(() => {
        btn.textContent = 'Copied!';
        setTimeout(() => btn.textContent = 'Copy', 2000);
    });
};

OSA.removeQueuedMessageElements = function() {
    // Legacy floating notices (pre-panel UI) plus the current panel rows.
    const view = OSA.getTranscriptView ? OSA.getTranscriptView() : null;
    if (view && view.floatingRoot) {
        view.floatingRoot.querySelectorAll('.queued-notice').forEach(el => el.remove());
    }
    const panel = document.getElementById('queue-panel');
    if (panel) panel.innerHTML = '';
};

// Follow-up behavior while the agent is working: "queue" appends behind the
// current turn, "steer" interrupts it and runs the message next. Stored per
// browser like the mic/speaker choices.
OSA.getFollowUpBehavior = function() {
    try {
        const value = localStorage.getItem('osa.queue.followUp');
        if (value === 'steer' || value === 'queue') return value;
    } catch (err) {}
    return 'queue';
};

OSA.setFollowUpBehavior = function(behavior) {
    const next = behavior === 'steer' ? 'steer' : 'queue';
    try {
        localStorage.setItem('osa.queue.followUp', next);
    } catch (err) {}
    OSA.updateFollowUpToggle();
    return next;
};

OSA.toggleFollowUpBehavior = function() {
    OSA.setFollowUpBehavior(OSA.getFollowUpBehavior() === 'queue' ? 'steer' : 'queue');
};

OSA.updateFollowUpToggle = function(queueItems) {
    const toggle = document.getElementById('followup-toggle');
    if (!toggle) return;
    const items = Array.isArray(queueItems) ? queueItems : (OSA.getSessionQueue() || []);
    const visible = OSA.isAgentProcessing() || items.length > 0 || !!OSA.queueEditingId;
    toggle.classList.toggle('hidden', !visible);
    const behavior = OSA.getFollowUpBehavior();
    const alternate = behavior === 'queue' ? 'steer' : 'queue';
    toggle.textContent = behavior === 'queue' ? 'Queue' : 'Steer';
    toggle.dataset.behavior = behavior;
    toggle.title = `Follow-ups will ${behavior} while working. Click for ${alternate}. Ctrl+Enter sends ${alternate}.`;
    toggle.setAttribute('aria-label', `Follow-up behavior: ${behavior}. Activate to switch to ${alternate}.`);
};

// Single Enter handler for the composer: edit confirmation, follow-up
// delivery (Enter = preference, Ctrl/Cmd+Enter = alternate), plain send.
OSA.handleComposerKeydown = function(event) {
    if (!event) return;
    if (event.key === 'Escape' && OSA.queueEditingId) {
        event.preventDefault();
        // Stop the global Escape handler (modal dismissal, stop-generation)
        // from also firing while cancelling a queue edit.
        if (event.stopPropagation) event.stopPropagation();
        OSA.cancelQueueEdit();
        return;
    }
    if (event.key !== 'Enter' || event.shiftKey) return;
    if (event.isComposing) return;
    event.preventDefault();
    if (OSA.queueEditingId) {
        OSA.confirmQueueEdit();
        return;
    }
    const working = OSA.isAgentProcessing() || (OSA.getSessionQueue() || []).length > 0;
    if (!working) {
        OSA.runSendMessage();
        return;
    }
    const behavior = OSA.getFollowUpBehavior();
    const alternate = event.ctrlKey || event.metaKey;
    OSA.sendFollowUp(alternate ? (behavior === 'queue' ? 'steer' : 'queue') : behavior);
};

// Sends a follow-up while the agent is working. "steer" queues then
// immediately promotes the message so it interrupts the current turn.
OSA.sendFollowUp = async function(delivery) {
    if (OSA.queueEditingId) {
        OSA.confirmQueueEdit();
        return;
    }
    const input = document.getElementById('message-input');
    const text = input ? input.value.trim() : '';
    if (!text && OSA.getAttachments().length === 0) return;
    if (!OSA.isAgentProcessing() && (OSA.getSessionQueue() || []).length === 0) {
        OSA.runSendMessage();
        return;
    }
    if (delivery !== 'steer') {
        OSA.runSendMessage();
        return;
    }
    try {
        const data = await OSA.sendMessage();
        const item = data && data.queue_item;
        if (data && data.queued && item && item.id) {
            await OSA.sendQueuedMessageNow(item);
        } else if (!data) {
            OSA.showErrorCard?.('Follow-up was not queued.');
        }
    } catch (error) {
        console.error('Failed to steer follow-up:', error);
    }
};

OSA.renderAttachmentMarkup = function(attachments = []) {
    const imageAttachments = attachments.filter(att => att.kind === 'image' || (att.mime || '').startsWith('image/'));
    const fileAttachments = attachments.filter(att => !(att.kind === 'image' || (att.mime || '').startsWith('image/')));

    let html = '';
    if (imageAttachments.length > 0) {
        html += '<div class="message-image-grid">';
        imageAttachments.forEach(att => {
            const src = OSA.getAttachmentImageSrc(att);
            html += `<div class="message-image-thumb"><img class="expandable-image" data-image-src="${OSA.escapeAttr(src)}" src="${OSA.escapeAttr(src)}" alt="${OSA.escapeAttr(att.filename || '')}" /></div>`;
        });
        html += '</div>';
    }

    if (fileAttachments.length > 0) {
        html += '<div class="message-attachment-list">';
        fileAttachments.forEach(att => {
            html += `<div class="message-attachment-chip">${OSA.escapeHtml(att.filename || '')}</div>`;
        });
        html += '</div>';
    }

    return html;
};

OSA.getAttachmentImageSrc = function(attachment) {
    return attachment?.previewUrl || attachment?.preview_url || attachment?.dataUrl || attachment?.data_url || '';
};

OSA.collectMessageAttachments = function(message) {
    const items = [];
    if (message?.role === 'user' && Array.isArray(message.images)) {
        message.images.forEach(img => items.push(img));
    }
    if (message?.role === 'user' && message.metadata && Array.isArray(message.metadata.attachments)) {
        message.metadata.attachments.forEach(att => items.push(att));
    }
    return items;
};

OSA.resetTranscriptView = function() {
    const view = OSA.getTranscriptView();
    if (view.ioTop) view.ioTop.disconnect();
    if (view.ioBottom) view.ioBottom.disconnect();
    view.isRendering = false;
    view.shiftInProgress = false;
    view.lastShiftAt = 0;
    view.avgMessageHeight = 132;
    view.messageHeights.clear();
    view.messageSignatures.clear();
    view.windowNodesByKey.clear();
    view.wrapperNodesByKey.clear();
    if (view.toolNodesByCallId) view.toolNodesByCallId.clear();
    if (view.ctxNodesByCallId) view.ctxNodesByCallId.clear();
    view.anchoredNodesByIndex.clear();
    view.descriptors = [];
    view.units = [];
    view.lastDescriptorCount = 0;
    view.renderedMessageIndices = new Set();
    view.windowStart = 0;
    view.windowEnd = 0;
    view.userPinnedToBottom = true;
    view.autoScrollPaused = false;
    view.lastScrollTop = 0;
    view.forceStickBottom = true;
    view.initialized = false;
    view.transcriptRoot = null;
    view.topSpacer = null;
    view.topSentinel = null;
    view.listRoot = null;
    view.bottomSentinel = null;
    view.bottomSpacer = null;
    view.floatingRoot = null;

    OSA.tmodelReset();

    const messagesDiv = document.getElementById('messages');
    if (messagesDiv) {
        messagesDiv.replaceChildren();
    }
    OSA.resetStreamingMessage();
};

OSA.renderEmptyTranscript = function(text = 'Start a new chat to begin') {
    OSA.resetTranscriptView();
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;
    const empty = document.createElement('div');
    empty.className = 'empty-state';
    empty.innerHTML = `<div class="empty-state-icon">+</div><div class="empty-state-title">Start a conversation</div><div class="empty-state-text">${OSA.escapeHtml(text)}</div>`;
    messagesDiv.appendChild(empty);
};

OSA.appendUserMessageToChat = function(content, options = {}) {
    const currentSession = OSA.getCurrentSession();
    const clientMessageId = options.clientMessageId || '';
    const attachments = options.attachments || options.images || [];
    let mirrorIndex = null;

    if (currentSession) {
        if (!Array.isArray(currentSession.messages)) currentSession.messages = [];
        const exists = currentSession.messages.some(message => {
            if (message.role !== 'user') return false;
            const existingClientId = message.metadata && message.metadata.client_message_id;
            return clientMessageId ? existingClientId === clientMessageId : message.content === content;
        });

        if (!exists) {
            currentSession.messages.push({
                role: 'user',
                content,
                thinking: null,
                timestamp: options.timestamp || new Date().toISOString(),
                tool_calls: null,
                tool_call_id: null,
                metadata: clientMessageId ? { client_message_id: clientMessageId, attachments: attachments.filter(att => att.kind !== 'image').map(att => ({ filename: att.filename, mime: att.mime, kind: att.kind || 'document', size_bytes: att.sizeBytes || 0, truncated: !!att.truncated })) } : { attachments: attachments.filter(att => att.kind !== 'image').map(att => ({ filename: att.filename, mime: att.mime, kind: att.kind || 'document', size_bytes: att.sizeBytes || 0, truncated: !!att.truncated })) },
                tokens: null,
                images: attachments.filter(att => att.kind === 'image' || (att.mime || '').startsWith('image/')).map(img => ({ filename: img.filename, mime: img.mime, preview_url: OSA.getAttachmentImageSrc(img) })),
            });
        }
        mirrorIndex = currentSession.messages.length - 1;
    }

    if (clientMessageId && OSA.tmodelGet('client:' + clientMessageId)) {
        return OSA.transcriptElementForItemKey('client:' + clientMessageId);
    }

    const messageShape = {
        role: 'user',
        content,
        thinking: null,
        timestamp: options.timestamp || new Date().toISOString(),
        metadata: {
            ...(clientMessageId ? { client_message_id: clientMessageId } : {}),
            attachments: attachments.filter(att => att.kind !== 'image').map(att => ({
                filename: att.filename,
                mime: att.mime,
                kind: att.kind || 'document',
                size_bytes: att.sizeBytes || 0,
                truncated: !!att.truncated,
            })),
        },
        images: attachments.filter(att => att.kind === 'image' || (att.mime || '').startsWith('image/')).map(img => ({ filename: img.filename, mime: img.mime, preview_url: OSA.getAttachmentImageSrc(img) })),
    };

    const item = OSA.tmodelAppend(OSA.tmodelMessageItem(
        clientMessageId ? 'client:' + clientMessageId : OSA.tmodelLiveKey('user'),
        messageShape,
        mirrorIndex,
        { live: true },
    ));
    // Sending always returns to the bottom: declare pin intent up front so
    // the render sticks even if another dirty reason coalesces over
    // 'user-message' before the frame fires.
    const pinView = OSA.getTranscriptView && OSA.getTranscriptView();
    if (pinView) {
        pinView.userPinnedToBottom = true;
        pinView.autoScrollPaused = false;
        pinView.forceStickBottom = true;
    }
    OSA.tmodelMarkDirty('user-message');
    return item ? OSA.transcriptElementForItemKey(item.key) : null;
};

OSA.transcriptElementForItemKey = function(key) {
    OSA.TModel.dirty = false;
    const reason = OSA.TModel.pendingReason;
    OSA.TModel.pendingReason = '';
    OSA.renderTranscript({ reason: reason });
    const view = OSA.getTranscriptView();
    const wrapper = view.wrapperNodesByKey.get(key);
    return wrapper ? wrapper.querySelector(':scope > .message') : null;
};

OSA.handleQueuedMessageDispatched = function(event) {
    const currentSession = OSA.getCurrentSession();
    if (currentSession) currentSession.task_status = 'running';
    OSA.setProcessing(true);
    OSA.setStopping(false);
    OSA.setSendButtonStopMode(true);
    const dispatchedId = event.queue_entry_id || '';
    const dispatchedClientId = event.client_message_id || '';
    const queue = (OSA.getSessionQueue() || []).filter(item => {
        if (dispatchedId && item.id === dispatchedId) return false;
        if (dispatchedClientId && item.client_message_id === dispatchedClientId) return false;
        return true;
    });
    OSA.setSessionQueue(queue);
    OSA.removeQueuedMessageElements();
    const dispatchedAttachments = [];
    if (Array.isArray(event.images)) {
        event.images.forEach(img => dispatchedAttachments.push({ ...img, kind: 'image' }));
    }
    if (Array.isArray(event.attachments)) {
        event.attachments.forEach(att => dispatchedAttachments.push(att));
    }
    OSA.appendUserMessageToChat(event.content || '', {
        clientMessageId: event.client_message_id || '',
        timestamp: event.timestamp,
        attachments: dispatchedAttachments,
    });
    OSA.renderQueuedMessages(queue);
};

OSA.renderQueuedMessages = function(queueItems) {
    OSA.removeQueuedMessageElements();

    const items = Array.isArray(queueItems) ? queueItems : [];
    OSA.updateFollowUpToggle(items);

    // A queued turn still owns the composer: drop the empty-state hint so the
    // panel is the visible state, matching the pre-panel behavior.
    if (items.length > 0) {
        const messagesDiv = document.getElementById('messages');
        const emptyState = messagesDiv ? messagesDiv.querySelector('.empty-state') : null;
        if (emptyState) emptyState.remove();
    }

    const panel = document.getElementById('queue-panel');
    if (!panel) {
        OSA.tmodelMarkDirty('queue');
        return;
    }
    if (items.length === 0) {
        panel.classList.add('hidden');
        OSA.tmodelMarkDirty('queue');
        return;
    }
    panel.classList.remove('hidden');

    const working = OSA.isAgentProcessing();
    let html = '';
    if (items.length > 3) {
        html += `<div class="queue-count">${items.length} queued</div>`;
    }
    html += '<div class="queue-list">';
    items.forEach(function(item, index) {
        const id = item.id || '';
        const isDispatching = item.status === 'dispatching';
        const isEditing = !!OSA.queueEditingId && OSA.queueEditingId === id;
        const text = item.content || '';
        const preview = text.length > 140 ? text.slice(0, 140) + '…' : text;
        const attCount = (Array.isArray(item.images) ? item.images.length : 0)
            + (Array.isArray(item.attachments) ? item.attachments.length : 0);
        const parked = !working && index === 0;
        const disabled = (isDispatching || OSA.queueBusy) ? ' disabled' : '';
        html += `<div class="queue-row${isEditing ? ' editing' : ''}${isDispatching ? ' dispatching' : ''}" data-queue-id="${OSA.escapeAttr(id)}">`
            + `<span class="queue-order" title="Position ${index + 1} of ${items.length}">${index + 1}</span>`
            + `<button type="button" class="queue-text" data-action="edit" title="${isEditing ? 'Editing — press Enter to confirm, Esc to cancel' : 'Edit queued message'}"${disabled}>${OSA.escapeHtml(preview) || '(empty)'}</button>`
            + (attCount ? `<span class="queue-atts" title="${attCount} attachment${attCount === 1 ? '' : 's'}">${attCount} file${attCount === 1 ? '' : 's'}</span>` : '')
            + `<span class="queue-actions${parked ? ' always' : ''}">`
            + (index > 0 && !isDispatching ? `<button type="button" class="queue-btn" data-action="up" title="Move up" aria-label="Move queued message up"${OSA.queueBusy ? ' disabled' : ''}>↑</button>` : '')
            + (index < items.length - 1 && !isDispatching ? `<button type="button" class="queue-btn" data-action="down" title="Move down" aria-label="Move queued message down"${OSA.queueBusy ? ' disabled' : ''}>↓</button>` : '')
            + (!isDispatching ? `<button type="button" class="queue-btn queue-steer" data-action="steer" title="Stop the current turn and send this now"${OSA.queueBusy ? ' disabled' : ''}>${parked ? 'Send' : 'Steer'}</button>` : '')
            + (!isDispatching ? `<button type="button" class="queue-btn queue-remove" data-action="remove" title="Remove from queue" aria-label="Remove queued message"${OSA.queueBusy ? ' disabled' : ''}>×</button>` : '')
            + `</span>`
            + `</div>`;
    });
    html += '</div>';
    panel.innerHTML = html;

    if (!panel.dataset.bound) {
        panel.dataset.bound = 'true';
        panel.addEventListener('click', function(event) {
            const btn = event.target.closest('[data-action]');
            if (!btn || btn.disabled) return;
            const row = event.target.closest('.queue-row');
            const queueId = row ? row.dataset.queueId : '';
            if (!queueId) return;
            const action = btn.dataset.action;
            if (action === 'edit') OSA.editQueuedMessage(queueId);
            else if (action === 'steer') OSA.steerQueuedMessage(queueId);
            else if (action === 'remove') OSA.removeQueuedMessage(queueId);
            else if (action === 'up') OSA.moveQueuedMessage(queueId, -1);
            else if (action === 'down') OSA.moveQueuedMessage(queueId, 1);
        });
    }

    OSA.tmodelMarkDirty('queue');
};

OSA.findQueuedItem = function(queueId) {
    return (OSA.getSessionQueue() || []).find(function(q) { return q && q.id === queueId; }) || null;
};

// Steer: interrupt the current turn (if any) and run this message next.
OSA.steerQueuedMessage = function(queueId) {
    const item = OSA.findQueuedItem(queueId);
    if (item) OSA.sendQueuedMessageNow(item);
};

OSA.removeQueuedMessage = async function(queueId) {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !queueId || OSA.queueBusy) return;
    if (OSA.queueEditingId === queueId) OSA.cancelQueueEdit();
    OSA.queueBusy = true;
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(currentSession.id)}/queue/${encodeURIComponent(queueId)}`, {
            method: 'DELETE',
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
    } catch (error) {
        console.error('Failed to remove queued message:', error);
        OSA.showErrorCard?.(error.message || 'Failed to remove queued message');
    } finally {
        OSA.queueBusy = false;
    }
    OSA.refreshCurrentSessionQueue?.();
};

OSA.moveQueuedMessage = async function(queueId, direction) {
    const currentSession = OSA.getCurrentSession();
    const queue = OSA.getSessionQueue() || [];
    if (!currentSession || !queueId || OSA.queueBusy) return;
    const index = queue.findIndex(function(q) { return q && q.id === queueId; });
    const target = index + direction;
    if (index < 0 || target < 0 || target >= queue.length) return;
    const ids = queue.map(function(q) { return q.id; });
    const moved = ids.splice(index, 1)[0];
    ids.splice(target, 0, moved);
    OSA.queueBusy = true;
    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(currentSession.id)}/queue/reorder`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ ids }),
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
    } catch (error) {
        console.error('Failed to reorder queue:', error);
        OSA.showErrorCard?.(error.message || 'Failed to reorder queue');
    } finally {
        OSA.queueBusy = false;
    }
    OSA.refreshCurrentSessionQueue?.();
};

// Edit: load the queued text into the composer, stashing the current draft.
// Enter confirms (PATCH), Escape restores the stash.
OSA.editQueuedMessage = function(queueId) {
    if (OSA.queueBusy) return;
    const item = OSA.findQueuedItem(queueId);
    if (!item || item.status === 'dispatching') return;
    const input = document.getElementById('message-input');
    if (!input) return;
    if (OSA.queueEditingId === queueId) {
        input.focus();
        return;
    }
    if (OSA.queueEditingId) OSA.cancelQueueEdit();
    OSA.queueEditingId = queueId;
    OSA.queueEditStash = { text: input.value, cursor: input.selectionStart || input.value.length };
    input.value = item.content || '';
    OSA.resizeMessageInput(input);
    input.focus();
    try { input.setSelectionRange(input.value.length, input.value.length); } catch (err) {}
    OSA.renderQueuedMessages(OSA.getSessionQueue());
};

OSA.cancelQueueEdit = function() {
    const input = document.getElementById('message-input');
    const stash = OSA.queueEditStash;
    OSA.queueEditingId = null;
    OSA.queueEditStash = null;
    if (input && stash) {
        input.value = stash.text || '';
        OSA.resizeMessageInput(input);
        input.focus();
        try { input.setSelectionRange(stash.cursor || 0, stash.cursor || 0); } catch (err) {}
    }
    OSA.renderQueuedMessages(OSA.getSessionQueue());
};

OSA.confirmQueueEdit = async function() {
    const queueId = OSA.queueEditingId;
    const currentSession = OSA.getCurrentSession();
    const input = document.getElementById('message-input');
    if (!queueId || !currentSession || !input) {
        OSA.queueEditingId = null;
        return;
    }
    const text = input.value.trim();
    if (!text) {
        OSA.cancelQueueEdit();
        return;
    }
    OSA.queueBusy = true;
    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(currentSession.id)}/queue/${encodeURIComponent(queueId)}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ content: text }),
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        const stash = OSA.queueEditStash;
        OSA.queueEditingId = null;
        OSA.queueEditStash = null;
        input.value = (stash && stash.text) || '';
        OSA.resizeMessageInput(input);
        input.focus();
    } catch (error) {
        console.error('Failed to edit queued message:', error);
        OSA.showErrorCard?.(error.message || 'Failed to edit queued message');
    } finally {
        OSA.queueBusy = false;
    }
    OSA.refreshCurrentSessionQueue?.();
};

// Interrupt the current turn and run this queued message now.
OSA.sendQueuedMessageNow = async function(item) {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !item || !item.id) return;
    if (OSA._sendNowInFlight) return;
    OSA._sendNowInFlight = true;
    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(currentSession.id)}/queue/${encodeURIComponent(item.id)}/send-now`, {
            method: 'POST',
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) {
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        // The promoted item stays in the queue until the server dispatches it
        // (and emits queued_message_dispatched). Mirror the promotion locally
        // so the chosen message visibly jumps to the front of the list.
        const queue = (OSA.getSessionQueue() || []).slice();
        const idx = queue.findIndex(q => q.id === item.id);
        if (idx > 0) {
            const [promoted] = queue.splice(idx, 1);
            queue.unshift(promoted);
            queue.forEach((q, i) => { if (q.position !== undefined) q.position = i + 1; });
            OSA.setSessionQueue(queue);
        }
        // Optimistically mark the agent as processing; the
        // cancelled-current-turn event resets the UI.
        OSA.setProcessing(true);
        OSA.setStopping(false);
        OSA.setSendButtonStopMode(true);
        if (currentSession) currentSession.task_status = 'running';
        OSA.showThinkingIndicator();
        OSA.renderQueuedMessages(OSA.getSessionQueue());
    } catch (error) {
        console.error('Failed to send queued message now:', error);
        OSA.showErrorCard?.(error.message || 'Failed to send queued message');
        OSA.refreshCurrentSessionQueue?.();
    } finally {
        OSA._sendNowInFlight = false;
    }
};

OSA.updateTodoDock = function() {
    const dock = document.getElementById('todo-dock');
    if (!dock) return;
    const todos = OSA.getSessionTodos() || [];
    const completed = todos.filter(t => t.status === 'completed');
    const total = todos.length;
    const active = todos.find(t => t.status === 'in_progress')
        || todos.find(t => t.status === 'pending')
        || [...todos].reverse().find(t => t.status === 'completed' || t.status === 'cancelled')
        || todos[0];

    if (total === 0) {
        dock.classList.add('hidden');
        return;
    }

    dock.classList.remove('hidden');

    const counterEl = dock.querySelector('.dock-counter');
    if (counterEl) {
        counterEl.textContent = `${completed.length} of ${total} todos completed`;
    }

    const activeEl = dock.querySelector('.dock-active-task');
    if (activeEl) {
        activeEl.textContent = active?.content || (completed.length === total ? 'All tasks completed' : 'No active task');
    }

    const chevron = dock.querySelector('.dock-chevron');
    if (chevron) {
        chevron.style.transform = OSA.getTodoDockExpanded() ? 'rotate(180deg)' : 'rotate(0deg)';
    }

    OSA.renderTodoDockList(dock, todos);
};

OSA.toggleTodoDock = function() {
    OSA.setTodoDockExpanded(!OSA.getTodoDockExpanded());
    const dock = document.getElementById('todo-dock');
    if (dock) OSA.renderTodoDockList(dock, OSA.getSessionTodos() || []);
};

OSA.renderTodoDockList = function(dock, todos) {
    const list = dock.querySelector('.dock-list');
    if (!list) return;

    if (!OSA.getTodoDockExpanded()) {
        list.classList.add('hidden');
        return;
    }

    list.classList.remove('hidden');
    const order = { in_progress: 0, pending: 1, completed: 2, cancelled: 3 };
    const sorted = [...todos].sort((a, b) => {
        const left = order[(a.status || 'pending').toLowerCase()] ?? 99;
        const right = order[(b.status || 'pending').toLowerCase()] ?? 99;
        if (left !== right) return left - right;
        return (a.position ?? 0) - (b.position ?? 0);
    });

    list.innerHTML = sorted.map(t => {
        const status = (t.status || 'pending').toLowerCase();
        const done = status === 'completed' || status === 'cancelled';
        const marker = status === 'in_progress'
            ? '<span class="dock-item-pulse"></span>'
            : `<span class="dock-item-check">${done ? '&#10003;' : ''}</span>`;
        return `<div class="dock-item ${status}"><span class="dock-item-marker">${marker}</span><span class="dock-item-text">${OSA.escapeHtml(t.content || '')}</span></div>`;
    }).join('');
};

window.copyAssistantMessage = OSA.copyAssistantMessage;

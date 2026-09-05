window.OSA = window.OSA || {};

OSA.isHiddenSyntheticMessage = function(message) {
    if (!message || !message.metadata) return false;
    if (!message.metadata.synthetic) return false;

    const syntheticKind = message.metadata.kind || '';
    if (message.role === 'assistant' && syntheticKind === 'tool_prelude') {
        const hasContent = !!(message.content || '').trim();
        const hasVisibleThinking = OSA.getShowThinkingBlocks && OSA.getShowThinkingBlocks()
            ? !!(message.thinking || '').trim()
            : false;
        const hasToolCalls = Array.isArray(message.tool_calls) && message.tool_calls.length > 0;
        if (hasContent || hasVisibleThinking || hasToolCalls) {
            return false;
        }
    }

    return true;
};

OSA.showThinkingIndicator = function() {
    const messagesDiv = document.getElementById('messages');
    const existing = document.getElementById('thinking-indicator');
    if (existing) existing.remove();

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

OSA.resetMessageChain = function() {
    const eventSessionId = OSA.messageChain?.eventSessionId || null;
    OSA.messageChain = {
        lastEventType: null,
        lastAssistantDomId: null,
        pendingToolCallIds: [],
        eventSessionId,
        eventSeqNumber: 0,
        lastThinkingEndSeq: 0,
        lastToolStartSeq: 0,
    };
};

OSA.ensureCurrentSessionAssistantMessage = function(forceNew = false) {
    const session = OSA.getCurrentSession();
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
    return next;
};

OSA.appendCurrentSessionAssistantThinking = function(content) {
    if (!content) return;
    const message = OSA.ensureCurrentSessionAssistantMessage();
    if (!message) return;
    const current = message.thinking || '';
    if (content.length >= 4 && current.endsWith(content)) return;
    message.thinking = current + content;
};

OSA.appendCurrentSessionAssistantContent = function(content) {
    if (!content) return;
    const message = OSA.ensureCurrentSessionAssistantMessage();
    if (!message) return;
    const current = message.content || '';
    if (content.length >= 4 && current.endsWith(content)) return;
    message.content = current + content;
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
    const session = OSA.getCurrentSession();
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

OSA.showErrorCard = function(errorMsg) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) emptyState.remove();

    const truncated = errorMsg.length > 120 ? errorMsg.slice(0, 120) + '...' : errorMsg;
    const card = document.createElement('div');
    card.className = 'error-card';
    card.innerHTML = `
        <div class="error-card-icon">!</div>
        <div class="error-card-body">
            <div class="error-card-title">Something went wrong</div>
            <div class="error-card-message" title="${OSA.escapeHtml(errorMsg)}">${OSA.escapeHtml(truncated)}</div>
        </div>
        <button class="error-card-retry" onclick="this.closest('.error-card').remove()">Dismiss</button>
    `;
    OSA.mountFloatingNode(card);
    OSA.tmodelMarkDirty('error-card');
};

OSA.formatInlineMarkdown = function(line) {
    let s = line
        .replace(/\[([^\]]+)\]\(([^)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/\*([^*]+)\*/g, '<em>$1</em>')
        .replace(/`([^`]+)`/g, '<code>$1</code>');
    s = s.replace(/(^|[^"=])(https?:\/\/[^\s<>"')\]]+)/g, '$1<a href="$2" target="_blank" rel="noopener">$2</a>');
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
        tail: null,
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
    const table = document.createElement('table');
    (md.tableRows || []).forEach((row, i) => {
        const tr = document.createElement('tr');
        const tag = (i === 0 && md.tableHeader) ? 'th' : 'td';
        row.forEach(cell => {
            const cellEl = document.createElement(tag);
            cellEl.innerHTML = OSA.formatInlineMarkdown(OSA.escapeHtml(cell.trim()));
            tr.appendChild(cellEl);
        });
        table.appendChild(tr);
    });
    md.tableRows = null;
    md.tableHeader = false;
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
            el.appendChild(md.listEl);
            md.listEl = null;
        }
        if (!md.tableRows) md.tableRows = [];
        md.tableRows.push(OSA.mdParseTableCells(trimmed));
        return;
    }

    if (md.tableRows) {
        el.appendChild(OSA.mdBuildTable(md));
    }

    if (OSA.mdIsHeader(trimmed)) {
        if (md.listEl) {
            el.appendChild(md.listEl);
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
            el.appendChild(md.listEl);
            md.listEl = null;
        }
        OSA.mdOpenCodeBlock(md, (trimmed.match(/^```(\w+)?$/) || ['', ''])[1], el);
        return;
    }

    if (trimmed.startsWith('- ')) {
        if (!md.listEl) md.listEl = document.createElement('ul');
        md.listEl.appendChild(OSA.mdBuildListItem(trimmed.slice(2)));
        return;
    }

    const numberedMatch = trimmed.match(/^(\d+)\.\s+(.*)/);
    if (numberedMatch) {
        if (!md.listEl) md.listEl = document.createElement('ul');
        md.listEl.appendChild(OSA.mdBuildListItem(numberedMatch[2]));
        return;
    }

    if (md.listEl) {
        el.appendChild(md.listEl);
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
    if (rawText.length < md.renderedLen) {
        el.innerHTML = '';
        el._md = OSA.createIncrementalMd();
        md = el._md;
    }
    const delta = rawText.slice(md.renderedLen);
    if (!delta) {
        el.dataset.renderedText = rawText;
        return;
    }
    const lastNl = delta.lastIndexOf('\n');
    const complete = lastNl >= 0 ? delta.slice(0, lastNl + 1) : '';
    const partial = lastNl >= 0 ? delta.slice(lastNl + 1) : delta;

    if (md.tail) {
        md.tail.remove();
        md.tail = null;
    }

    if (complete) {
        const lines = complete.split('\n');
        lines.pop();
        for (const line of lines) {
            OSA.mdAppendLine(md, line, el);
        }
    }

    if (partial && md.codeLang === null) {
        const tail = document.createElement('span');
        tail.className = 'md-partial';
        tail.innerHTML = OSA.formatInlineMarkdown(OSA.escapeHtml(partial));
        el.appendChild(tail);
        md.tail = tail;
    }

    md.renderedLen += complete.length;
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
        OSA.mdAppendLine(md, partial, el);
    }
    if (md.tail) {
        md.tail.remove();
        md.tail = null;
    }
    if (md.listEl) {
        el.appendChild(md.listEl);
        md.listEl = null;
    }
    if (md.tableRows) {
        el.appendChild(OSA.mdBuildTable(md));
    }
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
    const langKeywords = keywords[lang.toLowerCase()] || [];
    let highlighted = OSA.escapeHtml(code);
    if (langKeywords.length > 0) {
        const keywordRegex = new RegExp(`\\b(${langKeywords.join('|')})\\b`, 'g');
        highlighted = highlighted.replace(keywordRegex, '<span class="token-keyword">$1</span>');
    }
    highlighted = highlighted
        .replace(/(\/\/.*$)/gm, '<span class="token-comment">$1</span>')
        .replace(/(#.*$)/gm, '<span class="token-comment">$1</span>')
        .replace(/("[^"]*")/g, '<span class="token-string">$1</span>')
        .replace(/('[^']*')/g, '<span class="token-string">$1</span>')
        .replace(/(\b\d+\b)/g, '<span class="token-number">$1</span>');
    return highlighted;
};

OSA.copyCode = function(btn) {
    const code = btn.closest('.code-block').querySelector('code').textContent;
    navigator.clipboard.writeText(code).then(() => {
        btn.textContent = 'Copied!';
        setTimeout(() => btn.textContent = 'Copy', 2000);
    });
};

OSA.removeQueuedMessageElements = function() {
    const floatingRoot = OSA.getTranscriptView().floatingRoot;
    if (!floatingRoot) return;
    floatingRoot.querySelectorAll('.queued-notice').forEach(el => el.remove());
};

OSA.renderAttachmentMarkup = function(attachments = []) {
    const imageAttachments = attachments.filter(att => att.kind === 'image' || (att.mime || '').startsWith('image/'));
    const fileAttachments = attachments.filter(att => !(att.kind === 'image' || (att.mime || '').startsWith('image/')));

    let html = '';
    if (imageAttachments.length > 0) {
        html += '<div class="message-image-grid">';
        imageAttachments.forEach(att => {
            const src = OSA.getAttachmentImageSrc(att);
            html += `<div class="message-image-thumb"><img class="expandable-image" data-image-src="${src}" src="${src}" alt="${OSA.escapeHtml(att.filename || '')}" /></div>`;
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

OSA.renderEmptyTranscript = function(text = 'Click "New chat" to begin') {
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
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;
    const floatingRoot = OSA.getFloatingRoot();
    if (!floatingRoot) return;

    OSA.removeQueuedMessageElements();

    const items = Array.isArray(queueItems) ? queueItems : [];
    if (items.length === 0) {
        OSA.tmodelMarkDirty('queue');
        return;
    }

    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) emptyState.remove();

    items.forEach((item, index) => {
        const message = document.createElement('div');
        const isDispatching = item.status === 'dispatching';
        message.className = `queued-notice${isDispatching ? ' dispatching' : ''}`;
        if (item.id) message.dataset.queueId = item.id;
        const label = isDispatching ? 'Sending next' : `Queued ${index + 1}`;
        const preview = (item.content || '').slice(0, 80) + ((item.content || '').length > 80 ? '…' : '');
        const timeHtml = `<span class="queued-notice-time">${OSA.escapeHtml(OSA.formatRelativeDateTime(item.created_at))}</span>`;
        // Send-now interrupts the current turn (if any) and runs this queued
        // message next. Hidden on the item that is already being dispatched.
        const sendNowHtml = isDispatching
            ? ''
            : `<button type="button" class="queued-notice-send-now" data-queue-id="${OSA.escapeHtml(item.id || '')}" data-queue-client-id="${OSA.escapeHtml(item.client_message_id || '')}" title="Stop the current turn and send this now" aria-label="Send this queued message now">Send now</button>`;
        message.innerHTML = `<span class="queued-notice-label">${label}</span><span class="queued-notice-text">${OSA.escapeHtml(preview)}</span>${sendNowHtml}${timeHtml}`;
        floatingRoot.appendChild(message);
    });

    floatingRoot.querySelectorAll('.queued-notice-send-now').forEach(btn => {
        btn.addEventListener('click', () => {
            const queueId = btn.dataset.queueId;
            const clientId = btn.dataset.queueClientId;
            const item = (OSA.getSessionQueue() || []).find(q =>
                (queueId && q.id === queueId) || (!queueId && clientId && q.client_message_id === clientId)
            );
            if (item) OSA.sendQueuedMessageNow(item);
        });
    });

    OSA.tmodelMarkDirty('queue');
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

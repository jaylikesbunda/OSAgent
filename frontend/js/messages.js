window.OSA = window.OSA || {};

OSA.isHiddenSyntheticMessage = function(message) {
    if (!message || !message.metadata) return false;
    return !!message.metadata.synthetic;
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
    messagesDiv.scrollTop = messagesDiv.scrollHeight;

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
    OSA.getPendingFormattedElements().clear();
};

OSA.scheduleFormattedRender = function(element, rawText) {
    if (!element) return;
    element.dataset.rawText = rawText;
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
            el.innerHTML = OSA.formatMessage(el.dataset.rawText || '');
        });
    }));
};

OSA.getStreamingAssistantMessage = function() {
    const domId = OSA.getStreamingAssistantDomId();
    if (!domId) return null;
    return document.getElementById(domId);
};

OSA.transcriptHasBlockingSiblingAfter = function(element) {
    if (!element) return false;
    const wrapper = element.closest('.transcript-entry');
    const slot = wrapper ? wrapper.querySelector(':scope > .transcript-entry-extras') : null;
    return !!(slot && slot.children.length > 0);
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

OSA.ensureThinkingContainer = function(message) {
    if (!message) return null;
    let container = message.querySelector('.message-thinking');
    if (!container) {
        container = document.createElement('div');
        container.className = 'message-thinking expanded streaming';
        container.innerHTML = `
            <button type="button" class="thinking-toggle" onclick="OSA.toggleThinkingBlock(this)">
                <span class="thinking-toggle-label">Thinking</span>
                <span class="thinking-preview"></span>
            </button>
            <div class="thinking-body"></div>
        `;
        const contentEl = message.querySelector('.message-content');
        message.insertBefore(container, contentEl);
    }
    return container;
};

OSA.setThinkingPreview = function(container, text) {
    if (!container) return;
    const previewEl = container.querySelector('.thinking-preview');
    if (!previewEl) return;
    const preview = OSA.getThinkingPreview(text);
    previewEl.textContent = preview;
    previewEl.style.display = preview ? '' : 'none';
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
    message.thinking = (message.thinking || '') + content;
};

OSA.appendCurrentSessionAssistantContent = function(content) {
    if (!content) return;
    const message = OSA.ensureCurrentSessionAssistantMessage();
    if (!message) return;
    message.content = (message.content || '') + content;
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

OSA.finalizeAssistantSegmentForToolCall = function(event) {
    OSA.completeThinkingDisplay();
    OSA.pruneEmptyStreamingMessage();
    OSA.markStreamingBoundary();
    OSA.insertCurrentSessionToolBoundary(event);
};

OSA.markStreamingBoundary = function() {
    const chain = OSA.getMessageChain();
    const domId = OSA.getStreamingAssistantDomId();
    if (domId) {
        chain.lastAssistantDomId = domId;
        const message = document.getElementById(domId);
        if (message) {
            message.classList.remove('streaming');
            const thinking = message.querySelector('.message-thinking');
            if (thinking) {
                thinking.classList.remove('streaming');
                if (!thinking.dataset.userToggled) {
                    thinking.classList.remove('expanded');
                }
                delete thinking.dataset.userToggled;
            }
            message.dataset.boundaryAfter = 'tool';
        }
    }
    OSA.clearPendingFormattedRenders();
    OSA.setStreamingAssistantDomId(null);
};

OSA.prepareAssistantMessageElementForStreaming = function(messageEl, sourceMessage, expandThinking = false) {
    if (!messageEl) return null;
    if (!messageEl.id) {
        messageEl.id = `assistant-stream-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
    }
    messageEl.classList.add('streaming');

    const contentEl = messageEl.querySelector('.message-content');
    if (contentEl && !contentEl.dataset.rawText) {
        contentEl.dataset.rawText = sourceMessage?.content || '';
    }

    const thinkingEl = messageEl.querySelector('.thinking-body');
    if (thinkingEl && !thinkingEl.dataset.rawText) {
        thinkingEl.dataset.rawText = sourceMessage?.thinking || '';
    }

    const thinkingWrap = messageEl.querySelector('.message-thinking');
    if (thinkingWrap && OSA.getShowThinkingBlocks()) {
        thinkingWrap.classList.add('streaming');
        if (!thinkingWrap.dataset.userToggled) {
            if (expandThinking && !!(sourceMessage?.thinking || '').trim()) {
                thinkingWrap.classList.add('expanded');
            }
        }
        OSA.setThinkingPreview(thinkingWrap, sourceMessage?.thinking || '');
    }

    OSA.setStreamingAssistantDomId(messageEl.id);
    return messageEl;
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

OSA.adoptStreamingAssistantFromRenderedSession = function(session) {
    if (!session || session.task_status !== 'running' || !Array.isArray(session.messages)) {
        return null;
    }

    const assistant = OSA.getActiveTurnAssistantMessage(session);
    if (!assistant) return null;

    const candidates = Array.from(document.querySelectorAll('#messages .message.assistant'));
    const messageEl = candidates.at(-1);
    if (!messageEl) return null;

    return OSA.prepareAssistantMessageElementForStreaming(messageEl, assistant, OSA.getShowThinkingBlocks());
};

OSA.resetStreamingMessage = function() {
    OSA.clearPendingFormattedRenders();
    OSA.setStreamingAssistantDomId(null);
};

OSA.resetMessageChain = function() {
    OSA.messageChain = {
        lastEventType: null,
        lastAssistantDomId: null,
        pendingToolCallIds: [],
        eventSeqNumber: 0,
        lastThinkingEndSeq: 0,
        lastToolStartSeq: 0,
    };
};

OSA.releaseStreamingAssistantMessage = function() {
    const domId = OSA.getStreamingAssistantDomId();
    if (!domId) return;
    const message = document.getElementById(domId);
    if (!message) {
        OSA.resetStreamingMessage();
        return;
    }

    message.classList.remove('streaming');
    const thinking = message.querySelector('.message-thinking');
    if (thinking) {
        thinking.classList.remove('streaming');
        if (!thinking.dataset.userToggled) {
            thinking.classList.remove('expanded');
        }
        delete thinking.dataset.userToggled;
    }

    const chain = OSA.getMessageChain();
    chain.lastAssistantDomId = domId;

    OSA.resetStreamingMessage();
};

OSA.commitStreamingAssistantSegment = function() {
    const domId = OSA.getStreamingAssistantDomId();
    if (!domId) return;

    const message = document.getElementById(domId);
    if (!message) {
        OSA.resetStreamingMessage();
        return;
    }

    message.classList.remove('streaming');
    OSA.completeThinkingDisplay();

    const contentEl = message.querySelector('.message-content');
    const rawText = contentEl ? (contentEl.dataset.rawText || '') : '';
    const thinkingEl = message.querySelector('.thinking-body');
    const thinkingText = thinkingEl ? (thinkingEl.dataset.rawText || '') : '';
    if (!rawText && !thinkingText) {
        message.remove();
        const chain = OSA.getMessageChain();
        if (chain.lastAssistantDomId === domId) {
            chain.lastAssistantDomId = null;
        }
        OSA.resetStreamingMessage();
        return;
    }

    const session = OSA.getCurrentSession();
    const sourceMessage = OSA.getActiveTurnAssistantMessage(session);
    OSA.updateAssistantMessageActions(message, sourceMessage);

    const chain = OSA.getMessageChain();
    chain.lastAssistantDomId = domId;

    OSA.resetStreamingMessage();
};

OSA.describeCheckpointForUi = function(checkpoint) {
    const timeLabel = checkpoint?.created_at
        ? OSA.formatRelativeDateTime(checkpoint.created_at)
        : 'unknown time';
    const toolLabel = checkpoint?.tool_name ? ` via ${checkpoint.tool_name}` : '';
    return `${timeLabel}${toolLabel}`;
};

OSA.findNearestCheckpointForMessage = function(messageTimestamp) {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !currentSession.id || typeof OSA.getSessionCheckpoints !== 'function') return null;

    const messageTsMs = OSA.timestampToMs(messageTimestamp);
    if (messageTsMs === null) return null;

    const checkpoints = OSA.getSessionCheckpoints(currentSession.id);
    for (const checkpoint of checkpoints) {
        const checkpointTs = OSA.timestampToMs(checkpoint?.created_at);
        if (checkpointTs === null) continue;
        if (checkpointTs <= messageTsMs) {
            return checkpoint;
        }
    }

    return null;
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

    return html;
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

    const sourceTimestamp = sourceMessage?.timestamp || messageEl.dataset.messageTimestamp || '';
    if (sourceTimestamp) {
        messageEl.dataset.messageTimestamp = sourceTimestamp;
    }

    const checkpoint = OSA.findNearestCheckpointForMessage(sourceTimestamp);
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
    try {
        if (checkpoint) {
            plan = await OSA.fetchRestorePlan(sessionId, checkpoint);
        }
    } catch (error) {
        console.warn('Failed to fetch restore plan:', error);
    }

    const snapshotCount = plan.count || 0;
    const confirmMessage = snapshotCount > 0
        ? `Restore this session to ${checkpointLabel}? This will replace session state and revert ${snapshotCount} OSA file snapshot${snapshotCount === 1 ? '' : 's'} captured after that point.`
        : `Restore this session to ${checkpointLabel}? This will replace the current session state. No matching OSA file snapshots were found to revert.`;
    const confirmed = confirm(confirmMessage);
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

OSA.createAssistantMessageShell = function() {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

    const session = OSA.getCurrentSession();
    OSA.syncRenderedMessages((session && session.messages) || [], {
        resetStreaming: false,
        stickToBottom: true,
        preferTail: true,
    });

    const candidates = Array.from(document.querySelectorAll('#messages .message.assistant'));
    const message = candidates.at(-1) || null;
    if (!message) return null;

    const domId = message.id || `assistant-stream-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
    message.id = domId;
    OSA.setStreamingAssistantDomId(domId);
    const chain = OSA.getMessageChain();
    chain.lastAssistantDomId = domId;
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    return message;
};

OSA.ensureStreamingAssistantMessage = function() {
    const existingId = OSA.getStreamingAssistantDomId();
    if (existingId) {
        const existing = document.getElementById(existingId);
        if (existing) {
            if (!OSA.transcriptHasBlockingSiblingAfter(existing)) {
                return existing;
            }

            existing.classList.remove('streaming');
            const thinking = existing.querySelector('.message-thinking');
            if (thinking) {
                thinking.classList.remove('streaming');
                thinking.classList.remove('expanded');
            }
            OSA.setStreamingAssistantDomId(null);
        }
    }

    const chain = OSA.getMessageChain();
    if (chain.lastAssistantDomId && !existingId) {
        const lastMsg = document.getElementById(chain.lastAssistantDomId);
        if (lastMsg && lastMsg.isConnected && !lastMsg.classList.contains('streaming')) {
            if (OSA.transcriptHasBlockingSiblingAfter(lastMsg)) {
                chain.lastAssistantDomId = null;
            } else {
                const session = OSA.getCurrentSession();
                const sourceMsg = OSA.getActiveTurnAssistantMessage(session);
                const restored = OSA.prepareAssistantMessageElementForStreaming(lastMsg, sourceMsg, OSA.getShowThinkingBlocks());
                if (restored) return restored;
            }
        }
    }

    return OSA.createAssistantMessageShell();
};

OSA.beginAssistantResponse = function() {
    OSA.ensureCurrentSessionAssistantMessage();
    OSA.hideThinkingIndicator();
    return OSA.ensureStreamingAssistantMessage();
};

OSA.beginThinkingDisplay = function() {
    if (!OSA.getShowThinkingBlocks()) return null;

    const chain = OSA.getMessageChain();
    const currentMessage = OSA.getStreamingAssistantMessage();
    const currentContent = currentMessage
        ? ((currentMessage.querySelector('.message-content')?.dataset.rawText) || '').trim()
        : '';
    const session = OSA.getCurrentSession();
    const last = session && Array.isArray(session.messages)
        ? session.messages[session.messages.length - 1]
        : null;
    const shouldStartNewSegment = !!(
        currentContent
        || (last && last.role === 'assistant' && !OSA.isHiddenSyntheticMessage(last) && (last.content || '').trim())
    );

    if (shouldStartNewSegment) {
        OSA.commitStreamingAssistantSegment();
    }

    OSA.ensureCurrentSessionAssistantMessage(shouldStartNewSegment);
    OSA.hideThinkingIndicator();

    let message = null;
    if (!shouldStartNewSegment && chain.lastAssistantDomId) {
        const lastMsg = document.getElementById(chain.lastAssistantDomId);
        if (lastMsg && lastMsg.isConnected) {
            const sourceMsg = OSA.getActiveTurnAssistantMessage(session);
            message = OSA.prepareAssistantMessageElementForStreaming(lastMsg, sourceMsg, true);
        }
    }
    if (!message) {
        message = OSA.ensureStreamingAssistantMessage();
    }

    const existingContainer = message ? message.querySelector('.message-thinking') : null;
    const container = OSA.ensureThinkingContainer(message);
    if (!container) return null;
    container.classList.add('streaming');
    if (!existingContainer || !container.dataset.userToggled) {
        container.classList.add('expanded');
    }
    OSA.setThinkingPreview(container, '');
    return container;
};

OSA.appendThinkingChunk = function(content) {
    if (!content) return;
    OSA.appendCurrentSessionAssistantThinking(content);
    if (!OSA.getShowThinkingBlocks()) return;
    const message = OSA.ensureStreamingAssistantMessage();
    if (!message) return;
    const container = OSA.ensureThinkingContainer(message);
    const body = container ? container.querySelector('.thinking-body') : null;
    if (!body) return;

    const nextText = (body.dataset.rawText || '') + content;
    OSA.scheduleFormattedRender(body, nextText);
    OSA.setThinkingPreview(container, nextText);

    const messagesDiv = document.getElementById('messages');
    if (messagesDiv) {
        const nearBottom = messagesDiv.scrollHeight - messagesDiv.scrollTop - messagesDiv.clientHeight < 140;
        if (nearBottom) {
            messagesDiv.scrollTop = messagesDiv.scrollHeight;
        }
    }
};

OSA.completeThinkingDisplay = function() {
    if (!OSA.getShowThinkingBlocks()) return;
    const message = OSA.getStreamingAssistantMessage();
    if (!message) return;
    const container = message.querySelector('.message-thinking');
    if (!container) return;
    container.classList.remove('streaming');
    const body = container.querySelector('.thinking-body');
    const rawText = body ? (body.dataset.rawText || '').trim() : '';
    if (rawText) {
        OSA.setThinkingPreview(container, rawText);
        if (!container.dataset.userToggled) {
            container.classList.remove('expanded');
        }
    }
};

OSA.appendAssistantChunk = function(content) {
    if (!content) return;
    OSA.appendCurrentSessionAssistantContent(content);
    const message = OSA.ensureStreamingAssistantMessage();
    if (!message) return;
    const contentEl = message.querySelector('.message-content');
    const nextText = (contentEl.dataset.rawText || '') + content;
    OSA.scheduleFormattedRender(contentEl, nextText);

    const messagesDiv = document.getElementById('messages');
    if (messagesDiv) {
        const nearBottom = messagesDiv.scrollHeight - messagesDiv.scrollTop - messagesDiv.clientHeight < 140;
        if (nearBottom) {
            messagesDiv.scrollTop = messagesDiv.scrollHeight;
        }
    }
};

OSA.completeAssistantResponse = function(usage) {
    const domId = OSA.getStreamingAssistantDomId();
    if (!domId) return;
    const message = document.getElementById(domId);
    if (message) {
        message.classList.remove('streaming');
        OSA.completeThinkingDisplay();
        const contentEl = message.querySelector('.message-content');
        const rawText = contentEl ? (contentEl.dataset.rawText || '') : '';
        const thinkingEl = message.querySelector('.thinking-body');
        const thinkingText = thinkingEl ? (thinkingEl.dataset.rawText || '') : '';
        if (!rawText && !thinkingText) {
            message.remove();
            OSA.setTurnStartTime(null);
            OSA.resetStreamingMessage();
            OSA.updateTodoDock();
            return;
        }
        const session = OSA.getCurrentSession();
        const sourceMessage = OSA.getActiveTurnAssistantMessage(session);
        OSA.updateAssistantMessageActions(message, sourceMessage);
        const actionsEl = message.querySelector('.message-actions');

        const startTime = OSA.getTurnStartTime();
        if (startTime) {
            const elapsedMs = Date.now() - startTime;
            const elapsedSec = elapsedMs / 1000;
            const durationEl = message.querySelector('.turn-duration');
            if (durationEl) {
                const elapsed = Math.round(elapsedSec);
                durationEl.textContent = elapsed < 60 ? `${elapsed}s` : `${Math.floor(elapsed / 60)}m ${elapsed % 60}s`;
            }

            if (usage && usage.output > 0 && elapsedSec > 0) {
                const tps = (usage.output / elapsedSec).toFixed(1);
                let tpsEl = message.querySelector('.turn-tokens');
                if (!tpsEl) {
                    tpsEl = document.createElement('span');
                    tpsEl.className = 'turn-tokens';
                    const restoreBtn = actionsEl ? actionsEl.querySelector('.msg-action-restore') : null;
                    const copyBtn = actionsEl ? actionsEl.querySelector('.msg-action-copy') : null;
                    if (restoreBtn) {
                        restoreBtn.after(tpsEl);
                    } else if (copyBtn) {
                        copyBtn.after(tpsEl);
                    } else if (durationEl) {
                        durationEl.after(tpsEl);
                    } else {
                        actionsEl.prepend(tpsEl);
                    }
                }
                tpsEl.textContent = `${tps} tok/s`;
                tpsEl.title = `${usage.total} total tokens`;
            }
        }

        if (rawText && startTime && OSA.getTtsEnabled() && OSA.getVoiceConfig()?.enabled) {
            const activePersona = OSA.getActivePersona();
            const isRoleplay = activePersona?.id === 'custom';
            const speechText = OSA.prepareSpeechText(rawText, isRoleplay);
            if (speechText) {
                OSA.speakText(speechText);
            }
        }
    }
    OSA.setTurnStartTime(null);
    OSA.resetStreamingMessage();
    OSA.updateTodoDock();
    const currentSession = OSA.getCurrentSession();
    if (currentSession && currentSession.id && typeof OSA.loadSessionCheckpoints === 'function') {
        OSA.loadSessionCheckpoints(currentSession.id, { silent: true });
    }
};

OSA.pruneEmptyStreamingMessage = function() {
    const domId = OSA.getStreamingAssistantDomId();
    if (!domId) return;
    const message = document.getElementById(domId);
    if (!message) {
        OSA.resetStreamingMessage();
        return;
    }
    const contentEl = message.querySelector('.message-content');
    const rawText = contentEl ? (contentEl.dataset.rawText || '').trim() : '';
    const thinkingEl = message.querySelector('.thinking-body');
    const thinkingText = thinkingEl ? (thinkingEl.dataset.rawText || '').trim() : '';
    if (!rawText && !thinkingText) {
        message.remove();
        const chain = OSA.getMessageChain();
        if (chain.lastAssistantDomId === domId) {
            chain.lastAssistantDomId = null;
        }
        OSA.resetStreamingMessage();
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
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
};

OSA.formatMessage = function(text) {
    const escaped = OSA.escapeHtml((text || '').replace(/\n+$/, ''));
    const lines = escaped.split('\n');
    let html = '';
    let listItems = [];
    let codeBlock = null;
    let codeLines = [];

    const formatInlineMarkdown = (line) => line
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/\*([^*]+)\*/g, '<em>$1</em>')
        .replace(/`([^`]+)`/g, '<code>$1</code>');

    const flushList = () => {
        if (listItems.length) {
            html += `<ul>${listItems.join('')}</ul>`;
            listItems = [];
        }
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
        flushList();
        if (trimmed.length === 0) { html += '<br>'; } else { html += `<p>${formatInlineMarkdown(line)}</p>`; }
    }

    flushList();
    flushCodeBlock();
    return html;
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
            const src = att.dataUrl || att.data_url || '';
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
        view.ioTop = new IntersectionObserver(entries => {
            if (view.isRendering || view.shiftInProgress) return;
            if (!view.descriptors || view.descriptors.length <= view.maxWindowSize) return;
            if ((Date.now() - view.lastShiftAt) < 80) return;
            if (entries.some(entry => entry.isIntersecting)) {
                OSA.shiftTranscriptWindow(-1);
            }
        }, { root: messagesDiv, threshold: 0.01, rootMargin: '220px 0px 0px 0px' });
    }

    if (!view.ioBottom) {
        view.ioBottom = new IntersectionObserver(entries => {
            if (view.isRendering || view.shiftInProgress) return;
            if (!view.descriptors || view.descriptors.length <= view.maxWindowSize) return;
            if ((Date.now() - view.lastShiftAt) < 80) return;
            entries.forEach(entry => {
                if (entry.target === view.bottomSentinel) {
                    view.userPinnedToBottom = entry.isIntersecting;
                }
            });
            if (entries.some(entry => entry.isIntersecting)) {
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

OSA.getTranscriptSlotForMessageIndex = function(messageIndex) {
    const view = OSA.getTranscriptView();
    if (!view.listRoot) return null;
    const wrapper = view.listRoot.querySelector(`.transcript-entry[data-message-index="${messageIndex}"]`);
    return wrapper ? wrapper.querySelector('.transcript-entry-extras') : null;
};

OSA.storeAnchoredNode = function(node, messageIndex) {
    if (!node) return;
    const parsedIndex = Number.parseInt(String(messageIndex), 10);
    if (!Number.isInteger(parsedIndex)) return;
    OSA.removeStoredAnchoredNode(node);
    node.dataset.messageIndex = String(parsedIndex);
    node.dataset.anchorMessageIndex = String(parsedIndex);
    const view = OSA.getTranscriptView();
    if (!view.anchoredNodesByIndex.has(parsedIndex)) {
        view.anchoredNodesByIndex.set(parsedIndex, []);
    }
    const list = view.anchoredNodesByIndex.get(parsedIndex);
    if (!list.includes(node)) {
        list.push(node);
    }
};

OSA.removeStoredAnchoredNode = function(node) {
    if (!node) return;
    const view = OSA.getTranscriptView();
    const parsedIndex = Number.parseInt(node.dataset.anchorMessageIndex || '', 10);
    if (!Number.isInteger(parsedIndex)) return;
    const nodes = view.anchoredNodesByIndex.get(parsedIndex) || [];
    const next = nodes.filter(item => item !== node);
    if (next.length > 0) {
        view.anchoredNodesByIndex.set(parsedIndex, next);
    } else {
        view.anchoredNodesByIndex.delete(parsedIndex);
    }
    delete node.dataset.anchorMessageIndex;
};

OSA.mountAnchoredNode = function(node, messageIndex, insertBefore = null) {
    OSA.storeAnchoredNode(node, messageIndex);
    const slot = OSA.getTranscriptSlotForMessageIndex(messageIndex);
    if (!slot) return node;
    if (insertBefore && insertBefore.parentNode === slot) {
        slot.insertBefore(node, insertBefore);
    } else {
        slot.appendChild(node);
    }
    return node;
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

OSA.findAnchorMessageIndexForTimestamp = function(timestamp) {
    const session = OSA.getCurrentSession();
    if (!session || !Array.isArray(session.messages) || !timestamp) return -1;
    const targetMs = OSA.timestampToMs(timestamp);
    if (targetMs === null) return -1;

    let anchor = -1;
    session.messages.forEach((message, originalIndex) => {
        if (message.role === 'tool' || OSA.isHiddenSyntheticMessage(message)) return;
        const messageMs = OSA.timestampToMs(message.timestamp);
        if (messageMs !== null && messageMs <= targetMs) {
            anchor = originalIndex;
        }
    });
    return anchor;
};

OSA.createTranscriptEntry = function(message, originalIndex) {
    const key = OSA.getMessageRenderKey(message, originalIndex);
    const wrapper = document.createElement('div');
    wrapper.className = 'transcript-entry';
    wrapper.dataset.messageIndex = String(originalIndex);
    wrapper.dataset.messageKey = key;

    const messageEl = OSA.createMessageElement(message, originalIndex);
    const extrasSlot = document.createElement('div');
    extrasSlot.className = 'transcript-entry-extras';

    wrapper.append(messageEl, extrasSlot);
    return wrapper;
};

OSA.patchTranscriptEntry = function(wrapper, message, originalIndex) {
    if (!wrapper) return;
    const key = OSA.getMessageRenderKey(message, originalIndex);
    wrapper.dataset.messageIndex = String(originalIndex);
    wrapper.dataset.messageKey = key;

    let messageEl = wrapper.querySelector(':scope > .message');
    if (!messageEl) {
        messageEl = OSA.createMessageElement(message, originalIndex);
        wrapper.prepend(messageEl);
    }
    OSA.patchMessageElement(messageEl, message, originalIndex);

    let extrasSlot = wrapper.querySelector(':scope > .transcript-entry-extras');
    if (!extrasSlot) {
        extrasSlot = document.createElement('div');
        extrasSlot.className = 'transcript-entry-extras';
        wrapper.appendChild(extrasSlot);
    }
    return wrapper;
};

OSA.attachAnchoredNodesForEntry = function(wrapper, messageIndex) {
    const slot = wrapper ? wrapper.querySelector(':scope > .transcript-entry-extras') : null;
    if (!slot) return;
    const view = OSA.getTranscriptView();
    const nodes = view.anchoredNodesByIndex.get(messageIndex) || [];
    slot.replaceChildren(...nodes);
};

OSA.getVisibleMessages = function(messages) {
    const list = Array.isArray(messages) ? messages : [];
    const currentSession = OSA.getCurrentSession();
    const running = currentSession && currentSession.task_status === 'running';
    const lastIdx = list.length - 1;

    return list
        .map((message, originalIndex) => ({ message, originalIndex }))
        .filter(({ message }) => {
            if (message.role === 'tool') return false;
            if (OSA.isHiddenSyntheticMessage(message)) return false;
            if (message.role !== 'assistant') return true;
            const hasContent = !!(message.content || '').trim();
            const hasVisibleThinking = OSA.getShowThinkingBlocks() && !!(message.thinking || '').trim();
            if (hasContent || hasVisibleThinking) return true;
            return running && list[lastIdx] === message;
        });
};

OSA.getVisibleMessages = function(messages) {
    const list = Array.isArray(messages) ? messages : [];
    const currentSession = OSA.getCurrentSession();
    const running = currentSession && currentSession.task_status === 'running';
    const lastIdx = list.length - 1;

    return list
        .map((message, originalIndex) => ({ message, originalIndex }))
        .filter(({ message }) => {
            if (message.role === 'tool') return false;
            if (OSA.isHiddenSyntheticMessage(message)) return false;
            if (message.role !== 'assistant') return true;
            const hasContent = !!(message.content || '').trim();
            const hasVisibleThinking = OSA.getShowThinkingBlocks() && !!(message.thinking || '').trim();
            if (hasContent || hasVisibleThinking) return true;
            return running && list[lastIdx] === message;
        });
};

OSA.getMessageRenderKey = function(message, originalIndex) {
    const clientId = message && message.metadata && message.metadata.client_message_id;
    if (clientId) return `client:${clientId}`;
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

OSA.createNodeFromHtml = function(html, className = '') {
    const wrapper = document.createElement('div');
    if (className) wrapper.className = className;
    wrapper.innerHTML = html;
    return wrapper;
};

OSA.createMessageElement = function(message, originalIndex) {
    const el = document.createElement('div');
    OSA.patchMessageElement(el, message, originalIndex, true);
    return el;
};

OSA.patchMessageElement = function(element, message, originalIndex, force = false) {
    if (!element || !message) return;

    const key = OSA.getMessageRenderKey(message, originalIndex);
    const signature = OSA.getMessageRenderSignature(message);
    if (!force && element.dataset.renderSignature === signature) {
        return;
    }

    const ts = message.timestamp ? new Date(message.timestamp).getTime() : 0;
    element.className = `message ${message.role}`;
    element.dataset.ts = String(ts);
    element.dataset.messageIndex = String(originalIndex);
    element.dataset.messageTimestamp = message.timestamp || '';
    element.dataset.messageKey = key;
    element.dataset.renderSignature = signature;

    const roleEl = document.createElement('div');
    roleEl.className = 'message-role';
    roleEl.textContent = message.role === 'user' ? 'You' : 'OSA';

    const children = [roleEl];

    if (message.role === 'assistant') {
        const thinkingHtml = OSA.renderThinkingSection(message.thinking || '', false);
        if (thinkingHtml) {
            const thinkingWrap = OSA.createNodeFromHtml(thinkingHtml, 'message-thinking-wrap');
            if (thinkingWrap.firstElementChild) children.push(thinkingWrap.firstElementChild);
        }
    }

    const contentEl = document.createElement('div');
    contentEl.className = 'message-content';
    if (message.role === 'assistant') {
        const rawContent = message.content || '';
        contentEl.innerHTML = rawContent.trim() ? OSA.formatMessage(rawContent) : '';
        contentEl.dataset.rawText = rawContent;
    } else {
        contentEl.textContent = message.content || '';
    }
    children.push(contentEl);

    const attachments = OSA.collectMessageAttachments(message);
    if (attachments.length > 0) {
        const attachmentsHtml = OSA.renderAttachmentMarkup(attachments);
        if (attachmentsHtml) {
            const attachmentsWrap = OSA.createNodeFromHtml(attachmentsHtml, 'message-attachments-wrap');
            while (attachmentsWrap.firstChild) {
                children.push(attachmentsWrap.firstChild);
            }
        }
    }

    if (message.role === 'assistant') {
        const actionsEl = document.createElement('div');
        actionsEl.className = 'message-actions';
        const hasContent = !!(message.content || '').trim();
        actionsEl.style.display = hasContent ? '' : 'none';
        actionsEl.innerHTML = OSA.renderAssistantActionButtons(OSA.findNearestCheckpointForMessage(message.timestamp));
        children.push(actionsEl);
    }

    const clientId = message && message.metadata && message.metadata.client_message_id;
    if (clientId) {
        element.dataset.clientMessageId = clientId;
    } else {
        delete element.dataset.clientMessageId;
    }

    element.replaceChildren(...children);
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
    view.anchoredNodesByIndex.clear();
    view.descriptors = [];
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

OSA.isMessageIndexInRenderedWindow = function(messageIndex) {
    const parsed = Number.parseInt(String(messageIndex), 10);
    if (!Number.isInteger(parsed)) return false;
    const view = OSA.getTranscriptView();
    return view.renderedMessageIndices.has(parsed);
};

OSA.estimateMessageRangeHeight = function(descriptors, start, end, view) {
    let total = 0;
    for (let i = start; i < end; i++) {
        const item = descriptors[i];
        if (!item) continue;
        const key = OSA.getMessageRenderKey(item.message, item.originalIndex);
        total += view.messageHeights.get(key) || view.avgMessageHeight;
    }
    return total;
};

OSA.shiftTranscriptWindow = function(direction) {
    const view = OSA.getTranscriptView();
    if (!view.descriptors.length || view.shiftInProgress) return;

    const total = view.descriptors.length;
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
    OSA.syncRenderedMessages((OSA.getCurrentSession()?.messages) || [], {
        keepWindow: true,
        preserveScroll: true,
        resetStreaming: false,
        skipQueueRender: true,
        stickToBottom: false,
    });
    if (typeof OSA.restoreVisibleAnchoredArtifacts === 'function') {
        OSA.restoreVisibleAnchoredArtifacts();
    }
    requestAnimationFrame(() => {
        view.lastShiftAt = Date.now();
        view.shiftInProgress = false;
    });
};

OSA.syncRenderedMessages = function(messages, options = {}) {
    const perfStart = OSA.perfNow ? OSA.perfNow() : Date.now();
    const view = OSA.ensureMessageLayers();
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv || !view || !view.listRoot) return;
    if (view.isRendering) return;
    view.isRendering = true;

    const descriptors = OSA.getVisibleMessages(messages);
    view.descriptors = descriptors;

    const keepKeys = new Set(descriptors.map(item => OSA.getMessageRenderKey(item.message, item.originalIndex)));
    Array.from(view.messageSignatures.keys()).forEach(key => {
        if (!keepKeys.has(key)) view.messageSignatures.delete(key);
    });
    Array.from(view.messageHeights.keys()).forEach(key => {
        if (!keepKeys.has(key)) view.messageHeights.delete(key);
    });

    const total = descriptors.length;
    const shouldStickBottom = !!(options.stickToBottom || view.userPinnedToBottom);

    if (!options.keepWindow || options.forceFullWindowReset || view.windowEnd <= view.windowStart) {
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

    const anchorWrapper = view.listRoot.querySelector('.transcript-entry');
    const anchorKey = anchorWrapper ? anchorWrapper.dataset.messageKey : '';
    const anchorTop = anchorWrapper ? anchorWrapper.getBoundingClientRect().top : 0;

    const renderedDescriptors = descriptors.slice(view.windowStart, view.windowEnd);
    const renderedKeys = renderedDescriptors.map(item => OSA.getMessageRenderKey(item.message, item.originalIndex));
    const prevKeys = Array.from(view.wrapperNodesByKey.keys());

    let windowDirty = prevKeys.length !== renderedKeys.length;
    if (!windowDirty) {
        for (let i = 0; i < renderedKeys.length; i++) {
            if (renderedKeys[i] !== prevKeys[i]) {
                windowDirty = true;
                break;
            }
            const descriptor = renderedDescriptors[i];
            const signature = OSA.getMessageRenderSignature(descriptor.message);
            if (view.messageSignatures.get(renderedKeys[i]) !== signature) {
                windowDirty = true;
                break;
            }
        }
    }

    const renderedIndices = new Set(renderedDescriptors.map(item => item.originalIndex));
    view.renderedMessageIndices = renderedIndices;

    if (!windowDirty && view.lastDescriptorCount === total && options.keepWindow) {
        if (!options.skipQueueRender) {
            OSA.renderQueuedMessages(OSA.getSessionQueue());
        }
        view.lastDescriptorCount = total;
        view.isRendering = false;
        const elapsedMs = Math.round((OSA.perfNow ? OSA.perfNow() : Date.now()) - perfStart);
        if ((options.reason === 'session-switch' || elapsedMs > 24) && OSA.perfLog) {
            OSA.perfLog('syncRenderedMessages:noop', {
                reason: options.reason || '',
                totalMessages: total,
                renderedMessages: renderedDescriptors.length,
                elapsedMs,
            });
        }
        return;
    }

    const nextWrappers = [];
    const nextMessageMap = new Map();
    const nextWrapperMap = new Map();

    renderedDescriptors.forEach(item => {
        const key = OSA.getMessageRenderKey(item.message, item.originalIndex);
        const signature = OSA.getMessageRenderSignature(item.message);
        const prevWrapper = view.wrapperNodesByKey.get(key);
        const wrapper = prevWrapper || OSA.createTranscriptEntry(item.message, item.originalIndex);
        OSA.patchTranscriptEntry(wrapper, item.message, item.originalIndex);
        OSA.attachAnchoredNodesForEntry(wrapper, item.originalIndex);
        const messageEl = wrapper.querySelector(':scope > .message');
        if (messageEl) nextMessageMap.set(key, messageEl);
        nextWrapperMap.set(key, wrapper);
        nextWrappers.push(wrapper);
        view.messageSignatures.set(key, signature);
    });

    const heightBefore = OSA.estimateMessageRangeHeight(descriptors, 0, view.windowStart, view);
    const heightAfter = OSA.estimateMessageRangeHeight(descriptors, view.windowEnd, total, view);
    view.topSpacer.style.height = `${Math.max(0, Math.round(heightBefore))}px`;
    view.bottomSpacer.style.height = `${Math.max(0, Math.round(heightAfter))}px`;

    view.listRoot.replaceChildren(...nextWrappers);
    view.windowNodesByKey = nextMessageMap;
    view.wrapperNodesByKey = nextWrapperMap;

    let measuredTotal = 0;
    let measuredCount = 0;
    renderedDescriptors.forEach(item => {
        const key = OSA.getMessageRenderKey(item.message, item.originalIndex);
        const wrapper = nextWrapperMap.get(key);
        if (!wrapper) return;
        const height = wrapper.getBoundingClientRect().height;
        if (height > 0) {
            view.messageHeights.set(key, height);
            measuredTotal += height;
            measuredCount += 1;
        }
    });
    if (measuredCount > 0) {
        view.avgMessageHeight = measuredTotal / measuredCount;
    }

    if (shouldStickBottom) {
        messagesDiv.scrollTop = messagesDiv.scrollHeight;
    } else if (options.preserveScroll !== false && anchorKey) {
        const nextAnchor = nextWrapperMap.get(anchorKey) || null;
        if (nextAnchor) {
            const nextTop = nextAnchor.getBoundingClientRect().top;
            messagesDiv.scrollTop += (nextTop - anchorTop);
        }
    }

    if (options.resetStreaming !== false) {
        OSA.resetStreamingMessage();
    }

    if (!options.skipQueueRender) {
        OSA.renderQueuedMessages(OSA.getSessionQueue());
    }

    view.lastDescriptorCount = total;

    view.isRendering = false;

    const elapsedMs = Math.round((OSA.perfNow ? OSA.perfNow() : Date.now()) - perfStart);
    if ((options.reason === 'session-switch' || elapsedMs > 24) && OSA.perfLog) {
        OSA.perfLog('syncRenderedMessages', {
            reason: options.reason || '',
            totalMessages: total,
            renderedMessages: renderedDescriptors.length,
            windowStart: view.windowStart,
            windowEnd: view.windowEnd,
            elapsedMs,
        });
    }
};

OSA.appendUserMessageToChat = function(content, options = {}) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

    const currentSession = OSA.getCurrentSession();
    const clientMessageId = options.clientMessageId || '';
    const attachments = options.attachments || options.images || [];

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
                images: attachments.filter(att => att.kind === 'image' || (att.mime || '').startsWith('image/')).map(img => ({ filename: img.filename, mime: img.mime, data_url: img.dataUrl || img.data_url })),
            });
        }
    }

    if (clientMessageId) {
        const existing = Array.from(messagesDiv.querySelectorAll('[data-client-message-id]'))
            .find(el => el.dataset.clientMessageId === clientMessageId);
        if (existing) return existing;
    }

    if (currentSession) {
        OSA.syncRenderedMessages(currentSession.messages || [], {
            resetStreaming: false,
            stickToBottom: true,
            preferTail: true,
        });
    }

    if (!clientMessageId) {
        const allMessages = messagesDiv.querySelectorAll('.message.user');
        return allMessages.length ? allMessages[allMessages.length - 1] : null;
    }

    return Array.from(messagesDiv.querySelectorAll('[data-client-message-id]'))
        .find(el => el.dataset.clientMessageId === clientMessageId) || null;
};

OSA.handleQueuedMessageDispatched = function(event) {
    const currentSession = OSA.getCurrentSession();
    if (currentSession) currentSession.task_status = 'running';
    const queue = (OSA.getSessionQueue() || []).filter(item => item.id !== event.queue_entry_id);
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

OSA.renderMessages = function(messages, options = {}) {
    OSA.syncRenderedMessages(messages, {
        stickToBottom: true,
        preferTail: true,
        forceFullWindowReset: true,
        reason: options.reason || '',
    });
};

OSA.renderQueuedMessages = function(queueItems) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;
    const floatingRoot = OSA.getFloatingRoot();
    if (!floatingRoot) return;

    const nearBottom = (messagesDiv.scrollHeight - messagesDiv.scrollTop - messagesDiv.clientHeight) < 140;

    OSA.removeQueuedMessageElements();

    const items = Array.isArray(queueItems) ? queueItems : [];
    if (items.length === 0) return;

    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) emptyState.remove();

    items.forEach((item, index) => {
        const message = document.createElement('div');
        message.className = `queued-notice${item.status === 'dispatching' ? ' dispatching' : ''}`;
        if (item.id) message.dataset.queueId = item.id;
        const label = item.status === 'dispatching' ? 'Sending next' : `Queued ${index + 1}`;
        const preview = (item.content || '').slice(0, 80) + ((item.content || '').length > 80 ? '…' : '');
        message.innerHTML = `<span class="queued-notice-label">${label}</span><span class="queued-notice-text">${OSA.escapeHtml(preview)}</span><span class="queued-notice-time">${OSA.escapeHtml(OSA.formatRelativeDateTime(item.created_at))}</span>`;
        floatingRoot.appendChild(message);
    });

    if (nearBottom) {
        messagesDiv.scrollTop = messagesDiv.scrollHeight;
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

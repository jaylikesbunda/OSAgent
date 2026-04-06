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

    messagesDiv.appendChild(indicator);
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

    let sibling = element.nextElementSibling;
    while (sibling) {
        if (
            sibling.classList.contains('tool-container')
            || sibling.classList.contains('parallel-group')
            || sibling.classList.contains('subagent-card')
        ) {
            return true;
        }
        sibling = sibling.nextElementSibling;
    }

    return false;
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
    OSA.releaseStreamingAssistantMessage();
    OSA.insertCurrentSessionToolBoundary(event);
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
            thinkingWrap.classList.toggle('expanded', expandThinking && !!(sourceMessage?.thinking || '').trim());
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
        OSA.resetStreamingMessage();
        return;
    }

    const actionsEl = message.querySelector('.message-actions');
    if (actionsEl && rawText) {
        actionsEl.style.display = '';
    }

    OSA.resetStreamingMessage();
};

OSA.createAssistantMessageShell = function() {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) emptyState.remove();

    const domId = `assistant-stream-${Date.now()}-${Math.random().toString(36).slice(2, 7)}`;
    const message = document.createElement('div');
    message.className = 'message assistant streaming';
    message.id = domId;
    message.innerHTML = `
        <div class="message-role">OSA</div>
        <div class="message-content"></div>
        <div class="message-actions" style="display:none">
            <button class="msg-action-btn" onclick="OSA.copyAssistantMessage('${domId}')" title="Copy">Copy</button>
        </div>
    `;
    messagesDiv.appendChild(message);
    OSA.setStreamingAssistantDomId(domId);
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
    return OSA.createAssistantMessageShell();
};

OSA.beginAssistantResponse = function() {
    OSA.ensureCurrentSessionAssistantMessage();
    OSA.hideThinkingIndicator();
    return OSA.ensureStreamingAssistantMessage();
};

OSA.beginThinkingDisplay = function() {
    if (!OSA.getShowThinkingBlocks()) return null;

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
    const message = OSA.ensureStreamingAssistantMessage();
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

OSA.completeAssistantResponse = function() {
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
        const actionsEl = message.querySelector('.message-actions');
        if (actionsEl && rawText) actionsEl.style.display = '';

        const startTime = OSA.getTurnStartTime();
        const durationEl = message.querySelector('.turn-duration');
        if (durationEl && startTime) {
            const elapsed = Math.round((Date.now() - startTime) / 1000);
            durationEl.textContent = elapsed < 60 ? `${elapsed}s` : `${Math.floor(elapsed / 60)}m ${elapsed % 60}s`;
        }

        if (rawText && OSA.getTtsEnabled() && OSA.getVoiceConfig()?.enabled) {
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
        const btn = message.querySelector('.msg-action-btn');
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
    messagesDiv.appendChild(card);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
};

OSA.formatMessage = function(text) {
    const escaped = OSA.escapeHtml(text);
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
    document.querySelectorAll('#messages .queued-notice').forEach(el => el.remove());
};

OSA.appendUserMessageToChat = function(content, options = {}) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

    const currentSession = OSA.getCurrentSession();
    const clientMessageId = options.clientMessageId || '';

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
                metadata: clientMessageId ? { client_message_id: clientMessageId } : {},
                tokens: null,
            });
        }
    }

    if (clientMessageId) {
        const existing = Array.from(messagesDiv.querySelectorAll('[data-client-message-id]'))
            .find(el => el.dataset.clientMessageId === clientMessageId);
        if (existing) return existing;
    }

    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) emptyState.remove();

    const message = document.createElement('div');
    message.className = 'message user';
    if (clientMessageId) message.dataset.clientMessageId = clientMessageId;
    message.innerHTML = `
        <div class="message-role">You</div>
        <div class="message-content">${OSA.escapeHtml(content)}</div>
    `;
    messagesDiv.appendChild(message);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    return message;
};

OSA.handleQueuedMessageDispatched = function(event) {
    const currentSession = OSA.getCurrentSession();
    if (currentSession) currentSession.task_status = 'running';
    const queue = (OSA.getSessionQueue() || []).filter(item => item.id !== event.queue_entry_id);
    OSA.setSessionQueue(queue);
    OSA.removeQueuedMessageElements();
    OSA.appendUserMessageToChat(event.content || '', {
        clientMessageId: event.client_message_id || '',
        timestamp: event.timestamp,
    });
    OSA.renderQueuedMessages(queue);
};

OSA.renderMessages = function(messages) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    const visibleMessages = messages
        .map((message, originalIndex) => ({ message, originalIndex }))
        .filter(({ message }) => {
            if (message.role === 'tool') return false;
            if (OSA.isHiddenSyntheticMessage(message)) return false;
            if (message.role !== 'assistant') return true;
            const hasContent = !!(message.content || '').trim();
            const hasVisibleThinking = OSA.getShowThinkingBlocks() && !!(message.thinking || '').trim();
            return hasContent || hasVisibleThinking;
        })
        .slice(-120);

    messagesDiv.innerHTML = visibleMessages
        .map(({ message: m, originalIndex }) => {
            const ts = m.timestamp ? new Date(m.timestamp).getTime() : 0;
            const thinkingHtml = m.role === 'assistant' ? OSA.renderThinkingSection(m.thinking || '', false) : '';
            const contentHtml = m.role === 'assistant' ? OSA.formatMessage(m.content || '') : OSA.escapeHtml(m.content || '');
            const contentBlock = (m.role === 'assistant' && (!m.content || !m.content.trim()))
                ? ''
                : `<div class="message-content">${contentHtml}</div>`;
            const actionsHtml = (m.role === 'assistant' && (m.content || '').trim())
                ? `<div class="message-actions"><button class="msg-action-btn" onclick="OSA.copyAssistantMessageElement(this)" title="Copy">Copy</button></div>`
                : '';
            return `<div class="message ${m.role}" data-ts="${ts}" data-message-index="${originalIndex}">
                <div class="message-role">${m.role === 'user' ? 'You' : 'OSA'}</div>
                ${thinkingHtml}
                ${contentBlock}
                ${actionsHtml}
            </div>`;
        }).join('');

    OSA.resetStreamingMessage();
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    OSA.renderQueuedMessages(OSA.getSessionQueue());
};

OSA.renderQueuedMessages = function(queueItems) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

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
        messagesDiv.appendChild(message);
    });

    messagesDiv.scrollTop = messagesDiv.scrollHeight;
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
        list.style.display = 'none';
        return;
    }

    list.style.display = '';
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

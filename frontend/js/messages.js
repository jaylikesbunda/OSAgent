window.OSA = window.OSA || {};

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

OSA.resetStreamingMessage = function() {
    OSA.setStreamingAssistantDomId(null);
};

OSA.createAssistantMessageShell = function() {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

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
        if (existing) return existing;
    }
    return OSA.createAssistantMessageShell();
};

OSA.beginAssistantResponse = function() {
    OSA.hideThinkingIndicator();
    return OSA.createAssistantMessageShell();
};

OSA.appendAssistantChunk = function(content) {
    if (!content) return;
    const message = OSA.ensureStreamingAssistantMessage();
    if (!message) return;
    const contentEl = message.querySelector('.message-content');
    const nextText = (contentEl.dataset.rawText || '') + content;
    contentEl.dataset.rawText = nextText;
    contentEl.innerHTML = OSA.formatMessage(nextText);

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
        const contentEl = message.querySelector('.message-content');
        const rawText = contentEl ? (contentEl.dataset.rawText || '') : '';
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
    if (!rawText) {
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

OSA.renderMessages = function(messages) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    const visibleMessages = messages
        .filter(m => m.role !== 'tool' && !(m.role === 'assistant' && (!m.content || m.content.trim() === '')))
        .slice(-120);

    messagesDiv.innerHTML = visibleMessages
        .map(m => {
            const ts = m.timestamp ? new Date(m.timestamp).getTime() : 0;
            return `<div class="message ${m.role}" data-ts="${ts}">
                <div class="message-role">${m.role === 'user' ? 'You' : 'OSA'}</div>
                <div class="message-content">${m.role === 'assistant' ? OSA.formatMessage(m.content) : OSA.escapeHtml(m.content)}</div>
            </div>`;
        }).join('');

    OSA.resetStreamingMessage();
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
};

OSA.updateTodoDock = function() {
    const dock = document.getElementById('todo-dock');
    if (!dock) return;
    const todos = OSA.getSessionTodos() || [];
    const active = todos.filter(t => t.status === 'in_progress');
    const completed = todos.filter(t => t.status === 'completed');
    const total = todos.length;

    if (total === 0) {
        dock.classList.add('hidden');
        return;
    }

    dock.classList.remove('hidden');

    const counterEl = dock.querySelector('.dock-counter');
    if (counterEl) {
        counterEl.textContent = `${completed.length}/${total}`;
    }

    const activeEl = dock.querySelector('.dock-active-task');
    if (activeEl) {
        activeEl.textContent = active.length ? active[0].content : (completed.length === total ? 'All tasks completed' : 'No active task');
    }

    const progressBar = dock.querySelector('.dock-progress-fill');
    if (progressBar) {
        const pct = total > 0 ? Math.round((completed.length / total) * 100) : 0;
        progressBar.style.width = `${pct}%`;
    }

    if (OSA.getTodoDockExpanded()) {
        OSA.renderTodoDockList(dock, todos);
    }
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
    const byStatus = { in_progress: [], pending: [], completed: [], cancelled: [] };
    todos.forEach(t => {
        const s = (t.status || 'pending').toLowerCase();
        if (byStatus[s]) byStatus[s].push(t);
    });

    let html = '';
    const renderGroup = (items, label) => {
        if (!items.length) return '';
        let h = `<div class="dock-group-label">${label}</div>`;
        items.forEach(t => {
            const status = (t.status || 'pending').toLowerCase();
            const icon = status === 'completed' ? '&#x2713;' : status === 'in_progress' ? '&#x25CF;' : '&#x25CB;';
            h += `<div class="dock-item ${status}"><span class="dock-item-icon">${icon}</span><span class="dock-item-text">${OSA.escapeHtml(t.content || '')}</span></div>`;
        });
        return h;
    };
    html += renderGroup(byStatus.in_progress, 'In Progress');
    html += renderGroup(byStatus.pending, 'Pending');
    html += renderGroup(byStatus.completed, 'Completed');
    list.innerHTML = html;
};

window.copyAssistantMessage = OSA.copyAssistantMessage;

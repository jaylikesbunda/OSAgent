window.OSA = window.OSA || {};

OSA.getContextRingMetrics = function(contextState) {
    if (!contextState) return null;

    const used = (contextState.actual_usage && contextState.actual_usage.total > 0)
        ? contextState.actual_usage.total
        : (contextState.estimated_tokens || 0);
    const window = contextState.context_window || 1;
    const pct = Math.min(100, Math.round((used / Math.max(window, 1)) * 100));
    const circumference = 97.4;
    const offset = circumference - (pct / 100) * circumference;
    const colorClass = pct >= 90 ? 'danger' : pct >= 70 ? 'warning' : '';

    return { used, window, pct, circumference, offset, colorClass };
};

OSA.buildContextRingHtml = function(contextState, subagentId) {
    const metrics = OSA.getContextRingMetrics(contextState);
    if (!metrics) return '';
    return `
        <div class="context-ring subagent-context-ring ${metrics.colorClass}" id="subagent-context-ring-${subagentId}" title="Context: ${metrics.pct}%">
            <svg viewBox="0 0 36 36">
                <circle class="context-ring-bg" cx="18" cy="18" r="15.5"/>
                <circle class="context-ring-progress" cx="18" cy="18" r="15.5"
                    stroke-dasharray="97.4" stroke-dashoffset="${metrics.offset}"/>
            </svg>
            <span class="context-ring-text">${metrics.pct}%</span>
        </div>
    `;
};

OSA.TOOL_LABELS = {
    read_file: 'Read',
    list_files: 'List',
    glob: 'Find',
    grep: 'Search',
    bash: 'Shell',
    write_file: 'Write',
    edit_file: 'Edit',
    apply_patch: 'Patch',
    delete_file: 'Delete',
    task: 'Task',
    todowrite: 'Todos',
    todoread: 'Todos',
    question: 'Question',
    web_fetch: 'Fetch',
    web_search: 'Search',
    skill: 'Skill',
    subagent: 'Subagent',
};

OSA.TOOL_ICONS = {
    read_file: 'R',
    list_files: 'L',
    glob: 'F',
    grep: 'S',
    bash: '$',
    write_file: 'W',
    edit_file: 'E',
    apply_patch: 'P',
    delete_file: 'D',
    task: 'T',
    todowrite: '[]',
    question: '?',
    web_fetch: 'H',
    web_search: 'Q',
    subagent: 'A',
};

OSA.ROW_TOOLS = new Set(['read_file', 'list_files', 'task', 'skill', 'web_fetch', 'subagent']);
OSA.CONTEXT_TOOLS = new Set(['read_file', 'list_files', 'glob', 'grep']);

OSA.handleAgentEvent = function(event) {
    const isStopping = OSA.isAgentStopping();
    const ignoreDuringStop = ['thinking', 'thinking_start', 'thinking_delta', 'thinking_end', 'response_start', 'response_chunk', 'tool_start', 'tool_progress', 'tool_complete', 'context_update', 'subagent_created', 'subagent_progress', 'subagent_completed', 'retry', 'compaction', 'step_finish', 'reasoning', 'question_asked'];
    
    if (isStopping && ignoreDuringStop.includes(event.type)) {
        return;
    }

    const chain = OSA.getMessageChain();
    const seq = ++chain.eventSeqNumber;
    const prevType = chain.lastEventType;

    switch (event.type) {
        case 'thinking':
            OSA.setHasReceivedResponse(false);
            if (OSA.getCurrentSession()) OSA.getCurrentSession().task_status = 'running';
            OSA.showThinkingIndicator();
            OSA.setSendButtonStopMode(true);
            OSA.startToolSync();
            OSA.renderQueuedMessages(OSA.getSessionQueue());
            if (OSA.refreshCurrentSessionQueue) OSA.refreshCurrentSessionQueue();
            break;

        case 'thinking_start':
            if (prevType === 'thinking_start') {
                break;
            }
            OSA.beginThinkingDisplay();
            break;

        case 'thinking_delta':
            OSA.appendThinkingChunk(event.content || '');
            break;

        case 'thinking_end':
            chain.lastThinkingEndSeq = seq;
            OSA.completeThinkingDisplay();
            break;

        case 'response_start':
            chain.lastAssistantDomId = OSA.getStreamingAssistantDomId() || chain.lastAssistantDomId;
            OSA.beginAssistantResponse();
            OSA.renderQueuedMessages(OSA.getSessionQueue());
            if (OSA.refreshCurrentSessionQueue) OSA.refreshCurrentSessionQueue();
            break;

        case 'response_chunk':
            OSA.setHasReceivedResponse(true);
            OSA.appendAssistantChunk(event.content || '');
            break;

        case 'tool_start':
            chain.lastToolStartSeq = seq;
            chain.pendingToolCallIds = chain.pendingToolCallIds || [];
            if (event.tool_call_id && !chain.pendingToolCallIds.includes(event.tool_call_id)) {
                chain.pendingToolCallIds.push(event.tool_call_id);
            }
            OSA.finalizeAssistantSegmentForToolCall(event);
            OSA.createToolCard(event);
            OSA.persistToolStart(event);
            OSA.speakToolStart(event);
            OSA.renderQueuedMessages(OSA.getSessionQueue());
            break;

        case 'tool_progress':
            OSA.updateToolProgress(event);
            break;

        case 'tool_complete':
            if (event.tool_call_id) {
                chain.pendingToolCallIds = (chain.pendingToolCallIds || []).filter(id => id !== event.tool_call_id);
            }
            OSA.completeToolCard(event);
            OSA.persistToolComplete(event);
            if (event.tool_name === 'task') {
                OSA.renderTaskMessage(event);
            }
            if (event.tool_name === 'todowrite' || event.tool_name === 'todoread') {
                OSA.fetchAndRenderTodos();
            }
            if (event.tool_name === 'subagent') {
                OSA.handleSubagentComplete(event);
            }
            if (['write_file', 'edit_file', 'apply_patch', 'delete_file', 'batch'].includes(event.tool_name)) {
                OSA.scheduleSessionInspectorRefresh();
            }
            OSA.speakToolComplete(event);
            break;

        case 'response_complete':
            chain.pendingToolCallIds = [];
            chain.lastAssistantDomId = null;
            OSA.setHasReceivedResponse(true);
            if (OSA.getCurrentSession()) OSA.getCurrentSession().task_status = 'active';
            OSA.completeAssistantResponse(event.usage || null);
            OSA.hideThinkingIndicator();
            OSA.stopToolSync();
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
            OSA.scheduleSessionInspectorRefresh();
            if (OSA.refreshCurrentSessionQueue) OSA.refreshCurrentSessionQueue();
            break;

        case 'queued_message_dispatched':
            chain.lastAssistantDomId = null;
            OSA.handleQueuedMessageDispatched(event);
            break;

        case 'context_update':
            OSA.updateContextStatus(event);
            if (event.subagent_session_id) {
                OSA.updateSubagentContextRing(event.subagent_session_id, event);
            }
            break;

        case 'retry':
        case 'compaction':
        case 'step_finish':
        case 'reasoning':
            OSA.scheduleSessionInspectorRefresh();
            break;

        case 'question_asked':
            OSA.handleQuestionEvent(event);
            break;

        case 'error':
            chain.pendingToolCallIds = [];
            OSA.handleEventError(event);
            OSA.setStopping(false);
            OSA.setSendButtonStopMode(false);
            break;

        case 'cancelled':
            chain.pendingToolCallIds = [];
            OSA.stopToolSync();
            OSA.handleEventCancelled(event);
            break;

        case 'subagent_created':
            OSA.handleSubagentCreated(event);
            break;

        case 'subagent_progress':
            OSA.handleSubagentProgress(event);
            break;

        case 'subagent_completed':
            OSA.handleSubagentCompleted(event);
            break;

        case 'scheduled_job_fired':
            if (OSA.Jobs) {
                OSA.Jobs.showNotification(event.message, event.job_type || 'info');
            }
            break;

        default: break;
    }

    chain.lastEventType = event.type;
};

OSA._contextStates = {};
OSA._currentContextSessionId = null;

OSA.updateContextStatus = function(event) {
    const sessionId = event.session_id;
    if (sessionId) {
        OSA._contextStates[sessionId] = event;
    }
    if (sessionId && sessionId !== OSA._currentContextSessionId) return;
    
    const indicator = document.getElementById('context-indicator');
    const ringProgress = document.getElementById('context-ring-progress');
    const pctEl = document.getElementById('context-pct');
    
    if (!indicator || !ringProgress || !pctEl) return;

    const metrics = OSA.getContextRingMetrics(event);
    if (!metrics) return;

    ringProgress.style.strokeDashoffset = metrics.offset;
    pctEl.textContent = metrics.pct + '%';
    
    indicator.classList.remove('warning', 'danger');
    if (metrics.pct >= 90) {
        indicator.classList.add('danger');
    } else if (metrics.pct >= 70) {
        indicator.classList.add('warning');
    }
    
    indicator.classList.remove('hidden');
};

OSA.restoreContextState = function(sessionId, contextState) {
    OSA._currentContextSessionId = sessionId;
    
    if (contextState) {
        OSA._contextStates[sessionId] = contextState;
        OSA.updateContextStatus(contextState);
    } else {
        const indicator = document.getElementById('context-indicator');
        if (indicator) indicator.classList.add('hidden');
    }
};

OSA.toggleContextModal = function() {
    const modal = document.getElementById('context-modal');
    if (!modal) return;
    
    if (modal.classList.contains('hidden')) {
        OSA.openContextModal();
    } else {
        OSA.closeContextModal();
    }
};

OSA.openContextModal = function() {
    const modal = document.getElementById('context-modal');
    if (!modal) return;
    
    modal.classList.remove('hidden');
    OSA._updateContextModalContent();
};

OSA.closeContextModal = function(event) {
    if (event && event.target !== event.currentTarget) return;
    const modal = document.getElementById('context-modal');
    if (!modal) return;
    modal.classList.add('hidden');
};

OSA._updateContextModalContent = function() {
    const state = OSA._currentContextSessionId ? OSA._contextStates[OSA._currentContextSessionId] : null;
    if (!state) return;
    
    const used = state.estimated_tokens || 0;
    const window = state.context_window || 1;
    const budget = state.budget_tokens || window;
    const pct = Math.min(100, Math.round((used / Math.max(window, 1)) * 100));
    const actualUsage = state.actual_usage;
    
    const formatTokens = (n) => {
        if (n >= 1000000) return (n / 1000000).toFixed(1) + 'M';
        if (n >= 1000) return (n / 1000).toFixed(0) + 'K';
        return n.toString();
    };
    
    document.getElementById('ctx-window').textContent = formatTokens(window);
    document.getElementById('ctx-used').textContent = formatTokens(used);
    document.getElementById('ctx-budget').textContent = formatTokens(budget);
    document.getElementById('ctx-max').textContent = formatTokens(window);
    document.getElementById('ctx-progress-pct').textContent = pct + '%';
    
    const progressFill = document.getElementById('ctx-progress-fill');
    const progressPct = document.getElementById('ctx-progress-pct');
    progressFill.style.width = pct + '%';
    progressPct.style.left = pct + '%';
    progressFill.classList.remove('warning', 'danger');
    if (pct >= 90) {
        progressFill.classList.add('danger');
    } else if (pct >= 70) {
        progressFill.classList.add('warning');
    }
    
    const statusEl = document.getElementById('ctx-status');
    if (pct >= 90) {
        statusEl.textContent = 'Near limit';
        statusEl.className = 'context-detail-value status-danger';
    } else if (pct >= 70) {
        statusEl.textContent = 'High usage';
        statusEl.className = 'context-detail-value status-warning';
    } else {
        statusEl.textContent = 'OK';
        statusEl.className = 'context-detail-value status-ok';
    }
    
    const actualRow = document.getElementById('ctx-actual-row');
    const outputRow = document.getElementById('ctx-output-row');
    const cacheRow = document.getElementById('ctx-cache-row');
    
    if (actualUsage && (actualUsage.input > 0 || actualUsage.total > 0)) {
        actualRow.style.display = 'flex';
        document.getElementById('ctx-actual-input').textContent = formatTokens(actualUsage.input || actualUsage.total);
        
        if (actualUsage.output > 0) {
            outputRow.style.display = 'flex';
            document.getElementById('ctx-output').textContent = formatTokens(actualUsage.output);
        } else {
            outputRow.style.display = 'none';
        }
        
        const cacheRead = actualUsage.cached_read || 0;
        const cacheWrite = actualUsage.cached_write || 0;
        if (cacheRead > 0 || cacheWrite > 0) {
            cacheRow.style.display = 'flex';
            const parts = [];
            if (cacheRead > 0) parts.push('R:' + formatTokens(cacheRead));
            if (cacheWrite > 0) parts.push('W:' + formatTokens(cacheWrite));
            document.getElementById('ctx-cache').textContent = parts.join(' / ');
        } else {
            cacheRow.style.display = 'none';
        }
    } else {
        actualRow.style.display = 'none';
        outputRow.style.display = 'none';
        cacheRow.style.display = 'none';
    }
};

OSA.toolLabel = function(name) {
    return OSA.TOOL_LABELS[name] || name;
};

OSA.toolIcon = function(name) {
    return OSA.TOOL_ICONS[name] || '*';
};

OSA.isRowTool = function(name) {
    return OSA.ROW_TOOLS.has(name);
};

OSA.isContextTool = function(name) {
    return OSA.CONTEXT_TOOLS.has(name);
};

OSA.summarizeToolArgs = function(toolName, args) {
    if (!args) return '';
    if (toolName === 'read_file') {
        const p = args.path || args.filePath || '';
        const parts = p.replace(/\\/g, '/').split('/');
        return parts.length > 3 ? '...' + parts.slice(-3).join('/') : p;
    }
    if (toolName === 'list_files') return args.path || '.';
    if (toolName === 'glob') return args.pattern || '*';
    if (toolName === 'grep') return '"' + (args.pattern || 'search') + '"';
    if (toolName === 'bash') {
        const cmd = args.command || '';
        return cmd.length > 80 ? cmd.slice(0, 80) + '\u2026' : cmd;
    }
    if (toolName === 'write_file') {
        const p = args.path || args.filePath || '';
        const parts = p.replace(/\\/g, '/').split('/');
        return parts.length > 3 ? '...' + parts.slice(-3).join('/') : p;
    }
    if (toolName === 'edit_file') {
        const p = args.filePath || args.path || '';
        const parts = p.replace(/\\/g, '/').split('/');
        return parts.length > 3 ? '...' + parts.slice(-3).join('/') : p;
    }
    if (toolName === 'apply_patch') return '';
    if (toolName === 'web_fetch' || toolName === 'webfetch') {
        const u = args.url || '';
        try {
            const parsed = new URL(u);
            return parsed.hostname + parsed.pathname;
        } catch {
            return u.length > 50 ? u.slice(0, 50) + '\u2026' : u;
        }
    }
    if (toolName === 'subagent') {
        const desc = args.description || '';
        const type = args.subagent_type || 'general';
        return `${type}: ${desc.length > 40 ? desc.slice(0, 40) + '\u2026' : desc}`;
    }
    return '';
};

OSA.parseDiffChanges = function(output) {
    if (!output) return { additions: 0, deletions: 0 };
    let additions = 0;
    let deletions = 0;
    for (const line of output.split('\n')) {
        if (line.startsWith('+') && !line.startsWith('++')) additions++;
        else if (line.startsWith('-') && !line.startsWith('--')) deletions++;
    }
    return { additions, deletions };
};

OSA.createToolCard = function(event, insertBefore = null) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    OSA.pruneEmptyStreamingMessage();

    const toolName = event.tool_name;
    const callId = event.tool_call_id;

    if (OSA.isContextTool(toolName)) {
        const messageIndex = event.message_index !== undefined ? event.message_index : 0;
        OSA.addContextToolToGroup(event, false, false, messageIndex);
        return;
    }

    if (toolName === 'subagent') return;

    OSA._renderInlineToolCard({
        tool_call_id: callId,
        tool_name: toolName,
        arguments: event.arguments || {},
        completed: false,
        success: false,
        output: '',
        message_index: event.message_index !== undefined ? event.message_index : 0,
    }, insertBefore);

    const domId = `tool-${callId}`;
    const startTime = Date.now();
    const parallelTools = OSA.parallelToolGroups;
    const recentParallelStart = parallelTools.find(g =>
        g.startTime && (Date.now() - g.startTime) < OSA.parallelToolWindow
    );

    if (recentParallelStart) {
        recentParallelStart.callIds.push(callId);
        recentParallelStart.count++;
    } else {
        parallelTools.push({
            startTime,
            callIds: [callId],
            count: 1,
            groupId: null
        });
    }

    OSA.getActiveTools().set(callId, {
        domId,
        expanded: false,
        completed: false,
        toolName,
        isPanel: true,
        startTime,
        parallelGroupStart: recentParallelStart ? recentParallelStart.startTime : startTime,
    });
};

OSA._renderInlineToolCard = function(toolEvent, insertBefore, parent) {
    const toolName = toolEvent.tool_name;
    const args = toolEvent.arguments || {};
    const callId = toolEvent.tool_call_id;
    const isCompleted = toolEvent.completed === true;
    const isSuccess = toolEvent.success === true;
    const output = toolEvent.output || '';
    const domId = `tool-${callId}`;

    const existing = document.getElementById(domId);
    if (existing) {
        const statusEl = existing.querySelector('.tool-status-badge');
        if (statusEl) {
            const statusText = isCompleted ? (isSuccess ? 'done' : 'failed') : 'running';
            const statusClass = isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending';
            statusEl.textContent = statusText;
            statusEl.className = `tool-status-badge ${statusClass}`;
        }
        const titleEl = existing.querySelector('.tool-title');
        if (titleEl) titleEl.classList.toggle('tool-title-pending', !isCompleted);
        if (isCompleted && output) {
            const outputEl = existing.querySelector('.tool-output');
            if (outputEl && outputEl.style.display === 'none') {
                const formatted = OSA.formatToolOutput(toolName, output);
                if (formatted) {
                    outputEl.textContent = formatted;
                    outputEl.style.display = '';
                }
            }
        }
        return;
    }

    const label = OSA.toolLabel(toolName);
    const icon = OSA.toolIcon(toolName);
    const subtitle = OSA.summarizeToolArgs(toolName, args);

    const statusText = isCompleted ? (isSuccess ? 'done' : 'failed') : 'running';
    const statusClass = isCompleted ? (isSuccess ? 'done' : 'failed') : 'pending';
    const titleClass = isCompleted ? '' : 'tool-title-pending';
    const chevronOpacity = isCompleted ? '' : 'opacity:0';

    const container = document.createElement('div');
    container.id = domId;
    container.className = 'tool-container';
    container.dataset.messageIndex = toolEvent.message_index !== undefined ? toolEvent.message_index : 0;

    let html = `
        <div class="tool-card tool-inline" id="card-${domId}" data-tool="${OSA.escapeHtml(toolName)}">
            <div class="tool-trigger tool-trigger-inline" onclick="OSA.toggleToolCard('${domId}')">
                <span class="tool-icon">${icon}</span>
                <span class="tool-title ${titleClass}" id="title-${domId}">${OSA.escapeHtml(label)}</span>
                ${subtitle ? `<span class="tool-subtitle" id="subtitle-${domId}">${OSA.escapeHtml(subtitle)}</span>` : ''}
                <span class="tool-status-badge ${statusClass}" id="status-${domId}">${statusText}</span>
                <span class="tool-chevron" id="chevron-${domId}" style="${chevronOpacity}">&#x25B6;</span>
            </div>
            <div class="tool-body" id="body-${domId}">
                <div class="tool-body-inner">
                    <div class="tool-args" id="args-${domId}">${OSA.escapeHtml(JSON.stringify(args, null, 2))}</div>
                    <div class="tool-output" id="output-${domId}" style="display:none"></div>
                </div>
            </div>
        </div>`;

    container.innerHTML = html;

    const messagesDiv = document.getElementById('messages');
    const target = parent || messagesDiv;
    if (!target) return container;

    if (insertBefore && !parent) {
        target.insertBefore(container, insertBefore);
    } else {
        target.appendChild(container);
    }
    messagesDiv.scrollTop = messagesDiv.scrollHeight;

    if (isCompleted && output) {
        const outputEl = document.getElementById(`output-${domId}`);
        if (outputEl) {
            const formatted = OSA.formatToolOutput(toolName, output);
            if (formatted) {
                outputEl.textContent = formatted;
                outputEl.style.display = '';
            }
        }

        if (isSuccess && ['write_file', 'edit_file', 'apply_patch'].includes(toolName)) {
            const diff = OSA.parseDiffChanges(output);
            if (diff.additions > 0 || diff.deletions > 0) {
                const subtitleEl = document.getElementById(`subtitle-${domId}`);
                if (subtitleEl) {
                    let badges = subtitleEl.textContent;
                    badges += ` <span class="diff-add">+${diff.additions}</span><span class="diff-del">-${diff.deletions}</span>`;
                    subtitleEl.innerHTML = badges;
                }
            }
        }

        if (toolName === 'bash' && isSuccess) {
            const argsEl = document.getElementById(`args-${domId}`);
            const body = document.getElementById(`body-${domId}`);
            if (argsEl) argsEl.style.display = 'none';

            const cmd = (args.command || '').trim();
            if (cmd) {
                const cmdLine = document.createElement('div');
                cmdLine.className = 'shell-command-line';
                cmdLine.innerHTML = `<span class="shell-prompt">$</span> <span class="shell-cmd">${OSA.escapeHtml(cmd)}</span>`;
                const bodyInner = body?.querySelector('.tool-body-inner');
                if (bodyInner) bodyInner.insertBefore(cmdLine, bodyInner.firstChild);
            }
        }
    }

    return container;
};

OSA.restoreToolCard = function(toolEvent, insertBefore = null, parent = null) {
    const toolName = toolEvent.tool_name;

    if (OSA.isContextTool(toolName)) {
        const messageIndex = toolEvent.message_index !== undefined ? toolEvent.message_index : 0;
        OSA.addContextToolToGroup(toolEvent, toolEvent.completed === true, toolEvent.success === true, messageIndex);
        return;
    }

    if (toolName === 'subagent') return;

    OSA._renderInlineToolCard(toolEvent, insertBefore, parent);
};

OSA.restoreContextToolGroup = function(tools, insertBefore = null) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv || tools.length === 0) return;

    tools.sort((a, b) => (a.timestamp || 0) - (b.timestamp || 0));

    const messageIndex = tools.length > 0 ? (tools[0].message_index || 0) : 0;
    const groupId = `context-tool-group-${messageIndex}`;

    const group = document.createElement('div');
    group.id = groupId;
    group.className = 'tool-container context-inline-group';
    group.dataset.messageIndex = messageIndex;

    tools.forEach(t => {
        const label = OSA.toolLabel(t.tool_name);
        const detail = OSA.summarizeToolArgs(t.tool_name, t.arguments || {});
        const isSuccess = t.success === true;
        const statusText = isSuccess ? 'done' : 'failed';

        const item = document.createElement('div');
        item.className = 'context-inline-item';
        item.id = `ctx-${t.tool_call_id || Math.random().toString(36).slice(2)}`;
        item.innerHTML = `
            <span class="context-inline-action">${OSA.escapeHtml(label)}</span>
            <span class="context-inline-detail">${OSA.escapeHtml(detail)}</span>
            <span class="context-inline-status">${statusText}</span>
        `;
        group.appendChild(item);
    });

    if (insertBefore) {
        messagesDiv.insertBefore(group, insertBefore);
    } else {
        messagesDiv.appendChild(group);
    }

    return group;
};

OSA.addContextToolToGroup = function(event, isCompleted = false, isSuccess = false, messageIndex = 0) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    const groupId = `context-tool-group-${messageIndex}`;
    let group = document.getElementById(groupId);

    if (!group) {
        group = document.createElement('div');
        group.id = groupId;
        group.className = 'tool-container context-inline-group';
        group.dataset.messageIndex = messageIndex;

        const anchor = messagesDiv.querySelector(`.message[data-message-index="${messageIndex}"]`);
        if (anchor) {
            let next = anchor.nextElementSibling;
            while (next && !next.classList.contains('message')) {
                next = next.nextElementSibling;
            }
            if (next) {
                messagesDiv.insertBefore(group, next);
            } else {
                messagesDiv.appendChild(group);
            }
        } else {
            const allMsgs = Array.from(messagesDiv.querySelectorAll('.message'));
            const nextHigher = allMsgs.find(el => {
                const elIdx = parseInt(el.dataset.messageIndex || '', 10);
                return Number.isFinite(elIdx) && elIdx > messageIndex;
            });
            if (nextHigher) {
                messagesDiv.insertBefore(group, nextHigher);
            } else {
                messagesDiv.appendChild(group);
            }
        }
    }

    const toolName = event.tool_name;
    const args = event.arguments || {};
    const callId = event.tool_call_id;
    const existingItem = document.getElementById(`ctx-${callId}`);
    if (existingItem) {
        const statusEl = existingItem.querySelector('.context-inline-status');
        if (statusEl) {
            statusEl.textContent = isCompleted ? (isSuccess ? 'done' : 'failed') : 'running';
            statusEl.className = `context-inline-status${isCompleted ? (isSuccess ? ' done' : ' failed') : ' pending'}`;
        }
        return;
    }

    const label = OSA.toolLabel(toolName);
    const detail = OSA.summarizeToolArgs(toolName, args);
    const statusText = isCompleted ? (isSuccess ? 'done' : 'failed') : 'running';

    const item = document.createElement('div');
    item.className = 'context-inline-item';
    item.id = `ctx-${callId}`;
    item.innerHTML = `
        <span class="context-inline-action">${OSA.escapeHtml(label)}</span>
        <span class="context-inline-detail">${OSA.escapeHtml(detail)}</span>
        <span class="context-inline-status">${statusText}</span>
    `;
    group.appendChild(item);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;

    if (!isCompleted) {
        OSA.getActiveTools().set(callId, {
            contextItem: true,
            itemId: item.id,
            toolName,
            completed: false,
        });
    }
};

OSA.findLastContextGroupBefore = function(messagesDiv) {
    const groups = messagesDiv.querySelectorAll('.context-inline-group');
    return groups.length > 0 ? groups[groups.length - 1] : null;
};

OSA.ensureContextToolGroup = function() {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

    const activeGroupId = OSA._activeContextGroupId;
    if (activeGroupId) {
        const existing = document.getElementById(activeGroupId);
        if (existing) return existing;
    }

    const group = document.createElement('div');
    group.id = activeGroupId || `context-tool-group-${Date.now()}`;
    group.className = 'tool-container context-inline-group';
    messagesDiv.appendChild(group);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;

    OSA._contextGroupState = { expanded: false, allDone: false };
    return group;
};

OSA.updateContextGroupCounts = function() {};

OSA.toggleContextGroup = function() {};

OSA.toggleToolCard = function(domId) {
    const activeTools = OSA.getActiveTools();
    const toolData = activeTools.get
        ? activeTools.get(domId)
        : null;

    const card = document.getElementById(`card-${domId}`);
    const body = document.getElementById(`body-${domId}`);
    const chevron = document.getElementById(`chevron-${domId}`);

    if (!card) return;

    if (body && body.style.display !== 'none') {
        body.classList.remove('visible');
        body.style.display = 'none';
        if (chevron) chevron.classList.remove('open');
    } else {
        if (body) {
            body.classList.add('visible');
            body.style.display = '';
        }
        if (chevron) chevron.classList.add('open');
    }
};

OSA.updateToolProgress = function(event) {
    const activeTools = OSA.getActiveTools();
    const toolData = activeTools.get(event.tool_call_id);
    if (!toolData) return;

    if (toolData.contextItem) {
        const item = document.getElementById(toolData.itemId);
        if (!item) return;
        const state = item.querySelector('.context-tool-status');
        if (state && event.status) state.textContent = event.status.toLowerCase();
        return;
    }

    const status = document.getElementById(`status-${toolData.domId}`);
    if (status && event.status) {
        status.textContent = event.status.toLowerCase();
    }
};

OSA.completeToolCard = function(event) {
    const activeTools = OSA.getActiveTools();
    const toolData = activeTools.get(event.tool_call_id);
    if (!toolData) return;

    if (toolData.contextItem) {
        const item = document.getElementById(toolData.itemId);
        if (item) {
            const state = item.querySelector('.context-inline-status');
            if (state) {
                state.textContent = event.success ? 'done' : 'failed';
                state.classList.remove('pending');
                if (event.success) state.classList.add('done');
                else state.classList.add('failed');
            }
        }
        activeTools.delete(event.tool_call_id);
        return;
    }

    const card = document.getElementById(`card-${toolData.domId}`);
    if (card) {
        card.classList.add('tool-complete');
        setTimeout(() => card.classList.remove('tool-complete'), 400);

        card.querySelectorAll('.tool-title-pending').forEach(el => {
            el.classList.remove('tool-title-pending');
        });
    }

    const status = document.getElementById(`status-${toolData.domId}`);
    const chevron = document.getElementById(`chevron-${toolData.domId}`);

    if (status) {
        status.textContent = event.success ? 'done' : 'failed';
        status.classList.remove('pending');
        if (event.success) status.classList.add('done');
        else status.classList.add('failed');
    }

    if (chevron) {
        chevron.style.opacity = '';
    }

    if (toolData.isPanel) {
        const output = document.getElementById(`output-${toolData.domId}`);
        if (output) {
            const formatted = OSA.formatToolOutput(toolData.toolName, event.output || '');
            if (formatted) {
                if (toolData.toolName === 'subagent') {
                    const linkified = OSA.linkifySessionIds(OSA.escapeHtml(formatted));
                    output.innerHTML = linkified;
                } else {
                    output.textContent = formatted;
                }
                output.style.display = '';
            }
        }

        if (card && event.success && ['write_file', 'edit_file', 'apply_patch'].includes(toolData.toolName)) {
            const diff = OSA.parseDiffChanges(event.output || '');
            if (diff.additions > 0 || diff.deletions > 0) {
                const subtitle = document.getElementById(`subtitle-${toolData.domId}`);
                if (subtitle) {
                    let badges = subtitle.textContent;
                    badges += ` <span class="diff-add">+${diff.additions}</span><span class="diff-del">-${diff.deletions}</span>`;
                    subtitle.innerHTML = badges;
                }
            }
        }

        if (toolData.toolName === 'bash' && event.success) {
            const body = document.getElementById(`body-${toolData.domId}`);
            const argsEl = card?.querySelector('.tool-args');
            if (argsEl) argsEl.style.display = 'none';

            const cmd = (event.arguments?.command || '').trim();
            if (cmd) {
                const cmdLine = document.createElement('div');
                cmdLine.className = 'shell-command-line';
                cmdLine.innerHTML = `<span class="shell-prompt">$</span> <span class="shell-cmd">${OSA.escapeHtml(cmd)}</span>`;
                const bodyInner = body?.querySelector('.tool-body-inner');
                if (bodyInner) bodyInner.insertBefore(cmdLine, bodyInner.firstChild);
            }
        }
    }

    const parallelGroup = OSA.parallelToolGroups.find(g =>
        g.callIds.includes(event.tool_call_id)
    );

    if (parallelGroup && !parallelGroup.groupId && parallelGroup.count >= 2) {
        const firstCallId = parallelGroup.callIds[0];
        const firstToolData = activeTools.get(firstCallId);
        const firstContainer = firstToolData ? document.getElementById(firstToolData.domId) : null;

        if (firstContainer) {
            const messagesDiv = document.getElementById('messages');
            const groupId = `parallel-group-${Date.now()}`;
            parallelGroup.groupId = groupId;

            const groupDiv = document.createElement('div');
            groupDiv.className = 'parallel-group';
            groupDiv.id = groupId;
            groupDiv.innerHTML = `
                <div class="parallel-group-header">
                    <span class="parallel-count">${parallelGroup.count} tools running concurrently</span>
                </div>
            `;

            firstContainer.parentNode.insertBefore(groupDiv, firstContainer);
            groupDiv.appendChild(firstContainer);

            for (let i = 1; i < parallelGroup.callIds.length; i++) {
                const callId = parallelGroup.callIds[i];
                const toolData = activeTools.get(callId);
                if (toolData) {
                    const container = document.getElementById(toolData.domId);
                    if (container && container.parentNode !== groupDiv) {
                        groupDiv.appendChild(container);
                    }
                }
            }
        }
    } else if (parallelGroup && parallelGroup.groupId) {
        const groupDiv = document.getElementById(parallelGroup.groupId);
        if (groupDiv) {
            const container = document.getElementById(toolData.domId);
            if (container && container.parentNode !== groupDiv) {
                groupDiv.appendChild(container);
            }
        }
    }

    toolData.completed = true;
    activeTools.delete(event.tool_call_id);
};

OSA.formatToolOutput = function(toolName, output) {
    if (!output) return '';

    if (toolName === 'bash') {
        const lines = output.replace(/\r/g, '').split('\n');
        const trimmed = lines.map(l => l.trimEnd()).filter(Boolean);
        return trimmed.length > 80
            ? '\u2026\n' + trimmed.slice(-80).join('\n')
            : output;
    }

    if (['write_file', 'edit_file', 'apply_patch'].includes(toolName)) {
        return output.length > 4000
            ? output.slice(0, 4000) + '\n\u2026[truncated]'
            : output;
    }

    return output;
};

OSA.linkifySessionIds = function(text) {
    const uuidRegex = /\b([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\b/gi;
    return text.replace(uuidRegex, function(match, uuid) {
        return `<a class="subagent-link" href="#session=${uuid}" onclick="event.preventDefault(); event.stopPropagation(); OSA.openSubagentSession('${uuid}')">${uuid}</a>`;
    });
};

OSA.renderTaskMessage = function(event) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    let content = event.output || '';
    content = content.replace(/\s{2,}/g, ' ').trim();

    const uuidRegex = /\b([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\b/gi;
    content = content.replace(uuidRegex, function(match, uuid) {
        return `<a class="subagent-link" href="#session=${uuid}" onclick="event.preventDefault(); event.stopPropagation(); OSA.openSubagentSession('${uuid}')">${uuid}</a>`;
    });

    const message = document.createElement('div');
    message.className = 'message task';
    message.innerHTML = `
        <div class="message-role">Tasks</div>
        <div class="message-content">${OSA.formatMessage(content)}</div>
    `;
    messagesDiv.appendChild(message);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
};

OSA.handleEventError = function(event) {
    console.error('Agent error:', event.error);
    if (OSA.getCurrentSession()) OSA.getCurrentSession().task_status = 'active';
    OSA.setProcessing(false);
    OSA.resetSendButton();
    OSA.completeThinkingDisplay();
    OSA.pruneEmptyStreamingMessage();
    OSA.completeAssistantResponse();
    OSA.hideThinkingIndicator();

    const messagesDiv = document.getElementById('messages');
    messagesDiv.insertAdjacentHTML('beforeend', `
        <div class="message error">
            <div class="message-role">Error</div>
            <div class="message-content">${OSA.escapeHtml(event.error)}</div>
        </div>
    `);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    if (OSA.refreshCurrentSessionQueue) OSA.refreshCurrentSessionQueue();
};

OSA.handleEventCancelled = function(event) {
    if (OSA.getCurrentSession()) OSA.getCurrentSession().task_status = 'active';
    OSA.setProcessing(false);
    OSA.setStopping(false);
    OSA.resetSendButton();
    OSA.completeThinkingDisplay();
    OSA.pruneEmptyStreamingMessage();
    OSA.completeAssistantResponse();
    OSA.hideThinkingIndicator();

    if (OSA._stopTimeout) {
        clearTimeout(OSA._stopTimeout);
        OSA._stopTimeout = null;
    }

    const messagesDiv = document.getElementById('messages');
    messagesDiv.insertAdjacentHTML('beforeend', `
        <div class="message cancelled">
            <div class="message-role">Cancelled</div>
            <div class="message-content">Operation stopped by user</div>
        </div>
    `);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    if (OSA.refreshCurrentSessionQueue) OSA.refreshCurrentSessionQueue();
};

OSA._activeSubagents = new Map();

OSA.syncSubagentCardState = function(task) {
    if (!task || !task.session_id) return;

    const subagentId = task.session_id;
    const status = task.status || 'running';
    const isRunning = !!task.is_running;
    const toolCount = task.tool_count || 0;
    const result = task.result || '';

    const statusBadge = document.getElementById(`subagent-status-${subagentId}`);
    if (statusBadge) {
        const badgeStatus = isRunning ? 'running' : status;
        statusBadge.textContent = badgeStatus;
        statusBadge.className = `subagent-status-badge ${badgeStatus}`;
    }

    const countEl = document.getElementById(`subagent-count-${subagentId}`);
    if (countEl) {
        countEl.textContent = `${toolCount} tool${toolCount !== 1 ? 's' : ''}`;
    }

    const promptEl = document.getElementById(`subagent-prompt-${subagentId}`);
    if (promptEl && task.prompt) {
        promptEl.textContent = task.prompt;
    }

    const resultEl = document.getElementById(`subagent-result-${subagentId}`);
    if (resultEl) {
        if (result) {
            resultEl.style.display = 'block';
            resultEl.innerHTML = `<div class="subagent-result-label">Result:</div><div class="subagent-result-text">${OSA.escapeHtml(result.slice(0, 500))}${result.length > 500 ? '\u2026' : ''}</div>`;
        } else if (!isRunning) {
            resultEl.style.display = 'none';
            resultEl.innerHTML = '';
        }
    }

    const cancelBtnId = `subagent-cancel-${subagentId}`;
    let cancelBtn = document.getElementById(cancelBtnId);
    if (isRunning) {
        if (!cancelBtn) {
            const actions = document.querySelector(`#subagent-${subagentId} .subagent-actions`);
            if (actions) {
                cancelBtn = document.createElement('button');
                cancelBtn.id = cancelBtnId;
                cancelBtn.className = 'subagent-btn subagent-btn-cancel';
                cancelBtn.textContent = 'Cancel';
                cancelBtn.onclick = () => OSA.cancelSubagent(subagentId);
                actions.appendChild(cancelBtn);
            }
        }
    } else if (cancelBtn) {
        cancelBtn.remove();
    }

    OSA.updateSubagentContextRing(subagentId, task.context_state);

    if (isRunning) {
        OSA._activeSubagents.set(subagentId, {
            id: subagentId,
            description: task.description || 'Subagent task',
            agentType: task.agent_type || 'general',
            toolCount,
            status: 'running',
            result
        });
    } else {
        OSA._activeSubagents.delete(subagentId);
    }
};

OSA.updateSubagentContextRing = function(subagentId, contextState) {
    const metrics = OSA.getContextRingMetrics(contextState);
    if (!metrics) return;
    const ring = document.getElementById(`subagent-context-ring-${subagentId}`);
    if (!ring) return;
    const progress = ring.querySelector('.context-ring-progress');
    const pctEl = ring.querySelector('.context-ring-text');
    if (progress) {
        progress.style.strokeDashoffset = metrics.offset;
    }
    if (pctEl) pctEl.textContent = `${metrics.pct}%`;
    ring.classList.remove('warning', 'danger');
    if (metrics.pct >= 90) ring.classList.add('danger');
    else if (metrics.pct >= 70) ring.classList.add('warning');
    ring.title = `Context: ${metrics.pct}%`;
};

OSA.restoreSubagentCards = function(subagentTasks) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv || !subagentTasks || subagentTasks.length === 0) return;

    const sorted = [...subagentTasks].sort((a, b) => {
        const ta = a.created_at ? new Date(a.created_at).getTime() : 0;
        const tb = b.created_at ? new Date(b.created_at).getTime() : 0;
        return ta - tb;
    });

    const allMessages = Array.from(messagesDiv.querySelectorAll('.message'));

    const groupedByAnchor = new Map();

    sorted.forEach(task => {
        const subagentId = task.session_id;
        const existingCard = document.getElementById(`subagent-${subagentId}`);
        if (existingCard) {
            OSA.syncSubagentCardState(task);
            return;
        }

        const description = task.description || 'Subagent task';
        const agentType = task.agent_type || 'general';
        const status = task.status || 'running';
        const toolCount = task.tool_count || 0;
        const isRunning = task.is_running;
        const result = task.result || '';
        const prompt = task.prompt || '';
        const contextRingHtml = OSA.buildContextRingHtml(task.context_state, subagentId);

        const container = document.createElement('div');
        container.id = `subagent-${subagentId}`;
        container.className = 'subagent-card';
        container.innerHTML = `
            <div class="subagent-header" onclick="OSA.toggleSubagentCard('${subagentId}')">
                <div class="subagent-info">
                    <span class="subagent-icon">A</span>
                    <span class="subagent-title">${OSA.escapeHtml(description)}</span>
                    <span class="subagent-type">${OSA.escapeHtml(agentType)}</span>
                </div>
                <div class="subagent-status">
                    ${contextRingHtml}
                    <span class="subagent-status-badge ${isRunning ? 'running' : status}" id="subagent-status-${subagentId}">${isRunning ? 'running' : status}</span>
                    <span class="subagent-tool-count" id="subagent-count-${subagentId}">${toolCount} tool${toolCount !== 1 ? 's' : ''}</span>
                    <span class="subagent-chevron" id="subagent-chevron-${subagentId}">&#x25B6;</span>
                </div>
            </div>
            <div class="subagent-body" id="subagent-body-${subagentId}" style="display:none">
                <div class="subagent-body-inner">
                    ${prompt ? `<div class="subagent-prompt" id="subagent-prompt-${subagentId}">${OSA.escapeHtml(prompt)}</div>` : ''}
                    <div class="subagent-tools" id="subagent-tools-${subagentId}"></div>
                    <div class="subagent-result" id="subagent-result-${subagentId}" style="${result ? 'display:block' : 'display:none'}">${result ? `<div class="subagent-result-label">Result:</div><div class="subagent-result-text">${OSA.escapeHtml(result.slice(0, 500))}${result.length > 500 ? '\u2026' : ''}</div>` : ''}</div>
                    <div class="subagent-actions">
                        <button class="subagent-btn" onclick="OSA.openSubagentSession('${subagentId}')">Open Session</button>
                        ${isRunning ? `<button class="subagent-btn subagent-btn-cancel" id="subagent-cancel-${subagentId}" onclick="OSA.cancelSubagent('${subagentId}')">Cancel</button>` : ''}
                    </div>
                </div>
            </div>
        `;

        let anchorIdx = null;
        if (allMessages.length > 0 && task.created_at) {
            const subTs = new Date(task.created_at).getTime();
            for (let i = allMessages.length - 1; i >= 0; i--) {
                const msgTs = parseInt(allMessages[i].dataset.ts, 10) || 0;
                if (msgTs <= subTs) {
                    anchorIdx = i;
                    break;
                }
            }
        }
        const key = anchorIdx !== null ? anchorIdx : -1;
        if (!groupedByAnchor.has(key)) groupedByAnchor.set(key, []);
        groupedByAnchor.get(key).push(container);

        if (isRunning) {
            OSA._activeSubagents.set(subagentId, {
                id: subagentId,
                description,
                agentType,
                toolCount,
                status: 'running'
            });
        }
    });

    const sortedKeys = Array.from(groupedByAnchor.keys()).sort((a, b) => a - b);
    for (const key of sortedKeys) {
        const cards = groupedByAnchor.get(key);
        if (key === -1) {
            for (const card of cards) {
                messagesDiv.appendChild(card);
            }
        } else {
            const anchor = allMessages[key];
            let insertBefore = null;
            let sibling = anchor.nextElementSibling;
            while (sibling && !sibling.classList.contains('message')) {
                sibling = sibling.nextElementSibling;
            }
            insertBefore = sibling;
            for (const card of cards) {
                if (insertBefore) {
                    messagesDiv.insertBefore(card, insertBefore);
                } else {
                    messagesDiv.appendChild(card);
                }
            }
        }
    }
};

OSA.handleSubagentCreated = function(event) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    const subagentId = event.subagent_session_id;
    if (document.getElementById(`subagent-${subagentId}`)) return;
    const description = event.description || 'Subagent task';
    const agentType = event.agent_type || 'general';
    const prompt = event.prompt || '';

    const container = document.createElement('div');
    container.id = `subagent-${subagentId}`;
    container.className = 'subagent-card';
    container.innerHTML = `
        <div class="subagent-header" onclick="OSA.toggleSubagentCard('${subagentId}')">
            <div class="subagent-info">
                <span class="subagent-icon">A</span>
                <span class="subagent-title">${OSA.escapeHtml(description)}</span>
                <span class="subagent-type">${OSA.escapeHtml(agentType)}</span>
            </div>
            <div class="subagent-status">
                <span class="subagent-status-badge running" id="subagent-status-${subagentId}">running</span>
                <span class="subagent-tool-count" id="subagent-count-${subagentId}">0 tools</span>
                <span class="subagent-chevron" id="subagent-chevron-${subagentId}">&#x25B6;</span>
            </div>
        </div>
        <div class="subagent-body" id="subagent-body-${subagentId}" style="display:none">
            <div class="subagent-body-inner">
                <div class="subagent-prompt" id="subagent-prompt-${subagentId}">${prompt ? OSA.escapeHtml(prompt) : ''}</div>
                <div class="subagent-tools" id="subagent-tools-${subagentId}"></div>
                <div class="subagent-result" id="subagent-result-${subagentId}" style="display:none"></div>
                <div class="subagent-actions">
                    <button class="subagent-btn" onclick="OSA.openSubagentSession('${subagentId}')">Open Session</button>
                    <button class="subagent-btn subagent-btn-cancel" id="subagent-cancel-${subagentId}" onclick="OSA.cancelSubagent('${subagentId}')">Cancel</button>
                </div>
            </div>
        </div>
    `;
    messagesDiv.appendChild(container);
    messagesDiv.scrollTop = messagesDiv.scrollHeight;

    OSA._activeSubagents.set(subagentId, {
        id: subagentId,
        description,
        agentType,
        toolCount: 0,
        status: 'running'
    });

    OSA.loadSessions();
};

OSA.handleSubagentProgress = function(event) {
    const subagentId = event.subagent_session_id;
    const toolCount = event.tool_count || 0;
    const status = event.status || 'running';

    const countEl = document.getElementById(`subagent-count-${subagentId}`);
    if (countEl) {
        countEl.textContent = `${toolCount} tool${toolCount !== 1 ? 's' : ''}`;
    }

    const toolsEl = document.getElementById(`subagent-tools-${subagentId}`);
    if (toolsEl && event.tool_name) {
        const toolItem = document.createElement('div');
        toolItem.className = 'subagent-tool-item';
        toolItem.textContent = `${event.tool_name}`;
        toolsEl.appendChild(toolItem);
        toolsEl.scrollTop = toolsEl.scrollHeight;
    }

    const data = OSA._activeSubagents.get(subagentId);
    if (data) {
        data.toolCount = toolCount;
        data.status = status;
    }
};

OSA.handleSubagentCompleted = function(event) {
    const subagentId = event.subagent_session_id;
    const status = event.status || 'completed';
    const result = event.result || '';
    const toolCount = event.tool_count || 0;

    const statusBadge = document.getElementById(`subagent-status-${subagentId}`);
    if (statusBadge) {
        statusBadge.textContent = status;
        statusBadge.className = `subagent-status-badge ${status}`;
    }

    const countEl = document.getElementById(`subagent-count-${subagentId}`);
    if (countEl) {
        countEl.textContent = `${toolCount} tool${toolCount !== 1 ? 's' : ''}`;
    }

    const cancelBtn = document.getElementById(`subagent-cancel-${subagentId}`);
    if (cancelBtn) {
        cancelBtn.style.display = 'none';
    }

    const resultEl = document.getElementById(`subagent-result-${subagentId}`);
    if (resultEl && result) {
        resultEl.style.display = 'block';
        resultEl.innerHTML = `<div class="subagent-result-label">Result:</div><div class="subagent-result-text">${OSA.escapeHtml(result.slice(0, 500))}${result.length > 500 ? '\u2026' : ''}</div>`;
    }

    const data = OSA._activeSubagents.get(subagentId);
    if (data) {
        data.status = status;
        data.result = result;
    }

    OSA._activeSubagents.delete(subagentId);
    OSA.loadSessions();
};

OSA.handleSubagentComplete = function(event) {
    const output = event.output || '';
    const sessionMatch = output.match(/session:\s*([a-f0-9-]{36})/i) || output.match(/task_id:\s*([a-f0-9-]+)/i) || output.match(/Subagent Session ID:\s*([a-f0-9-]+)/i);
    if (sessionMatch) {
        const subagentId = sessionMatch[1];
        const data = OSA._activeSubagents.get(subagentId);
        if (data) {
            return;
        }
    }
};

OSA.toggleSubagentCard = function(subagentId) {
    const body = document.getElementById(`subagent-body-${subagentId}`);
    const chevron = document.getElementById(`subagent-chevron-${subagentId}`);
    if (!body) return;

    const isExpanded = body.style.display !== 'none';
    body.style.display = isExpanded ? 'none' : 'block';
    if (chevron) {
        chevron.style.transform = isExpanded ? '' : 'rotate(90deg)';
    }
};

OSA.openSubagentSession = function(subagentId) {
    if (OSA.selectSession) {
        OSA.selectSession(subagentId);
    } else {
        window.location.hash = `session=${subagentId}`;
        window.location.reload();
    }
};

OSA.persistToolStart = async function(event) {
    const session = OSA.getCurrentSession();
    if (!session) return;
    try {
        await fetch(`/api/sessions/${session.id}/tools`, {
            method: 'POST',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({
                tool_call_id: event.tool_call_id,
                tool_name: event.tool_name,
                arguments: event.arguments || {},
                message_index: event.message_index !== undefined ? event.message_index : 0
            })
        });
    } catch (e) {
        console.error('Failed to persist tool start:', e);
    }
};

OSA.persistToolComplete = async function(event) {
    const session = OSA.getCurrentSession();
    if (!session) return;
    try {
        await fetch(`/api/sessions/${session.id}/tools/${event.tool_call_id}`, {
            method: 'POST',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({
                success: event.success,
                output: typeof event.output === 'string' ? event.output : ''
            })
        });
    } catch (e) {
        console.error('Failed to persist tool complete:', e);
    }
};

OSA.cancelSubagent = async function(subagentId) {
    try {
        const response = await fetch(`/api/subagents/${subagentId}`, {
            method: 'DELETE',
            headers: OSA.getAuthHeaders()
        });
        if (response.ok) {
            const statusBadge = document.getElementById(`subagent-status-${subagentId}`);
            if (statusBadge) {
                statusBadge.textContent = 'cancelled';
                statusBadge.className = `subagent-status-badge cancelled`;
            }
            const cancelBtn = document.getElementById(`subagent-cancel-${subagentId}`);
            if (cancelBtn) {
                cancelBtn.style.display = 'none';
            }
        }
    } catch (err) {
        console.error('Failed to cancel subagent:', err);
    }
};

OSA.handleCoordinatorPhase = function(event) {
    const phase = event.phase || 'unknown';
    const workers = event.workers_spawned || 0;
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return;

    const phaseLabels = {
        research: 'Researching',
        synthesis: 'Synthesizing plan',
        implementation: 'Implementing',
        verification: 'Verifying',
        complete: 'Complete'
    };
    const label = phaseLabels[phase] || phase;

    let container = document.getElementById('coordinator-status');
    if (!container) {
        container = document.createElement('div');
        container.id = 'coordinator-status';
        container.className = 'coordinator-card';
        const lastMsg = messagesDiv.querySelector('.message:last-child');
        if (lastMsg) {
            lastMsg.appendChild(container);
        } else {
            messagesDiv.appendChild(container);
        }
    }

    if (phase === 'complete') {
        container.className = 'coordinator-card coordinator-complete';
        container.innerHTML = `<div class="coordinator-header"><span class="coordinator-icon">&#x2713;</span> <span class="coordinator-title">Coordinator finished</span></div>`;
        return;
    }

    container.className = 'coordinator-card coordinator-active';
    container.innerHTML = `<div class="coordinator-header"><span class="coordinator-icon coordinator-spinner">&#x26A1;</span> <span class="coordinator-title">Coordinator: ${label}</span> <span class="coordinator-workers">${workers} worker${workers !== 1 ? 's' : ''}</span></div>`;
};

OSA.startToolSync = function() {
    OSA.stopToolSync();
    const session = OSA.getCurrentSession();
    if (!session || !session.id) return;

    OSA._toolSyncInterval = setInterval(() => {
        OSA.syncToolsFromBackend();
    }, 2500);
};

OSA.stopToolSync = function() {
    if (OSA._toolSyncInterval) {
        clearInterval(OSA._toolSyncInterval);
        OSA._toolSyncInterval = null;
    }
};

OSA.syncToolsFromBackend = async function() {
    const session = OSA.getCurrentSession();
    if (!session || !session.id || session.task_status !== 'running') {
        OSA.stopToolSync();
        return;
    }
    try {
        const res = await fetch(`/api/sessions/${session.id}/tools`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (!res.ok) return;
        const tools = await res.json();
        if (!tools || tools.length === 0) return;

        const messagesDiv = document.getElementById('messages');
        if (!messagesDiv) return;

        // Build set of tool call IDs already in the DOM
        const existingContextIds = new Set();
        messagesDiv.querySelectorAll('.context-inline-item').forEach(el => {
            if (el.id && el.id.startsWith('ctx-')) existingContextIds.add(el.id);
        });
        const existingCardIds = new Set();
        messagesDiv.querySelectorAll('.tool-container:not(.context-inline-group)').forEach(el => {
            existingCardIds.add(el.id);
        });

        tools.forEach(t => {
            if (t.tool_name === 'subagent') return;
            const callId = t.tool_call_id;
            if (OSA.isContextTool(t.tool_name)) {
                if (!existingContextIds.has(`ctx-${callId}`)) {
                    OSA.restoreToolCard(t);
                }
            } else {
                if (!existingCardIds.has(`tool-${callId}`)) {
                    OSA.restoreToolCard(t);
                }
            }
        });
    } catch (e) {
        // swallow - will retry on next tick
    }
};

window.toggleToolCard = OSA.toggleToolCard;

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
    if (OSA.debug) {
        const summary = {};
        if (event.tool_name !== undefined) summary.tool = event.tool_name;
        if (event.tool_call_id !== undefined) summary.call = event.tool_call_id;
        if (event.message_index !== undefined) summary.idx = event.message_index;
        if (event.error !== undefined) summary.error = typeof event.error === 'string' ? event.error.slice(0, 200) : event.error;
        if (event.reason !== undefined) summary.reason = typeof event.reason === 'string' ? event.reason.slice(0, 200) : event.reason;
        if (event.sequence !== undefined) summary.seq = event.sequence;
        OSA.debug.log('event.' + event.type, summary);
    }
    const isStopping = OSA.isAgentStopping();
    const ignoreDuringStop = ['thinking', 'thinking_start', 'thinking_delta', 'thinking_end', 'response_start', 'response_chunk', 'tool_start', 'tool_progress', 'tool_complete', 'context_update', 'subagent_created', 'subagent_progress', 'subagent_completed', 'retry', 'compaction', 'step_finish', 'reasoning', 'question_asked', 'workflow_started', 'workflow_node_started', 'workflow_node_completed', 'workflow_node_failed', 'workflow_completed', 'workflow_failed'];
    
    if (isStopping && ignoreDuringStop.includes(event.type)) {
        return;
    }

    const chain = OSA.getMessageChain();
    // The sequence counter is per-session. A counter left over from another
    // session must not filter this one: the server resumes the live channel
    // from it, and a stale high value makes the server drop every event
    // (endless "thinking"), while a zeroed one replays the whole history.
    const eventSessionId = event.session_id || '';
    if (eventSessionId && chain.eventSessionId !== eventSessionId) {
        chain.eventSessionId = eventSessionId;
        chain.eventSeqNumber = 0;
    }
    const hasServerSeq = Number.isFinite(event.sequence);
    const seq = hasServerSeq ? Number(event.sequence) : (chain.eventSeqNumber + 1);
    // Drop anything already seen within this session: reconnect replays,
    // overlapping transports and mid-turn session snapshots can redeliver an
    // event, and without this the duplicate chunks leak into thinking and
    // content as stuttered words. Sequence 0 is the connection-local "session
    // is processing" placeholder, which is never replayed, so it is exempt.
    if (hasServerSeq && seq > 0 && seq <= chain.eventSeqNumber) {
        return;
    }
    // Never lower the counter: events can arrive out of order across a
    // reconnect, and the synthetic sequence-0 event must not wipe it.
    chain.eventSeqNumber = Math.max(chain.eventSeqNumber, seq);
    const prevType = chain.lastEventType;

    switch (event.type) {
        case 'thinking':
            OSA.setHasReceivedResponse(false);
            if (OSA.getCurrentSession()) OSA.getCurrentSession().task_status = 'running';
            OSA.setProcessing(true);
            OSA.setStopping(false);
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
            OSA.clearRetryNotice();
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
            OSA.clearRetryNotice();
            chain.lastToolStartSeq = seq;
            chain.pendingToolCallIds = chain.pendingToolCallIds || [];
            if (event.tool_call_id && !chain.pendingToolCallIds.includes(event.tool_call_id)) {
                chain.pendingToolCallIds.push(event.tool_call_id);
            }
            OSA.completeThinkingDisplay();
            OSA.tmodelFinalizeSegmentForToolCall();
            OSA.tmodelToolStart(event);
            OSA.persistToolStart(event);
            OSA.speakToolStart(event);
            OSA.renderQueuedMessages(OSA.getSessionQueue());
            break;

        case 'tool_progress':
            OSA.tmodelToolProgress(event);
            break;

        case 'tool_complete':
            if (event.tool_call_id) {
                chain.pendingToolCallIds = (chain.pendingToolCallIds || []).filter(id => id !== event.tool_call_id);
            }
            OSA.tmodelToolComplete(event);
            OSA.persistToolComplete(event);
            if (event.tool_name === 'todowrite' || event.tool_name === 'todoread') {
                OSA.fetchAndRenderTodos();
            }
            if (['write_file', 'edit_file', 'apply_patch', 'delete_file', 'batch'].includes(event.tool_name)) {
                OSA.scheduleSessionInspectorRefresh();
            }
            OSA.speakToolComplete(event);
            OSA.previewReadToolOutput(event);
            break;

        case 'response_complete':
            chain.pendingToolCallIds = [];
            chain.lastAssistantDomId = null;
            OSA.setHasReceivedResponse(true);
            if (OSA.getCurrentSession()) OSA.getCurrentSession().task_status = 'active';
            OSA.completeAssistantResponse(event.usage || null);
            OSA.hideThinkingIndicator();
            OSA.stopToolSync();
            if (OSA._stopTimeout) { clearTimeout(OSA._stopTimeout); OSA._stopTimeout = null; }
            var queueStillHasItems = (OSA.getSessionQueue() || []).length > 0;
            if (!queueStillHasItems) {
                OSA.setProcessing(false);
                OSA.setStopping(false);
                OSA.resetSendButton();
            } else {
                OSA.setStopping(false);
                OSA.setSendButtonStopMode(true);
            }
            OSA.scheduleSessionInspectorRefresh();
            if (OSA.refreshCurrentSessionQueue) OSA.refreshCurrentSessionQueue();
            OSA.maybeAutoNameSession();
            break;

        case 'queued_message_dispatched':
            chain.lastAssistantDomId = null;
            OSA.handleQueuedMessageDispatched(event);
            break;

        case 'context_update':
            OSA.updateContextStatus(event);
            if (event.subagent_session_id) {
                OSA.tmodelSubagentContextUpdate(event);
                OSA.updateSubagentContextRing(event.subagent_session_id, event);
            }
            break;

        case 'retry':
            if (event.subagent_session_id) {
                OSA.handleSubagentRetry(event);
            } else {
                OSA.showRetryNotice(event);
            }
            OSA.scheduleSessionInspectorRefresh();
            break;

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
            OSA.stopToolSync();
            if (OSA._stopTimeout) { clearTimeout(OSA._stopTimeout); OSA._stopTimeout = null; }
            OSA.handleEventError(event);
            OSA.setStopping(false);
            var errorQueueStillHasItems = (OSA.getSessionQueue() || []).length > 0;
            if (!errorQueueStillHasItems) {
                OSA.setProcessing(false);
                OSA.resetSendButton();
            } else {
                OSA.setSendButtonStopMode(true);
            }
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

        case 'workflow_started':
            OSA.handleWorkflowStarted(event);
            break;

        case 'workflow_node_started':
            OSA.handleWorkflowNodeStarted(event);
            break;

        case 'workflow_node_completed':
            OSA.handleWorkflowNodeCompleted(event);
            break;

        case 'workflow_node_failed':
            OSA.handleWorkflowNodeFailed(event);
            break;

        case 'workflow_completed':
            OSA.handleWorkflowCompleted(event);
            break;

        case 'workflow_failed':
            OSA.handleWorkflowFailed(event);
            break;

        case 'workflow_approval_requested':
            OSA.handleWorkflowApprovalRequested(event);
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
    const toolsRow = document.getElementById('ctx-tools-row');

    if ((state.tool_schema_tokens || 0) > 0) {
        toolsRow.classList.remove('hidden');
        document.getElementById('ctx-tools').textContent = formatTokens(state.tool_schema_tokens);
    } else {
        toolsRow.classList.add('hidden');
    }
    
    if (actualUsage && (actualUsage.input > 0 || actualUsage.total > 0)) {
        actualRow.classList.remove('hidden');
        document.getElementById('ctx-actual-input').textContent = formatTokens(actualUsage.input || actualUsage.total);
        
        if (actualUsage.output > 0) {
            outputRow.classList.remove('hidden');
            document.getElementById('ctx-output').textContent = formatTokens(actualUsage.output);
        } else {
            outputRow.classList.add('hidden');
        }
        
        const cacheRead = actualUsage.cached_read || 0;
        const cacheWrite = actualUsage.cached_write || 0;
        if (cacheRead > 0 || cacheWrite > 0) {
            cacheRow.classList.remove('hidden');
            const parts = [];
            if (cacheRead > 0) parts.push('R:' + formatTokens(cacheRead));
            if (cacheWrite > 0) parts.push('W:' + formatTokens(cacheWrite));
            document.getElementById('ctx-cache').textContent = parts.join(' / ');
        } else {
            cacheRow.classList.add('hidden');
        }
    } else {
        actualRow.classList.add('hidden');
        outputRow.classList.add('hidden');
        cacheRow.classList.add('hidden');
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

OSA.getToolDiffFiles = function(toolEvent) {
    const metadata = toolEvent && toolEvent.metadata && typeof toolEvent.metadata === 'object'
        ? toolEvent.metadata
        : null;
    if (!metadata || !Array.isArray(metadata.diff_files)) return [];
    return metadata.diff_files.filter(function(item) {
        return item && typeof item.path === 'string';
    });
};

OSA.renderToolDiff = function(outputEl, toolEvent) {
    if (!outputEl || !toolEvent) return false;
    const files = OSA.getToolDiffFiles(toolEvent);
    if (!files.length || typeof OSA.renderDiffView !== 'function') return false;

    outputEl.innerHTML = '';
    files.slice(0, 3).forEach(function(fileDiff) {
        const wrapper = document.createElement('div');
        wrapper.className = 'tool-file-diff';

        const header = document.createElement('div');
        header.className = 'tool-file-diff-header';
        header.innerHTML = '<span class="tool-file-diff-path">' + OSA.escapeHtml(fileDiff.path) + '</span>'
            + '<span class="tool-file-diff-status">' + OSA.escapeHtml(fileDiff.status || 'modified') + '</span>';
        wrapper.appendChild(header);

        const diffView = OSA.renderDiffView(fileDiff.old_content || '', fileDiff.new_content || '');
        wrapper.appendChild(diffView);
        outputEl.appendChild(wrapper);
    });

    return true;
};

OSA.previewReadToolOutput = function(toolEvent) {
    if (!toolEvent || toolEvent.tool_name !== 'read_file' || !toolEvent.success) return;
    if (typeof OSA.openFilePreview !== 'function') return;
    const payload = OSA.toolEventPreviewPayload(toolEvent);
    if (!payload || !payload.path) return;
    OSA.openFilePreview(payload.path, payload.content || '');
};

OSA.extractReadFileContent = function(rawOutput) {
    const raw = typeof rawOutput === 'string' ? rawOutput : '';
    if (!raw.trim()) return '';

    const match = raw.match(/<content>\s*([\s\S]*?)\s*<\/content>/i);
    let content = match ? match[1] : raw;
    content = content.split(/<system-reminder>/i)[0];
    let lines = content.replace(/\r\n/g, '\n').split('\n');

    const numberedLines = lines.filter(function(line) {
        return line.trim() !== '' && !/^\(End of file/i.test(line.trim());
    });
    const looksNumbered = numberedLines.length > 0 && numberedLines.every(function(line) {
        return /^\d+:\s/.test(line);
    });

    if (looksNumbered) {
        lines = lines
            .filter(function(line) { return !/^\(End of file/i.test(line.trim()); })
            .map(function(line) { return line.replace(/^\d+:\s?/, ''); });
    }

    return lines.join('\n').replace(/\n+$/, '');
};

OSA.extractReadFilePath = function(toolEvent) {
    const args = toolEvent && toolEvent.arguments ? toolEvent.arguments : {};
    const argPath = args.path || args.filePath || '';
    if (argPath) return argPath;
    const raw = typeof toolEvent?.output === 'string' ? toolEvent.output : '';
    const match = raw.match(/<path>([\s\S]*?)<\/path>/i);
    return match ? match[1].trim() : '';
};

OSA.ensureToolPreviewButton = function(card, domId) {
    if (!card) return null;
    let button = card.querySelector('.tool-preview-btn');
    if (button) return button;

    const trigger = card.querySelector('.tool-trigger-inline');
    const status = card.querySelector('.tool-status-badge');
    if (!trigger || !status) return null;

    button = document.createElement('button');
    button.type = 'button';
    button.className = 'tool-preview-btn hidden';
    button.title = 'Open in preview';
    button.setAttribute('aria-label', 'Open in preview');
    button.setAttribute('onclick', `OSA.openPreviewFromButton('${domId}', event)`);
    button.innerHTML = `
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 5h18"></path>
            <path d="M3 12h7"></path>
            <path d="M3 19h7"></path>
            <rect x="12" y="8" width="9" height="11" rx="1"></rect>
        </svg>
    `;
    trigger.insertBefore(button, status);
    return button;
};

OSA.ensureContextPreviewButton = function(item) {
    if (!item) return null;
    let button = item.querySelector('.context-inline-preview-btn');
    if (button) return button;

    const status = item.querySelector('.context-inline-status');
    if (!status) return null;
    button = document.createElement('button');
    button.type = 'button';
    button.className = 'context-inline-preview-btn hidden';
    button.title = 'Open in preview';
    button.setAttribute('aria-label', 'Open in preview');
    button.setAttribute('onclick', `OSA.openPreviewFromContextButton('${item.id}', event)`);
    button.innerHTML = `
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
            <path d="M3 5h18"></path>
            <path d="M3 12h7"></path>
            <path d="M3 19h7"></path>
            <rect x="12" y="8" width="9" height="11" rx="1"></rect>
        </svg>
    `;
    item.insertBefore(button, status);
    return button;
};

OSA.toolEventPreviewPayload = function(toolEvent) {
    if (!toolEvent || toolEvent.success !== true) return null;

    if (toolEvent.tool_name === 'read_file') {
        const path = OSA.extractReadFilePath(toolEvent);
        const output = OSA.extractReadFileContent(toolEvent.output);
        if (!path || !output.trim()) return null;
        return {
            path,
            content: output,
        };
    }

    if (toolEvent.tool_name === 'list_files') {
        const args = toolEvent.arguments || {};
        const path = args.path || args.filePath || '.';
        const content = OSA.formatToolOutput(toolEvent.tool_name, toolEvent.output || '');
        if (!content.trim()) return null;
        return { path, content };
    }

    if (toolEvent.tool_name === 'glob') {
        const pattern = (toolEvent.arguments && toolEvent.arguments.pattern) || '*';
        const content = OSA.formatToolOutput(toolEvent.tool_name, toolEvent.output || '');
        if (!content.trim()) return null;
        return {
            path: `glob: ${pattern}`,
            content,
        };
    }

    if (toolEvent.tool_name === 'grep') {
        const pattern = (toolEvent.arguments && toolEvent.arguments.pattern) || 'search';
        const content = OSA.formatToolOutput(toolEvent.tool_name, toolEvent.output || '');
        if (!content.trim()) return null;
        return {
            path: `grep: ${pattern}`,
            content,
        };
    }

    if (['write_file', 'edit_file', 'apply_patch'].includes(toolEvent.tool_name)) {
        const files = OSA.getToolDiffFiles(toolEvent);
        const first = files[0];
        if (!first || !first.path) return null;
        return {
            path: first.path,
            mode: 'diff',
            content: typeof first.new_content === 'string'
                ? first.new_content
                : (typeof first.old_content === 'string' ? first.old_content : ''),
            oldContent: typeof first.old_content === 'string' ? first.old_content : '',
            newContent: typeof first.new_content === 'string'
                ? first.new_content
                : (typeof first.old_content === 'string' ? first.old_content : ''),
        };
    }

    return null;
};

OSA.setToolCardPreviewData = function(domId, toolEvent) {
    const card = document.getElementById(`card-${domId}`);
    if (!card) return;
    const btn = OSA.ensureToolPreviewButton(card, domId);
    const payload = OSA.toolEventPreviewPayload(toolEvent);
    if (!payload) {
        delete card.dataset.previewPath;
        delete card.dataset.previewMode;
        delete card.dataset.previewContent;
        delete card.dataset.previewOldContent;
        delete card.dataset.previewNewContent;
        card.classList.remove('tool-previewable');
        if (btn) btn.classList.add('hidden');
        return;
    }

    card.dataset.previewPath = payload.path;
    card.dataset.previewMode = payload.mode || 'file';
    card.dataset.previewContent = payload.content;
    card.dataset.previewOldContent = payload.oldContent || '';
    card.dataset.previewNewContent = payload.newContent || '';
    card.classList.add('tool-previewable');
    if (btn) btn.classList.remove('hidden');
};

OSA.openPreviewFromCard = function(domId) {
    const card = document.getElementById(`card-${domId}`);
    if (!card || typeof OSA.openFilePreview !== 'function') return false;
    const path = card.dataset.previewPath || '';
    const content = card.dataset.previewContent || '';
    const mode = card.dataset.previewMode || 'file';
    if (!path) return false;
    OSA.openFilePreview(path, content, mode === 'diff'
        ? {
            mode: 'diff',
            oldContent: card.dataset.previewOldContent || '',
            newContent: card.dataset.previewNewContent || content,
        }
        : undefined);
    return true;
};

OSA.handleToolCardClick = function(domId) {
    OSA.toggleToolCard(domId);
};

OSA.openPreviewFromButton = function(domId, event) {
    if (event) {
        event.preventDefault();
        event.stopPropagation();
    }
    OSA.openPreviewFromCard(domId);
};

OSA.setContextToolPreviewData = function(item, toolEvent) {
    if (!item) return;
    const payload = OSA.toolEventPreviewPayload(toolEvent);
    const btn = OSA.ensureContextPreviewButton(item);
    if (!payload) {
        delete item.dataset.previewPath;
        delete item.dataset.previewMode;
        delete item.dataset.previewContent;
        delete item.dataset.previewOldContent;
        delete item.dataset.previewNewContent;
        item.classList.remove('previewable');
        if (btn) btn.classList.add('hidden');
        return;
    }

    item.dataset.previewPath = payload.path;
    item.dataset.previewMode = payload.mode || 'file';
    item.dataset.previewContent = payload.content;
    item.dataset.previewOldContent = payload.oldContent || '';
    item.dataset.previewNewContent = payload.newContent || '';
    item.classList.add('previewable');
    if (btn) btn.classList.remove('hidden');
};

OSA.openPreviewFromContextItem = function(itemId) {
    const item = document.getElementById(itemId);
    if (!item || typeof OSA.openFilePreview !== 'function') return false;
    const path = item.dataset.previewPath || '';
    const content = item.dataset.previewContent || '';
    const mode = item.dataset.previewMode || 'file';
    if (!path) return false;
    OSA.openFilePreview(path, content, mode === 'diff'
        ? {
            mode: 'diff',
            oldContent: item.dataset.previewOldContent || '',
            newContent: item.dataset.previewNewContent || content,
        }
        : undefined);
    return true;
};

OSA.handleContextToolClick = function(itemId) {
    OSA.openPreviewFromContextItem(itemId);
};

OSA.openPreviewFromContextButton = function(itemId, event) {
    if (event) {
        event.preventDefault();
        event.stopPropagation();
    }
    OSA.openPreviewFromContextItem(itemId);
};

OSA.toggleToolCard = function(domId) {
    const activeTools = OSA.getActiveTools();
    const toolData = activeTools.get
        ? activeTools.get(domId)
        : null;

    const card = document.getElementById(`card-${domId}`);
    const body = document.getElementById(`body-${domId}`);
    const chevron = document.getElementById(`chevron-${domId}`);

    if (!card) return;

    // `.tool-body` is collapsed by the stylesheet, so a freshly rendered card
    // has no inline display at all. Deriving the state from `style.display`
    // read that as "open" and spent the first click collapsing an already
    // collapsed body — which is why expanding took two clicks. The `.visible`
    // class is the real state; the inline style is only cleared so cards
    // toggled by the previous logic are not stuck behind `display: none`.
    const isOpen = body ? body.classList.contains('visible') : false;

    if (body) {
        body.classList.toggle('visible', !isOpen);
        body.style.display = '';
    }
    if (chevron) chevron.classList.toggle('open', !isOpen);
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
        return output.length > 1200
            ? output.slice(0, 1200) + '\n\u2026[truncated]'
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

OSA.showRetryNotice = function(event) {
    const root = OSA.getFloatingRoot();
    if (!root) return;
    const existing = document.getElementById('osa-retry-notice');
    if (existing) existing.remove();
    const delay = event.next_retry_in_ms ? Math.max(1, Math.round(event.next_retry_in_ms / 1000)) : null;
    const attempt = event.attempt_count || 0;
    const max = event.max_attempts || 0;
    let text = 'Provider request failed — retrying';
    if (delay) text += ` in ~${delay}s`;
    if (attempt && max) text += ` (attempt ${attempt}/${max})`;
    const chip = document.createElement('div');
    chip.id = 'osa-retry-notice';
    chip.className = 'retry-notice';
    chip.textContent = text;
    root.appendChild(chip);
    if (OSA.debug) {
        OSA.debug.log('retry.notice', { text, attempt, max, delay });
    }
};

OSA.clearRetryNotice = function() {
    const existing = document.getElementById('osa-retry-notice');
    if (existing) existing.remove();
};

OSA.handleEventError = function(event) {
    OSA.clearRetryNotice();
    console.error('Agent error:', event.error);
    if (OSA.getCurrentSession()) OSA.getCurrentSession().task_status = 'active';
    OSA.completeThinkingDisplay();
    OSA.pruneEmptyStreamingMessage();
    OSA.completeAssistantResponse();
    OSA.hideThinkingIndicator();

    OSA.tmodelAddError(event.error);
    OSA.tmodelMarkDirty('error');
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

    OSA.tmodelAddCancelled();
    OSA.tmodelMarkDirty('cancelled');
    OSA.renderQueuedMessages(OSA.getSessionQueue());
    if (OSA.refreshCurrentSessionQueue) OSA.refreshCurrentSessionQueue();
};

OSA._activeSubagents = new Map();

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

OSA.formatSubagentDuration = function(ms) {
    if (!Number.isFinite(ms) || ms <= 0) return '';
    const totalSeconds = Math.round(ms / 1000);
    if (totalSeconds < 60) return `${totalSeconds}s`;
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = totalSeconds % 60;
    if (minutes < 60) return seconds > 0 ? `${minutes}m ${seconds}s` : `${minutes}m`;
    const hours = Math.floor(minutes / 60);
    const restMinutes = minutes % 60;
    return restMinutes > 0 ? `${hours}h ${restMinutes}m` : `${hours}h`;
};

OSA.completedDurationMs = function(createdAt, completedAt) {
    const start = createdAt ? new Date(createdAt).getTime() : 0;
    const end = completedAt ? new Date(completedAt).getTime() : 0;
    if (!start || !end || end < start) return 0;
    return end - start;
};

OSA.handleSubagentCreated = function(event) {
    OSA.tmodelSubagentCreated(event);
    OSA.loadSessions();
};

OSA.handleSubagentProgress = function(event) {
    OSA.tmodelSubagentProgress(event);
};

OSA.handleSubagentRetry = function(event) {
    OSA.tmodelSubagentRetry(event);
};

OSA.handleSubagentCompleted = function(event) {
    OSA.tmodelSubagentCompleted(event);
    OSA.loadSessions();
};

OSA._activeWorkflows = OSA._activeWorkflows || new Map();

OSA.workflowNodeDomId = function(runId, nodeId) {
    return `workflow-node-${runId}-${String(nodeId || '').replace(/[^a-zA-Z0-9_-]/g, '_')}`;
};

OSA.ensureWorkflowCard = function(runId, workflowName) {
    let card = document.getElementById(`workflow-${runId}`);
    if (card) return card;

    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv) return null;

    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) emptyState.remove();

    card = document.createElement('div');
    card.id = `workflow-${runId}`;
    card.className = 'workflow-card';
    card.innerHTML = `
        <div class="workflow-header" onclick="OSA.toggleWorkflowCard('${runId}')">
            <div class="workflow-info">
                <span class="workflow-icon">WF</span>
                <span class="workflow-title">${OSA.escapeHtml(workflowName || 'Workflow')}</span>
            </div>
            <div class="workflow-status">
                <span class="workflow-status-badge running" id="workflow-status-${runId}">running</span>
                <span class="workflow-chevron" id="workflow-chevron-${runId}">&#x25B6;</span>
            </div>
        </div>
        <div class="workflow-body" id="workflow-body-${runId}" style="display:none">
            <div class="workflow-body-inner">
                <div class="workflow-nodes" id="workflow-nodes-${runId}"></div>
                <div class="workflow-output" id="workflow-output-${runId}" style="display:none"></div>
            </div>
        </div>
    `;

    OSA.mountFloatingNode(card);
    return card;
};

OSA.toggleWorkflowCard = function(runId) {
    const body = document.getElementById(`workflow-body-${runId}`);
    const chevron = document.getElementById(`workflow-chevron-${runId}`);
    if (!body) return;

    const isExpanded = body.style.display !== 'none';
    body.style.display = isExpanded ? 'none' : 'block';
    if (chevron) {
        chevron.style.transform = isExpanded ? '' : 'rotate(90deg)';
    }
};

OSA.setWorkflowStatus = function(runId, status) {
    const statusEl = document.getElementById(`workflow-status-${runId}`);
    if (!statusEl) return;
    statusEl.textContent = status;
    statusEl.className = `workflow-status-badge ${status}`;
};

OSA.upsertWorkflowNode = function(runId, nodeId, nodeType, status, details = '') {
    const nodesEl = document.getElementById(`workflow-nodes-${runId}`);
    if (!nodesEl) return;

    const domId = OSA.workflowNodeDomId(runId, nodeId);
    let row = document.getElementById(domId);
    if (!row) {
        row = document.createElement('div');
        row.id = domId;
        row.className = 'workflow-node-row';
        nodesEl.appendChild(row);
    }

    row.innerHTML = `
        <span class="workflow-node-state ${status}"></span>
        <span class="workflow-node-label">${OSA.escapeHtml(nodeType || nodeId || 'node')}</span>
        <span class="workflow-node-detail">${OSA.escapeHtml(details || status)}</span>
    `;
};

OSA.handleWorkflowStarted = function(event) {
    const runId = event.run_id;
    if (!runId) return;
    OSA.ensureWorkflowCard(runId, event.workflow_name || event.workflow_id || 'Workflow');
    OSA.setWorkflowStatus(runId, 'running');
    OSA._activeWorkflows.set(runId, {
        runId,
        workflowId: event.workflow_id,
        workflowName: event.workflow_name || event.workflow_id,
        status: 'running'
    });
};

OSA.handleWorkflowNodeStarted = function(event) {
    const runId = event.run_id;
    if (!runId) return;
    OSA.ensureWorkflowCard(runId, event.workflow_name || event.workflow_id || 'Workflow');
    OSA.upsertWorkflowNode(runId, event.node_id, event.node_type, 'running', 'running');
};

OSA.handleWorkflowNodeCompleted = function(event) {
    const runId = event.run_id;
    if (!runId) return;
    const detail = event.output_preview || 'completed';
    OSA.upsertWorkflowNode(runId, event.node_id, event.node_type, 'completed', detail);
};

OSA.handleWorkflowNodeFailed = function(event) {
    const runId = event.run_id;
    if (!runId) return;
    OSA.upsertWorkflowNode(runId, event.node_id, event.node_type, 'failed', event.error || 'failed');
    OSA.setWorkflowStatus(runId, 'failed');
};

OSA.handleWorkflowCompleted = function(event) {
    const runId = event.run_id;
    if (!runId) return;
    OSA.ensureWorkflowCard(runId, event.workflow_name || event.workflow_id || 'Workflow');
    OSA.setWorkflowStatus(runId, 'completed');
    const outputEl = document.getElementById(`workflow-output-${runId}`);
    if (outputEl) {
        let outputText = '';
        if (event.output && typeof event.output === 'object') {
            outputText = JSON.stringify(event.output, null, 2);
        } else if (typeof event.output === 'string') {
            outputText = event.output;
        }
        if (outputText) {
            outputEl.style.display = 'block';
            outputEl.innerHTML = `<div class="workflow-output-label">Output:</div><pre class="workflow-output-text">${OSA.escapeHtml(outputText)}</pre>`;
        }
    }

    const data = OSA._activeWorkflows.get(runId);
    if (data) {
        data.status = 'completed';
    }
};

OSA.handleWorkflowFailed = function(event) {
    const runId = event.run_id;
    if (!runId) return;
    OSA.ensureWorkflowCard(runId, event.workflow_name || event.workflow_id || 'Workflow');
    OSA.setWorkflowStatus(runId, 'failed');
    const outputEl = document.getElementById(`workflow-output-${runId}`);
    if (outputEl) {
        outputEl.style.display = 'block';
        outputEl.innerHTML = `<div class="workflow-output-label">Error:</div><pre class="workflow-output-text">${OSA.escapeHtml(event.error || 'Workflow failed')}</pre>`;
    }

    const data = OSA._activeWorkflows.get(runId);
    if (data) {
        data.status = 'failed';
    }
};

OSA.answerWorkflowApproval = async function(questionId, answer, button) {
    if (!questionId) return;
    try {
        await OSA.fetchWithAuth('/api/questions/answer', {
            method: 'POST',
            body: JSON.stringify({
                question_id: questionId,
                answers: [[answer]]
            })
        });

        const row = button?.closest('.workflow-approval-actions');
        if (row) {
            row.querySelectorAll('button').forEach(btn => {
                btn.disabled = true;
            });
        }
    } catch (err) {
        console.error('Failed to answer workflow approval:', err);
    }
};

OSA.handleWorkflowApprovalRequested = function(event) {
    const runId = event.run_id;
    if (!runId) return;
    OSA.ensureWorkflowCard(runId, event.workflow_name || event.workflow_id || 'Workflow');
    OSA.upsertWorkflowNode(runId, event.node_id, 'approval', 'running', 'awaiting approval');

    const nodesEl = document.getElementById(`workflow-nodes-${runId}`);
    if (!nodesEl) return;

    const container = document.createElement('div');
    container.className = 'workflow-approval';
    container.innerHTML = `
        <div class="workflow-approval-prompt">${OSA.escapeHtml(event.prompt || 'Approval required')}</div>
        <div class="workflow-approval-actions">
            <button class="workflow-approval-btn approve" onclick="OSA.answerWorkflowApproval('${OSA.escapeHtml(event.question_id || '')}', '${OSA.escapeHtml(event.approve_label || 'Approve')}', this)">${OSA.escapeHtml(event.approve_label || 'Approve')}</button>
            <button class="workflow-approval-btn reject" onclick="OSA.answerWorkflowApproval('${OSA.escapeHtml(event.question_id || '')}', '${OSA.escapeHtml(event.reject_label || 'Reject')}', this)">${OSA.escapeHtml(event.reject_label || 'Reject')}</button>
        </div>
    `;
    nodesEl.appendChild(container);
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
        await OSA.fetchWithAuth(`/api/sessions/${session.id}/tools`, {
            method: 'POST',
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
        await OSA.fetchWithAuth(`/api/sessions/${session.id}/tools/${event.tool_call_id}`, {
            method: 'POST',
            body: JSON.stringify({
                success: event.success,
                output: typeof event.output === 'string' ? event.output : '',
                title: typeof event.title === 'string' ? event.title : null,
                metadata: event.metadata && typeof event.metadata === 'object' ? event.metadata : null
            })
        });
    } catch (e) {
        console.error('Failed to persist tool complete:', e);
    }
};

OSA.cancelSubagent = async function(subagentId) {
    try {
        const response = await OSA.fetchWithAuth(`/api/subagents/${subagentId}`, {
            method: 'DELETE'
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
        OSA.mountFloatingNode(container);
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
    if (!session || !session.id) return;
    if (session.task_status !== 'running') {
        if (OSA.isAgentProcessing()) {
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
            OSA.hideThinkingIndicator();
            OSA.stopToolSync();
            OSA.refreshCurrentSessionQueue();
        } else {
            OSA.stopToolSync();
        }
        return;
    }
    if (OSA._toolSyncInFlight) return;
    OSA._toolSyncInFlight = true;
    try {
        const res = await fetch(`/api/sessions/${session.id}/tools`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (res.ok) {
            const tools = await res.json();
            if (Array.isArray(tools) && tools.length > 0) {
                let changed = false;
                tools.forEach(t => {
                    if (!t || !t.tool_call_id || t.tool_name === 'subagent') return;
                    const key = 'tool:' + t.tool_call_id;
                    const existing = OSA.tmodelGet(key);
                    if (existing) {
                        if (t.completed === true) {
                            const nextOutput = typeof t.output === 'string' ? t.output : existing.output;
                            const nextSuccess = t.success === true;
                            if (!existing.completed
                                || existing.output !== nextOutput
                                || existing.success !== nextSuccess
                                || existing.title !== (t.title || '')) {
                                existing.output = nextOutput;
                                existing.success = nextSuccess;
                                existing.completed = true;
                                existing.status = nextSuccess ? 'done' : 'failed';
                                existing.title = typeof t.title === 'string' ? t.title : existing.title;
                                existing.metadata = t.metadata || existing.metadata;
                                changed = true;
                            }
                        }
                        return;
                    }
                    OSA.tmodelAppend(OSA.tmodelToolItem({
                        tool_call_id: t.tool_call_id,
                        tool_name: t.tool_name,
                        arguments: t.arguments || {},
                        output: typeof t.output === 'string' ? t.output : '',
                        title: typeof t.title === 'string' ? t.title : '',
                        metadata: t.metadata,
                        message_index: t.message_index,
                        timestamp: t.timestamp,
                    }, { completed: t.completed === true, success: t.success === true, live: false }));
                    changed = true;
                });
                if (changed) {
                    OSA.tmodelMarkDirty('tool-sync');
                }
                if (OSA.debug) {
                    OSA.debug.log('tool.sync', { total: tools.length, changed });
                }
            }
        }

        if (typeof OSA.syncRunningSessionSnapshot === 'function') {
            OSA.syncRunningSessionSnapshot(session.id);
        }
    } catch (e) {
        // swallow - will retry on next tick
    } finally {
        OSA._toolSyncInFlight = false;
    }
};

window.toggleToolCard = OSA.toggleToolCard;

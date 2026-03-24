window.OSA = window.OSA || {};

OSA.getSessionDisplayName = function(session) {
    if (session.metadata?.name) return session.metadata.name;
    if (session.agent_type) return session.agent_type.charAt(0).toUpperCase() + session.agent_type.slice(1) + ' Agent';
    return 'Session';
};

OSA.checkAuthAndInit = async function() {
    try {
        const res = await fetch('/api/auth/status');
        const data = await res.json();
        
        if (!data.required) {
            const loginRes = await fetch('/api/auth/login', {
                method: 'POST',
                headers: { 'Content-Type': 'application/json' },
                body: JSON.stringify({ password: '' })
            });
            
            if (loginRes.ok) {
                const loginData = await loginRes.json();
                OSA.setToken(loginData.token);
                OSA.showApp();
                return;
            }
        }
        
        const token = OSA.getToken();
        if (token) {
            const validRes = await fetch('/api/sessions', {
                headers: { 'Authorization': `Bearer ${token}` }
            });
            if (validRes.ok) {
                OSA.showApp();
                return;
            } else {
                OSA.clearToken();
            }
        }
        
        OSA.showLogin();
    } catch (error) {
        console.error('Auth check failed:', error);
        OSA.showLogin();
    }
};

OSA.login = async function() {
    const password = document.getElementById('password-input').value;
    const errorDiv = document.getElementById('login-error');
    
    try {
        const res = await fetch('/api/auth/login', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ password })
        });
        
        const data = await res.json();
        
        if (!res.ok) {
            throw new Error(data.error || 'Invalid password');
        }
        
        OSA.setToken(data.token);
        OSA.showApp();
    } catch (error) {
        errorDiv.textContent = error.message;
        errorDiv.classList.remove('hidden');
    }
};

OSA.logout = function() {
    OSA.clearToken();
    OSA.setCurrentSession(null);
    OSA.setSessionInspectorState({ history: [], snapshots: [] });
    
    const es = OSA.getEventSource();
    if (es) {
        es.close();
        OSA.setEventSource(null);
    }
    const reconnectTimer = OSA.getEventReconnectTimer();
    if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        OSA.setEventReconnectTimer(null);
    }
    
    OSA.showLogin();
};

OSA.showLogin = function() {
    document.getElementById('login-view').classList.remove('hidden');
    document.getElementById('app-view').classList.add('hidden');
};

OSA.showApp = function() {
    document.getElementById('login-view').classList.add('hidden');
    document.getElementById('app-view').classList.remove('hidden');
    document.getElementById('app-view').style.display = 'flex';
    
    OSA.initSidebarState();
    OSA.loadSessions();
    OSA.loadWorkspaces();
    OSA.loadPersonaCatalog();
    OSA.loadSessionPersona();
    OSA.initVoice();
    OSA.loadModel();
    OSA.loadProviderCatalog();
    OSA.initTheme();
};

OSA.loadModel = async function() {
    try {
        const res = await fetch('/api/model', {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        if (!res.ok) {
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        const input = document.getElementById('model-input');
        if (input) {
            OSA.currentModelId = data.model || '';
            OSA.currentModelProviderId = data.provider_id || '';
            const provider = (OSA.providerCatalog.providers || []).find(function(item) { return item.id === data.provider_id; });
            input.value = provider ? provider.name + ' · ' + (data.model || '') : (data.model || '');
            input.title = provider ? provider.name + ' / ' + (data.model || '') : (data.model || '');
        }
        if (typeof OSA.refreshThinkingOptions === 'function') {
            const selected = OSA.getCachedConfig?.()?.agent?.thinking_level || 'auto';
            await OSA.refreshThinkingOptions(data.provider_id || '', data.model || '', selected);
        }
    } catch (error) {
        console.error('Failed to load model:', error);
    }
};

OSA.updateModel = async function() {
    const input = document.getElementById('model-input');
    if (!input) return;
    const model = input.value.trim();
    if (!model) {
        alert('Enter a model id');
        return;
    }
    try {
        const res = await fetch('/api/model', {
            method: 'POST',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ model })
        });
        const data = await res.json().catch(() => ({}));
        if (!res.ok) {
            throw new Error(data.error || `HTTP ${res.status}`);
        }
    } catch (error) {
        console.error('Failed to update model:', error);
        alert(error.message);
    }
};

OSA.loadSessions = async function() {
    try {
        const res = await fetch('/api/sessions', {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const sessions = await res.json();
        
        const currentSession = OSA.getCurrentSession();

        const childMap = new Map();
        const rootSessions = [];
        const orphanChildren = [];

        sessions.forEach(s => {
            if (s.parent_id) {
                const parentExists = sessions.some(p => p.id === s.parent_id);
                if (parentExists) {
                    if (!childMap.has(s.parent_id)) {
                        childMap.set(s.parent_id, []);
                    }
                    childMap.get(s.parent_id).push(s);
                } else {
                    orphanChildren.push(s);
                }
            } else {
                rootSessions.push(s);
            }
        });

        const renderSession = (s, isChild, hasRunningChildren) => {
            const indent = isChild ? 'padding-left: 24px;' : '';
            const childClass = isChild ? ' session-child' : '';
            const isActive = currentSession && currentSession.id === s.id;
            const displayName = OSA.getSessionDisplayName(s);
            const isRunning = s.task_status === 'running' || hasRunningChildren;
            const iconHtml = isRunning
                ? `<canvas class="session-icon-canvas" data-session-running="${s.id}"></canvas>`
                : (isChild ? 'A' : '#');
            const iconStyle = isChild && !isRunning ? 'style="width:24px;height:24px;font-size:10px;border-radius:4px;background:var(--bg-tertiary);color:var(--text-secondary);border:1px solid var(--border);"' : '';
            const iconClass = isRunning ? ' session-icon-running' : '';
            return `
            <div class="session-item${childClass} ${isActive ? 'active' : ''}" onclick="OSA.selectSession('${s.id}')" style="${indent}">
                <div class="session-icon${iconClass}" ${iconStyle}>${iconHtml}</div>
                <div class="session-info">
                    <div class="session-name">${OSA.escapeHtml(displayName)}</div>
                    <div class="session-date">${new Date(s.created_at).toLocaleDateString()}</div>
                </div>
                <div class="session-actions">
                    <button class="session-action-btn rename-btn" onclick="event.stopPropagation(); OSA.startRenameSession('${s.id}', this)" title="Rename">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
                            <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
                        </svg>
                    </button>
                    <button class="session-action-btn delete-btn" onclick="event.stopPropagation(); OSA.deleteSession('${s.id}')" title="Delete">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <polyline points="3 6 5 6 21 6"/>
                            <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/>
                        </svg>
                    </button>
                </div>
            </div>
            `;
        };

        const renderSessionGroup = (s) => {
            const children = childMap.get(s.id);
            const hasRunningChildren = children && children.some(c => c.task_status === 'running');
            let html = renderSession(s, false, hasRunningChildren);
            if (children && children.length > 0) {
                html += '<div class="session-children">';
                children.forEach(c => {
                    html += renderSession(c, true, false);
                });
                html += '</div>';
            }
            return html;
        };

        const list = document.getElementById('sessions-list');
        let sessionsHtml = rootSessions.map(s => renderSessionGroup(s)).join('');

        if (orphanChildren.length > 0) {
            sessionsHtml += '<div class="session-children" style="margin-left:0;border-left:none;padding-left:0;">';
            orphanChildren.forEach(c => {
                sessionsHtml += renderSession(c, true);
            });
            sessionsHtml += '</div>';
        }

        list.innerHTML = `
            <div class="session-search">
                <input type="text" id="session-search-input" placeholder="Search sessions..." oninput="OSA.filterSessions(this.value)" />
            </div>
            ${sessionsHtml}
        `;

        const activeEl = list.querySelector('.session-item.active');
        if (activeEl) {
            activeEl.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        }

        OSA._initSessionIconCanvases();
    } catch (error) {
        console.error('Failed to load sessions:', error);
    }
};

OSA.createSession = async function() {
    const ws = OSA.getWorkspaceState();
    const workspaceId = ws.activeWorkspace || 'default';
    
    try {
        const res = await fetch('/api/sessions', {
            method: 'POST',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ workspace_id: workspaceId })
        });
        const session = await res.json();
        OSA.setCurrentSession(session);
        OSA.getActiveTools().clear();
        OSA.parallelToolGroups = [];
        
        OSA.restoreContextState(session.id, null);
        OSA.connectEventSource(session.id);

        document.getElementById('messages').innerHTML = '';
        OSA.resetStreamingMessage();
        const sessionName = OSA.getSessionDisplayName(session);
        OSA.setHeaderBaseTitle(sessionName);
        document.getElementById('header-title').textContent = sessionName;
        OSA.setHeaderTitleRenameable(true);
        OSA.loadSessions();
        OSA.loadSessionWorkspace();
        OSA.loadSessionPersona();
    } catch (error) {
        console.error('Failed to create session:', error);
    }
};

OSA.syncRunningSessionSnapshot = async function(sessionId) {
    try {
        const currentSession = OSA.getCurrentSession();
        if (!currentSession || currentSession.id !== sessionId || currentSession.task_status !== 'running') {
            return;
        }

        const res = await fetch(`/api/sessions/${sessionId}`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (!res.ok) return;

        const session = await res.json();
        if (!OSA.getCurrentSession() || OSA.getCurrentSession().id !== sessionId) return;
        OSA.setCurrentSession(session);

        const streamingMessage = OSA.getStreamingAssistantMessage();
        const latestAssistant = OSA.getActiveTurnAssistantMessage(session);
        if (!latestAssistant) {
            if (streamingMessage) {
                OSA.releaseStreamingAssistantMessage();
            }
            return;
        }

        if (!streamingMessage) {
            OSA.renderMessages(session.messages || []);
            OSA.adoptStreamingAssistantFromRenderedSession(session);
            return;
        }

        const contentEl = streamingMessage.querySelector('.message-content');
        const nextContent = latestAssistant.content || '';
        if (contentEl && (contentEl.dataset.rawText || '') !== nextContent) {
            OSA.scheduleFormattedRender(contentEl, nextContent);
        }

        if (OSA.getShowThinkingBlocks() && (latestAssistant.thinking || '').trim()) {
            const container = OSA.ensureThinkingContainer(streamingMessage);
            const body = container ? container.querySelector('.thinking-body') : null;
            if (body && (body.dataset.rawText || '') !== (latestAssistant.thinking || '')) {
                OSA.scheduleFormattedRender(body, latestAssistant.thinking || '');
                OSA.setThinkingPreview(container, latestAssistant.thinking || '');
            }
        }

        OSA.prepareAssistantMessageElementForStreaming(streamingMessage, latestAssistant, OSA.getShowThinkingBlocks());
    } catch (error) {
        console.error('Failed to sync running session snapshot:', error);
    }
};

OSA.selectSession = async function(sessionId) {
    try {
        const res = await fetch(`/api/sessions/${sessionId}`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const session = await res.json();
        OSA.setCurrentSession(session);
        OSA.restoreContextState(session.id, session.context_state || null);
        
        document.querySelectorAll('.tool-card, .context-tool-group, .subagent-card, .parallel-group').forEach(el => el.remove());
        OSA.getActiveTools().clear();
        OSA.parallelToolGroups = [];
        OSA._contextGroupState = null;
        
        OSA.connectEventSource(sessionId);
        OSA.resetStreamingMessage();
        
        const sessionName = OSA.getSessionDisplayName(session);
        OSA.setHeaderBaseTitle(sessionName);
        document.getElementById('header-title').textContent = sessionName;
        OSA.setHeaderTitleRenameable(true);
        
        const messagesDiv = document.getElementById('messages');
        messagesDiv.innerHTML = '';
        
        const [toolStartsRes, subagentsRes] = await Promise.all([
            fetch(`/api/sessions/${sessionId}/tools`, {
                headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
            }).catch(() => null),
            fetch(`/api/sessions/${sessionId}/subagents`, {
                headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
            }).catch(() => null)
        ]);
        const tools = (toolStartsRes && toolStartsRes.ok) ? await toolStartsRes.json() : [];
        const subagentsData = (subagentsRes && subagentsRes.ok) ? await subagentsRes.json() : { subagents: [], has_running: false };
        
        if (session.messages.length === 0) {
            messagesDiv.innerHTML = `
                <div class="empty-state">
                    <div class="empty-state-icon">+</div>
                    <div class="empty-state-title">Start a conversation</div>
                    <div class="empty-state-text">Type a message below</div>
                </div>
            `;
        } else {
            OSA.renderMessages(session.messages);
            if (session.task_status === 'running') {
                OSA.adoptStreamingAssistantFromRenderedSession(session);
            }
            if (tools && tools.length > 0) {
                OSA.restoreToolsAtPositions(tools);
            }
        }

        if (subagentsData && subagentsData.subagents && subagentsData.subagents.length > 0) {
            OSA.restoreSubagentCards(subagentsData.subagents);
        }

        const sessionIsRunning = session.task_status === 'running';
        const subagentsRunning = subagentsData && subagentsData.has_running;
        const isDirectlyRunning = sessionIsRunning && !subagentsRunning;

        if (sessionIsRunning || subagentsRunning) {
            OSA.setProcessing(true);
            OSA.setSendButtonStopMode(true);
        } else {
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
        }

        if (isDirectlyRunning) {
            const msgs = session.messages || [];
            const hasPendingTools = tools.some(t => !t.completed);
            const lastUserMsgIdx = msgs.reduce((acc, m, i) => m.role === 'user' ? i : acc, -1);
            const lastMsg = msgs[msgs.length - 1];
            const waitingForResponse = lastUserMsgIdx >= 0 && (
                !lastMsg || lastMsg.role === 'user' || lastMsg.role === 'tool'
            );
            if ((waitingForResponse || hasPendingTools) && !OSA.getStreamingAssistantMessage()) {
                OSA.showThinkingIndicator();
            }
        }
        
        messagesDiv.scrollTop = messagesDiv.scrollHeight;
        OSA.loadSessions();
        OSA.loadSessionWorkspace();
        OSA.loadSessionPersona();
        OSA.loadSessionBreadcrumb(sessionId);
    } catch (error) {
        console.error('Failed to load session:', error);
    }
};

OSA.restoreToolsAtPositions = function(tools) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv || tools.length === 0) return;

    const allMessages = Array.from(messagesDiv.querySelectorAll('.message'));
    if (allMessages.length === 0) {
        tools.forEach(t => {
            if (OSA.isContextTool(t.tool_name)) return;
            if (t.tool_name === 'subagent') return;
            OSA.restoreToolCard(t);
        });
        return;
    }

    const toolTs = (t) => (t.timestamp || 0) * 1000;
    const msgTs = (el) => parseInt(el.dataset.ts, 10) || 0;

    const PARALLEL_WINDOW_MS = 3000;

    const contextTools = [];
    const regularTools = [];

    tools.forEach(t => {
        if (OSA.isContextTool(t.tool_name)) {
            contextTools.push(t);
        } else if (t.tool_name !== 'subagent') {
            regularTools.push(t);
        }
    });

    regularTools.sort((a, b) => toolTs(a) - toolTs(b));

    const grouped = [];
    let currentGroup = null;

    for (const tool of regularTools) {
        if (currentGroup && toolTs(tool) - currentGroup.startTs < PARALLEL_WINDOW_MS) {
            currentGroup.tools.push(tool);
        } else {
            currentGroup = { startTs: toolTs(tool), tools: [tool] };
            grouped.push(currentGroup);
        }
    }

    for (const group of grouped) {
        const firstTs = group.tools[0] ? toolTs(group.tools[0]) : 0;
        let anchor = null;

        for (let i = allMessages.length - 1; i >= 0; i--) {
            if (msgTs(allMessages[i]) <= firstTs) {
                anchor = allMessages[i];
                break;
            }
        }

        let insertBefore = null;
        if (anchor) {
            let sibling = anchor.nextElementSibling;
            while (sibling && !sibling.classList.contains('message')) {
                sibling = sibling.nextElementSibling;
            }
            insertBefore = sibling;
        }

        if (group.tools.length >= 2) {
            const groupDiv = document.createElement('div');
            groupDiv.className = 'parallel-group';
            groupDiv.innerHTML = `
                <div class="parallel-group-header">
                    <span class="parallel-count">${group.tools.length} tools executed concurrently</span>
                </div>
            `;

            if (insertBefore) {
                messagesDiv.insertBefore(groupDiv, insertBefore);
            } else {
                messagesDiv.appendChild(groupDiv);
            }

            group.tools.forEach(t => {
                OSA.restoreToolCard(t, null, groupDiv);
            });
        } else {
            OSA.restoreToolCard(group.tools[0], insertBefore);
        }
    }

    if (contextTools.length > 0) {
        contextTools.sort((a, b) => toolTs(a) - toolTs(b));
        const firstCtxTs = toolTs(contextTools[0]);

        let insertBefore = null;
        for (let i = allMessages.length - 1; i >= 0; i--) {
            if (msgTs(allMessages[i]) <= firstCtxTs) {
                let sibling = allMessages[i].nextElementSibling;
                while (sibling && !sibling.classList.contains('message')) {
                    sibling = sibling.nextElementSibling;
                }
                insertBefore = sibling;
                break;
            }
        }

        OSA.restoreContextToolGroup(contextTools, insertBefore);
    }
};

OSA.clearSessions = async function() {
    if (!confirm('Delete all sessions?')) return;
    try {
        const res = await fetch('/api/sessions', {
            method: 'DELETE',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        OSA.setCurrentSession(null);
        const es = OSA.getEventSource();
        if (es) {
            es.close();
            OSA.setEventSource(null);
        }
        document.getElementById('messages').innerHTML = `
            <div class="empty-state">
                <div class="empty-state-icon">+</div>
                <div class="empty-state-title">Start a conversation</div>
                <div class="empty-state-text">Click "New chat" to begin</div>
            </div>
        `;
        OSA.setHeaderBaseTitle('Select a session');
        document.getElementById('header-title').textContent = 'Select a session';
        OSA.setHeaderTitleRenameable(false);
        OSA.loadSessions();
        OSA.loadSessionWorkspace();
        OSA.loadSessionPersona();
    } catch (error) {
        alert(error.message);
    }
};

OSA.sendMessage = async function() {
    let currentSession = OSA.getCurrentSession();
    if (!currentSession) {
        await OSA.createSession();
        currentSession = OSA.getCurrentSession();
        if (!currentSession) return;
    }

    if (OSA.isAgentProcessing()) {
        return;
    }

    const input = document.getElementById('message-input');
    const message = input.value.trim();
    if (!message) return;

    input.value = '';
    OSA.hideSlashMenu();
    OSA.setInputHistoryIndex(-1);
    OSA.getInputHistory().push(message);
    if (OSA.getInputHistory().length > 100) OSA.getInputHistory().shift();
    OSA.hideThinkingIndicator();
    OSA.releaseStreamingAssistantMessage();
    OSA.setProcessing(true);
    OSA.setHasReceivedResponse(false);
    OSA.setSendButtonStopMode(true);
    if (currentSession) currentSession.task_status = 'running';
    if (currentSession) {
        if (!Array.isArray(currentSession.messages)) currentSession.messages = [];
        currentSession.messages.push({
            role: 'user',
            content: message,
            thinking: null,
            timestamp: new Date().toISOString(),
            tool_calls: null,
            tool_call_id: null,
            metadata: {},
            tokens: null,
        });
    }

    const messagesDiv = document.getElementById('messages');
    
    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) {
        emptyState.remove();
    }
    
    messagesDiv.innerHTML += `
        <div class="message user">
            <div class="message-role">You</div>
            <div class="message-content">${OSA.escapeHtml(message)}</div>
        </div>
    `;
    messagesDiv.scrollTop = messagesDiv.scrollHeight;

    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/send`, {
            method: 'POST',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ message, session_id: currentSession.id })
        });
        const data = await res.json().catch(() => ({}));
        
        if (!res.ok) {
            throw new Error(data.error || `HTTP ${res.status}`);
        }
    } catch (error) {
        console.error('Failed to send message:', error);
        if (currentSession && Array.isArray(currentSession.messages)) {
            const last = currentSession.messages[currentSession.messages.length - 1];
            if (last && last.role === 'user' && last.content === message) {
                currentSession.messages.pop();
            }
        }
        OSA.showErrorCard(error.message);
        OSA.setProcessing(false);
        OSA.resetSendButton();
        OSA.hideThinkingIndicator();
    }
};

OSA.stopGeneration = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession) return;

    if (OSA.isAgentStopping()) return;
    
    OSA.setStopping(true);
    
    if (OSA._stopTimeout) {
        clearTimeout(OSA._stopTimeout);
    }
    OSA._stopTimeout = setTimeout(() => {
        OSA._forceResetState();
    }, 5000);

    try {
        await OSA.cancelSession(currentSession.id);
    } catch (error) {
        console.error('Failed to cancel session:', error);
        OSA._forceResetState();
    }
};

OSA._forceResetState = function() {
    OSA.setProcessing(false);
    OSA.setStopping(false);
    OSA.resetSendButton();
    OSA.hideThinkingIndicator();
    OSA.pruneEmptyStreamingMessage();
    OSA.completeAssistantResponse();
    if (OSA._stopTimeout) {
        clearTimeout(OSA._stopTimeout);
        OSA._stopTimeout = null;
    }
};

OSA.setSendButtonStopMode = function(isStop) {
    const sendBtn = document.getElementById('send-btn');
    const sendIcon = document.getElementById('send-icon');
    const stopIcon = document.getElementById('stop-icon');

    if (!sendBtn) return;

    if (isStop) {
        sendBtn.classList.add('stop-btn');
        sendBtn.disabled = false;
        if (sendIcon) sendIcon.classList.add('hidden');
        if (stopIcon) stopIcon.classList.remove('hidden');
    } else {
        sendBtn.classList.remove('stop-btn');
        sendBtn.disabled = false;
        if (sendIcon) sendIcon.classList.remove('hidden');
        if (stopIcon) stopIcon.classList.add('hidden');
    }
};

OSA.resetSendButton = function() {
    OSA.setSendButtonStopMode(false);
};

window.handleSendButtonClick = function() {
    if (OSA.isAgentProcessing()) {
        OSA.stopGeneration();
    } else {
        OSA.sendMessage();
    }
};

OSA.connectEventSource = function(sessionId) {
    const existingES = OSA.getEventSource();
    if (existingES) {
        existingES.close();
    }
    const reconnectTimer = OSA.getEventReconnectTimer();
    if (reconnectTimer) {
        clearTimeout(reconnectTimer);
        OSA.setEventReconnectTimer(null);
    }

    OSA.setEventSourceSessionId(sessionId);

    OSA.showConnectionStatus('connecting', 'Connecting...');
    
    const es = new EventSource(`/api/sessions/${sessionId}/events`);
    
    es.onopen = () => {
        OSA.showConnectionStatus('connected', 'Connected');

        const session = OSA.getCurrentSession();
        if (session && session.task_status === 'running' && OSA.isAgentProcessing()) {
            const indicator = document.getElementById('thinking-indicator');
            if (!indicator) {
                OSA.showThinkingIndicator();
            }
        }

        if (session && session.id === sessionId && session.task_status === 'running') {
            OSA.syncRunningSessionSnapshot(sessionId);
        }
    };
    
    es.onmessage = (event) => {
        try {
            const data = JSON.parse(event.data);
            const activeSessionId = OSA.getEventSourceSessionId();
            const currentSession = OSA.getCurrentSession();
            if (
                data.session_id &&
                (data.session_id !== activeSessionId || !currentSession || currentSession.id !== data.session_id)
            ) {
                return;
            }
            OSA.handleAgentEvent(data);
        } catch (e) {
            console.error('Failed to parse event:', e);
        }
    };
    
    es.onerror = (error) => {
        console.error('EventSource error:', error);
        OSA.showConnectionStatus('disconnected', 'Disconnected');
        if (OSA.getCurrentSession() && OSA.getCurrentSession().id === sessionId) {
            if (!OSA.getEventReconnectTimer()) {
                const timer = setTimeout(() => {
                    OSA.setEventReconnectTimer(null);
                    OSA.connectEventSource(sessionId);
                }, 2000);
                OSA.setEventReconnectTimer(timer);
            }
        }
    };
    
    OSA.setEventSource(es);
};

OSA.showConnectionStatus = function(status, message) {
    const statusEl = document.getElementById('connection-status');
    const textEl = document.getElementById('connection-text');
    
    if (!statusEl || !textEl) return;
    
    statusEl.classList.remove('hidden', 'connected', 'disconnected');
    statusEl.classList.add(status);
    textEl.textContent = message;
    
    if (status === 'connected') {
        setTimeout(() => statusEl.classList.add('hidden'), 2000);
    }
};

OSA.toggleSidebar = function() {
    const sidebar = document.querySelector('.sidebar');
    const backdrop = document.getElementById('sidebar-backdrop');
    const toggleBtn = document.getElementById('sidebar-toggle');
    if (!sidebar) return;

    const isMobile = window.innerWidth <= 900;

    if (isMobile) {
        OSA.sidebarOpen = !OSA.sidebarOpen;
        if (OSA.sidebarOpen) {
            sidebar.classList.add('open');
            sidebar.classList.remove('collapsed');
            if (backdrop) backdrop.classList.add('visible');
            if (toggleBtn) toggleBtn.classList.add('sidebar-open');
        } else {
            sidebar.classList.remove('open');
            if (backdrop) backdrop.classList.remove('visible');
            if (toggleBtn) toggleBtn.classList.remove('sidebar-open');
        }
    } else {
        const collapsed = !OSA.getSidebarCollapsed();
        OSA.setSidebarCollapsed(collapsed);
        if (collapsed) {
            sidebar.classList.add('collapsed');
            sidebar.classList.remove('open');
            if (toggleBtn) toggleBtn.classList.remove('sidebar-open');
        } else {
            sidebar.classList.remove('collapsed');
            if (toggleBtn) toggleBtn.classList.add('sidebar-open');
        }
    }
};

OSA.closeSidebar = function() {
    const sidebar = document.querySelector('.sidebar');
    const backdrop = document.getElementById('sidebar-backdrop');
    const toggleBtn = document.getElementById('sidebar-toggle');
    if (sidebar) {
        sidebar.classList.remove('open');
    }
    if (backdrop) backdrop.classList.remove('visible');
    if (toggleBtn) toggleBtn.classList.remove('sidebar-open');
    OSA.sidebarOpen = false;
};

OSA.initSidebarState = function() {
    const sidebar = document.querySelector('.sidebar');
    const toggleBtn = document.getElementById('sidebar-toggle');
    if (!sidebar) return;

    const isMobile = window.innerWidth <= 900;
    const collapsed = OSA.getSidebarCollapsed();

    if (isMobile) {
        sidebar.classList.remove('collapsed');
        sidebar.classList.remove('open');
    } else {
        if (collapsed) {
            sidebar.classList.add('collapsed');
        } else {
            sidebar.classList.remove('collapsed');
            if (toggleBtn) toggleBtn.classList.add('sidebar-open');
        }
    }
};

document.addEventListener('click', (event) => {
    const workspaceDropdown = document.querySelector('.workspace-dropdown');
    const personaDropdown = document.querySelector('.persona-dropdown');
    if (workspaceDropdown && !workspaceDropdown.contains(event.target)) {
        OSA.closeWorkspaceMenu();
    }
    if (personaDropdown && !personaDropdown.contains(event.target)) {
        OSA.closePersonaMenu();
    }
    if (!event.target.closest('.slash-menu')) {
        OSA.hideSlashMenu();
    }
});

document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
        const settingsModal = document.getElementById('settings-modal');
        if (settingsModal && !settingsModal.classList.contains('hidden')) {
            OSA.closeSettings();
            return;
        }
        const questionModal = document.getElementById('question-modal');
        if (questionModal && !questionModal.classList.contains('hidden')) {
            questionModal.classList.add('hidden');
            return;
        }
        OSA.hideSlashMenu();
        return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key === 'l') {
        event.preventDefault();
        const input = document.getElementById('message-input');
        if (input) input.focus();
        return;
    }
    if ((event.ctrlKey || event.metaKey) && event.key === 'n') {
        event.preventDefault();
        OSA.createSession();
        return;
    }
    const input = document.getElementById('message-input');
    if (input && document.activeElement === input && event.key === 'ArrowUp') {
        const history = OSA.getInputHistory();
        const idx = OSA.getInputHistoryIndex();
        if (history.length > 0 && (idx === -1 || history[idx] !== input.value)) {
            if (idx === -1) {
                OSA.setInputHistoryIndex(history.length - 1);
            } else if (idx > 0) {
                OSA.setInputHistoryIndex(idx - 1);
            }
            input.value = history[OSA.getInputHistoryIndex()];
        }
        return;
    }
    if (input && document.activeElement === input && event.key === 'ArrowDown') {
        const history = OSA.getInputHistory();
        const idx = OSA.getInputHistoryIndex();
        if (idx >= 0) {
            if (idx < history.length - 1) {
                OSA.setInputHistoryIndex(idx + 1);
                input.value = history[OSA.getInputHistoryIndex()];
            } else {
                OSA.setInputHistoryIndex(-1);
                input.value = '';
            }
        }
        return;
    }
});

window.addEventListener('resize', () => {
    OSA.initSidebarState();
});

OSA.SLASH_COMMANDS = [
    { cmd: '/new', label: 'New session', desc: 'Create a new chat session', action: () => OSA.createSession() },
    { cmd: '/model', label: 'Set model', desc: 'Focus the model input', action: () => { const m = document.getElementById('model-input'); if (m) m.focus(); } },
    { cmd: '/settings', label: 'Settings', desc: 'Open settings panel', action: () => OSA.openSettings() },
    { cmd: '/compact', label: 'Compact', desc: 'Summarize and compact the conversation', action: () => { const i = document.getElementById('message-input'); if (i) { i.value = 'Summarize our conversation so far and continue'; OSA.sendMessage(); } } },
    { cmd: '/clear', label: 'Clear screen', desc: 'Clear the message display', action: () => { document.getElementById('messages').innerHTML = ''; } },
    { cmd: '/reset', label: 'Reset session', desc: 'Clear messages and start fresh', action: () => OSA.createSession() },
    { cmd: '/help', label: 'Help', desc: 'Show available commands', action: () => {} },
];

OSA.handleSlashInput = function() {
    const input = document.getElementById('message-input');
    if (!input) return;
    const value = input.value;
    const menu = document.getElementById('slash-menu');

    if (!value.startsWith('/')) {
        OSA.hideSlashMenu();
        return;
    }

    const query = value.toLowerCase();
    const matches = OSA.SLASH_COMMANDS.filter(c => c.cmd.startsWith(query));

    if (matches.length === 0 || value === '/') {
        OSA.hideSlashMenu();
        return;
    }

    if (!menu) {
        const menuEl = document.createElement('div');
        menuEl.id = 'slash-menu';
        menuEl.className = 'slash-menu';
        document.querySelector('.input-wrapper').appendChild(menuEl);
    }

    const menuEl = document.getElementById('slash-menu');
    menuEl.innerHTML = matches.map(c => `
        <div class="slash-menu-item" data-cmd="${OSA.escapeHtml(c.cmd)}">
            <span class="slash-cmd">${OSA.escapeHtml(c.cmd)}</span>
            <span class="slash-desc">${OSA.escapeHtml(c.desc)}</span>
        </div>
    `).join('');
    menuEl.classList.remove('hidden');

    menuEl.querySelectorAll('.slash-menu-item').forEach(item => {
        item.addEventListener('mousedown', (e) => {
            e.preventDefault();
            const cmd = item.dataset.cmd;
            const command = OSA.SLASH_COMMANDS.find(c => c.cmd === cmd);
            if (command) {
                input.value = '';
                command.action();
                OSA.hideSlashMenu();
            }
        });
    });
};

OSA.hideSlashMenu = function() {
    const menu = document.getElementById('slash-menu');
    if (menu) menu.classList.add('hidden');
};

OSA.filterSessions = function(query) {
    const items = document.querySelectorAll('.session-item');
    const childrenGroups = document.querySelectorAll('.session-children');
    const q = query.toLowerCase();
    
    items.forEach(item => {
        const text = (item.textContent || '').toLowerCase();
        item.style.display = (!q || text.includes(q)) ? '' : 'none';
    });

    if (q) {
        childrenGroups.forEach(group => {
            const visibleChildren = group.querySelectorAll('.session-item:not([style*="display: none"])');
            group.style.display = visibleChildren.length > 0 ? '' : 'none';
        });
    } else {
        childrenGroups.forEach(group => {
            group.style.display = '';
        });
    }
};

OSA.loadSessionBreadcrumb = async function(sessionId) {
    try {
        const res = await fetch(`/api/sessions/${sessionId}/parent`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        
        const breadcrumb = [];
        if (data.session) {
            breadcrumb.unshift({ id: data.session.id, title: 'Parent Session' });
            let current = data.session;
            while (current.parent_id) {
                const parentRes = await fetch(`/api/sessions/${current.parent_id}/parent`, {
                    headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
                });
                const parentData = await parentRes.json();
                if (parentData.session) {
                    breadcrumb.unshift({ id: parentData.session.id, title: 'Parent' });
                    current = parentData.session;
                } else {
                    break;
                }
            }
        }
        
        breadcrumb.push({ id: sessionId, title: 'Current', current: true });
        
        OSA.setSessionHierarchy({ 
            parentId: data.session?.id || null, 
            children: [],
            breadcrumb 
        });
        
        OSA.renderBreadcrumb();
    } catch (error) {
        console.error('Failed to load session breadcrumb:', error);
        OSA.setSessionHierarchy({ parentId: null, children: [], breadcrumb: [{ id: sessionId, title: 'Current', current: true }] });
        OSA.renderBreadcrumb();
    }
};

OSA.renderBreadcrumb = function() {
    const hierarchy = OSA.getSessionHierarchy();
    const breadcrumb = hierarchy.breadcrumb || [];
    
    let container = document.getElementById('session-breadcrumb');
    if (!container) {
        container = document.createElement('div');
        container.id = 'session-breadcrumb';
        container.className = 'session-breadcrumb';
        const header = document.querySelector('.header');
        if (header) {
            header.insertAdjacentElement('afterend', container);
        } else {
            const messagesDiv = document.getElementById('messages');
            if (messagesDiv) {
                messagesDiv.insertAdjacentElement('beforebegin', container);
            }
        }
    }
    
    if (breadcrumb.length <= 1) {
        container.style.display = 'none';
        return;
    }
    
    container.style.display = 'flex';
    container.innerHTML = breadcrumb.map((item, idx) => {
        const isLast = idx === breadcrumb.length - 1;
        const separator = isLast ? '' : '<span class="breadcrumb-separator">/</span>';
        const className = item.current ? 'breadcrumb-item current' : 'breadcrumb-item';
        const onclick = item.current ? `OSA.startRenameCurrentSession(this)` : `OSA.selectSession('${item.id}')`;
        return `<span class="${className}" onclick="${onclick}">${OSA.escapeHtml(item.title)}</span>${separator}`;
    }).join('');
};

OSA.navigateToParent = async function() {
    const hierarchy = OSA.getSessionHierarchy();
    if (hierarchy.parentId) {
        await OSA.selectSession(hierarchy.parentId);
    }
};

OSA.navigateToChild = async function(childId) {
    await OSA.selectSession(childId);
};

OSA.deleteSession = async function(sessionId) {
    if (!confirm('Delete this session? This cannot be undone.')) return;
    try {
        const res = await fetch(`/api/sessions/${sessionId}`, {
            method: 'DELETE',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        if (OSA.getCurrentSession() && OSA.getCurrentSession().id === sessionId) {
            OSA.setCurrentSession(null);
            const es = OSA.getEventSource();
            if (es) {
                es.close();
                OSA.setEventSource(null);
            }
            document.getElementById('messages').innerHTML = `
                <div class="empty-state">
                    <div class="empty-state-icon">+</div>
                    <div class="empty-state-title">Start a conversation</div>
                    <div class="empty-state-text">Click "New chat" to begin</div>
                </div>
            `;
        }
        OSA.loadSessions();
    } catch (error) {
        alert(error.message);
    }
};

OSA.setHeaderTitleRenameable = function(enabled) {
    const headerTitle = document.getElementById('header-title');
    if (!headerTitle) return;
    headerTitle.classList.toggle('renameable', !!enabled);
    headerTitle.title = enabled ? 'Click to rename the session' : '';
};

OSA.startRenameSession = function(sessionId, btnEl) {
    const item = btnEl?.closest('.session-item');
    const nameEl = item?.querySelector('.session-name');
    if (!nameEl) return;
    OSA.startRenameSessionInline(sessionId, nameEl);
};

OSA.startRenameCurrentSession = function(sourceEl) {
    const session = OSA.getCurrentSession();
    if (!session?.id) return;
    const nameEl = sourceEl || document.getElementById('header-title');
    if (!nameEl) return;
    OSA.startRenameSessionInline(session.id, nameEl);
};

OSA.startRenameSessionInline = function(sessionId, nameEl) {
    const currentName = nameEl.textContent;
    const originalClass = nameEl.className;
    const originalStyle = window.getComputedStyle(nameEl);
    const rect = nameEl.getBoundingClientRect();
    const input = document.createElement('input');
    input.type = 'text';
    input.value = currentName;
    input.className = 'session-rename-input';
    input.maxLength = 100;
    input.dataset.replacementClass = originalClass;
    input.style.width = `${Math.max(rect.width, 40)}px`;
    input.style.height = `${rect.height}px`;
    input.style.minHeight = `${rect.height}px`;
    input.style.fontSize = originalStyle.fontSize;
    nameEl.replaceWith(input);
    input.focus();
    input.select();
    input.onblur = () => OSA.finishRenameSession(sessionId, input.value.trim() || currentName, input);
    input.onkeydown = (e) => {
        if (e.key === 'Enter') input.blur();
        if (e.key === 'Escape') { input.value = currentName; input.blur(); }
    };
};

OSA.finishRenameSession = async function(sessionId, newName, inputEl) {
    const nameSpan = document.createElement('span');
    nameSpan.className = inputEl.dataset.replacementClass || 'session-name';
    nameSpan.textContent = newName;
    inputEl.replaceWith(nameSpan);
    try {
        const res = await fetch(`/api/sessions/${sessionId}`, {
            method: 'PATCH',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ name: newName })
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        const current = OSA.getCurrentSession();
        if (current?.id === sessionId) {
            document.getElementById('header-title').textContent = newName;
            OSA.setHeaderBaseTitle(newName);
            OSA.setHeaderTitleRenameable(true);
        }
        OSA.loadSessions();
    } catch (error) {
        console.error('Failed to rename session:', error);
    }
};

OSA.loadChildSessions = async function(sessionId) {
    try {
        const res = await fetch(`/api/sessions/${sessionId}/children`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        const hierarchy = OSA.getSessionHierarchy();
        hierarchy.children = data.sessions || [];
        OSA.setSessionHierarchy(hierarchy);
        return data.sessions || [];
    } catch (error) {
        console.error('Failed to load child sessions:', error);
        return [];
    }
};

window.login = OSA.login;
window.logout = OSA.logout;
window.createSession = OSA.createSession;
window.clearSessions = OSA.clearSessions;
window.selectSession = OSA.selectSession;
window.sendMessage = OSA.sendMessage;
window.toggleSidebar = OSA.toggleSidebar;
window.closeSidebar = OSA.closeSidebar;
window.updateModel = OSA.updateModel;
window.openSettings = function() { OSA.openSettings(); };
window.closeSettings = function() { OSA.closeSettings(); };
window.saveSettings = function() { OSA.saveSettings(); };
window.toggleTodoDock = OSA.toggleTodoDock;
window.filterSessions = OSA.filterSessions;

OSA.openWorkflowEditor = function() {
    const appView = document.getElementById('app-view');
    const workflowEditor = document.getElementById('workflow-editor');
    
    if (appView) {
        appView.classList.add('hidden');
        appView.style.display = 'none';
    }
    
    if (workflowEditor) {
        workflowEditor.classList.remove('hidden');
        workflowEditor.style.display = 'flex';
    }
    
    if (window.workflowEditor) {
        window.workflowEditor.init().catch(err => {
            console.error('Failed to init workflow editor:', err);
        });
    }
};

OSA.closeWorkflowEditor = function() {
    const appView = document.getElementById('app-view');
    const workflowEditor = document.getElementById('workflow-editor');
    
    if (workflowEditor) {
        workflowEditor.classList.add('hidden');
        workflowEditor.style.display = 'none';
        
        if (window.workflowEditor && window.workflowEditor.adapter) {
            window.workflowEditor.adapter.destroy();
        }
    }
    
    if (appView) {
        appView.classList.remove('hidden');
        appView.style.display = 'flex';
    }
};

window.openWorkflowEditor = OSA.openWorkflowEditor;

OSA._sidebarCanvasAnims = [];

OSA._initSessionIconCanvases = function() {
    OSA._sidebarCanvasAnims.forEach(fn => fn());
    OSA._sidebarCanvasAnims = [];

    const canvases = document.querySelectorAll('.session-icon-canvas');
    canvases.forEach(canvas => {
        const fn = OSA._initMiniCanvas(canvas);
        if (fn) OSA._sidebarCanvasAnims.push(fn);
    });
};

OSA._initMiniCanvas = function(canvas) {
    const dpr = window.devicePixelRatio || 1;
    const size = 32;
    canvas.width = size * dpr;
    canvas.height = size * dpr;
    canvas.style.width = size + 'px';
    canvas.style.height = size + 'px';

    const ctx = canvas.getContext('2d');
    ctx.scale(dpr, dpr);

    let frame;
    const center = size / 2;
    const orbits = [
        { rx: 11, ry: 5, tilt: -0.4, speed: 2.2, phase: 0, dotSize: 1.2, trailLen: 5 },
        { rx: 11, ry: 5, tilt: 0.9, speed: 1.6, phase: 2.1, dotSize: 1.0, trailLen: 4 },
        { rx: 11, ry: 5, tilt: -1.7, speed: 2.8, phase: 4.2, dotSize: 0.8, trailLen: 6 },
    ];
    const trailBuf = orbits.map(() => []);

    function draw(t) {
        ctx.clearRect(0, 0, size, size);
        const time = t * 0.001;

        const grad = ctx.createRadialGradient(center, center, 0, center, center, 5);
        grad.addColorStop(0, 'rgba(255,255,255,0.25)');
        grad.addColorStop(1, 'rgba(255,255,255,0)');
        ctx.beginPath();
        ctx.arc(center, center, 5, 0, Math.PI * 2);
        ctx.fillStyle = grad;
        ctx.fill();

        ctx.beginPath();
        ctx.arc(center, center, 1.5, 0, Math.PI * 2);
        ctx.fillStyle = 'rgba(255,255,255,0.6)';
        ctx.fill();

        orbits.forEach((orbit, idx) => {
            const cosT = Math.cos(orbit.tilt);
            const sinT = Math.sin(orbit.tilt);
            const angle = time * orbit.speed + orbit.phase;

            ctx.beginPath();
            ctx.strokeStyle = 'rgba(255,255,255,0.04)';
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
                const a = ((i + 1) / trailBuf[idx].length) * 0.2;
                const s = orbit.dotSize * (0.3 + 0.7 * (i / trailBuf[idx].length));
                ctx.beginPath();
                ctx.arc(tp.x, tp.y, s, 0, Math.PI * 2);
                ctx.fillStyle = `rgba(255,255,255,${a})`;
                ctx.fill();
            }

            ctx.beginPath();
            ctx.arc(px, py, orbit.dotSize, 0, Math.PI * 2);
            ctx.fillStyle = 'rgba(255,255,255,0.7)';
            ctx.fill();
        });

        frame = requestAnimationFrame(draw);
    }

    frame = requestAnimationFrame(draw);
    return function cancel() { cancelAnimationFrame(frame); };
};

OSA.checkAuthAndInit();

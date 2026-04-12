window.OSA = window.OSA || {};

OSA._debounceTimers = {};
OSA.debounce = function(key, fn, delay) {
    if (OSA._debounceTimers[key]) clearTimeout(OSA._debounceTimers[key]);
    OSA._debounceTimers[key] = setTimeout(() => { delete OSA._debounceTimers[key]; fn(); }, delay);
};

OSA.prefetchedSessions = null;
OSA._startupDeferredQueued = false;
OSA.runWhenIdle = function(callback, timeout = 1200) {
    if (typeof window.requestIdleCallback === 'function') {
        window.requestIdleCallback(() => callback(), { timeout });
        return;
    }
    setTimeout(callback, 0);
};

OSA.queueDeferredStartupTasks = function() {
    if (OSA._startupDeferredQueued) return;
    OSA._startupDeferredQueued = true;

    setTimeout(function() {
        OSA.loadPersonaCatalog();
        OSA.loadSessionPersona();
    }, 0);

    OSA.runWhenIdle(function() {
        OSA.initVoice();
        OSA.loadProviderCatalog();
        OSA.refreshWorkflowAvailability?.();
    });
};

OSA.WORKFLOW_STYLESHEETS = [
    '/static/css/workflow.css',
    '/static/css/litegraph.min.css'
];

OSA.WORKFLOW_SCRIPTS = [
    '/static/js/litegraph.min.js',
    '/static/js/workflow/services/api.js',
    '/static/js/workflow/services/execution.js',
    '/static/js/workflow/store/state.js',
    '/static/js/workflow/nodes/base.js',
    '/static/js/workflow/litegraph_adapter.js',
    '/static/js/workflow/views/editor.js',
    '/static/js/workflow/main.js'
];

OSA.loadStylesheet = function(href) {
    if (!href) return Promise.resolve();
    const existing = document.querySelector(`link[rel="stylesheet"][href="${href}"]`);
    if (existing) {
        return existing.dataset.loaded === 'true'
            ? Promise.resolve()
            : new Promise((resolve, reject) => {
                existing.addEventListener('load', resolve, { once: true });
                existing.addEventListener('error', () => reject(new Error(`Failed to load ${href}`)), { once: true });
            });
    }

    return new Promise((resolve, reject) => {
        const link = document.createElement('link');
        link.rel = 'stylesheet';
        link.href = href;
        link.addEventListener('load', function() {
            link.dataset.loaded = 'true';
            resolve();
        }, { once: true });
        link.addEventListener('error', function() {
            reject(new Error(`Failed to load ${href}`));
        }, { once: true });
        document.head.appendChild(link);
    });
};

OSA.loadScript = function(src) {
    if (!src) return Promise.resolve();
    const existing = document.querySelector(`script[src="${src}"]`);
    if (existing) {
        return existing.dataset.loaded === 'true'
            ? Promise.resolve()
            : new Promise((resolve, reject) => {
                existing.addEventListener('load', resolve, { once: true });
                existing.addEventListener('error', () => reject(new Error(`Failed to load ${src}`)), { once: true });
            });
    }

    return new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = src;
        script.async = false;
        script.addEventListener('load', function() {
            script.dataset.loaded = 'true';
            resolve();
        }, { once: true });
        script.addEventListener('error', function() {
            reject(new Error(`Failed to load ${src}`));
        }, { once: true });
        document.body.appendChild(script);
    });
};

OSA.ensureWorkflowAssetsLoaded = function() {
    if (window.ensureWorkflowEditor) {
        return Promise.resolve();
    }
    if (OSA.workflowAssetsPromise) {
        return OSA.workflowAssetsPromise;
    }

    OSA.workflowAssetsPromise = (async function() {
        await Promise.all(OSA.WORKFLOW_STYLESHEETS.map(OSA.loadStylesheet));
        for (const src of OSA.WORKFLOW_SCRIPTS) {
            await OSA.loadScript(src);
        }
    })().catch(function(error) {
        OSA.workflowAssetsPromise = null;
        throw error;
    });

    return OSA.workflowAssetsPromise;
};

OSA.getSessionDisplayName = function(session) {
    if (session.metadata?.name) return session.metadata.name;
    if (session.agent_type) return session.agent_type.charAt(0).toUpperCase() + session.agent_type.slice(1) + ' Agent';
    return 'Session';
};

OSA.getSessionSourceKey = function(session) {
    const source = (session && session.metadata && typeof session.metadata.source === 'string')
        ? session.metadata.source.trim().toLowerCase()
        : '';
    if (source === 'discord' || source === 'web') return source;
    if (source === 'discord-shared' || source === 'shared') return 'discord-shared';

    const owner = (session && session.metadata && typeof session.metadata.owner === 'string')
        ? session.metadata.owner
        : '';
    if (owner.startsWith('discord-channel:')) return 'discord-shared';
    if (owner.startsWith('discord:')) return 'discord';
    return 'web';
};

OSA.getSessionSourceLabel = function(sourceKey) {
    if (sourceKey === 'discord') return 'Discord';
    if (sourceKey === 'discord-shared') return 'Shared';
    return 'Web';
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
            const validRes = await OSA.fetchWithAuth('/api/sessions');
            if (validRes.ok) {
                OSA.prefetchedSessions = await validRes.json().catch(() => null);
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
    OSA.resetSessionCheckpoints();
    OSA.setSessionInspectorState({ history: [], snapshots: [] });
    
    OSA.disconnectLiveSessionChannel();
    
    OSA.showLogin();
};

OSA.disconnectLiveSessionChannel = function() {
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

    const ws = OSA.getWebSocket ? OSA.getWebSocket() : null;
    if (ws) {
        ws._osaSuppressReconnect = true;
        ws.close();
        OSA.setWebSocket(null);
    }

    if (typeof OSA.setEventSourceSessionId === 'function') {
        OSA.setEventSourceSessionId(null);
    }
    if (typeof OSA.cancelSpeechOutput === 'function') {
        OSA.cancelSpeechOutput();
    }
};

OSA.showLogin = function() {
    document.getElementById('login-view').classList.remove('hidden');
    document.getElementById('app-view').classList.add('hidden');
    OSA._startupDeferredQueued = false;
    OSA.prefetchedSessions = null;
};

OSA.showApp = function() {
    document.getElementById('login-view').classList.add('hidden');
    document.getElementById('app-view').classList.remove('hidden');
    document.getElementById('app-view').style.display = 'grid';
    
    OSA.initSidebarState();
    OSA.initTheme();
    OSA.loadSessions();
    OSA.loadWorkspaces();
    OSA.loadModel();
    OSA.queueDeferredStartupTasks();
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
        let sessions = null;
        if (Array.isArray(OSA.prefetchedSessions)) {
            sessions = OSA.prefetchedSessions;
            OSA.prefetchedSessions = null;
        } else {
            const res = await OSA.fetchWithAuth('/api/sessions');
            sessions = await res.json();
        }

        if (!Array.isArray(sessions)) {
            sessions = [];
        }

        const sessionIds = new Set(sessions.map(function(session) { return session.id; }));
        
        const currentSession = OSA.getCurrentSession();

        const childMap = new Map();
        const rootSessions = [];
        const orphanChildren = [];

        sessions.forEach(s => {
            if (s.parent_id) {
                const parentExists = sessionIds.has(s.parent_id);
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
            const sourceKey = OSA.getSessionSourceKey(s);
            const sourceLabel = OSA.getSessionSourceLabel(sourceKey);
            const isRunning = s.task_status === 'running' || hasRunningChildren;
            const iconHtml = isRunning
                ? `
                    <span class="session-running-orbits" aria-hidden="true">
                        <span class="session-running-track track-a"></span>
                        <span class="session-running-track track-b"></span>
                        <span class="session-running-track track-c"></span>
                        <span class="session-running-core"></span>
                    </span>`
                : (isChild ? 'A' : '#');
            const iconStyle = isChild && !isRunning ? 'style="width:24px;height:24px;font-size:10px;border-radius:4px;background:var(--bg-tertiary);color:var(--text-secondary);border:1px solid var(--border);"' : '';
            const iconClass = isRunning ? ' session-icon-running' : '';
            return `
            <div class="session-item${childClass} ${isActive ? 'active' : ''}" data-session-id="${OSA.escapeHtml(s.id)}" data-session-source="${OSA.escapeHtml(sourceKey)}" onclick="OSA.selectSession('${s.id}')" style="${indent}">
                <div class="session-icon${iconClass}" ${iconStyle}>${iconHtml}</div>
                <div class="session-info">
                    <div class="session-name">${OSA.escapeHtml(displayName)}</div>
                    <div class="session-meta">
                        <div class="session-date">${new Date(s.created_at).toLocaleDateString()}</div>
                        <span class="session-source-badge source-${OSA.escapeHtml(sourceKey)}">${OSA.escapeHtml(sourceLabel)}</span>
                    </div>
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
                <input type="text" id="session-search-input" placeholder="Search sessions..." oninput="OSA.debounce('sessionSearch', () => OSA.filterSessions(this.value), 200)" />
                <select id="session-source-filter" onchange="OSA.setSessionSourceFilter(this.value); OSA.filterSessions(document.getElementById('session-search-input')?.value || '')">
                    <option value="all">All sources</option>
                    <option value="web">Web</option>
                    <option value="discord">Discord</option>
                    <option value="discord-shared">Shared</option>
                </select>
            </div>
            ${sessionsHtml}
        `;

        const sourceFilter = document.getElementById('session-source-filter');
        if (sourceFilter) {
            sourceFilter.value = OSA.getSessionSourceFilter ? OSA.getSessionSourceFilter() : 'all';
        }

        OSA.filterSessions(document.getElementById('session-search-input')?.value || '');

        const activeEl = list.querySelector('.session-item.active');
        if (activeEl) {
            activeEl.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
        }

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
        OSA.setSessionCheckpoints(session.id, []);
        OSA.getActiveTools().clear();
        OSA.parallelToolGroups = [];
        OSA.setSessionQueue([]);
        OSA.renderQueuedMessages([]);
        OSA.resetMessageChain();
        OSA.stopToolSync();
        
        OSA.restoreContextState(session.id, null);
        OSA.connectEventSource(session.id);

        OSA.resetTranscriptView();
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

OSA.refreshSessionQueue = async function(sessionId) {
    const res = await OSA.fetchWithAuth(`/api/sessions/${sessionId}/queue`);
    if (!res.ok) {
        throw new Error(`HTTP ${res.status}`);
    }

    const queue = await res.json();
    const currentSession = OSA.getCurrentSession();
    if (currentSession && currentSession.id === sessionId) {
        OSA.setSessionQueue(queue);
        OSA.renderQueuedMessages(queue);
    }
    return queue;
};

OSA.loadSessionCheckpoints = async function(sessionId, options = {}) {
    if (!sessionId) return [];

    const requestId = options.requestId || 0;
    const silent = !!options.silent;
    const signal = options.signal;

    try {
        const res = await OSA.fetchWithAuth(`/api/sessions/${sessionId}/checkpoints`, { signal });
        if (requestId && !OSA.isSessionSelectionCurrent(requestId)) return [];

        const data = await res.json().catch(() => []);
        if (requestId && !OSA.isSessionSelectionCurrent(requestId)) return [];

        if (!res.ok) {
            throw new Error(data.error || `HTTP ${res.status}`);
        }

        const checkpoints = Array.isArray(data) ? data : [];
        checkpoints.sort(function(a, b) {
            const left = OSA.timestampToMs(a?.created_at) || 0;
            const right = OSA.timestampToMs(b?.created_at) || 0;
            return right - left;
        });

        OSA.setSessionCheckpoints(sessionId, checkpoints);

        const currentSession = OSA.getCurrentSession();
        if (currentSession && currentSession.id === sessionId && typeof OSA.updateAssistantRestoreButtons === 'function') {
            OSA.updateAssistantRestoreButtons();
        }

        return checkpoints;
    } catch (error) {
        if (error && error.name === 'AbortError') {
            return [];
        }
        OSA.setSessionCheckpoints(sessionId, []);
        if (!silent) {
            console.error('Failed to load session checkpoints:', error);
        }
        return [];
    }
};

OSA.refreshCurrentSessionQueue = function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !currentSession.id) return Promise.resolve([]);
    return OSA.refreshSessionQueue(currentSession.id).catch(error => {
        console.error('Failed to refresh session queue:', error);
        return [];
    });
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

        if (session.task_status !== 'running') {
            OSA.syncRenderedMessages(session.messages || [], {
                resetStreaming: false,
                stickToBottom: false,
                preferTail: true,
                keepWindow: true,
                preserveScroll: true,
            });

            if (OSA.getStreamingAssistantMessage()) {
                OSA.completeAssistantResponse();
            } else {
                document.querySelectorAll('#messages .message.assistant.streaming').forEach(function(el) {
                    el.classList.remove('streaming');
                });
                document.querySelectorAll('#messages .message-thinking.streaming').forEach(function(el) {
                    el.classList.remove('streaming');
                });
            }

            OSA.hideThinkingIndicator();
            OSA.stopToolSync();
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
            OSA.refreshCurrentSessionQueue();
            OSA.loadSessions();
            return;
        }

        const streamingMessage = OSA.getStreamingAssistantMessage();
        const latestAssistant = OSA.getActiveTurnAssistantMessage(session);
        if (!latestAssistant) {
            if (streamingMessage) {
                OSA.releaseStreamingAssistantMessage();
            }
            if (OSA.shouldShowThinkingIndicatorForRunningSession(session)) {
                OSA.showThinkingIndicator();
            } else {
                OSA.hideThinkingIndicator();
            }
            return;
        }

        OSA.hideThinkingIndicator();

        if (!streamingMessage) {
            OSA.syncRenderedMessages(session.messages || [], {
                resetStreaming: false,
                stickToBottom: false,
                preferTail: true,
                keepWindow: true,
                preserveScroll: true,
            });
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
    const perfStart = OSA.perfNow ? OSA.perfNow() : Date.now();
    const requestId = OSA.beginSessionSelection ? OSA.beginSessionSelection() : 0;
    const previousController = OSA.getSessionSelectionAbortController ? OSA.getSessionSelectionAbortController() : null;
    if (previousController) {
        previousController.abort();
    }
    const selectionController = new AbortController();
    OSA.setSessionSelectionAbortController?.(selectionController);
    const { signal } = selectionController;
    OSA.markSessionListSelection(sessionId);

    try {
        await new Promise((resolve, reject) => {
            const timer = setTimeout(resolve, 75);
            signal.addEventListener('abort', () => {
                clearTimeout(timer);
                reject(new DOMException('Selection aborted', 'AbortError'));
            }, { once: true });
        });

        const res = await fetch(`/api/sessions/${sessionId}`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
            signal,
        });
        if (requestId && !OSA.isSessionSelectionCurrent(requestId)) return;
        const session = await res.json();
        if (requestId && !OSA.isSessionSelectionCurrent(requestId)) return;
        OSA.perfLog?.('selectSession:session', {
            sessionId,
            requestId,
            fetchMs: Math.round((OSA.perfNow ? OSA.perfNow() : Date.now()) - perfStart),
            messages: Array.isArray(session.messages) ? session.messages.length : 0,
        });

        const isCurrentSelection = () => {
            if (requestId && !OSA.isSessionSelectionCurrent(requestId)) return false;
            const activeSession = OSA.getCurrentSession();
            return !!activeSession && activeSession.id === sessionId;
        };

        const pendingToolsRequest = fetch(`/api/sessions/${sessionId}/tools`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
            signal,
        }).catch(() => null);
        const pendingSubagentsRequest = fetch(`/api/sessions/${sessionId}/subagents`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
            signal,
        }).catch(() => null);
        const pendingQueueRequest = fetch(`/api/sessions/${sessionId}/queue`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
            signal,
        }).catch(() => null);
        const pendingCheckpointsRequest = OSA.loadSessionCheckpoints(sessionId, {
            requestId,
            silent: true,
            signal,
        });

        OSA.setCurrentSession(session);
        OSA.restoreContextState(session.id, session.context_state || null);
        OSA.setSessionQueue([]);
        OSA.setSessionToolEvents([]);
        OSA.setSessionSubagentTasks([]);
        
        document.querySelectorAll('.tool-card, .context-tool-group, .subagent-card, .parallel-group').forEach(el => el.remove());
        OSA.getActiveTools().clear();
        OSA.parallelToolGroups = [];
        OSA._contextGroupState = null;
        OSA.resetMessageChain();
        OSA.stopToolSync();
        OSA.disconnectLiveSessionChannel();

        OSA.hideThinkingIndicator();
        OSA.setTurnStartTime(null);
        OSA.resetStreamingMessage();
        
        const sessionName = OSA.getSessionDisplayName(session);
        OSA.setHeaderBaseTitle(sessionName);
        document.getElementById('header-title').textContent = sessionName;
        OSA.setHeaderTitleRenameable(true);
        
        const messagesDiv = document.getElementById('messages');
        OSA.resetTranscriptView();

        if (session.messages.length === 0) {
            OSA.renderEmptyTranscript('Type a message below');
        } else {
            OSA.renderMessages(session.messages, { reason: 'session-switch' });
            if (session.task_status === 'running') {
                const adopted = OSA.adoptStreamingAssistantFromRenderedSession(session);
                if (!adopted && OSA.shouldShowThinkingIndicatorForRunningSession(session)) {
                    OSA.showThinkingIndicator();
                }
            }
        }

        const sessionIsRunning = session.task_status === 'running';
        if (sessionIsRunning) {
            OSA.setProcessing(true);
            OSA.setStopping(false);
            OSA.setSendButtonStopMode(true);
            OSA.startToolSync();
        } else {
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
        }

        messagesDiv.scrollTop = messagesDiv.scrollHeight;
        OSA.fetchAndRenderTodos();
        OSA.loadSessionWorkspace();
        OSA.loadSessionPersona();
        OSA.loadSessionBreadcrumb(sessionId);

        const [toolStartsRes, subagentsRes, queueRes] = await Promise.all([
            pendingToolsRequest,
            pendingSubagentsRequest,
            pendingQueueRequest
        ]);
        if (!isCurrentSelection()) return;

        await pendingCheckpointsRequest;
        if (!isCurrentSelection()) return;

        const tools = (toolStartsRes && toolStartsRes.ok) ? await toolStartsRes.json() : [];
        if (!isCurrentSelection()) return;
        const subagentsData = (subagentsRes && subagentsRes.ok) ? await subagentsRes.json() : { subagents: [], has_running: false };
        if (!isCurrentSelection()) return;
        const queueItems = (queueRes && queueRes.ok) ? await queueRes.json() : [];
        if (!isCurrentSelection()) return;

        OSA.setSessionQueue(queueItems);
        OSA.setSessionToolEvents(tools);
        OSA.setSessionSubagentTasks(subagentsData && Array.isArray(subagentsData.subagents) ? subagentsData.subagents : []);
        OSA.restoreVisibleAnchoredArtifacts();
        OSA.perfLog?.('selectSession:artifacts', {
            sessionId,
            requestId,
            tools: tools.length,
            subagents: Array.isArray(subagentsData?.subagents) ? subagentsData.subagents.length : 0,
            queue: queueItems.length,
            totalMs: Math.round((OSA.perfNow ? OSA.perfNow() : Date.now()) - perfStart),
        });

        OSA.renderQueuedMessages(queueItems);

        const subagentsRunning = !!(subagentsData && subagentsData.has_running);
        const isDirectlyRunning = sessionIsRunning && !subagentsRunning;

        if (sessionIsRunning || subagentsRunning) {
            OSA.connectEventSource(sessionId);
        }

        if (sessionIsRunning || subagentsRunning) {
            OSA.setProcessing(true);
            OSA.setStopping(false);
            OSA.setSendButtonStopMode(true);
        } else {
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
        }

        if (isDirectlyRunning && OSA.shouldShowThinkingIndicatorForRunningSession(session, tools) && !OSA.getStreamingAssistantMessage()) {
            OSA.showThinkingIndicator();
        }

        messagesDiv.scrollTop = messagesDiv.scrollHeight;
    } catch (error) {
        if (error && error.name === 'AbortError') {
            OSA.perfLog?.('selectSession:aborted', {
                sessionId,
                requestId,
                elapsedMs: Math.round((OSA.perfNow ? OSA.perfNow() : Date.now()) - perfStart),
            });
            return;
        }
        console.error('Failed to load session:', error);
    } finally {
        if (OSA.getSessionSelectionAbortController?.() === selectionController) {
            OSA.setSessionSelectionAbortController(null);
        }
    }
};

OSA.shouldShowThinkingIndicatorForRunningSession = function(session, tools = []) {
    if (!session || session.task_status !== 'running') {
        return false;
    }

    const msgs = Array.isArray(session.messages) ? session.messages : [];
    const lastUserMsgIdx = msgs.reduce((acc, message, index) => message.role === 'user' ? index : acc, -1);
    if (lastUserMsgIdx < 0) {
        return Array.isArray(tools) && tools.some(tool => !tool.completed);
    }

    const lastMsg = msgs[msgs.length - 1];
    const lastAssistantIsPlaceholder = !!(
        lastMsg
        && lastMsg.role === 'assistant'
        && !OSA.isHiddenSyntheticMessage(lastMsg)
        && !(lastMsg.content || '').trim()
        && !(OSA.getShowThinkingBlocks() && (lastMsg.thinking || '').trim())
    );

    return !!(
        !lastMsg
        || lastMsg.role === 'user'
        || lastMsg.role === 'tool'
        || lastAssistantIsPlaceholder
        || (Array.isArray(tools) && tools.some(tool => !tool.completed))
    );
};

OSA.markSessionListSelection = function(sessionId) {
    document.querySelectorAll('.session-item.active').forEach(item => {
        item.classList.remove('active');
    });

    if (!sessionId) return;

    document.querySelectorAll('.session-item').forEach(item => {
        item.classList.toggle('active', item.dataset.sessionId === sessionId);
    });
};

OSA.findToolInsertBefore = function(messagesDiv, messageIndex, fallbackTimestampMs = 0) {
    if (!messagesDiv) return null;

    const allMessages = Array.from(messagesDiv.querySelectorAll('.message'));
    const parsedMessageIndex = Number.isFinite(messageIndex) ? messageIndex : parseInt(messageIndex, 10);

    if (Number.isFinite(parsedMessageIndex)) {
        const nextByIndex = allMessages.find(el => {
            const elIndex = parseInt(el.dataset.messageIndex || '', 10);
            return Number.isFinite(elIndex) && elIndex > parsedMessageIndex;
        });
        if (nextByIndex) return nextByIndex;

        const anchorByIndex = allMessages.find(el => parseInt(el.dataset.messageIndex || '', 10) === parsedMessageIndex);
        if (anchorByIndex) return null;
    }

    if (fallbackTimestampMs > 0) {
        for (let i = allMessages.length - 1; i >= 0; i--) {
            const msgTs = parseInt(allMessages[i].dataset.ts, 10) || 0;
            if (msgTs <= fallbackTimestampMs) {
                let sibling = allMessages[i].nextElementSibling;
                while (sibling && !sibling.classList.contains('message')) {
                    sibling = sibling.nextElementSibling;
                }
                return sibling;
            }
        }
    }

    return allMessages[0] || null;
};

OSA.restoreVisibleAnchoredArtifacts = function() {
    const toolEvents = typeof OSA.getSessionToolEvents === 'function' ? OSA.getSessionToolEvents() : [];
    if (Array.isArray(toolEvents) && toolEvents.length > 0) {
        OSA.restoreToolsAtPositions(toolEvents);
    }

    const subagentTasks = typeof OSA.getSessionSubagentTasks === 'function' ? OSA.getSessionSubagentTasks() : [];
    if (Array.isArray(subagentTasks) && subagentTasks.length > 0) {
        OSA.restoreSubagentCards(subagentTasks);
    }
};

OSA.restoreToolsAtPositions = function(tools) {
    const messagesDiv = document.getElementById('messages');
    if (!messagesDiv || tools.length === 0) return;

    const filteredTools = tools.filter(t => {
        if (typeof OSA.isMessageIndexInRenderedWindow !== 'function') return true;
        return OSA.isMessageIndexInRenderedWindow(t.message_index);
    });

    if (filteredTools.length === 0) return;

    if (messagesDiv.querySelectorAll('.message').length === 0) {
        filteredTools.forEach(t => {
            if (t.tool_name === 'subagent') return;
            OSA.restoreToolCard(t);
        });
        return;
    }

    const toolTs = (t) => (t.timestamp || 0) * 1000;
    const toolMessageIndex = (t) => {
        const parsed = parseInt(t.message_index, 10);
        return Number.isFinite(parsed) ? parsed : Number.MAX_SAFE_INTEGER;
    };

    const PARALLEL_WINDOW_MS = 3000;

    const regularTools = filteredTools
        .filter(t => t.tool_name !== 'subagent' && !OSA.isContextTool(t.tool_name))
        .sort((a, b) => {
            const messageDelta = toolMessageIndex(a) - toolMessageIndex(b);
            if (messageDelta !== 0) return messageDelta;
            return toolTs(a) - toolTs(b);
        });

    const contextTools = filteredTools.filter(t => OSA.isContextTool(t.tool_name));

    const grouped = [];
    let currentGroup = null;

    for (const tool of regularTools) {
        if (
            currentGroup
            && currentGroup.messageIndex === toolMessageIndex(tool)
            && toolTs(tool) - currentGroup.startTs < PARALLEL_WINDOW_MS
        ) {
            currentGroup.tools.push(tool);
        } else {
            currentGroup = {
                startTs: toolTs(tool),
                messageIndex: toolMessageIndex(tool),
                tools: [tool]
            };
            grouped.push(currentGroup);
        }
    }

    for (const group of grouped) {
        const firstTs = group.tools[0] ? toolTs(group.tools[0]) : 0;
        const insertBefore = OSA.findToolInsertBefore(messagesDiv, group.messageIndex, firstTs);

        if (group.tools.length >= 2) {
            const groupDiv = document.createElement('div');
            groupDiv.className = 'parallel-group';
            groupDiv.dataset.messageIndex = group.messageIndex;
            groupDiv.innerHTML = `
                <div class="parallel-group-header">
                    <span class="parallel-count">${group.tools.length} tools executed concurrently</span>
                </div>
            `;

            OSA.mountAnchoredNode(groupDiv, group.messageIndex, insertBefore);

            group.tools.forEach(t => {
                OSA.restoreToolCard(t, null, groupDiv);
            });
        } else {
            OSA.restoreToolCard(group.tools[0], insertBefore);
        }
    }

    if (contextTools.length > 0) {
        contextTools
            .sort((a, b) => toolTs(a) - toolTs(b))
            .forEach(t => {
                const mi = toolMessageIndex(t);
                OSA.addContextToolToGroup(t, t.completed === true, t.success === true, mi);
            });
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
        OSA.resetSessionCheckpoints();
        const es = OSA.getEventSource();
        if (es) {
            es.close();
            OSA.setEventSource(null);
        }
        const ws = OSA.getWebSocket ? OSA.getWebSocket() : null;
        if (ws) {
            ws.close();
            OSA.setWebSocket(null);
        }
        OSA.renderEmptyTranscript('Click "New chat" to begin');
        OSA.setHeaderBaseTitle('Select a session');
        document.getElementById('header-title').textContent = 'Select a session';
        OSA.setHeaderTitleRenameable(false);
        OSA.setSessionQueue([]);
        OSA.renderQueuedMessages([]);
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

    const input = document.getElementById('message-input');
    const message = input.value.trim();
    const attachments = OSA.getAttachments().slice();
    if (!message && attachments.length === 0) return;

    const clientMessageId = OSA.generateClientMessageId();
    const shouldQueueLocally = OSA.isAgentProcessing() || (OSA.getSessionQueue() || []).length > 0;
    let optimisticDomId = '';

    input.value = '';
    OSA.hideSlashMenu();
    OSA.clearAttachments();
    OSA.renderAttachmentPreviews();
    OSA.setInputHistoryIndex(-1);
    OSA.getInputHistory().push(message);
    if (OSA.getInputHistory().length > 100) OSA.getInputHistory().shift();
    if (!shouldQueueLocally) {
        OSA.hideThinkingIndicator();
        OSA.releaseStreamingAssistantMessage();
        OSA.resetMessageChain();
        OSA.setProcessing(true);
        OSA.setHasReceivedResponse(false);
        OSA.setSendButtonStopMode(true);
        if (currentSession) currentSession.task_status = 'running';
        OSA.showThinkingIndicator();
    }
    const messagesDiv = document.getElementById('messages');
    
    const emptyState = messagesDiv.querySelector('.empty-state');
    if (emptyState) {
        emptyState.remove();
    }
    
    if (!shouldQueueLocally) {
        optimisticDomId = `message-user-${clientMessageId}`;
        const optimisticMessage = OSA.appendUserMessageToChat(message, {
            clientMessageId,
            timestamp: new Date().toISOString(),
            attachments: attachments,
        });
        if (optimisticMessage) optimisticMessage.id = optimisticDomId;
    }

    const attachmentPayload = attachments.map(att => ({
        filename: att.filename,
        mime: att.mime,
        data_url: att.dataUrl,
    }));

    try {
        OSA.clearAttachmentStatus();
        const ws = OSA.getWebSocket ? OSA.getWebSocket() : null;
        const useWs = ws && ws.readyState === WebSocket.OPEN && attachmentPayload.length === 0 && OSA.wsRequest;

        let data;
        if (useWs) {
            data = await OSA.wsRequest('session.send', {
                session_id: currentSession.id,
                content: message,
                client_message_id: clientMessageId,
            });
        } else {
            const res = await fetch(`/api/sessions/${currentSession.id}/send`, {
                method: 'POST',
                headers: {
                    'Authorization': `Bearer ${OSA.getToken()}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({ message, session_id: currentSession.id, client_message_id: clientMessageId, attachments: attachmentPayload })
            });
            data = await res.json().catch(() => ({}));

            if (!res.ok) {
                throw new Error(data.error || `HTTP ${res.status}`);
            }
        }

        if (data.queued) {
            if (currentSession && Array.isArray(currentSession.messages) && !shouldQueueLocally) {
                const last = currentSession.messages[currentSession.messages.length - 1];
                if (last && last.role === 'user' && last.content === message) {
                    currentSession.messages.pop();
                }
            }
            if (optimisticDomId) {
                const optimisticMessage = document.getElementById(optimisticDomId);
                if (optimisticMessage) optimisticMessage.remove();
            }

            const nextQueue = Array.isArray(OSA.getSessionQueue()) ? [...OSA.getSessionQueue()] : [];
            if (!nextQueue.some(item => item.client_message_id === clientMessageId)) {
                nextQueue.push(data.queue_item || {
                    id: clientMessageId,
                    client_message_id: clientMessageId,
                    content: message,
                    status: 'pending',
                    position: data.queue_position || (nextQueue.length + 1),
                    created_at: new Date().toISOString(),
                });
            }
            nextQueue.sort((a, b) => (a.position || 0) - (b.position || 0));
            OSA.setSessionQueue(nextQueue);
            OSA.renderQueuedMessages(nextQueue);
        } else {
            OSA.refreshCurrentSessionQueue();
        }
    } catch (error) {
        console.error('Failed to send message:', error);
        if ((error.message || '').toLowerCase().includes('attachment')) {
            OSA.setAttachmentStatus(error.message, 'error');
        }
        if (currentSession && Array.isArray(currentSession.messages) && !shouldQueueLocally) {
            const last = currentSession.messages[currentSession.messages.length - 1];
            if (last && last.role === 'user' && last.content === message) {
                currentSession.messages.pop();
            }
        }
        if (optimisticDomId) {
            const optimisticMessage = document.getElementById(optimisticDomId);
            if (optimisticMessage) optimisticMessage.remove();
        }
        OSA.showErrorCard(error.message);
        if (!shouldQueueLocally) {
            OSA.setProcessing(false);
            OSA.resetSendButton();
            OSA.hideThinkingIndicator();
        }
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
        const input = document.getElementById('message-input');
        if (input && input.value.trim()) {
            OSA.sendMessage();
        } else {
            OSA.stopGeneration();
        }
    } else {
        OSA.sendMessage();
    }
};

OSA.connectEventSource = function(sessionId) {
    if (OSA.connectWebSocket && OSA.connectWebSocket(sessionId)) {
        const existingES = OSA.getEventSource();
        if (existingES) {
            existingES.close();
            OSA.setEventSource(null);
        }
        return;
    }

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

    const chain = OSA.getMessageChain ? OSA.getMessageChain() : null;
    const lastSeq = chain && Number.isFinite(chain.eventSeqNumber)
        ? chain.eventSeqNumber
        : 0;

    const token = OSA.getToken ? OSA.getToken() : '';
    const queryParts = [];
    if (token) queryParts.push(`token=${encodeURIComponent(token)}`);
    if (lastSeq > 0) queryParts.push(`last_seq=${encodeURIComponent(lastSeq)}`);
    const query = queryParts.length ? `?${queryParts.join('&')}` : '';
    const sseUrl = token
        ? `/api/sessions/${sessionId}/events${query}`
        : `/api/sessions/${sessionId}/events${query}`;
    const es = new EventSource(sseUrl);
    
    es.onopen = () => {
        OSA.showConnectionStatus('connected', 'Connected');

        const session = OSA.getCurrentSession();
        if (session && session.id === sessionId && session.task_status === 'running') {
            if (!OSA.getStreamingAssistantMessage() && OSA.shouldShowThinkingIndicatorForRunningSession(session)) {
                OSA.showThinkingIndicator();
            }
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
    { cmd: '/workflow', label: 'Workflows', desc: 'Open workflow editor', action: () => OSA.openWorkflowEditor() },
    { cmd: '/compact', label: 'Compact', desc: 'Summarize and compact the conversation', action: () => { const i = document.getElementById('message-input'); if (i) { i.value = 'Summarize our conversation so far and continue'; OSA.sendMessage(); } } },
    { cmd: '/clear', label: 'Clear screen', desc: 'Clear the message display', action: () => { OSA.resetTranscriptView(); } },
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
    const sourceFilter = OSA.getSessionSourceFilter ? OSA.getSessionSourceFilter() : 'all';
    
    items.forEach(item => {
        const text = (item.textContent || '').toLowerCase();
        const source = item.dataset.sessionSource || 'web';
        const matchesSource = sourceFilter === 'all' || source === sourceFilter;
        item.style.display = ((!q || text.includes(q)) && matchesSource) ? '' : 'none';
    });

    if (q || sourceFilter !== 'all') {
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
        const currentSession = OSA.getCurrentSession();
        if (!currentSession || currentSession.id !== sessionId) return;
        
        const breadcrumb = [];
        if (data.session) {
            breadcrumb.unshift({ id: data.session.id, title: 'Parent Session' });
            let current = data.session;
            while (current.parent_id) {
                const parentRes = await fetch(`/api/sessions/${current.parent_id}/parent`, {
                    headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
                });
                const parentData = await parentRes.json();
                const activeSession = OSA.getCurrentSession();
                if (!activeSession || activeSession.id !== sessionId) return;
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
        const currentSession = OSA.getCurrentSession();
        if (!currentSession || currentSession.id !== sessionId) return;
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
        OSA.clearSessionCheckpoints(sessionId);
        if (OSA.getCurrentSession() && OSA.getCurrentSession().id === sessionId) {
            OSA.setCurrentSession(null);
            const es = OSA.getEventSource();
            if (es) {
                es.close();
                OSA.setEventSource(null);
            }
            const ws = OSA.getWebSocket ? OSA.getWebSocket() : null;
            if (ws) {
                ws.close();
                OSA.setWebSocket(null);
            }
            OSA.renderEmptyTranscript('Click "New chat" to begin');
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

OSA.openWorkflowEditor = async function() {
    try {
        await OSA.ensureWorkflowAssetsLoaded();
    } catch (error) {
        console.error('Failed to load workflow assets:', error);
        alert(error.message || 'Failed to load workflow editor');
        return;
    }

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

    const editor = window.ensureWorkflowEditor ? window.ensureWorkflowEditor() : window.workflowEditor;
    if (editor) {
        editor.init().catch(err => {
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

OSA.ACCEPTED_IMAGE_TYPES = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
OSA.ACCEPTED_ATTACHMENT_EXTENSIONS = ['pdf', 'txt', 'md', 'markdown', 'json', 'csv', 'js', 'jsx', 'ts', 'tsx', 'rs', 'py', 'html', 'css', 'toml', 'yaml', 'yml', 'xml', 'sql', 'sh', 'ps1', 'bat', 'ini', 'log'];
OSA.MAX_ATTACHMENT_SIZE = 12 * 1024 * 1024;

OSA.setAttachmentStatus = function(message, tone = 'info') {
    const status = document.getElementById('attachment-status');
    if (!status) return;

    if (OSA._attachmentStatusTimer) {
        clearTimeout(OSA._attachmentStatusTimer);
        OSA._attachmentStatusTimer = null;
    }

    if (!message) {
        status.innerHTML = '';
        status.classList.add('hidden');
        status.dataset.state = 'hidden';
        return;
    }

    status.innerHTML = '';
    const text = document.createElement('span');
    text.className = 'attachment-status-text';
    text.textContent = message;

    const dismiss = document.createElement('button');
    dismiss.type = 'button';
    dismiss.className = 'attachment-status-dismiss';
    dismiss.setAttribute('aria-label', 'Dismiss attachment status');
    dismiss.textContent = 'x';
    dismiss.addEventListener('click', () => OSA.clearAttachmentStatus());

    status.appendChild(text);
    status.appendChild(dismiss);
    status.classList.remove('hidden');
    status.dataset.state = tone;

    if (tone === 'error') {
        OSA._attachmentStatusTimer = setTimeout(() => OSA.clearAttachmentStatus(), 6000);
    }
};

OSA.clearAttachmentStatus = function() {
    OSA.setAttachmentStatus('');
};

OSA.getAttachmentExtension = function(filename) {
    const parts = String(filename || '').split('.');
    return parts.length > 1 ? parts[parts.length - 1].toLowerCase() : '';
};

OSA.isSupportedAttachmentFile = function(file) {
    if (OSA.ACCEPTED_IMAGE_TYPES.includes(file.type)) return true;
    return OSA.ACCEPTED_ATTACHMENT_EXTENSIONS.includes(OSA.getAttachmentExtension(file.name));
};

OSA.handleAttachmentFile = async function(file) {
    if (!OSA.isSupportedAttachmentFile(file)) {
        OSA.setAttachmentStatus(`Unsupported attachment type: ${file.name}`, 'error');
        return;
    }
    if (file.size > OSA.MAX_ATTACHMENT_SIZE) {
        OSA.setAttachmentStatus(
            `Attachment too large: ${file.name}. Limit is ${Math.round(OSA.MAX_ATTACHMENT_SIZE / (1024 * 1024))} MB.`,
            'error'
        );
        return;
    }
    const reader = new FileReader();
    reader.onload = () => {
        OSA.clearAttachmentStatus();
        OSA.addAttachment({
            kind: OSA.ACCEPTED_IMAGE_TYPES.includes(file.type) ? 'image' : 'document',
            id: 'att-' + Date.now() + '-' + Math.random().toString(36).slice(2, 8),
            filename: file.name,
            mime: file.type || 'application/octet-stream',
            sizeBytes: file.size,
            dataUrl: reader.result,
        });
        OSA.renderAttachmentPreviews();
    };
    reader.onerror = () => {
        OSA.setAttachmentStatus(`Failed to read attachment: ${file.name}`, 'error');
    };
    reader.readAsDataURL(file);
};

OSA.renderAttachmentPreviews = function() {
    const container = document.getElementById('image-preview-container');
    if (!container) return;
    const attachments = OSA.getAttachments();
    if (attachments.length === 0) {
        container.classList.add('hidden');
        container.innerHTML = '';
        return;
    }
    container.classList.remove('hidden');
    container.innerHTML = '';
    attachments.forEach(att => {
        const thumb = document.createElement('div');
        thumb.className = 'image-preview-thumb';
        if (att.kind === 'image') {
            thumb.innerHTML = `
                <img class="expandable-image" data-image-src="${att.dataUrl}" src="${att.dataUrl}" alt="${OSA.escapeHtml(att.filename)}" />
                <button class="image-preview-remove" onclick="OSA.handleRemoveAttachment('${att.id}')">&times;</button>
                <div class="image-preview-filename">${OSA.escapeHtml(att.filename)}</div>
            `;
        } else {
            const ext = OSA.getAttachmentExtension(att.filename) || 'file';
            thumb.classList.add('file-preview-thumb');
            thumb.innerHTML = `
                <div class="file-preview-icon">${OSA.escapeHtml(ext.toUpperCase().slice(0, 4))}</div>
                <button class="image-preview-remove" onclick="OSA.handleRemoveAttachment('${att.id}')">&times;</button>
                <div class="image-preview-filename">${OSA.escapeHtml(att.filename)}</div>
            `;
        }
        container.appendChild(thumb);
    });
};

OSA.handleRemoveAttachment = function(id) {
    const attachments = OSA.getAttachments();
    OSA.setAttachments(attachments.filter(a => a.id !== id));
    OSA.renderAttachmentPreviews();
};

OSA.ensureImagePreviewModal = function() {
    let modal = document.getElementById('image-preview-modal');
    if (modal) return modal;

    modal = document.createElement('div');
    modal.id = 'image-preview-modal';
    modal.className = 'image-preview-modal hidden';
    modal.innerHTML = `
        <div class="image-preview-modal-backdrop"></div>
        <div class="image-preview-modal-content">
            <button class="image-preview-modal-close" type="button" aria-label="Close image preview">&times;</button>
            <img id="image-preview-modal-img" src="" alt="Expanded attachment preview" />
        </div>
    `;
    document.body.appendChild(modal);

    const close = () => modal.classList.add('hidden');
    modal.querySelector('.image-preview-modal-backdrop').addEventListener('click', close);
    modal.querySelector('.image-preview-modal-close').addEventListener('click', close);
    modal.addEventListener('click', (event) => {
        if (event.target === modal) close();
    });
    document.addEventListener('keydown', (event) => {
        if (event.key === 'Escape') close();
    });

    return modal;
};

OSA.openImagePreviewModal = function(src) {
    if (!src) return;
    const modal = OSA.ensureImagePreviewModal();
    const img = document.getElementById('image-preview-modal-img');
    if (!img) return;
    img.src = src;
    modal.classList.remove('hidden');
};

OSA.setupAttachmentPicker = function() {
    const fileInput = document.getElementById('image-upload');
    if (fileInput) {
        fileInput.addEventListener('change', (e) => {
            const files = Array.from(e.target.files);
            files.forEach(file => OSA.handleAttachmentFile(file));
            e.target.value = '';
        });
    }

    const input = document.getElementById('message-input');
    if (input) {
        input.addEventListener('paste', async (e) => {
            const items = Array.from(e.clipboardData.items);
            const fileItems = items.filter(item => item.kind === 'file');
            if (fileItems.length > 0) {
                e.preventDefault();
                for (const item of fileItems) {
                    const file = item.getAsFile();
                    if (file) await OSA.handleAttachmentFile(file);
                }
                return;
            }
        });
    }

    const inputArea = document.querySelector('.input-area');
    if (inputArea) {
        inputArea.addEventListener('dragover', (e) => {
            e.preventDefault();
            inputArea.classList.add('drag-over');
        });
        inputArea.addEventListener('dragleave', (e) => {
            e.preventDefault();
            inputArea.classList.remove('drag-over');
        });
        inputArea.addEventListener('drop', async (e) => {
            e.preventDefault();
            inputArea.classList.remove('drag-over');
            const files = Array.from(e.dataTransfer.files);
            for (const file of files) {
                if (OSA.isSupportedAttachmentFile(file)) {
                    await OSA.handleAttachmentFile(file);
                }
            }
        });
    }
};

document.addEventListener('DOMContentLoaded', () => {
    OSA.setupAttachmentPicker();
    OSA.ensureImagePreviewModal();
    document.body.addEventListener('click', (event) => {
        const image = event.target.closest('.expandable-image');
        if (!image) return;
        const src = image.dataset.imageSrc || image.getAttribute('src');
        OSA.openImagePreviewModal(src);
    });
});

OSA.initTheme();
OSA.checkAuthAndInit();

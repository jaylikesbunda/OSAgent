window.OSA = window.OSA || {};

// Mobile browser chrome and the on-screen keyboard change the visible height
// without always changing CSS viewport units (notably in Safari).
OSA.syncMobileViewportHeight = function() {
    if (!window.matchMedia('(max-width: 900px)').matches) {
        document.documentElement.style.removeProperty('--app-viewport-height');
        return;
    }
    const viewport = window.visualViewport;
    const height = viewport ? viewport.height : window.innerHeight;
    if (height > 0) {
        document.documentElement.style.setProperty('--app-viewport-height', `${height}px`);
    }
};
OSA.syncMobileViewportHeight();
window.addEventListener('resize', OSA.syncMobileViewportHeight);
window.visualViewport?.addEventListener('resize', OSA.syncMobileViewportHeight);

OSA.closeMobileComposerMenu = function() {
    document.getElementById('mobile-composer-menu')?.classList.add('hidden');
    document.getElementById('mobile-actions-btn')?.setAttribute('aria-expanded', 'false');
    document.removeEventListener('click', OSA._mobileComposerOutsideClick);
    document.removeEventListener('keydown', OSA._mobileComposerEscape, true);
};

OSA._mobileComposerOutsideClick = function(event) {
    if (!event.target.closest?.('#mobile-composer-menu, #mobile-actions-btn')) {
        OSA.closeMobileComposerMenu();
    }
};

OSA._mobileComposerEscape = function(event) {
    if (event.key !== 'Escape') return;
    event.stopPropagation();
    OSA.closeMobileComposerMenu();
    document.getElementById('mobile-actions-btn')?.focus();
};

OSA.toggleMobileComposerMenu = function(event) {
    event?.stopPropagation();
    const menu = document.getElementById('mobile-composer-menu');
    const button = document.getElementById('mobile-actions-btn');
    if (!menu || !button) return;
    if (!menu.classList.contains('hidden')) {
        OSA.closeMobileComposerMenu();
        return;
    }
    const ttsAction = document.getElementById('mobile-tts-action');
    const speaking = !!OSA.getTtsEnabled?.();
    if (ttsAction) {
        ttsAction.textContent = speaking ? 'Stop reading replies' : 'Read replies aloud';
        ttsAction.setAttribute('aria-pressed', String(speaking));
    }
    const followupAction = document.getElementById('mobile-followup-action');
    const followupToggle = document.getElementById('followup-toggle');
    if (followupAction && followupToggle) {
        followupAction.classList.toggle('hidden', followupToggle.classList.contains('hidden'));
        followupAction.textContent = `Follow-ups: ${OSA.getFollowUpBehavior?.() === 'steer' ? 'Steer' : 'Queue'}`;
    }
    menu.classList.remove('hidden');
    button.setAttribute('aria-expanded', 'true');
    document.addEventListener('click', OSA._mobileComposerOutsideClick);
    document.addEventListener('keydown', OSA._mobileComposerEscape, true);
};

OSA.mobileComposerAction = function(action, event) {
    event?.stopPropagation();
    OSA.closeMobileComposerMenu();
    if (action === 'attach') document.getElementById('image-upload')?.click();
    else if (action === 'context') OSA.toggleContextMenu();
    else if (action === 'voice') OSA.openVoiceMode();
    else if (action === 'tts') OSA.toggleTTS();
    else if (action === 'followup') OSA.toggleFollowUpBehavior();
};

OSA._debounceTimers = {};
OSA.debounce = function(key, fn, delay) {
    if (OSA._debounceTimers[key]) clearTimeout(OSA._debounceTimers[key]);
    OSA._debounceTimers[key] = setTimeout(() => { delete OSA._debounceTimers[key]; fn(); }, delay);
};

OSA.resizeMessageInput = function(input) {
    if (!input) return;
    input.style.height = 'auto';
    input.style.height = Math.min(input.scrollHeight, 180) + 'px';
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
        OSA.initPushToTalk?.();
        OSA.loadProviderCatalog();
        OSA.refreshWorkflowAvailability?.();
        if (typeof OSA.checkForUpdatesOnStartup === 'function') {
            OSA.checkForUpdatesOnStartup().catch(function(error) {
                console.debug('Startup update check failed:', error);
            });
        }
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

OSA._autoNamedSessions = new Set();

// Sessions are created with a "Session N" placeholder, so a name being present
// does not mean the user (or the auto-namer) ever set a real one.
OSA.isDefaultSessionName = function(name) {
    return typeof name === 'string' && /^Session \d+$/.test(name.trim());
};

OSA.maybeAutoNameSession = function(sessionId) {
    const session = sessionId ? OSA.getSessionObjectFor(sessionId) : OSA.getCurrentSession();
    if (!session?.id) return;
    const existingName = session.metadata?.name;
    if (existingName && !OSA.isDefaultSessionName(existingName)) return;
    OSA._autoNamedSessions = OSA._autoNamedSessions || new Set();
    if (OSA._autoNamedSessions.has(session.id)) return;
    OSA._autoNamedSessions.add(session.id);
    OSA.fetchWithAuth('/api/sessions/' + encodeURIComponent(session.id) + '/auto-name', { method: 'POST' })
        .then(function(res) { return res.json(); })
        .then(function(data) {
            if (!data?.name) return;
            // The entry may have been replaced by a snapshot refetch while the
            // name was generating; write through to whichever object holds it.
            const target = OSA.getSessionObjectFor(session.id) || session;
            if (target.metadata) target.metadata.name = data.name;
            // Only touch the header when this session is actually viewed.
            if (OSA.getCurrentSession()?.id !== session.id) {
                OSA.loadSessions();
                return;
            }
            document.getElementById('header-title').textContent = data.name;
            OSA.setHeaderBaseTitle(data.name);
            OSA.setHeaderTitleRenameable(true);
            OSA.loadSessions();
        })
        .catch(function() {});
};

OSA.getSessionSourceKey = function(session) {
    const source = (session && session.metadata && typeof session.metadata.source === 'string')
        ? session.metadata.source.trim().toLowerCase()
        : '';
    if (source === 'discord' || source === 'discord-community' || source === 'web') return source;
    if (source === 'discord-shared' || source === 'shared') return 'discord-shared';

    const owner = (session && session.metadata && typeof session.metadata.owner === 'string')
        ? session.metadata.owner
        : '';
    if (owner.startsWith('discord-community:')) return 'discord-community';
    if (owner.startsWith('discord-channel:')) return 'discord-shared';
    if (owner.startsWith('discord:')) return 'discord';
    return 'web';
};

OSA.getSessionSourceLabel = function(sourceKey) {
    if (sourceKey === 'discord') return 'Discord';
    if (sourceKey === 'discord-community') return 'Community';
    if (sourceKey === 'discord-shared') return 'Shared';
    return 'Web';
};

OSA.getSessionWorkspaceLabel = function(session) {
    const wsId = session && session.metadata && session.metadata.workspace_id;
    if (!wsId) return '';
    const ws = OSA.getWorkspaceState();
    const match = Array.isArray(ws.workspaces) && ws.workspaces.find(w => w.id === wsId);
    return (match && (match.name || match.id)) || wsId;
};

OSA.formatSessionListDate = function(dateStr) {
    const d = new Date(dateStr);
    if (isNaN(d.getTime())) return '';
    const diffMs = Date.now() - d.getTime();
    const minute = 60000;
    const hour = 3600000;
    const day = 86400000;
    if (diffMs < minute) return 'now';
    if (diffMs < hour) return Math.floor(diffMs / minute) + 'm';
    if (diffMs < day) return Math.floor(diffMs / hour) + 'h';
    if (diffMs < 7 * day) return d.toLocaleDateString(undefined, { weekday: 'short' });
    return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
};

// Collapsed subagent groups: persisted per parent session id.
OSA._collapsedGroups = new Set();
try {
    OSA._collapsedGroups = new Set(JSON.parse(localStorage.getItem('osa.sidebar.collapsedGroups') || '[]'));
} catch (_) {
    OSA._collapsedGroups = new Set();
}

OSA.persistCollapsedGroups = function() {
    try {
        localStorage.setItem('osa.sidebar.collapsedGroups', JSON.stringify([...OSA._collapsedGroups]));
    } catch (_) { /* storage unavailable */ }
};

OSA.setGroupCollapsed = function(parentId, collapsed) {
    if (!parentId) return;
    if (collapsed) {
        OSA._collapsedGroups.add(parentId);
    } else {
        OSA._collapsedGroups.delete(parentId);
    }
    OSA.persistCollapsedGroups();

    const group = document.querySelector(`.session-children[data-parent="${parentId}"]`);
    if (group) group.classList.toggle('collapsed', collapsed);

    const toggle = document.querySelector(`.session-item[data-session-id="${parentId}"] .session-group-toggle`);
    if (toggle) toggle.classList.toggle('open', !collapsed);
};

OSA.toggleSessionGroup = function(parentId, ev) {
    if (ev) {
        ev.preventDefault();
        ev.stopPropagation();
    }
    OSA.setGroupCollapsed(parentId, !OSA._collapsedGroups.has(parentId));
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
            const validRes = await OSA.fetchWithAuth('/api/session-summaries');
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
    OSA.SessionStore = {};
    OSA.unreadSessions = {};
    OSA.saveUnreadMap();
    OSA.sessionQueue = [];
    OSA.sessionToolEvents = [];
    OSA.sessionSubagentTasks = [];
    OSA.resetSessionCheckpoints();
    OSA.setSessionInspectorState({ history: [], snapshots: [] });

    if (typeof OSA.stopProgressListener === 'function') {
        OSA.stopProgressListener();
    }
    if (typeof OSA.stopToolSync === 'function') {
        OSA.stopToolSync();
    }

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
    OSA.wsSubscribedSessions = {};

    if (typeof OSA.setEventSourceSessionId === 'function') {
        OSA.setEventSourceSessionId(null);
    }
    if (typeof OSA.cancelSpeechOutput === 'function') {
        OSA.cancelSpeechOutput();
    }
};

// Session switch keeps the multiplexed WebSocket (background turns stay
// subscribed) and only tears down the per-session SSE fallback channel.
OSA.disconnectSessionChannel = function() {
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
    // Load workspaces first so session rows can resolve workspace names on first paint.
    OSA.loadWorkspaces().catch(() => {}).then(() => {
        OSA.loadSessions();
    });
    OSA.loadModel();
    OSA.startPermissionPolling();
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
            OSA.setModelTrigger(data.provider_id || '', data.model || '');
        }
        if (typeof OSA.refreshThinkingOptions === 'function') {
            const selected = OSA.getCachedConfig?.()?.agent?.thinking_level || 'auto';
            await OSA.refreshThinkingOptions(data.provider_id || '', data.model || '', selected);
        }
        if (typeof OSA.renderDiscordActiveModel === 'function') {
            OSA.renderDiscordActiveModel();
        }
    } catch (error) {
        console.error('Failed to load model:', error);
    }
};

OSA.updateModel = async function() {
    const input = document.getElementById('model-input');
    if (!input) return;
    const model = OSA.currentModelId || input.dataset.modelId || '';
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
            body: JSON.stringify({
                model,
                provider_id: OSA.currentModelProviderId || input.dataset.providerId || undefined
            })
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
            const res = await OSA.fetchWithAuth('/api/session-summaries');
            sessions = await res.json();
        }

        if (!Array.isArray(sessions)) {
            sessions = [];
        }

        // The sidebar list is authoritative for running icons, but the live
        // event stream knows the instant a turn ends. A summaries fetch that
        // started mid-turn can arrive after the terminal event and resurrect
        // the orbit animation, so trust the local turn state for the active
        // session over any stale fetch result.
        const currentSession = OSA.getCurrentSession();
        if (currentSession && currentSession.id) {
            const localRunning = OSA.isAgentProcessing()
                || ((OSA.getSessionQueue() || []).length > 0)
                || !!OSA.tmodelStreamingItem?.();
            for (const s of sessions) {
                if (s && s.id === currentSession.id) {
                    if (currentSession.task_status === 'active') s.task_status = 'active';
                    else if (localRunning || currentSession.task_status === 'running') s.task_status = 'running';
                    break;
                }
            }
        }

        const sessionIds = new Set(sessions.map(function(session) { return session.id; }));
        OSA.pruneUnreadMap(Array.from(sessionIds));

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

        // Prune stored collapse state for parents that no longer have children.
        if (OSA._collapsedGroups.size > 0) {
            let pruned = false;
            for (const id of [...OSA._collapsedGroups]) {
                if (!childMap.has(id)) {
                    OSA._collapsedGroups.delete(id);
                    pruned = true;
                }
            }
            if (pruned) OSA.persistCollapsedGroups();
        }

        const renderSession = (s, isChild, hasRunningChildren, extraIndent, childCount) => {
            const indent = isChild ? (extraIndent || '') : '';
            const childClass = isChild ? ' session-child' : '';
            const isActive = currentSession && currentSession.id === s.id;
            const displayName = OSA.getSessionDisplayName(s);
            const sourceKey = OSA.getSessionSourceKey(s);
            const sourceLabel = OSA.getSessionSourceLabel(sourceKey);
            const workspaceLabel = OSA.getSessionWorkspaceLabel(s);
            const dateLabel = OSA.formatSessionListDate(s.created_at);
            const isRunning = s.task_status === 'running' || hasRunningChildren;
            const unread = !isActive && OSA.isSessionUnread(s.id);
            const iconHtml = isRunning
                ? OSA.sessionRunningOrbitHtml()
                : (isChild ? 'A' : OSA.escapeHtml(OSA.sessionIconLetterFor(workspaceLabel, displayName)));
            const iconStyle = isChild && !isRunning
                ? 'style="width:22px;height:22px;font-size:10px;border-radius:4px;background:var(--bg-tertiary);color:var(--text-secondary);border:1px solid var(--border);"'
                : (!isRunning ? `style="--session-hue:${OSA.sessionHueFor(s.id)}"` : '');
            const iconClass = isRunning ? ' session-icon-running' : (isChild ? '' : ' session-letter');
            const badgeHtml = sourceKey !== 'web'
                ? `<span class="session-source-badge source-${OSA.escapeAttr(sourceKey)}">${OSA.escapeHtml(sourceLabel)}</span>`
                : '';
            const groupToggleHtml = (!isChild && childCount > 0)
                ? `
                    <button class="session-group-toggle${OSA._collapsedGroups.has(s.id) ? '' : ' open'}" onclick="OSA.toggleSessionGroup(${OSA.jsArg(s.id)}, event)" title="Show/hide subagents" aria-label="Show/hide subagents">
                        <span class="session-group-count">${childCount}</span>
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>
                    </button>`
                : '';
            return `
            <div class="session-item${childClass} ${isActive ? 'active' : ''}${unread ? ' has-unread' : ''}" data-session-id="${OSA.escapeAttr(s.id)}" data-session-source="${OSA.escapeAttr(sourceKey)}" onclick="OSA.selectSession(${OSA.jsArg(s.id)})" style="${indent}">
                <div class="session-icon${iconClass}" ${iconStyle}>${iconHtml}${unread ? '<span class="session-unread-dot" aria-label="Unread response"></span>' : ''}</div>
                <div class="session-info">
                    <div class="session-name">${OSA.escapeHtml(displayName)}</div>
                    <div class="session-meta">
                        ${workspaceLabel ? `<span class="session-workspace" title="${OSA.escapeHtml(workspaceLabel)}"><svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg><span class="session-workspace-name">${OSA.escapeHtml(workspaceLabel)}</span></span><span class="session-meta-sep">&middot;</span>` : ''}
                        <span class="session-date" title="${new Date(s.created_at).toLocaleString()}">${OSA.escapeHtml(dateLabel)}</span>
                        ${badgeHtml}
                    </div>
                </div>${groupToggleHtml}
                <div class="session-actions">
                    <button class="session-action-btn rename-btn" onclick="event.stopPropagation(); OSA.startRenameSession(${OSA.jsArg(s.id)}, this)" title="Rename">
                        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                            <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"/>
                            <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"/>
                        </svg>
                    </button>
                    <button class="session-action-btn delete-btn" onclick="event.stopPropagation(); OSA.deleteSession(${OSA.jsArg(s.id)})" title="Delete">
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
            let html = renderSession(s, false, hasRunningChildren, '', children ? children.length : 0);
            if (children && children.length > 0) {
                html += `<div class="session-children${OSA._collapsedGroups.has(s.id) ? ' collapsed' : ''}" data-parent="${OSA.escapeHtml(s.id)}">`;
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
                sessionsHtml += renderSession(c, true, false, 'padding-left: 14px;');
            });
            sessionsHtml += '</div>';
        }

        list.innerHTML = `
            <div class="session-search">
                <input type="text" id="session-search-input" placeholder="Search sessions..." oninput="OSA.debounce('sessionSearch', () => OSA.filterSessionsWithContent(this.value), 250)" />
                <select id="session-source-filter" onchange="OSA.setSessionSourceFilter(this.value); OSA.filterSessions(document.getElementById('session-search-input')?.value || '')">
                    <option value="all">All sources</option>
                    <option value="web">Web</option>
                    <option value="discord">Discord</option>
                    <option value="discord-community">Community</option>
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
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 15000);
    
    try {
        const res = await OSA.fetchWithAuth('/api/sessions', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ workspace_id: workspaceId }),
            signal: controller.signal,
        });
        const session = await res.json().catch(() => ({}));
        if (!res.ok || !session.id) {
            throw new Error(session.error || `Could not create session (HTTP ${res.status})`);
        }
        OSA.setCurrentSession(session);
        OSA.markSessionSeen(session.id);
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
        OSA.renderEmptyTranscript('Type a message below to start.');
        const sessionName = OSA.getSessionDisplayName(session);
        OSA.setHeaderBaseTitle(sessionName);
        document.getElementById('header-title').textContent = sessionName;
        OSA.setHeaderTitleRenameable(true);
        OSA.loadSessions();
        OSA.loadSessionWorkspace();
        OSA.loadSessionPersona();
        return session;
    } catch (error) {
        console.error('Failed to create session:', error);
        const message = error.name === 'AbortError'
            ? 'Session creation timed out. Please try again.'
            : error.message;
        OSA.showErrorCard?.(message);
        return null;
    } finally {
        clearTimeout(timeout);
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
    const requestId = (OSA._runningSnapshotRequestId || 0) + 1;
    OSA._runningSnapshotRequestId = requestId;
    try {
        const currentSession = OSA.getCurrentSession();
        if (!currentSession || currentSession.id !== sessionId) return;
        const entry = OSA.getSessionEntry(sessionId);
        const sequenceAtRequest = entry.chain.eventSeqNumber;

        const res = await fetch(`/api/sessions/${sessionId}`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (!res.ok) return;

        const session = await res.json();
        if (OSA._runningSnapshotRequestId !== requestId) return;
        if (!OSA.getCurrentSession() || OSA.getCurrentSession().id !== sessionId) return;
        OSA.reconcileSessionSnapshot(entry, session, sequenceAtRequest);

        const hasLiveAgentActivity = OSA.tmodelHasLiveAgentActivity();
        if (session.task_status !== 'running') {
            if (hasLiveAgentActivity) {
                OSA.completeThinkingDisplay();
                OSA.completeAssistantResponse();
            }
            OSA.setCurrentSession(session);
        OSA.markSessionSeen(session.id);
            OSA.rebuildTranscriptFromSession(
                session,
                OSA.getSessionToolEvents() || [],
                OSA.getSessionSubagentTasks() || [],
                {
                    reason: 'snapshot-final',
                    preserveKeys: true,
                    keepCurrentArtifacts: true,
                    adoptStreaming: false,
                },
            );
            OSA.hideThinkingIndicator();
            OSA.stopToolSync();
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
            OSA.refreshCurrentSessionQueue();
            OSA.loadSessions();
            return;
        }

        // Mid-turn the live event stream owns the transcript; a fetched snapshot
        // only lags it. Fall back to it exclusively when no live agent output
        // exists (fresh page attach or an optimistic send whose events were
        // missed by both transports).
        if (hasLiveAgentActivity) {
            currentSession.task_status = session.task_status;
            return;
        }

        OSA.setCurrentSession(session);
        OSA.markSessionSeen(session.id);
        OSA.rebuildTranscriptFromSession(session, OSA.getSessionToolEvents() || [], OSA.getSessionSubagentTasks() || [], { reason: 'snapshot-adopt' });

        if (!OSA.getActiveTurnAssistantMessage(session)) {
            OSA.releaseStreamingAssistantMessage();
            if (OSA.shouldShowThinkingIndicatorForRunningSession(session)) {
                OSA.showThinkingIndicator();
            } else {
                OSA.hideThinkingIndicator();
            }
            return;
        }

        OSA.hideThinkingIndicator();
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
    const selectionEntry = OSA.getSessionEntry(sessionId);
    const sequenceAtSnapshotRequest = selectionEntry && selectionEntry.chain
        && Number.isFinite(selectionEntry.chain.eventSeqNumber)
        ? selectionEntry.chain.eventSeqNumber
        : 0;

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
        let session = await res.json();
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
        const pendingHistoryRequest = fetch(`/api/sessions/${sessionId}/history`, {
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

        const entry = OSA.getSessionEntry(sessionId);
        // Reconcile before setCurrentSession replaces entry.session. Otherwise
        // a slightly stale GET discards text streamed while this task was in
        // the background and can leave a partial bubble marked as typing.
        OSA.reconcileSessionSnapshot(entry, session, sequenceAtSnapshotRequest);
        OSA.setCurrentSession(session);
        OSA.markSessionSeen(session.id);
        OSA.markSessionSeen(sessionId);
        OSA.restoreContextState(session.id, session.context_state || null);

        OSA.getActiveTools().clear();
        OSA.parallelToolGroups = [];
        OSA._contextGroupState = null;
        OSA.stopToolSync();
        OSA.disconnectSessionChannel();

        OSA.hideThinkingIndicator();
        OSA.setTurnStartTime(null);
        OSA.resetStreamingMessage();

        const sessionName = OSA.getSessionDisplayName(session);
        OSA.setHeaderBaseTitle(sessionName);
        document.getElementById('header-title').textContent = sessionName;
        OSA.setHeaderTitleRenameable(true);

        const messagesDiv = document.getElementById('messages');
        OSA.resetTranscriptView();

        // Snapshot of live entry state to merge against fetch results: events
        // arriving while the artifact requests are in flight must not be
        // clobbered by slightly-stale responses.
        const preToolsById = {};
        (entry.tools || []).forEach(function(t) { if (t && t.tool_call_id) preToolsById[t.tool_call_id] = t; });
        const preQueueIds = new Set((entry.queue || []).map(function(q) { return q && q.id; }).filter(Boolean));
        const preSubById = {};
        (entry.subagents || []).forEach(function(s) { if (s && s.session_id) preSubById[s.session_id] = s; });

        if (session.messages.length === 0) {
            OSA.renderEmptyTranscript('Type a message below');
        } else {
            OSA.rebuildTranscriptFromSession(session, entry.tools || [], entry.subagents || [], { reason: 'session-switch' });
            if ((session.task_status === 'running' || entry.processing) && !OSA.tmodelStreamingItem()
                && OSA.shouldShowThinkingIndicatorForRunningSession(session)) {
                OSA.showThinkingIndicator();
            }
        }

        const sessionIsRunning = session.task_status === 'running' || entry.processing === true;
        if (sessionIsRunning) {
            entry.processing = true;
            OSA.setProcessing(true);
            OSA.setStopping(false);
            OSA.setSendButtonStopMode(true);
            OSA.startToolSync();
        } else {
            entry.processing = false;
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
        }

        OSA.fetchAndRenderTodos();
        OSA.loadSessionWorkspace();
        OSA.loadSessionPersona();
        OSA.loadSessionBreadcrumb(sessionId);

        const [toolStartsRes, subagentsRes, historyRes, queueRes] = await Promise.all([
            pendingToolsRequest,
            pendingSubagentsRequest,
            pendingHistoryRequest,
            pendingQueueRequest
        ]);
        if (!isCurrentSelection()) return;

        await pendingCheckpointsRequest;
        if (!isCurrentSelection()) return;

        const tools = (toolStartsRes && toolStartsRes.ok) ? await toolStartsRes.json() : [];
        if (!isCurrentSelection()) return;
        const subagentsData = (subagentsRes && subagentsRes.ok) ? await subagentsRes.json() : { subagents: [], has_running: false };
        if (!isCurrentSelection()) return;
        let historyData = (historyRes && historyRes.ok) ? await historyRes.json() : [];
        if (!isCurrentSelection()) return;
        const queueItems = (queueRes && queueRes.ok) ? await queueRes.json() : [];
        if (!isCurrentSelection()) return;

        // The history read can finish after the first session GET. Refresh a
        // running snapshot before using history's sequence as the replay
        // cursor, or a completed reply in that gap will be skipped forever.
        if (session.task_status === 'running' || entry.processing) {
            const sequenceBeforeRefresh = entry.chain.eventSeqNumber;
            const refreshedRes = await fetch(`/api/sessions/${sessionId}`, {
                headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
                signal,
            }).catch(() => null);
            if (!isCurrentSelection()) return;
            if (refreshedRes && refreshedRes.ok) {
                const refreshedSession = await refreshedRes.json();
                if (!isCurrentSelection()) return;
                if (refreshedSession.task_status !== 'running') {
                    // A final snapshot can contain chunks newer than the
                    // first history read. Advance the cursor with a history
                    // read taken after that snapshot to avoid replaying them.
                    const refreshedHistoryRes = await fetch(`/api/sessions/${sessionId}/history`, {
                        headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
                        signal,
                    }).catch(() => null);
                    if (!isCurrentSelection()) return;
                    if (refreshedHistoryRes && refreshedHistoryRes.ok) {
                        historyData = await refreshedHistoryRes.json();
                        if (!isCurrentSelection()) return;
                        OSA.reconcileSessionSnapshot(entry, refreshedSession, sequenceBeforeRefresh);
                        session = refreshedSession;
                        OSA.setCurrentSession(session);
                        OSA.restoreContextState(session.id, session.context_state || null);
                    }
                }
            }
        }

        const latestEventSequence = Array.isArray(historyData) && historyData.length > 0
            ? Number(historyData[historyData.length - 1]?.sequence || 0)
            : 0;
        const chain = OSA.getMessageChain();
        // Never move the counter backwards: live events may have arrived while
        // the history request was in flight, and rewinding would re-apply
        // them as duplicates (stuttered chunks) or force a full replay.
        if (Number.isFinite(latestEventSequence)) {
            chain.eventSeqNumber = Math.max(chain.eventSeqNumber || 0, latestEventSequence);
        }
        chain.eventSessionId = sessionId;

        // Merge fetched artifacts with live entry state (see pre-fetch
        // snapshots above): union tools/subagents by id with the live version
        // winning on completion, and drop queue items dispatched mid-load.
        const fetchedTools = Array.isArray(tools) ? tools : [];
        const mergedToolsById = {};
        fetchedTools.forEach(function(t) { if (t && t.tool_call_id) mergedToolsById[t.tool_call_id] = t; });
        Object.keys(preToolsById).forEach(function(id) {
            const live = preToolsById[id];
            const fetched = mergedToolsById[id];
            if (!fetched) {
                // Live-only tool (started after the GET): keep it.
                mergedToolsById[id] = {
                    tool_call_id: live.tool_call_id,
                    tool_name: live.tool_name,
                    arguments: live.arguments,
                    output: live.output,
                    title: live.title,
                    metadata: live.metadata,
                    message_index: live.message_index,
                    timestamp: live.timestamp,
                    completed: live.completed,
                    success: live.success,
                };
            } else if (live.completed && !fetched.completed) {
                mergedToolsById[id] = fetched;
                mergedToolsById[id].completed = true;
                mergedToolsById[id].success = live.success;
                if (live.output) mergedToolsById[id].output = live.output;
            }
        });
        // Also fold in anything the live channel added while awaiting.
        (entry.tools || []).forEach(function(t) {
            if (t && t.tool_call_id && !mergedToolsById[t.tool_call_id]) {
                mergedToolsById[t.tool_call_id] = {
                    tool_call_id: t.tool_call_id,
                    tool_name: t.tool_name,
                    arguments: t.arguments,
                    output: t.output,
                    title: t.title,
                    metadata: t.metadata,
                    message_index: t.message_index,
                    timestamp: t.timestamp,
                    completed: t.completed,
                    success: t.success,
                };
            }
        });
        const mergedTools = Object.keys(mergedToolsById).map(function(id) { return mergedToolsById[id]; });

        const fetchedSubs = (subagentsData && Array.isArray(subagentsData.subagents)) ? subagentsData.subagents : [];
        const mergedSubsById = {};
        fetchedSubs.forEach(function(s) { if (s && s.session_id) mergedSubsById[s.session_id] = s; });
        Object.keys(preSubById).forEach(function(id) {
            if (!mergedSubsById[id]) mergedSubsById[id] = preSubById[id];
        });
        (entry.subagents || []).forEach(function(s) {
            if (s && s.session_id && !mergedSubsById[s.session_id]) mergedSubsById[s.session_id] = s;
        });
        const mergedSubs = Object.keys(mergedSubsById).map(function(id) { return mergedSubsById[id]; });

        // Queue: server wins, except items dispatched (removed from the entry)
        // while the GET was in flight must stay removed, and local-only
        // optimistic items must stay.
        const fetchedQueue = Array.isArray(queueItems) ? queueItems : [];
        const postQueueIds = new Set((entry.queue || []).map(function(q) { return q && q.id; }).filter(Boolean));
        const dispatchedDuringLoad = new Set();
        preQueueIds.forEach(function(id) { if (!postQueueIds.has(id)) dispatchedDuringLoad.add(id); });
        const mergedQueue = fetchedQueue.filter(function(q) { return q && !dispatchedDuringLoad.has(q.id); });
        (entry.queue || []).forEach(function(q) {
            if (q && q.id && !mergedQueue.some(function(m) { return m.id === q.id; })) mergedQueue.push(q);
        });

        OSA.setSessionQueue(mergedQueue);
        OSA.setSessionToolEvents(mergedTools);
        OSA.setSessionSubagentTasks(mergedSubs);
        if (session.task_status !== 'running' && !entry.processing) {
            OSA.hideThinkingIndicator();
            OSA.stopToolSync();
            OSA.setProcessing(false);
        }
        OSA.rebuildTranscriptFromSession(session, mergedTools, mergedSubs, {
            reason: 'session-artifacts',
            adoptStreaming: session.task_status === 'running' || entry.processing,
        });
        OSA.perfLog?.('selectSession:artifacts', {
            sessionId,
            requestId,
            tools: mergedTools.length,
            subagents: mergedSubs.length,
            queue: mergedQueue.length,
            totalMs: Math.round((OSA.perfNow ? OSA.perfNow() : Date.now()) - perfStart),
        });

        OSA.renderQueuedMessages(mergedQueue);

        const subagentsRunning = !!(subagentsData && subagentsData.has_running);
        // Re-check liveness: a turn may have started via the live channel while
        // the snapshot was loading. Entry flags are authoritative over the
        // possibly-stale fetch-time task_status.
        const liveRunning = entry.processing === true || session.task_status === 'running';
        const isDirectlyRunning = liveRunning && !subagentsRunning;

        OSA.connectEventSource(sessionId);

        if (liveRunning || subagentsRunning) {
            entry.processing = true;
            OSA.setProcessing(true);
            OSA.setStopping(false);
            OSA.setSendButtonStopMode(true);
        } else {
            entry.processing = false;
            OSA.setProcessing(false);
            OSA.setStopping(false);
            OSA.resetSendButton();
        }

        if (isDirectlyRunning && OSA.shouldShowThinkingIndicatorForRunningSession(session, mergedTools) && !OSA.getStreamingAssistantMessage()) {
            OSA.showThinkingIndicator();
        }

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
        OSA.showErrorCard?.(`Failed to load session: ${error.message}`);
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

    // If the selected session is a subagent inside a collapsed group, open it.
    const selectedEl = document.querySelector(`.session-item[data-session-id="${sessionId}"]`);
    const group = selectedEl ? selectedEl.closest('.session-children') : null;
    if (group && group.dataset.parent && group.classList.contains('collapsed')) {
        OSA.setGroupCollapsed(group.dataset.parent, false);
    }
};

// Flip one sidebar row between the running orbit animation and its static
// icon without rebuilding the whole list, so the animation can never linger
// after a turn ends while a background loadSessions refresh is in flight.
OSA.sessionRunningOrbitHtml = function() {
    return `<span class="session-running-orbits" aria-hidden="true">`
        + `<span class="session-running-track track-a"></span>`
        + `<span class="session-running-track track-b"></span>`
        + `<span class="session-running-track track-c"></span>`
        + `<span class="session-running-core"></span>`
        + `</span>`;
};

OSA.setSessionSidebarRunning = function(sessionId, running) {
    if (!sessionId) return;
    const item = document.querySelector(`.session-item[data-session-id="${sessionId}"]`);
    if (!item) return;
    const icon = item.querySelector(':scope > .session-icon');
    if (!icon) return;
    const isChild = item.classList.contains('session-child');
    if (running) {
        icon.classList.add('session-icon-running');
        icon.classList.remove('session-letter');
        icon.removeAttribute('style');
        icon.innerHTML = OSA.sessionRunningOrbitHtml();
        OSA.renderSessionUnreadIndicator(sessionId);
        return;
    }
    icon.classList.remove('session-icon-running');
    if (isChild) {
        icon.setAttribute('style', 'width:22px;height:22px;font-size:10px;border-radius:4px;background:var(--bg-tertiary);color:var(--text-secondary);border:1px solid var(--border);');
        icon.textContent = 'A';
    } else {
        const session = OSA.getSessionObjectFor(sessionId);
        const name = item.querySelector('.session-name')?.textContent || '';
        const workspaceLabel = session ? OSA.getSessionWorkspaceLabel(session) : '';
        icon.classList.add('session-letter');
        icon.setAttribute('style', `--session-hue:${OSA.sessionHueFor(sessionId)}`);
        icon.textContent = OSA.sessionIconLetterFor(workspaceLabel, name);
    }
    OSA.renderSessionUnreadIndicator(sessionId);
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
        OSA.SessionStore = {};
        OSA.unreadSessions = {};
        OSA.saveUnreadMap();
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
        OSA.renderEmptyTranscript('Start a new chat to begin');
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
    const inputEl = document.getElementById('message-input');
    const draftMessage = inputEl ? inputEl.value.trim() : '';
    OSA.debug?.log('send.start', {
        hasSession: !!OSA.getCurrentSession()?.id,
        chars: draftMessage.length,
        attachments: OSA.getAttachments().length,
    });

    if (draftMessage && OSA.getAttachments().length === 0) {
        // /compact accepts trailing focus text ("/compact focus on X");
        // everything else requires an exact match to avoid hijacking chat.
        const lower = draftMessage.toLowerCase();
        const compactMatch = lower === '/compact' || lower.startsWith('/compact ');
        const match = compactMatch
            ? OSA.SLASH_COMMANDS.find(c => c.cmd === '/compact')
            : OSA.SLASH_COMMANDS.find(c => c.cmd === lower);
        if (match) {
            if (inputEl) {
                inputEl.value = '';
                OSA.resizeMessageInput(inputEl);
            }
            OSA.hideSlashMenu();
            if (match.cmd === '/compact') {
                const focus = draftMessage.slice('/compact'.length).trim();
                OSA.compactSession(focus ? { focus } : undefined);
            } else {
                match.action();
            }
            return;
        }
    }

    let currentSession = OSA.getCurrentSession();
    if (!currentSession) {
        OSA.debug?.log('send.session.create', {});
        currentSession = await OSA.createSession();
        if (!currentSession?.id) return;
    }
    OSA.debug?.log('send.session.ready', { sessionId: currentSession.id });

    const input = document.getElementById('message-input');
    const message = input.value.trim();
    const attachments = OSA.getAttachments().slice();
    if (!message && attachments.length === 0) return;

    // Barge-in: sending is an explicit signal the user is done listening.
    if (typeof OSA.cancelSpeechOutput === 'function') {
        OSA.cancelSpeechOutput();
    }
    OSA.resetSpeechStream?.();
    // Clear the voice-mode reply panel here rather than in resetSpeechStream:
    // that also runs at turn end, which would wipe the text while it is still
    // being spoken. Clearing on send keeps the last answer on screen until the
    // next question, and stops replies accumulating across turns.
    OSA.resetVoiceModeReply?.();

    // Voice metadata improves response shaping, but a stalled PATCH must never
    // block the primary send path. The next turn can safely use the previous
    // value while this best-effort update completes in the background.
    if (typeof OSA.ensureVoiceModeSynced === 'function') {
        OSA.debug?.log('send.voice-sync.start', {});
        Promise.resolve().then(() => OSA.ensureVoiceModeSynced()).then(
            () => OSA.debug?.log('send.voice-sync.complete', {}),
            error => OSA.debug?.warn('send.voice-sync', 'failed', error?.message || String(error))
        );
    }

    const clientMessageId = OSA.generateClientMessageId();
    const shouldQueueLocally = OSA.isAgentProcessing() || (OSA.getSessionQueue() || []).length > 0;
    let optimisticDomId = '';

    input.value = '';
    OSA.resizeMessageInput(input);
    OSA.hideSlashMenu();
    OSA.clearAttachments({ preserveObjectUrls: true });
    OSA.renderAttachmentPreviews();
    OSA.setInputHistoryIndex(-1);
    OSA.getInputHistory().push(message);
    if (OSA.getInputHistory().length > 100) OSA.getInputHistory().shift();
    if (!shouldQueueLocally) {
        OSA.hideThinkingIndicator();
        OSA.releaseStreamingAssistantMessage();
        // Preserve the session's event sequence across turns: it is the
        // monotonic counter the live channel resumes from, and the websocket
        // subscribe / SSE last_seq are derived from it. Zeroing it here would
        // make a mid-turn reconnect replay the whole history (duplicating
        // chunks) or, when the counter belongs to another session, make the
        // server filter out every event — the UI then sits on "thinking"
        // forever even though the reply was generated.
        const prevEventSeq = OSA.getMessageChain()?.eventSeqNumber || 0;
        OSA.resetMessageChain();
        const chain = OSA.getMessageChain();
        if (chain) chain.eventSeqNumber = prevEventSeq;
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

    const hasAttachments = attachments.length > 0;

    try {
        OSA.clearAttachmentStatus();
        const ws = OSA.getWebSocket ? OSA.getWebSocket() : null;
        const useWs = ws && ws.readyState === WebSocket.OPEN && !hasAttachments && OSA.wsRequest;
        OSA.debug?.log('send.dispatch', {
            transport: useWs ? 'websocket' : (hasAttachments ? 'multipart' : 'http'),
            sessionId: currentSession.id,
        });

        let data;
        if (useWs) {
            data = await OSA.wsRequest('session.send', {
                session_id: currentSession.id,
                content: message,
                client_message_id: clientMessageId,
            });
        } else if (hasAttachments) {
            const formData = new FormData();
            formData.append('message', message);
            formData.append('session_id', currentSession.id);
            formData.append('client_message_id', clientMessageId);
            attachments.forEach(att => {
                if (att && att.file instanceof File) {
                    formData.append('attachments', att.file, att.filename || att.file.name || 'attachment');
                }
            });

            const res = await fetch(`/api/sessions/${currentSession.id}/send-multipart`, {
                method: 'POST',
                headers: {
                    'Authorization': `Bearer ${OSA.getToken()}`,
                },
                body: formData,
            });
            data = await res.json().catch(() => ({}));

            if (!res.ok) {
                throw new Error(data.error || `HTTP ${res.status}`);
            }
        } else {
            const res = await fetch(`/api/sessions/${currentSession.id}/send`, {
                method: 'POST',
                headers: {
                    'Authorization': `Bearer ${OSA.getToken()}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({ message, session_id: currentSession.id, client_message_id: clientMessageId, attachments: [] })
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
            // Drop the optimistic transcript item too, or the next render
            // rebuilds the bubble even though the message is only queued.
            if (clientMessageId && OSA.tmodelRemove) {
                OSA.tmodelRemove('client:' + clientMessageId);
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
        OSA.debug?.log('send.accepted', {
            queued: !!data.queued,
            status: data.status || '',
        });

        if (!shouldQueueLocally || !data.queued) {
            const ws = OSA.getWebSocket ? OSA.getWebSocket() : null;
            const es = OSA.getEventSource ? OSA.getEventSource() : null;
            const esSessionId = OSA.getEventSourceSessionId ? OSA.getEventSourceSessionId() : null;
            if ((!ws || ws.readyState !== WebSocket.OPEN) && (!es || esSessionId !== currentSession.id)) {
                OSA.connectEventSource(currentSession.id);
            }
        }
        // Returned so follow-up steer can promote the queued item.
        return data;
    } catch (error) {
        console.error('Failed to send message:', error);
        OSA.debug?.warn('send.failed', 'dispatch', error?.message || String(error));
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
        // Remove the optimistic transcript item, otherwise the failed bubble
        // reappears on the next render (e.g. showErrorCard's mark-dirty).
        if (clientMessageId && OSA.tmodelRemove) {
            OSA.tmodelRemove('client:' + clientMessageId);
        }
        // Keep the message: restore it into the composer and offer a retry so a
        // dropped request does not silently discard what the user typed.
        const failedInput = document.getElementById('message-input');
        if (failedInput && !failedInput.value.trim()) {
            failedInput.value = message;
            OSA.resizeMessageInput(failedInput);
            failedInput.focus();
        }
        OSA.showErrorCard(error.message, {
            title: 'Message not sent',
            retryLabel: 'Retry send',
            onRetry: function() { OSA.runSendMessage(); },
        });
        if (!shouldQueueLocally) {
            OSA.setProcessing(false);
            OSA.resetSendButton();
            OSA.hideThinkingIndicator();
        }
        return null;
    }
};

OSA.runSendMessage = function() {
    return Promise.resolve()
        .then(() => OSA.sendMessage())
        .catch(error => {
            console.error('Message send failed before dispatch:', error);
            OSA.debug?.warn('send.failed', 'pre-dispatch', error?.stack || error?.message || String(error));
            OSA.showErrorCard?.(error?.message || 'Could not send message');
        });
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
        sendBtn.setAttribute('aria-label', 'Stop generating');
        sendBtn.title = 'Stop generating';
        if (sendIcon) sendIcon.classList.add('hidden');
        if (stopIcon) stopIcon.classList.remove('hidden');
    } else {
        sendBtn.classList.remove('stop-btn');
        sendBtn.disabled = false;
        sendBtn.setAttribute('aria-label', 'Send message');
        sendBtn.title = 'Send message';
        if (sendIcon) sendIcon.classList.remove('hidden');
        if (stopIcon) stopIcon.classList.add('hidden');
    }
};

OSA.resetSendButton = function() {
    OSA.setSendButtonStopMode(false);
};

window.handleSendButtonClick = function() {
    // Confirming a queue edit takes precedence over sending.
    if (OSA.queueEditingId) {
        OSA.confirmQueueEdit();
        return;
    }
    // While a turn is running the button is the Stop control. It must never
    // dispatch a draft: a half-typed message being fired off on what the user
    // read as "stop" is worse than making them use the keyboard to send.
    if (OSA.isAgentProcessing()) {
        OSA.stopGeneration();
    } else {
        OSA.runSendMessage();
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

OSA.applySidebarWidth = function(width) {
    const minWidth = 220;
    const maxWidth = 420;
    const nextWidth = Math.max(minWidth, Math.min(maxWidth, Math.round(Number(width) || 260)));
    document.documentElement.style.setProperty('--sidebar-width', `${nextWidth}px`);
    localStorage.setItem('sidebarWidth', String(nextWidth));
    return nextWidth;
};

OSA.initSidebarResize = function() {
    const handle = document.getElementById('sidebar-resize-handle');
    const sidebar = document.querySelector('.sidebar');
    if (!handle || !sidebar || handle.dataset.bound === 'true') return;

    handle.dataset.bound = 'true';
    OSA.applySidebarWidth(localStorage.getItem('sidebarWidth') || 260);

    let resizing = false;
    const stopResize = () => {
        if (!resizing) return;
        resizing = false;
        document.body.classList.remove('resizing-sidebar');
        if (handle.hasPointerCapture?.(OSA.sidebarResizePointerId)) {
            handle.releasePointerCapture(OSA.sidebarResizePointerId);
        }
        OSA.sidebarResizePointerId = null;
    };

    handle.addEventListener('pointerdown', (event) => {
        if (window.innerWidth <= 900 || event.button !== 0) return;
        event.preventDefault();
        resizing = true;
        OSA.sidebarResizePointerId = event.pointerId;
        handle.setPointerCapture?.(event.pointerId);
        document.body.classList.add('resizing-sidebar');
    });

    handle.addEventListener('pointermove', (event) => {
        if (!resizing) return;
        const left = sidebar.getBoundingClientRect().left;
        OSA.applySidebarWidth(event.clientX - left);
    });

    handle.addEventListener('pointerup', stopResize);
    handle.addEventListener('pointercancel', stopResize);
    handle.addEventListener('keydown', (event) => {
        if (window.innerWidth <= 900) return;
        const current = parseInt(getComputedStyle(document.documentElement).getPropertyValue('--sidebar-width'), 10) || 260;
        if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
            event.preventDefault();
            OSA.applySidebarWidth(current + (event.key === 'ArrowRight' ? 16 : -16));
        } else if (event.key === 'Home') {
            event.preventDefault();
            OSA.applySidebarWidth(220);
        } else if (event.key === 'End') {
            event.preventDefault();
            OSA.applySidebarWidth(420);
        }
    });
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

    OSA.initSidebarResize();

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
    if (!event.target.closest('.slash-menu')) {
        OSA.hideSlashMenu();
    }
});

document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape') {
        // Voice mode is the outermost surface, so it closes first.
        if (OSA.isVoiceModeOpen?.()) {
            event.preventDefault();
            OSA.closeVoiceMode();
            return;
        }
        if (OSA.modelDropdownOpen) {
            OSA.closeModelDropdown();
            return;
        }
        const permissionModal = document.getElementById('permission-modal');
        if (permissionModal && !permissionModal.classList.contains('hidden')) {
            event.preventDefault();
            OSA.respondToPermission(false, false);
            return;
        }
        const settingsModal = document.getElementById('settings-modal');
        if (settingsModal && !settingsModal.classList.contains('hidden')) {
            OSA.closeSettings();
            return;
        }
        const questionModal = document.getElementById('question-modal');
        if (questionModal && !questionModal.classList.contains('hidden')) {
            // Escape must not just hide the card: the agent is waiting on the
            // answer, so dismissing has to unblock it too.
            OSA.cancelQuestion();
            return;
        }
        const contextModal = document.getElementById('context-modal');
        if (contextModal && !contextModal.classList.contains('hidden')) {
            contextModal.classList.add('hidden');
            return;
        }
        const memoryModal = document.getElementById('memory-edit-modal');
        if (memoryModal && !memoryModal.classList.contains('hidden')) {
            OSA.closeMemoryEdit();
            return;
        }
        const providerModal = document.getElementById('add-provider-modal');
        if (providerModal && !providerModal.classList.contains('hidden')) {
            OSA.closeAddProviderModal();
            return;
        }
        const jobsModal = document.getElementById('jobs-modal');
        if (jobsModal && !jobsModal.classList.contains('hidden')) {
            jobsModal.classList.add('hidden');
            return;
        }
        const slashMenu = document.getElementById('slash-menu');
        if (slashMenu && !slashMenu.classList.contains('hidden')) {
            OSA.hideSlashMenu();
            return;
        }
        OSA.hideSlashMenu();

        // With no overlay to dismiss, Escape is the interrupt. This is the only
        // way to cancel a turn while there is text in the input, since the send
        // button prioritises sending over stopping once the draft is non-empty.
        if (OSA.isAgentProcessing() && !OSA.isAgentStopping()) {
            event.preventDefault();
            OSA.stopGeneration();
        } else if (typeof OSA.cancelSpeechOutput === 'function' && OSA.isSpeaking?.()) {
            event.preventDefault();
            OSA.cancelSpeechOutput();
        }
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
            OSA.resizeMessageInput(input);
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
                OSA.resizeMessageInput(input);
            } else {
                OSA.setInputHistoryIndex(-1);
                input.value = '';
                OSA.resizeMessageInput(input);
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
    { cmd: '/compact', label: 'Compact', desc: 'Verify notes and compact the conversation (resets context)', action: () => OSA.compactSession() },
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

    if (matches.length === 0) {
        OSA.hideSlashMenu();
        return;
    }

    if (!menu) {
        const menuEl = document.createElement('div');
        menuEl.id = 'slash-menu';
        menuEl.className = 'slash-menu hidden';
        const host = document.querySelector('.composer-card') || document.querySelector('.input-area');
        if (!host) return;
        host.appendChild(menuEl);
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
                OSA.resizeMessageInput(input);
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

// Searches message content on the server and folds the matching session ids
// into the sidebar filter. The local pass below still matches titles, so a
// query hits both. Sidebar filtering used to be DOM-only, which meant anything
// not currently rendered — i.e. most of your history — was unsearchable.
OSA.searchSessionContent = async function(query) {
    const q = (query || '').trim();
    if (q.length < 3) {
        OSA._contentMatchIds = null;
        return;
    }

    try {
        const res = await OSA.fetchWithAuth(`/api/session-search?q=${encodeURIComponent(q)}`);
        if (!res.ok) return;
        const hits = await res.json();
        OSA._contentMatchIds = new Set(hits.map(hit => hit.session_id));
    } catch (err) {
        // Title matching still works; content search is additive.
        console.warn('Session content search failed:', err);
        OSA._contentMatchIds = null;
    }
};

OSA.filterSessionsWithContent = async function(query) {
    await OSA.searchSessionContent(query);
    OSA.filterSessions(query);
};

OSA.filterSessions = function(query) {
    const items = document.querySelectorAll('.session-item');
    const childrenGroups = document.querySelectorAll('.session-children');
    const q = query.toLowerCase();
    const sourceFilter = OSA.getSessionSourceFilter ? OSA.getSessionSourceFilter() : 'all';
    const filtering = !!(q || sourceFilter !== 'all');

    const contentMatches = OSA._contentMatchIds;

    items.forEach(item => {
        const text = (item.textContent || '').toLowerCase();
        const source = item.dataset.sessionSource || 'web';
        const matchesSource = sourceFilter === 'all' || source === sourceFilter;
        const matchesTitle = !q || text.includes(q);
        const matchesContent = !!q && !!contentMatches && contentMatches.has(item.dataset.sessionId);
        item.style.display = ((matchesTitle || matchesContent) && matchesSource) ? '' : 'none';
    });

    childrenGroups.forEach(group => {
        if (!group.dataset.parent) return; // orphan group: no toggle state
        const visibleChildren = group.querySelectorAll('.session-item:not([style*="display: none"])');
        if (filtering) {
            // While searching, force matching groups open regardless of stored state.
            group.classList.remove('collapsed');
            group.style.display = visibleChildren.length > 0 ? '' : 'none';
        } else {
            group.style.display = '';
            // Restore the user's stored collapsed/expanded state.
            const collapsed = OSA._collapsedGroups.has(group.dataset.parent);
            group.classList.toggle('collapsed', collapsed);
        }
        const toggle = document.querySelector(`.session-item[data-session-id="${group.dataset.parent}"] .session-group-toggle`);
        if (toggle) toggle.classList.toggle('open', !group.classList.contains('collapsed'));
    });
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
        OSA.markSessionSeen(sessionId);
        if (OSA.SessionStore) delete OSA.SessionStore[sessionId];
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
            OSA.renderEmptyTranscript('Start a new chat to begin');
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
window.sendMessage = OSA.runSendMessage;
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
    OSA.clearAttachmentStatus();
    OSA.addAttachment({
        kind: OSA.ACCEPTED_IMAGE_TYPES.includes(file.type) ? 'image' : 'document',
        id: 'att-' + Date.now() + '-' + Math.random().toString(36).slice(2, 8),
        filename: file.name,
        mime: file.type || 'application/octet-stream',
        sizeBytes: file.size,
        file,
        previewUrl: (OSA.ACCEPTED_IMAGE_TYPES.includes(file.type) && typeof URL !== 'undefined' && URL.createObjectURL)
            ? URL.createObjectURL(file)
            : '',
    });
    OSA.renderAttachmentPreviews();
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
            const src = OSA.getAttachmentImageSrc(att);
            thumb.innerHTML = `
                <img class="expandable-image" data-image-src="${OSA.escapeAttr(src)}" src="${OSA.escapeAttr(src)}" alt="${OSA.escapeAttr(att.filename)}" />
                <button class="image-preview-remove" onclick="OSA.handleRemoveAttachment(${OSA.jsArg(att.id)})">&times;</button>
                <div class="image-preview-filename">${OSA.escapeHtml(att.filename)}</div>
            `;
        } else {
            const ext = OSA.getAttachmentExtension(att.filename) || 'file';
            thumb.classList.add('file-preview-thumb');
            thumb.innerHTML = `
                <div class="file-preview-icon">${OSA.escapeHtml(ext.toUpperCase().slice(0, 4))}</div>
                <button class="image-preview-remove" onclick="OSA.handleRemoveAttachment(${OSA.jsArg(att.id)})">&times;</button>
                <div class="image-preview-filename">${OSA.escapeHtml(att.filename)}</div>
            `;
        }
        container.appendChild(thumb);
    });
};

OSA.handleRemoveAttachment = function(id) {
    OSA.removeAttachment(id);
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
        // Barge-in: the user starting to type means they have stopped listening.
        // Fires once per playback rather than on every keystroke.
        input.addEventListener('input', () => {
            if (typeof OSA.cancelSpeechOutput === 'function' && OSA.isSpeaking?.()) {
                OSA.cancelSpeechOutput();
            }
        });

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

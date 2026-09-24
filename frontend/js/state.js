window.OSA = window.OSA || {};

// Safe storage access: the Node test harness loads this file without a DOM.
OSA._safeStorageGet = function(key) {
    try {
        if (typeof localStorage !== 'undefined' && localStorage.getItem) return localStorage.getItem(key);
    } catch (err) {}
    return null;
};
OSA._safeStorageSet = function(key, value) {
    try {
        if (typeof localStorage !== 'undefined' && localStorage.setItem) localStorage.setItem(key, value);
    } catch (err) {}
};

OSA.token = OSA._safeStorageGet('token');
OSA.currentSession = null;
OSA.currentSessionId = null;
// Per-session store. Previously every piece of live state (session object,
// queue, tool events, subagent tasks, event-sequence chain, processing flag)
// was a single global that selectSession wiped on every switch — so background
// turns kept running on the server while the UI dropped their events, and
// returning to a session depended on a racy snapshot refetch. Each entry keeps
// its own copy; the legacy get*/set* helpers below are thin views onto the
// currently viewed entry, so most call sites are unchanged.
OSA.SessionStore = {};
OSA.getSessionEntry = function(sessionId) {
    if (!sessionId) return null;
    let entry = OSA.SessionStore[sessionId];
    if (!entry) {
        entry = OSA.SessionStore[sessionId] = {
            session: null,
            queue: [],
            tools: [],
            subagents: [],
            chain: {
                lastEventType: null,
                lastAssistantDomId: null,
                pendingToolCallIds: [],
                eventSeqNumber: 0,
                eventSessionId: sessionId,
                lastThinkingEndSeq: 0,
                lastToolStartSeq: 0,
            },
            processing: false,
            stopping: false,
            hasReceivedResponse: false,
            streamText: '',
            streamThinking: '',
            messagesDirty: false,
        };
    }
    return entry;
};

OSA.mergeSessionSnapshotMessages = function(freshMessages, priorMessages, options = {}) {
    const fresh = Array.isArray(freshMessages) ? freshMessages : [];
    const prior = Array.isArray(priorMessages) ? priorMessages : [];
    const merged = fresh.map(function(message) { return message; });
    let aligned = true;

    const compatibleAt = function(current, cached) {
        if (!current || !cached || current.role !== cached.role) return false;
        const currentClientId = current.metadata && current.metadata.client_message_id;
        const cachedClientId = cached.metadata && cached.metadata.client_message_id;
        if (currentClientId || cachedClientId) return !!currentClientId && currentClientId === cachedClientId;
        if (current.role === 'tool' && (current.tool_call_id || cached.tool_call_id)) {
            return !!current.tool_call_id && current.tool_call_id === cached.tool_call_id;
        }
        return true;
    };

    const extendedText = function(current, cached) {
        const freshText = typeof current === 'string' ? current : '';
        const cachedText = typeof cached === 'string' ? cached : '';
        return cachedText.startsWith(freshText) ? cachedText : freshText;
    };

    const overlap = Math.min(fresh.length, prior.length);
    for (let index = 0; index < overlap; index += 1) {
        if (!compatibleAt(fresh[index], prior[index])) {
            aligned = false;
            break;
        }
        if (options.preserveStreamed && fresh[index].role === 'assistant') {
            merged[index] = Object.assign({}, fresh[index], {
                content: extendedText(fresh[index].content, prior[index].content),
                thinking: extendedText(fresh[index].thinking, prior[index].thinking) || null,
            });
        }
    }

    // A background stream can append whole assistant/tool segments after the
    // GET snapshot was created. Preserve that suffix only when the two message
    // arrays still describe the same conversation prefix.
    if (options.preserveStreamed && aligned && prior.length > fresh.length) {
        prior.slice(fresh.length).forEach(function(message) { merged.push(message); });
    }

    // Optimistic user messages may not have reached the snapshot yet. Preserve
    // them by client id even if compaction or another structural change made
    // the positional merge above unsafe.
    const clientIds = new Set(merged.map(function(message) {
        return message && message.metadata && message.metadata.client_message_id;
    }).filter(Boolean));
    prior.forEach(function(message) {
        const clientId = message && message.metadata && message.metadata.client_message_id;
        if (message && message.role === 'user' && clientId && !clientIds.has(clientId)) {
            merged.push(message);
            clientIds.add(clientId);
        }
    });

    return merged;
};

OSA.reconcileSessionSnapshot = function(entry, session, sequenceAtRequest) {
    if (!entry || !session) return session;
    const priorSession = entry.session;
    const priorMessages = priorSession && Array.isArray(priorSession.messages)
        ? priorSession.messages
        : [];
    session.messages = OSA.mergeSessionSnapshotMessages(session.messages, priorMessages, {
        preserveStreamed: entry.messagesDirty === true,
    });
    entry.messagesDirty = false;

    const currentSequence = entry.chain && Number.isFinite(entry.chain.eventSeqNumber)
        ? entry.chain.eventSeqNumber
        : 0;
    const liveAdvanced = currentSequence > (Number.isFinite(sequenceAtRequest) ? sequenceAtRequest : currentSequence);
    if (liveAdvanced && priorSession && priorSession.task_status) {
        // Events received after the GET began are newer than its status field.
        session.task_status = priorSession.task_status;
    } else if (session.task_status !== 'running') {
        // With no newer live event, an idle server snapshot is authoritative.
        entry.processing = false;
        entry.stopping = false;
    }
    return session;
};
OSA.getCurrentSessionId = function() {
    return OSA.currentSessionId || (OSA.currentSession && OSA.currentSession.id) || null;
};
OSA.currentModelId = null;
OSA.currentModelProviderId = null;
OSA.eventSource = null;
OSA.activeTools = new Map();
OSA.isProcessing = false;
OSA.isStopping = false;
OSA.hasReceivedResponse = false;
OSA.headerBaseTitle = 'Select a session';
OSA.sidebarOpen = false;
OSA.voiceConfig = null;
OSA.recognition = null;
OSA.isRecording = false;
OSA.isTranscribing = false;
OSA.ttsEnabled = false;
OSA.mediaRecorder = null;
OSA.mediaStream = null;
OSA.mediaChunks = [];
OSA.voiceStatusMessage = '';
OSA.availablePersonas = [];
OSA.activePersona = null;
OSA.editingWorkspaceId = null;
OSA.selectedPersonaId = 'default';
OSA.inspectorRefreshTimeout = null;
OSA.sessionInspectorState = { history: [], snapshots: [] };
OSA.workspaceState = { activeWorkspace: 'default', workspaces: [] };
OSA.cachedConfig = null;
OSA.sessionTodos = [];
OSA.sessionQueue = [];
OSA.queueBusy = false;
OSA.queueEditingId = null;
OSA.queueEditStash = null;
// Sidebar unread markers: session ids with a finished turn the user hasn't
// opened yet. Explicit flags (not timestamps) so clock skew between client
// and server can never flip them.
OSA.unreadSessions = null;
OSA.getUnreadMap = function() {
    if (!OSA.unreadSessions) {
        try {
            OSA.unreadSessions = JSON.parse(localStorage.getItem('osa.sidebar.unread') || '{}') || {};
        } catch (err) {
            OSA.unreadSessions = {};
        }
    }
    return OSA.unreadSessions;
};
OSA.saveUnreadMap = function() {
    try {
        const map = OSA.getUnreadMap();
        const ids = Object.keys(map);
        // Bound growth: drop the oldest beyond 200 entries.
        if (ids.length > 200) {
            ids.sort(function(a, b) { return (map[a] || 0) - (map[b] || 0); });
            ids.slice(0, ids.length - 200).forEach(function(id) { delete map[id]; });
        }
        localStorage.setItem('osa.sidebar.unread', JSON.stringify(map));
    } catch (err) {}
};
OSA.isSessionUnread = function(sessionId) {
    return !!sessionId && !!OSA.getUnreadMap()[sessionId];
};
OSA.renderSessionUnreadIndicator = function(sessionId) {
    if (!sessionId || typeof document === 'undefined') return;
    const esc = (window.CSS && window.CSS.escape) ? window.CSS.escape(sessionId) : String(sessionId).replace(/[^a-zA-Z0-9_-]/g, '\\$&');
    const row = document.querySelector(`.session-item[data-session-id="${esc}"]`);
    if (!row) return;
    const unread = OSA.isSessionUnread(sessionId) && !row.classList.contains('active');
    row.classList.toggle('has-unread', unread);
    const icon = row.querySelector(':scope > .session-icon');
    if (!icon) return;
    const dot = icon.querySelector('.session-unread-dot');
    if (unread && !dot) {
        const marker = document.createElement('span');
        marker.className = 'session-unread-dot';
        marker.setAttribute('aria-label', 'Unread response');
        icon.appendChild(marker);
    } else if (!unread && dot) {
        dot.remove();
    }
};
OSA.markSessionUnread = function(sessionId) {
    if (!sessionId || sessionId === OSA.getCurrentSessionId()) return;
    OSA.getUnreadMap()[sessionId] = Date.now();
    OSA.saveUnreadMap();
    OSA.renderSessionUnreadIndicator(sessionId);
};
OSA.markSessionSeen = function(sessionId) {
    if (!sessionId) return;
    const map = OSA.getUnreadMap();
    if (map[sessionId]) {
        delete map[sessionId];
        OSA.saveUnreadMap();
    }
    OSA.renderSessionUnreadIndicator(sessionId);
};
OSA.pruneUnreadMap = function(validIds) {
    const map = OSA.getUnreadMap();
    let changed = false;
    Object.keys(map).forEach(function(id) {
        if (validIds.indexOf(id) === -1) {
            delete map[id];
            changed = true;
        }
    });
    if (changed) OSA.saveUnreadMap();
};
OSA.sessionCheckpoints = {};
OSA.sessionToolEvents = [];
OSA.sessionSubagentTasks = [];
OSA.pendingQuestions = [];
OSA.pendingQuestionId = '';
OSA.currentQuestionIndex = 0;
OSA.selectedAnswers = [];
OSA.currentAudio = null;
OSA.currentAudioUrl = null;
OSA.speechQueue = [];
OSA.streamingAssistantDomId = null;
OSA.eventSourceSessionId = null;
OSA.eventReconnectTimer = null;
OSA.parallelToolGroups = [];
OSA.parallelToolWindow = 500;
OSA.pendingFormattedElements = new Set();
OSA.pendingFormattedFrame = null;
OSA.sessionSelectionRequestId = 0;
OSA.sessionSelectionAbortController = null;
OSA._toolSyncInterval = null;
OSA.perfDebugEnabled = OSA._safeStorageGet('osa-debug-perf') === '1'
    || (function() {
        try {
            return new URLSearchParams(window.location.search).get('debugPerf') === '1';
        } catch (err) {
            return false;
        }
    })();
OSA.transcriptView = {
    initialized: false,
    transcriptRoot: null,
    topSpacer: null,
    topSentinel: null,
    listRoot: null,
    bottomSentinel: null,
    bottomSpacer: null,
    floatingRoot: null,
    ioTop: null,
    ioBottom: null,
    scrollHandlerAttached: false,
    userPinnedToBottom: true,
    autoScrollPaused: false,
    lastScrollTop: 0,
    // Set on session open/send: every render sticks to the bottom until the
    // user scrolls up themselves. Survives the async fetch burst on open,
    // where non-stick renders would otherwise yank the viewport mid-load.
    forceStickBottom: false,
    isRendering: false,
    avgMessageHeight: 132,
    messageHeights: new Map(),
    messageSignatures: new Map(),
    windowNodesByKey: new Map(),
    wrapperNodesByKey: new Map(),
    toolNodesByCallId: new Map(),
    ctxNodesByCallId: new Map(),
    anchoredNodesByIndex: new Map(),
    descriptors: [],
    units: [],
    lastDescriptorCount: 0,
    renderedMessageIndices: new Set(),
    windowStart: 0,
    windowEnd: 0,
    maxWindowSize: 180,
    windowShiftSize: 48,
    shiftInProgress: false,
    lastShiftAt: 0,
};

OSA.getToken = () => OSA.token;
OSA.setToken = t => { OSA.token = t; OSA._safeStorageSet('token', t); };
OSA.clearToken = () => { OSA.token = null; try { if (typeof localStorage !== 'undefined' && localStorage.removeItem) localStorage.removeItem('token'); } catch (err) {} };
OSA.getCurrentSession = () => OSA.currentSession;
OSA.setCurrentSession = function(s) {
    OSA.currentSession = s;
    OSA.currentSessionId = (s && s.id) || null;
    if (s && s.id) {
        const entry = OSA.getSessionEntry(s.id);
        entry.session = s;
        // Keep the entry's processing flag consistent with the snapshot so a
        // fresh entry created by background events starts from server truth.
        if (s.task_status !== 'running' && !entry.processing) entry.processing = false;
    }
    return s;
};
OSA.getEventSource = () => OSA.eventSource;
OSA.setEventSource = es => OSA.eventSource = es;
OSA.getActiveTools = () => OSA.activeTools;
OSA.isAgentProcessing = () => {
    const id = OSA.getCurrentSessionId();
    if (id && OSA.SessionStore[id]) return !!OSA.SessionStore[id].processing;
    return !!OSA.isProcessing;
};
// Notifies the voice-mode orb. Processing is set from several places
// (send, turn complete, stop, forced reset) and none of them touched the mic
// button, which was previously the only thing that re-rendered voice mode — so
// the orb sat on "Tap to speak" for the whole time the agent was thinking.
OSA.setProcessing = function(p) {
    OSA.isProcessing = !!p;
    const id = OSA.getCurrentSessionId();
    if (id) OSA.getSessionEntry(id).processing = !!p;
    OSA.renderVoiceModeState?.();
    return p;
};
OSA.isAgentStopping = () => {
    const id = OSA.getCurrentSessionId();
    if (id && OSA.SessionStore[id]) return !!OSA.SessionStore[id].stopping;
    return !!OSA.isStopping;
};
OSA.setStopping = function(s) {
    OSA.isStopping = !!s;
    const id = OSA.getCurrentSessionId();
    if (id) OSA.getSessionEntry(id).stopping = !!s;
    return s;
};
OSA.getHasReceivedResponse = () => {
    const id = OSA.getCurrentSessionId();
    if (id && OSA.SessionStore[id]) return !!OSA.SessionStore[id].hasReceivedResponse;
    return !!OSA.hasReceivedResponse;
};
OSA.setHasReceivedResponse = function(v) {
    OSA.hasReceivedResponse = !!v;
    const id = OSA.getCurrentSessionId();
    if (id) OSA.getSessionEntry(id).hasReceivedResponse = !!v;
    return v;
};
OSA.getHeaderBaseTitle = () => OSA.headerBaseTitle;
OSA.setHeaderBaseTitle = t => OSA.headerBaseTitle = t;
OSA.getSidebarOpen = () => OSA.sidebarOpen;
OSA.setSidebarOpen = o => OSA.sidebarOpen = o;
OSA.getVoiceConfig = () => OSA.voiceConfig;
OSA.setVoiceConfig = c => OSA.voiceConfig = c;
OSA.getRecognition = () => OSA.recognition;
OSA.setRecognition = r => OSA.recognition = r;
OSA.getIsRecording = () => OSA.isRecording;
OSA.setIsRecording = r => OSA.isRecording = r;
OSA.getIsTranscribing = () => OSA.isTranscribing;
OSA.setIsTranscribing = t => OSA.isTranscribing = t;
OSA.getTtsEnabled = () => OSA.ttsEnabled;
OSA.setTtsEnabled = e => OSA.ttsEnabled = e;
OSA.getMediaRecorder = () => OSA.mediaRecorder;
OSA.setMediaRecorder = r => OSA.mediaRecorder = r;
OSA.getMediaStream = () => OSA.mediaStream;
OSA.setMediaStream = s => OSA.mediaStream = s;
OSA.getMediaChunks = () => OSA.mediaChunks;
OSA.setMediaChunks = c => OSA.mediaChunks = c;
OSA.getVoiceStatusMessage = () => OSA.voiceStatusMessage;
OSA.setVoiceStatusMessage = m => OSA.voiceStatusMessage = m;
OSA.getAvailablePersonas = () => OSA.availablePersonas;
OSA.setAvailablePersonas = p => OSA.availablePersonas = p;
OSA.getActivePersona = () => OSA.activePersona;
OSA.setActivePersona = p => OSA.activePersona = p;
OSA.getEditingWorkspaceId = () => OSA.editingWorkspaceId;
OSA.setEditingWorkspaceId = id => OSA.editingWorkspaceId = id;
OSA.getSelectedPersonaId = () => OSA.selectedPersonaId;
OSA.setSelectedPersonaId = id => OSA.selectedPersonaId = id;
OSA.getInspectorRefreshTimeout = () => OSA.inspectorRefreshTimeout;
OSA.setInspectorRefreshTimeout = t => OSA.inspectorRefreshTimeout = t;
OSA.getSessionInspectorState = () => OSA.sessionInspectorState;
OSA.setSessionInspectorState = s => OSA.sessionInspectorState = s;
OSA.getWorkspaceState = () => OSA.workspaceState;
OSA.setWorkspaceState = s => OSA.workspaceState = s;
OSA.getCachedConfig = () => OSA.cachedConfig;
OSA.setCachedConfig = c => OSA.cachedConfig = c;
OSA.getSessionTodos = () => OSA.sessionTodos;
OSA.setSessionTodos = t => OSA.sessionTodos = t;
OSA.getSessionQueue = () => {    const id = OSA.getCurrentSessionId();
    if (id && OSA.SessionStore[id]) return OSA.SessionStore[id].queue;
    return OSA.sessionQueue;
};
OSA.setSessionQueue = function(q) {
    const next = Array.isArray(q) ? q : [];
    OSA.sessionQueue = next;
    const id = OSA.getCurrentSessionId();
    if (id) OSA.getSessionEntry(id).queue = next;
    return next;
};
OSA.getSessionQueueFor = function(sessionId) {
    if (sessionId && OSA.SessionStore[sessionId]) return OSA.SessionStore[sessionId].queue;
    return [];
};
OSA.setSessionQueueFor = function(sessionId, q) {
    const next = Array.isArray(q) ? q : [];
    if (sessionId) OSA.getSessionEntry(sessionId).queue = next;
    if (!sessionId || sessionId === OSA.getCurrentSessionId()) OSA.sessionQueue = next;
    return next;
};
OSA.getSessionToolEvents = () => {
    const id = OSA.getCurrentSessionId();
    if (id && OSA.SessionStore[id]) return OSA.SessionStore[id].tools;
    return OSA.sessionToolEvents;
};
OSA.setSessionToolEvents = function(tools) {
    const next = Array.isArray(tools) ? tools : [];
    OSA.sessionToolEvents = next;
    const id = OSA.getCurrentSessionId();
    if (id) OSA.getSessionEntry(id).tools = next;
    return next;
};
OSA.getSessionSubagentTasks = () => {
    const id = OSA.getCurrentSessionId();
    if (id && OSA.SessionStore[id]) return OSA.SessionStore[id].subagents;
    return OSA.sessionSubagentTasks;
};
OSA.setSessionSubagentTasks = function(tasks) {
    const next = Array.isArray(tasks) ? tasks : [];
    OSA.sessionSubagentTasks = next;
    const id = OSA.getCurrentSessionId();
    if (id) OSA.getSessionEntry(id).subagents = next;
    return next;
};
OSA.getSessionCheckpoints = sessionId => {
    if (!sessionId) return [];
    const checkpoints = OSA.sessionCheckpoints[sessionId];
    return Array.isArray(checkpoints) ? checkpoints : [];
};
OSA.setSessionCheckpoints = (sessionId, checkpoints) => {
    if (!sessionId) return;
    OSA.sessionCheckpoints[sessionId] = Array.isArray(checkpoints) ? checkpoints : [];
};
OSA.clearSessionCheckpoints = sessionId => {
    if (!sessionId) return;
    delete OSA.sessionCheckpoints[sessionId];
};
OSA.resetSessionCheckpoints = () => OSA.sessionCheckpoints = {};
OSA.getPendingQuestions = () => OSA.pendingQuestions;
OSA.setPendingQuestions = q => OSA.pendingQuestions = q;
OSA.getPendingQuestionId = () => OSA.pendingQuestionId;
OSA.setPendingQuestionId = id => OSA.pendingQuestionId = id;
OSA.getCurrentQuestionIndex = () => OSA.currentQuestionIndex;
OSA.setCurrentQuestionIndex = i => OSA.currentQuestionIndex = i;
OSA.getSelectedAnswers = () => OSA.selectedAnswers;
OSA.setSelectedAnswers = a => OSA.selectedAnswers = a;
OSA.getCurrentAudio = () => OSA.currentAudio;
OSA.setCurrentAudio = a => OSA.currentAudio = a;
OSA.getCurrentAudioUrl = () => OSA.currentAudioUrl;
OSA.setCurrentAudioUrl = u => OSA.currentAudioUrl = u;
OSA.getSpeechQueue = () => OSA.speechQueue;
OSA.clearSpeechQueue = () => OSA.speechQueue = [];
OSA.pushToSpeechQueue = t => OSA.speechQueue.push(t);
OSA.getStreamingAssistantDomId = () => OSA.streamingAssistantDomId;
OSA.setStreamingAssistantDomId = id => OSA.streamingAssistantDomId = id;
OSA.getEventSourceSessionId = () => OSA.eventSourceSessionId;
OSA.setEventSourceSessionId = id => OSA.eventSourceSessionId = id;
OSA.getEventReconnectTimer = () => OSA.eventReconnectTimer;
OSA.setEventReconnectTimer = t => OSA.eventReconnectTimer = t;
OSA.getPendingFormattedElements = () => OSA.pendingFormattedElements;
OSA.getPendingFormattedFrame = () => OSA.pendingFormattedFrame;
OSA.setPendingFormattedFrame = f => OSA.pendingFormattedFrame = f;
OSA.getPerfDebugEnabled = () => OSA.perfDebugEnabled;
OSA.setPerfDebugEnabled = enabled => {
    OSA.perfDebugEnabled = !!enabled;
    OSA._safeStorageSet('osa-debug-perf', enabled ? '1' : '0');
};
OSA.perfNow = () => (typeof performance !== 'undefined' && performance.now ? performance.now() : Date.now());
OSA.perfLog = (label, data = {}) => {
    if (!OSA.perfDebugEnabled) return;
    console.log(`[OSA perf] ${label}`, data);
};
OSA.getSessionSelectionAbortController = () => OSA.sessionSelectionAbortController;
OSA.setSessionSelectionAbortController = controller => OSA.sessionSelectionAbortController = controller;
OSA.getTranscriptView = () => OSA.transcriptView;
OSA.beginSessionSelection = () => ++OSA.sessionSelectionRequestId;
OSA.isSessionSelectionCurrent = id => OSA.sessionSelectionRequestId === id;
OSA.inspectorExpanded = false;
OSA.getInspectorExpanded = () => OSA.inspectorExpanded;
OSA.setInspectorExpanded = e => OSA.inspectorExpanded = e;
OSA.turnStartTime = null;
OSA.inputHistory = [];
OSA.inputHistoryIndex = -1;
OSA.todoDockExpanded = false;
OSA.getTurnStartTime = () => OSA.turnStartTime;
OSA.setTurnStartTime = t => OSA.turnStartTime = t;
OSA.getInputHistory = () => OSA.inputHistory;
OSA.setInputHistory = h => OSA.inputHistory = h;
OSA.getInputHistoryIndex = () => OSA.inputHistoryIndex;
OSA.setInputHistoryIndex = i => OSA.inputHistoryIndex = i;
OSA.getTodoDockExpanded = () => OSA.todoDockExpanded;
OSA.setTodoDockExpanded = e => OSA.todoDockExpanded = e;
OSA.sessionHierarchy = { parentId: null, children: [], breadcrumb: [] };
OSA.getSessionHierarchy = () => OSA.sessionHierarchy;
OSA.setSessionHierarchy = h => OSA.sessionHierarchy = h;
OSA.sidebarCollapsed = OSA._safeStorageGet('sidebarCollapsed') === 'true';
OSA.getSidebarCollapsed = () => OSA.sidebarCollapsed;
OSA.setSidebarCollapsed = c => { OSA.sidebarCollapsed = c; OSA._safeStorageSet('sidebarCollapsed', c); };
OSA.sessionSourceFilter = OSA._safeStorageGet('osagent-session-source-filter') || 'all';
OSA.getSessionSourceFilter = () => OSA.sessionSourceFilter;
OSA.setSessionSourceFilter = value => {
    OSA.sessionSourceFilter = value || 'all';
    OSA._safeStorageSet('osagent-session-source-filter', OSA.sessionSourceFilter);
};
OSA.showThinkingBlocks = OSA._safeStorageGet('osagent-show-thinking-blocks') !== 'false';
OSA.getShowThinkingBlocks = () => OSA.showThinkingBlocks;
OSA.setShowThinkingBlocks = value => {
    OSA.showThinkingBlocks = value;
    OSA._safeStorageSet('osagent-show-thinking-blocks', value ? 'true' : 'false');
};
OSA.messageChain = {
    lastEventType: null,
    lastAssistantDomId: null,
    pendingToolCallIds: [],
    eventSeqNumber: 0,
    eventSessionId: null,
    lastThinkingEndSeq: 0,
    lastToolStartSeq: 0,
};
OSA.getMessageChain = () => {
    const id = OSA.getCurrentSessionId();
    if (id && OSA.SessionStore[id]) return OSA.SessionStore[id].chain;
    return OSA.messageChain;
};
OSA.getMessageChainFor = function(sessionId) {
    if (!sessionId) return OSA.messageChain;
    return OSA.getSessionEntry(sessionId).chain;
};
OSA.revokeAttachmentPreviewUrl = attachment => {
    const url = attachment && attachment.previewUrl;
    if (!url || typeof url !== 'string' || !url.startsWith('blob:')) return;
    try {
        URL.revokeObjectURL(url);
    } catch (error) {
        console.warn('Failed to revoke attachment preview URL:', error);
    }
};
OSA.attachments = [];
OSA.getAttachments = () => OSA.attachments;
OSA.setAttachments = arr => OSA.attachments = arr;
OSA.addAttachment = attachment => OSA.attachments.push(attachment);
OSA.removeAttachment = id => {
    const next = [];
    OSA.attachments.forEach(attachment => {
        if (attachment.id === id) {
            OSA.revokeAttachmentPreviewUrl(attachment);
        } else {
            next.push(attachment);
        }
    });
    OSA.attachments = next;
};
OSA.clearAttachments = (options = {}) => {
    if (!options.preserveObjectUrls) {
        OSA.attachments.forEach(attachment => OSA.revokeAttachmentPreviewUrl(attachment));
    }
    OSA.attachments = [];
};

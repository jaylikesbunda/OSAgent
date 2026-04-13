window.OSA = window.OSA || {};

OSA.token = localStorage.getItem('token');
OSA.currentSession = null;
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
OSA.perfDebugEnabled = localStorage.getItem('osa-debug-perf') === '1'
    || new URLSearchParams(window.location.search).get('debugPerf') === '1';
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
    isRendering: false,
    avgMessageHeight: 132,
    messageHeights: new Map(),
    messageSignatures: new Map(),
    windowNodesByKey: new Map(),
    wrapperNodesByKey: new Map(),
    anchoredNodesByIndex: new Map(),
    descriptors: [],
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
OSA.setToken = t => { OSA.token = t; localStorage.setItem('token', t); };
OSA.clearToken = () => { OSA.token = null; localStorage.removeItem('token'); };
OSA.getCurrentSession = () => OSA.currentSession;
OSA.setCurrentSession = s => OSA.currentSession = s;
OSA.getEventSource = () => OSA.eventSource;
OSA.setEventSource = es => OSA.eventSource = es;
OSA.getActiveTools = () => OSA.activeTools;
OSA.isAgentProcessing = () => OSA.isProcessing;
OSA.setProcessing = p => OSA.isProcessing = p;
OSA.isAgentStopping = () => OSA.isStopping;
OSA.setStopping = s => OSA.isStopping = s;
OSA.getHasReceivedResponse = () => OSA.hasReceivedResponse;
OSA.setHasReceivedResponse = v => OSA.hasReceivedResponse = v;
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
OSA.getSessionQueue = () => OSA.sessionQueue;
OSA.setSessionQueue = q => OSA.sessionQueue = Array.isArray(q) ? q : [];
OSA.getSessionToolEvents = () => OSA.sessionToolEvents;
OSA.setSessionToolEvents = tools => OSA.sessionToolEvents = Array.isArray(tools) ? tools : [];
OSA.getSessionSubagentTasks = () => OSA.sessionSubagentTasks;
OSA.setSessionSubagentTasks = tasks => OSA.sessionSubagentTasks = Array.isArray(tasks) ? tasks : [];
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
    localStorage.setItem('osa-debug-perf', enabled ? '1' : '0');
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
OSA.sidebarCollapsed = localStorage.getItem('sidebarCollapsed') === 'true';
OSA.getSidebarCollapsed = () => OSA.sidebarCollapsed;
OSA.setSidebarCollapsed = c => { OSA.sidebarCollapsed = c; localStorage.setItem('sidebarCollapsed', c); };
OSA.sessionSourceFilter = localStorage.getItem('osagent-session-source-filter') || 'all';
OSA.getSessionSourceFilter = () => OSA.sessionSourceFilter;
OSA.setSessionSourceFilter = value => {
    OSA.sessionSourceFilter = value || 'all';
    localStorage.setItem('osagent-session-source-filter', OSA.sessionSourceFilter);
};
OSA.showThinkingBlocks = localStorage.getItem('osagent-show-thinking-blocks') !== 'false';
OSA.getShowThinkingBlocks = () => OSA.showThinkingBlocks;
OSA.setShowThinkingBlocks = value => {
    OSA.showThinkingBlocks = value;
    localStorage.setItem('osagent-show-thinking-blocks', value ? 'true' : 'false');
};
OSA.messageChain = {
    lastEventType: null,
    lastAssistantDomId: null,
    pendingToolCallIds: [],
    eventSeqNumber: 0,
    lastThinkingEndSeq: 0,
    lastToolStartSeq: 0,
};
OSA.getMessageChain = () => OSA.messageChain;
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

window.OSA = window.OSA || {};

OSA.wsSocket = null;
OSA.wsReconnectTimer = null;
OSA.wsSessionId = null;
OSA.wsPending = {};
OSA.wsEventListeners = new Set();

OSA.getWebSocket = () => OSA.wsSocket;
OSA.setWebSocket = ws => OSA.wsSocket = ws;
OSA.getWsReconnectTimer = () => OSA.wsReconnectTimer;
OSA.setWsReconnectTimer = timer => OSA.wsReconnectTimer = timer;

OSA.addWsEventListener = function(listener) {
    if (typeof listener === 'function') {
        OSA.wsEventListeners.add(listener);
    }
};

OSA.removeWsEventListener = function(listener) {
    OSA.wsEventListeners.delete(listener);
};

OSA.wsSubscribedSessions = OSA.wsSubscribedSessions || {};

OSA.wsLastSeqFor = function(sessionId) {
    try {
        const entry = OSA.getSessionEntry ? OSA.getSessionEntry(sessionId) : null;
        const seq = entry && entry.chain ? entry.chain.eventSeqNumber : 0;
        return Number.isFinite(seq) ? seq : 0;
    } catch (err) {
        return 0;
    }
};

OSA.wsSubscribeSession = function(sessionId, lastSeq = 0) {
    if (!OSA.wsRequest) {
        return Promise.reject(new Error('WebSocket RPC unavailable'));
    }
    return OSA.wsRequest('session.subscribe', {
        session_id: sessionId,
        last_seq: Number.isFinite(lastSeq) ? lastSeq : 0,
    });
};

OSA.wsUnsubscribeSession = function(sessionId) {
    if (!OSA.wsRequest) {
        return Promise.reject(new Error('WebSocket RPC unavailable'));
    }
    return OSA.wsRequest('session.unsubscribe', {
        session_id: sessionId,
    });
};

OSA.wsRequest = function(method, payload = {}) {
    const ws = OSA.getWebSocket();
    if (!ws || ws.readyState !== WebSocket.OPEN) {
        return Promise.reject(new Error('WebSocket is not connected'));
    }

    const requestId = `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    const message = {
        method,
        request_id: requestId,
        ...payload,
    };

    return new Promise((resolve, reject) => {
        OSA.wsPending[requestId] = { resolve, reject };
        ws.send(JSON.stringify(message));
        setTimeout(() => {
            const pending = OSA.wsPending[requestId];
            if (!pending) return;
            delete OSA.wsPending[requestId];
            reject(new Error(`RPC timeout: ${method}`));
        }, 30000);
    });
};

OSA.connectWebSocket = function(sessionId) {
    if (!('WebSocket' in window)) {
        return false;
    }

    // Multiplexed connection: one socket carries all subscribed sessions, so
    // switching chats subscribes the new session without dropping background
    // turns. The server already supports concurrent subscriptions.
    const existing = OSA.getWebSocket();
    if (existing && existing.readyState === WebSocket.OPEN) {
        if (sessionId) {
            OSA.wsSubscribeSession(sessionId, OSA.wsLastSeqFor(sessionId)).catch(err => {
                console.error('Failed to subscribe over websocket:', err);
            });
            OSA.wsSubscribedSessions[sessionId] = true;
            const session = OSA.getCurrentSession ? OSA.getCurrentSession() : null;
            if (session && session.id === sessionId && session.task_status === 'running') {
                if (OSA.syncRunningSessionSnapshot) OSA.syncRunningSessionSnapshot(sessionId);
            }
        }
        return true;
    }
    if (existing) {
        existing._osaSuppressReconnect = true;
        try { existing.close(); } catch (err) {}
        OSA.setWebSocket(null);
    }

    const timer = OSA.getWsReconnectTimer();
    if (timer) {
        clearTimeout(timer);
        OSA.setWsReconnectTimer(null);
    }

    OSA.wsSessionId = null;
    const token = OSA.getToken ? OSA.getToken() : '';
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const host = window.location.host;
    const qs = token ? `?token=${encodeURIComponent(token)}` : '';
    const url = `${protocol}//${host}/ws${qs}`;

    let ws;
    try {
        ws = new WebSocket(url);
    } catch (e) {
        console.error('Failed to create WebSocket:', e);
        return false;
    }

    ws.onopen = () => {
        if (OSA.getWebSocket() && OSA.getWebSocket() !== ws) {
            ws._osaSuppressReconnect = true;
            ws.close();
            return;
        }
        OSA.setWebSocket(ws);
        OSA.showConnectionStatus('connected', 'Connected');
        // (Re)subscribe every known session so background turns resume after a
        // reconnect without revisiting each chat.
        const ids = Object.keys(OSA.wsSubscribedSessions || {});
        if (sessionId && ids.indexOf(sessionId) === -1) ids.push(sessionId);
        ids.forEach(function(id) {
            OSA.wsSubscribeSession(id, OSA.wsLastSeqFor(id)).catch(err => {
                console.error('Failed to subscribe over websocket:', err);
            });
            OSA.wsSubscribedSessions[id] = true;
        });
        const session = OSA.getCurrentSession ? OSA.getCurrentSession() : null;
        if (session && session.id === sessionId && session.task_status === 'running') {
            if (!OSA.getStreamingAssistantMessage() && OSA.shouldShowThinkingIndicatorForRunningSession(session)) {
                OSA.showThinkingIndicator();
            }
            OSA.syncRunningSessionSnapshot(sessionId);
        }
        OSA.wsEventListeners.forEach(listener => {
            try {
                listener({ method: 'ws.open' });
            } catch (err) {
                console.warn('WebSocket open listener failed:', err);
            }
        });
    };

    ws.onmessage = event => {
        let payload;
        try {
            payload = JSON.parse(event.data);
        } catch (e) {
            console.error('Failed to parse WebSocket payload:', e);
            return;
        }

        if (payload.method === 'rpc.result' || payload.method === 'rpc.error') {
            const requestId = payload.request_id;
            const pending = requestId ? OSA.wsPending[requestId] : null;
            if (pending) {
                delete OSA.wsPending[requestId];
                if (payload.method === 'rpc.result') {
                    pending.resolve(payload.result);
                } else {
                    pending.reject(new Error(payload.error || 'RPC error'));
                }
            }
            return;
        }

        if (payload.method !== 'session.event' || !payload.event) {
            return;
        }

        OSA.wsEventListeners.forEach(listener => {
            try {
                listener(payload);
            } catch (err) {
                console.warn('WebSocket event listener failed:', err);
            }
        });

        // No current-session filter: handleAgentEvent routes by event
        // session_id into the per-session store, so background turns keep
        // accumulating while another chat is viewed.
        // Do not touch chain.eventSeqNumber here: the envelope sequence is the
        // event's own sequence, and handleAgentEvent drops anything at or below
        // the counter as an already-seen replay. Advancing it first would make
        // every live event look like a duplicate of itself, so nothing renders
        // and the UI sits on "thinking" until the session is reloaded from
        // history. handleAgentEvent owns the counter.
        if (typeof OSA.handleAgentEvent === 'function') {
            OSA.handleAgentEvent(payload.event);
        }
    };

    ws.onerror = err => {
        console.error('WebSocket error:', err);
    };

    ws.onclose = () => {
        if (OSA.getWebSocket() === ws) {
            OSA.setWebSocket(null);
        }

        if (ws._osaSuppressReconnect) {
            return;
        }

        OSA.showConnectionStatus('disconnected', 'Disconnected');
        // Reconnect the multiplexed socket as long as any session needs it.
        if (Object.keys(OSA.wsSubscribedSessions || {}).length > 0 || OSA.getCurrentSession()) {
            const reconnectTimer = setTimeout(() => {
                OSA.setWsReconnectTimer(null);
                const currentId = OSA.getCurrentSessionId ? OSA.getCurrentSessionId() : null;
                OSA.connectWebSocket(currentId);
            }, 2000);
            OSA.setWsReconnectTimer(reconnectTimer);
        }
    };

    OSA.showConnectionStatus('connecting', 'Connecting...');
    return true;
};

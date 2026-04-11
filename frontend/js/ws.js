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

    const existing = OSA.getWebSocket();
    if (existing) {
        existing.close();
        OSA.setWebSocket(null);
    }

    const timer = OSA.getWsReconnectTimer();
    if (timer) {
        clearTimeout(timer);
        OSA.setWsReconnectTimer(null);
    }

    OSA.wsSessionId = sessionId;
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
        OSA.setWebSocket(ws);
        OSA.showConnectionStatus('connected', 'Connected');
        const chain = OSA.getMessageChain ? OSA.getMessageChain() : null;
        const lastSeq = chain && Number.isFinite(chain.eventSeqNumber) ? chain.eventSeqNumber : 0;
        OSA.wsSubscribeSession(sessionId, lastSeq).catch(err => {
            console.error('Failed to subscribe over websocket:', err);
        });
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

        const currentSession = OSA.getCurrentSession ? OSA.getCurrentSession() : null;
        if (!currentSession || currentSession.id !== payload.session_id) {
            return;
        }

        const chain = OSA.getMessageChain ? OSA.getMessageChain() : null;
        if (chain && Number.isFinite(payload.sequence)) {
            chain.eventSeqNumber = Math.max(chain.eventSeqNumber || 0, payload.sequence);
        }

        if (typeof OSA.handleAgentEvent === 'function') {
            OSA.handleAgentEvent(payload.event);
        }
    };

    ws.onerror = err => {
        console.error('WebSocket error:', err);
    };

    ws.onclose = () => {
        OSA.setWebSocket(null);
        OSA.showConnectionStatus('disconnected', 'Disconnected');
        if (OSA.wsSessionId && OSA.getCurrentSession() && OSA.getCurrentSession().id === OSA.wsSessionId) {
            const reconnectTimer = setTimeout(() => {
                OSA.setWsReconnectTimer(null);
                OSA.connectWebSocket(OSA.wsSessionId);
            }, 2000);
            OSA.setWsReconnectTimer(reconnectTimer);
        }
    };

    OSA.showConnectionStatus('connecting', 'Connecting...');
    return true;
};

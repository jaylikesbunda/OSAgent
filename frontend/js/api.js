window.OSA = window.OSA || {};

OSA.fetchWithAuth = async function(url, options = {}) {
    const headers = options.headers || {};
    if (!headers['Authorization'] && OSA.getToken()) {
        headers['Authorization'] = `Bearer ${OSA.getToken()}`;
    }
    if (!headers['Content-Type'] && options.body && typeof options.body === 'string') {
        headers['Content-Type'] = 'application/json';
    }
    return fetch(url, { ...options, headers });
};

OSA.getJson = async function(url) {
    const res = await OSA.fetchWithAuth(url);
    return res.json();
};

OSA.postJson = async function(url, body) {
    const res = await OSA.fetchWithAuth(url, {
        method: 'POST',
        body: JSON.stringify(body)
    });
    return res.json();
};

OSA.putJson = async function(url, body) {
    const res = await OSA.fetchWithAuth(url, {
        method: 'PUT',
        body: JSON.stringify(body)
    });
    return res.json();
};

OSA.deleteJson = async function(url) {
    const res = await OSA.fetchWithAuth(url, {
        method: 'DELETE'
    });
    return res.json().catch(() => ({}));
};

OSA.cancelSession = async function(sessionId) {
    const res = await OSA.fetchWithAuth(`/api/sessions/${sessionId}/cancel`, {
        method: 'POST'
    });
    return res.json().catch(() => ({}));
};

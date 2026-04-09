window.OSA = window.OSA || {};

OSA.getAuthHeaders = function(extraHeaders = {}) {
    const headers = { ...extraHeaders };
    if (!headers.Authorization && OSA.getToken()) {
        headers.Authorization = `Bearer ${OSA.getToken()}`;
    }
    return headers;
};

OSA.fetchWithAuth = async function(url, options = {}) {
    const headers = OSA.getAuthHeaders(options.headers || {});
    if (!headers['Content-Type'] && !headers['content-type'] && options.body && typeof options.body === 'string') {
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

OSA.getScheduledJobs = async function() {
    const res = await OSA.fetchWithAuth('/api/scheduler/jobs');
    return res.json();
};

OSA.createScheduledJob = async function(data) {
    const res = await OSA.fetchWithAuth('/api/scheduler/jobs', {
        method: 'POST',
        body: JSON.stringify(data)
    });
    return res.json();
};

OSA.deleteScheduledJob = async function(id) {
    const res = await OSA.fetchWithAuth(`/api/scheduler/jobs/${id}`, {
        method: 'DELETE'
    });
    return res.ok;
};

OSA.toggleScheduledJob = async function(id) {
    const res = await OSA.fetchWithAuth(`/api/scheduler/jobs/${id}/toggle`, {
        method: 'PATCH'
    });
    return res.json();
};

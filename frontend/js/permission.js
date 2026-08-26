window.OSA = window.OSA || {};

OSA.permissionPrompts = [];
OSA.activePermissionPrompt = null;
OSA.permissionPollTimer = null;
OSA.permissionPreviousFocus = null;

OSA.startPermissionPolling = function() {
    if (OSA.permissionPollTimer) return;
    OSA.refreshPermissionPrompts();
    OSA.permissionPollTimer = window.setInterval(OSA.refreshPermissionPrompts, 1000);
};

OSA.refreshPermissionPrompts = async function() {
    if (!OSA.getToken()) return;
    try {
        const res = await OSA.fetchWithAuth('/api/permissions');
        if (!res.ok) return;
        const data = await res.json();
        OSA.permissionPrompts = Array.isArray(data.prompts) ? data.prompts : [];
        OSA.showNextPermissionPrompt();
    } catch (error) {
        console.error('Failed to load permission requests:', error);
    }
};

OSA.showNextPermissionPrompt = function() {
    const modal = document.getElementById('permission-modal');
    if (!modal) return;

    if (OSA.activePermissionPrompt) {
        const stillPending = OSA.permissionPrompts.some(function(prompt) { return prompt.id === OSA.activePermissionPrompt.id; });
        if (stillPending) return;
        OSA.activePermissionPrompt = null;
    }

    var currentSessionId = OSA.getCurrentSession()?.id;
    var prompt = OSA.permissionPrompts.find(function(item) { return item.session_id === currentSessionId; })
        || OSA.permissionPrompts[0];
    if (!prompt) {
        modal.classList.add('hidden');
        return;
    }

    OSA.activePermissionPrompt = prompt;
    var toolName = String(prompt.source || 'tool').split(':')[0];
    var sessionRead = prompt.path_type === 'session_read';
    document.getElementById('permission-tool').textContent = toolName;
    document.getElementById('permission-operation').textContent =
        sessionRead ? 'read another conversation' : (prompt.path_type || 'access');
    document.getElementById('permission-path').textContent =
        sessionRead ? String(prompt.path || '').replace(/^session:\/\//, '') : (prompt.path || '');
    OSA.permissionPreviousFocus = document.activeElement;
    modal.classList.remove('hidden');
    window.setTimeout(function() { document.getElementById('permission-once')?.focus(); }, 0);
};

OSA.respondToPermission = async function(allowed, always) {
    var prompt = OSA.activePermissionPrompt;
    if (!prompt) return;
    var buttons = document.querySelectorAll('#permission-modal button');
    buttons.forEach(function(button) { button.disabled = true; });
    try {
        var res = await OSA.fetchWithAuth('/api/permissions/respond', {
            method: 'POST',
            body: JSON.stringify({ prompt_id: prompt.id, allowed: allowed, always: always })
        });
        if (!res.ok && res.status !== 404) {
            var data = await res.json().catch(function() { return {}; });
            throw new Error(data.error || 'HTTP ' + res.status);
        }
        OSA.permissionPrompts = OSA.permissionPrompts.filter(function(item) { return item.id !== prompt.id; });
        OSA.activePermissionPrompt = null;
        document.getElementById('permission-modal')?.classList.add('hidden');
        if (OSA.permissionPreviousFocus?.focus) OSA.permissionPreviousFocus.focus();
        OSA.showNextPermissionPrompt();
    } catch (error) {
        console.error('Failed to answer permission request:', error);
    } finally {
        buttons.forEach(function(button) { button.disabled = false; });
    }
};

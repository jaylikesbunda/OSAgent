window.OSA = window.OSA || {};

OSA.escapeHtml = function(text) {
    const div = document.createElement('div');
    div.textContent = text || '';
    return div.innerHTML;
};

OSA.timestampToMs = function(value) {
    if (value === null || value === undefined || value === '') return null;
    if (typeof value === 'number' && Number.isFinite(value)) {
        return value > 1e12 ? value : value * 1000;
    }
    const parsed = Date.parse(value);
    return Number.isNaN(parsed) ? null : parsed;
};

OSA.formatRelativeDateTime = function(value) {
    if (!value) return 'unknown time';
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return String(value);

    const deltaMs = Date.now() - date.getTime();
    const deltaMinutes = Math.round(deltaMs / 60000);
    if (Math.abs(deltaMinutes) < 1) return 'just now';
    if (Math.abs(deltaMinutes) < 60) return `${deltaMinutes}m ago`;

    const deltaHours = Math.round(deltaMinutes / 60);
    if (Math.abs(deltaHours) < 24) return `${deltaHours}h ago`;

    const deltaDays = Math.round(deltaHours / 24);
    if (Math.abs(deltaDays) < 7) return `${deltaDays}d ago`;

    return date.toLocaleString();
};

OSA.formatDiscordAllowedUsers = function(users) {
    if (!Array.isArray(users) || users.length === 0) return '';
    return users.join('\n');
};

OSA.parseDiscordAllowedUsers = function(raw) {
    return Array.from(new Set(
        (raw || '')
            .split(/[\n,]/)
            .map(value => value.trim())
            .filter(Boolean)
            .map(value => {
                if (!/^\d+$/.test(value)) {
                    throw new Error(`Invalid Discord user ID: ${value}`);
                }
                return Number(value);
            })
    ));
};

OSA.generateClientMessageId = function() {
    if (window.crypto && typeof window.crypto.randomUUID === 'function') {
        return window.crypto.randomUUID();
    }
    return `client-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
};

OSA.summarizeHistoryEvent = function(event) {
    const data = event?.data || {};
    switch (event.event_type) {
        case 'tool':
            return `${data.tool_name || 'tool'} ${data.success ? 'completed' : 'failed'} in ${data.duration_ms ?? '?'}ms`;
        case 'retry':
            return `${data.scope || 'operation'} retried ${data.attempt_count || 0} time(s)${data.context_compressed ? ' after context compression' : ''}`;
        case 'compaction':
            return `compacted ${data.compacted_messages || 0} messages, pruned ${data.pruned_messages || 0}${data.replayed ? ', replay preserved' : ''}`;
        case 'batch':
            return `${data.successful || 0}/${data.total || 0} batched calls succeeded`;
        case 'snapshot':
            return `captured pre-edit snapshot for ${data.tool_name || 'tool'} (${data.snapshot_id || 'snapshot'})`;
        case 'snapshot_revert':
            return `restored ${Array.isArray(data.paths) ? data.paths.length : 0} path(s) from snapshot ${data.snapshot_id || ''}`;
        case 'reasoning':
            return data.summary || 'runtime reasoning note';
        case 'step_finish':
            return `iteration ${data.iteration ?? '?'} finished (${data.tool_success_count || 0} ok / ${data.tool_failure_count || 0} failed) with ${data.finish_reason || 'stop'}`;
        default:
            return Object.keys(data).length ? JSON.stringify(data) : event.event_type;
    }
};

OSA.workspacePermissionLabel = function(permission) {
    return permission === 'read_only' ? 'Read only' : 'Read + write';
};

OSA.workspacePermissionBadge = function(permission) {
    return permission === 'read_only' ? '[RO]' : '[RW]';
};

OSA.personaLabel = function(personaId) {
    if (personaId === 'default') return 'Default';
    const match = OSA.availablePersonas.find(p => p.id === personaId);
    return match?.name || personaId;
};

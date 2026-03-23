window.OSA = window.OSA || {};

OSA.toggleInspector = function() {
    const inspector = document.getElementById('session-inspector');
    const countEl = document.getElementById('inspector-count');
    if (!inspector) return;
    const isCollapsed = inspector.classList.contains('collapsed');
    inspector.classList.toggle('collapsed', !isCollapsed);
    OSA.setInspectorExpanded(isCollapsed);
    if (countEl) {
        countEl.style.display = isCollapsed ? 'none': 'inline-flex';
    }
    if (isCollapsed) {
        OSA.refreshSessionInspector();
    }
};

OSA.updateInspectorCount = function() {
    const countEl = document.getElementById('inspector-count');
    if (!countEl) return;
    const state = OSA.getSessionInspectorState();
    const todos = OSA.getSessionTodos() || [];
    const historyCount = (state.history || []).length;
    const snapshotCount = (state.snapshots || []).length;
    const todoCount = todos.filter(t => t.status !== 'completed').length;
    const total = historyCount + snapshotCount + todoCount;
    countEl.textContent = total > 0 ? total : '';
};

OSA.renderSessionHistory = function() {
    const list = document.getElementById('session-history-list');
    const meta = document.getElementById('session-history-meta');
    if (!list || !meta) return;
    const state = OSA.getSessionInspectorState();
    const history = state.history || [];
    const currentSession = OSA.getCurrentSession();
    meta.textContent = currentSession?.id ? `${history.length} event${history.length === 1 ? '' : 's'}` : 'No session selected';
    if (!currentSession?.id) {
        list.innerHTML = '<div class="inspector-empty">Select a session to inspect stored runtime history.</div>';
        OSA.updateInspectorCount();
        return;
    }
    if (!history.length) {
        list.innerHTML = '<div class="inspector-empty">No stored history yet for this session.</div>';
        OSA.updateInspectorCount();
        return;
    }
    list.innerHTML = history.slice(-30).reverse().map(event => `
        <div class="history-item">
            <div class="history-item-header">
                <span class="history-type">${OSA.escapeHtml(event.event_type)}</span>
                <span class="history-time">${OSA.escapeHtml(OSA.formatRelativeDateTime(event.timestamp))}</span>
            </div>
            <div class="history-summary">${OSA.escapeHtml(OSA.summarizeHistoryEvent(event))}</div>
        </div>
    `).join('');
    OSA.updateInspectorCount();
};

OSA.renderSessionSnapshots = function() {
    const list = document.getElementById('session-snapshots-list');
    const meta = document.getElementById('session-snapshots-meta');
    if (!list || !meta) return;
    const state = OSA.getSessionInspectorState();
    const snapshots = state.snapshots || [];
    const currentSession = OSA.getCurrentSession();
    meta.textContent = currentSession?.id ? `${snapshots.length} snapshot${snapshots.length === 1 ? '' : 's'}` : 'No session selected';
    if (!currentSession?.id) {
        list.innerHTML = '<div class="inspector-empty">OSA file snapshots will appear here after edit tools run.</div>';
        OSA.updateInspectorCount();
        return;
    }
    if (!snapshots.length) {
        list.innerHTML = '<div class="inspector-empty">No OSA-owned file snapshots recorded yet.</div>';
        OSA.updateInspectorCount();
        return;
    }
    list.innerHTML = snapshots.map(snapshot => {
        const paths = (snapshot.paths || []).slice(0, 4).map(path => `<code>${OSA.escapeHtml(path)}</code>`).join(' ');
        const more = snapshot.paths?.length > 4 ? ` +${snapshot.paths.length - 4} more` : '';
        return `
            <div class="snapshot-item">
                <div class="snapshot-item-header">
                    <span class="snapshot-tool">${OSA.escapeHtml(snapshot.tool_name || 'tool')}</span>
                    <span class="snapshot-time">${OSA.escapeHtml(OSA.formatRelativeDateTime(snapshot.created_at))}</span>
                </div>
                <div class="snapshot-paths">${paths || '<span class="inspector-empty">No paths recorded</span>'}${OSA.escapeHtml(more)}</div>
                <div class="snapshot-actions">
                    <button class="snapshot-revert-btn" type="button" onclick="OSA.revertSessionSnapshot('${snapshot.snapshot_id}')">Revert</button>
                </div>
            </div>
        `;
    }).join('');
    OSA.updateInspectorCount();
};

OSA.refreshSessionInspector = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id) {
        OSA.setSessionInspectorState({ history: [], snapshots: [] });
        OSA.renderSessionHistory();
        OSA.renderSessionSnapshots();
        OSA.fetchAndRenderTodos();
        return;
    }
    try {
        const [historyRes, snapshotsRes] = await Promise.all([
            fetch(`/api/sessions/${currentSession.id}/history`, { headers: { 'Authorization': `Bearer ${OSA.getToken()}` } }),
            fetch(`/api/sessions/${currentSession.id}/snapshots`, { headers: { 'Authorization': `Bearer ${OSA.getToken()}` } })
        ]);
        const historyData = await historyRes.json();
        const snapshotsData = await snapshotsRes.json();
        if (!historyRes.ok) throw new Error(historyData.error || `History HTTP ${historyRes.status}`);
        if (!snapshotsRes.ok) throw new Error(snapshotsData.error || `Snapshots HTTP ${snapshotsRes.status}`);
        OSA.setSessionInspectorState({ history: Array.isArray(historyData) ? historyData : [], snapshots: Array.isArray(snapshotsData) ? snapshotsData : [] });
        OSA.renderSessionHistory();
        OSA.renderSessionSnapshots();
        await OSA.fetchAndRenderTodos();
    } catch (error) {
        console.error('Failed to refresh session inspector:', error);
    }
};

OSA.scheduleSessionInspectorRefresh = function() {
    const existingTimeout = OSA.getInspectorRefreshTimeout();
    if (existingTimeout) clearTimeout(existingTimeout);
    OSA.setInspectorRefreshTimeout(setTimeout(() => {
        OSA.setInspectorRefreshTimeout(null);
        OSA.refreshSessionInspector();
    }, 350));
};

OSA.revertSessionSnapshot = async function(snapshotId) {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id || !snapshotId) return;
    if (!confirm(`Restore OSA snapshot ${snapshotId}? This only reverts OSA-tracked file edits.`)) return;
    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/snapshots/revert`, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({ snapshot_id: snapshotId })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.selectSession(currentSession.id);
        await OSA.refreshSessionInspector();
    } catch (error) {
        alert(`Failed to revert snapshot: ${error.message}`);
    }
};

window.refreshSessionInspector = OSA.refreshSessionInspector;
window.revertSessionSnapshot = OSA.revertSessionSnapshot;
window.toggleInspector = OSA.toggleInspector;

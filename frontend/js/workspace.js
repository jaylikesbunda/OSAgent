window.OSA = window.OSA || {};

OSA.updateWorkspaceChip = function(workspaceId, workspacePath) {
    const label = document.getElementById('workspace-trigger-label');
    const ws = OSA.getWorkspaceState();
    const workspace = ws.workspaces.find(w => w.id === (workspaceId || 'default'));
    if (label) {
        label.textContent = workspace?.name || workspaceId || 'default';
        label.title = workspacePath || workspace?.name || workspaceId || 'default';
    }
};

OSA.toggleWorkspaceMenu = function() {
    const menu = document.getElementById('workspace-menu');
    const trigger = document.getElementById('workspace-trigger');
    if (!menu || !trigger) return;
    OSA.closePersonaMenu();
    menu.classList.toggle('hidden');
    trigger.classList.toggle('open');
};

OSA.closeWorkspaceMenu = function() {
    const menu = document.getElementById('workspace-menu');
    const trigger = document.getElementById('workspace-trigger');
    if (menu) menu.classList.add('hidden');
    if (trigger) trigger.classList.remove('open');
};

OSA.togglePersonaMenu = function() {
    const menu = document.getElementById('persona-menu');
    const trigger = document.getElementById('persona-trigger');
    if (!menu || !trigger) return;
    OSA.closeWorkspaceMenu();
    menu.classList.toggle('hidden');
    trigger.classList.toggle('open');
};

OSA.closePersonaMenu = function() {
    const menu = document.getElementById('persona-menu');
    const trigger = document.getElementById('persona-trigger');
    if (menu) menu.classList.add('hidden');
    if (trigger) trigger.classList.remove('open');
};

OSA.setWorkspaceInlineStatus = function(message, isError = false) {
    const status = document.getElementById('workspace-inline-status');
    if (!status) return;
    if (!message) {
        status.textContent = '';
        status.classList.add('hidden');
        return;
    }
    status.textContent = message;
    status.classList.remove('hidden');
    status.classList.toggle('error', isError);
};

OSA.selectedWorkspaceId = function() {
    const ws = OSA.getWorkspaceState();
    return ws.activeWorkspace || 'default';
};

OSA.renderWorkspaceMenu = function() {
    const list = document.getElementById('workspace-menu-list');
    if (!list) return;
    const ws = OSA.getWorkspaceState();
    if (!ws.workspaces.length) {
        list.innerHTML = '<div class="workspace-inline-status">No workspaces yet.</div>';
        return;
    }
    list.innerHTML = ws.workspaces.map(w => {
        const isActive = OSA.selectedWorkspaceId() === w.id;
        return `
            <div class="menu-row ${isActive ? 'active' : ''}">
                <button class="menu-row-main" type="button" onclick="OSA.selectWorkspaceFromMenu('${OSA.escapeHtml(w.id)}')">
                    <span class="permission-pill ${w.permission === 'read_only' ? 'read-only' : 'read-write'}">${w.permission === 'read_only' ? 'RO' : 'RW'}</span>
                    <span class="menu-row-copy">
                        <span class="menu-row-title">${OSA.escapeHtml(w.name || w.id)}</span>
                        <span class="menu-row-meta">${OSA.escapeHtml(w.path || w.id)}</span>
                    </span>
                </button>
                <button class="menu-icon-btn" type="button" onclick="event.stopPropagation(); OSA.openWorkspaceEditorForEdit('${OSA.escapeHtml(w.id)}')">Edit</button>
            </div>
        `;
    }).join('');
};

OSA.selectWorkspaceFromMenu = function(workspaceId) {
    const ws = OSA.getWorkspaceState();
    ws.activeWorkspace = workspaceId;
    OSA.setWorkspaceState(ws);
    OSA.onWorkspaceSelectionChange();
    const currentSession = OSA.getCurrentSession();
    if (currentSession?.id) {
        OSA.applySessionWorkspace();
    }
    OSA.closeWorkspaceMenu();
};

OSA.openWorkspaceEditorForEdit = function(workspaceId) {
    const ws = OSA.getWorkspaceState();
    const workspace = ws.workspaces.find(w => w.id === workspaceId);
    if (!workspace) return;
    document.getElementById('workspace-inline-id').value = workspace.id || '';
    document.getElementById('workspace-inline-name').value = workspace.name || '';
    document.getElementById('workspace-inline-path').value = workspace.path || '';
    document.getElementById('workspace-inline-description').value = workspace.description || '';
    document.getElementById('workspace-inline-permission').value = workspace.permission || 'read_write';
    document.getElementById('workspace-inline-id').readOnly = true;
    OSA.setEditingWorkspaceId(workspaceId);
    document.getElementById('workspace-inline-editor').classList.remove('hidden');
    OSA.setWorkspaceInlineStatus(`Editing ${workspace.name || workspace.id}`);
};

OSA.openWorkspaceEditorForCreate = function() {
    document.getElementById('workspace-inline-id').value = '';
    document.getElementById('workspace-inline-name').value = '';
    document.getElementById('workspace-inline-path').value = '';
    document.getElementById('workspace-inline-description').value = '';
    document.getElementById('workspace-inline-permission').value = 'read_write';
    document.getElementById('workspace-inline-id').readOnly = false;
    OSA.setEditingWorkspaceId(null);
    document.getElementById('workspace-inline-editor').classList.remove('hidden');
    OSA.setWorkspaceInlineStatus('Adding a new workspace.');
};

OSA.closeWorkspaceEditor = function() {
    document.getElementById('workspace-inline-editor').classList.add('hidden');
    OSA.setWorkspaceInlineStatus('');
};

OSA.browseWorkspacePath = async function() {
    OSA.setWorkspaceInlineStatus('');
    try {
        const res = await fetch('/api/workspaces/browse', {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        document.getElementById('workspace-inline-path').value = data.path || '';
        const parts = data.path.replace(/\\/g, '/').split('/').filter(Boolean);
        const nameInput = document.getElementById('workspace-inline-name');
        const idInput = document.getElementById('workspace-inline-id');
        if (nameInput && !nameInput.value.trim()) nameInput.value = parts[parts.length - 1] || '';
        if (idInput && !idInput.value.trim()) {
            const slug = (parts[parts.length - 1] || 'workspace').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
            idInput.value = slug || 'workspace';
        }
        OSA.setWorkspaceInlineStatus('Folder selected.');
    } catch (error) {
        if (error.message !== 'Folder selection was cancelled') {
            OSA.setWorkspaceInlineStatus(error.message, true);
        }
    }
};

OSA.onWorkspaceSelectionChange = function() {
    const id = OSA.selectedWorkspaceId();
    const ws = OSA.getWorkspaceState();
    const workspace = ws.workspaces.find(w => w.id === id);
    OSA.updateWorkspaceChip(id, workspace?.path || '');
    OSA.renderWorkspaceMenu();
};

OSA.applySessionWorkspace = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id) {
        alert('Select a session first.');
        return;
    }
    const workspaceId = OSA.selectedWorkspaceId();
    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/workspace`, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({ workspace_id: workspaceId })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        const ws = OSA.getWorkspaceState();
        ws.activeWorkspace = data.id;
        OSA.setWorkspaceState(ws);
        OSA.updateWorkspaceChip(data.id, data.path);
        OSA.renderWorkspaceMenu();
        OSA.setWorkspaceInlineStatus(`Using ${data.name || data.id} for this chat.`);
    } catch (error) {
        OSA.setWorkspaceInlineStatus(`Failed to set session workspace: ${error.message}`, true);
    }
};

OSA.saveWorkspaceInline = async function() {
    const id = document.getElementById('workspace-inline-id').value.trim();
    const name = document.getElementById('workspace-inline-name').value.trim();
    const path = document.getElementById('workspace-inline-path').value.trim();
    const description = document.getElementById('workspace-inline-description').value.trim();
    const permission = document.getElementById('workspace-inline-permission').value || 'read_write';
    
    if (!id || !name || !path) {
        OSA.setWorkspaceInlineStatus('Workspace id, name, and folder are required.', true);
        return null;
    }
    
    const ws = OSA.getWorkspaceState();
    const exists = ws.workspaces.some(w => w.id === id);
    const url = exists ? `/api/workspaces/${encodeURIComponent(id)}` : '/api/workspaces';
    
    try {
        const res = await fetch(url, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({ id, name, path, description: description || null, permission })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadWorkspaces();
        OSA.setWorkspaceInlineStatus(exists ? `Updated ${data.name || data.id}.` : `Added ${data.name || data.id}.`);
        return data;
    } catch (error) {
        OSA.setWorkspaceInlineStatus(`Failed to save workspace: ${error.message}`, true);
        return null;
    }
};

OSA.applyInlineWorkspaceToSession = async function() {
    const saved = await OSA.saveWorkspaceInline();
    if (!saved) return;
    const ws = OSA.getWorkspaceState();
    ws.activeWorkspace = saved.id;
    OSA.setWorkspaceState(ws);
    OSA.onWorkspaceSelectionChange();
    const currentSession = OSA.getCurrentSession();
    if (currentSession?.id) {
        await OSA.applySessionWorkspace();
    } else {
        OSA.setWorkspaceInlineStatus(`Saved. Start a chat to apply it.`);
    }
};

OSA.renderWorkspaceSelect = function() {
    const activeSelect = document.getElementById('setting-active-workspace');
    const ws = OSA.getWorkspaceState();
    const optionsHtml = ws.workspaces.map(w => `<option value="${OSA.escapeHtml(w.id)}">${w.permission === 'read_only' ? '[RO]' : '[RW]'} ${OSA.escapeHtml(w.name || w.id)}</option>`).join('');
    if (activeSelect) {
        activeSelect.innerHTML = optionsHtml || '<option value="default">default</option>';
        activeSelect.value = ws.activeWorkspace || 'default';
    }
    const active = ws.workspaces.find(w => w.id === ws.activeWorkspace);
    OSA.updateWorkspaceChip(ws.activeWorkspace, active?.path || '');
};

OSA.renderWorkspaceList = function() {
    const list = document.getElementById('workspace-list');
    if (!list) return;
    const ws = OSA.getWorkspaceState();
    if (!ws.workspaces.length) {
        list.innerHTML = '<div class="workspace-meta">No workspaces configured.</div>';
        return;
    }
    list.innerHTML = ws.workspaces.map(w => {
        const isActive = w.id === ws.activeWorkspace;
        return `
            <div class="workspace-item">
                <div>
                    <div class="decision-key">${OSA.escapeHtml(w.name || w.id)} ${isActive ? '(active)' : ''}</div>
                    <div class="decision-value">${OSA.escapeHtml(w.path)}</div>
                    <div class="workspace-meta">id: ${OSA.escapeHtml(w.id)} · ${w.permission === 'read_only' ? 'Read only' : 'Read + write'}</div>
                </div>
                <div class="workspace-actions">
                    <button type="button" class="btn-secondary" onclick="OSA.editWorkspaceInForm('${OSA.escapeHtml(w.id)}')">Edit</button>
                    ${w.id === 'default' ? '' : `<button type="button" class="btn-danger" onclick="OSA.deleteWorkspace('${OSA.escapeHtml(w.id)}')">Delete</button>`}
                </div>
            </div>
        `;
    }).join('');
};

OSA.loadWorkspaces = async function() {
    try {
        const res = await fetch('/api/workspaces', { headers: { 'Authorization': `Bearer ${OSA.getToken()}` } });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        const ws = OSA.getWorkspaceState();
        ws.workspaces = data.workspaces || [];
        ws.activeWorkspace = data.active_workspace || 'default';
        OSA.setWorkspaceState(ws);
        OSA.renderWorkspaceSelect();
        OSA.renderWorkspaceList();
        OSA.renderWorkspaceMenu();
    } catch (error) {
        console.error('Failed to load workspaces:', error);
    }
};

OSA.loadSessionWorkspace = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id) {
        const ws = OSA.getWorkspaceState();
        const active = ws.workspaces.find(w => w.id === ws.activeWorkspace);
        OSA.updateWorkspaceChip(ws.activeWorkspace, active?.path || '');
        OSA.renderWorkspaceMenu();
        return;
    }
    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/workspace`, { headers: { 'Authorization': `Bearer ${OSA.getToken()}` } });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        const ws = OSA.getWorkspaceState();
        ws.activeWorkspace = data.id;
        OSA.setWorkspaceState(ws);
        OSA.updateWorkspaceChip(data.id, data.path);
        OSA.renderWorkspaceMenu();
    } catch (error) {
        console.error('Failed to load session workspace:', error);
    }
};

OSA.editWorkspaceInForm = function(workspaceId) {
    const ws = OSA.getWorkspaceState();
    const w = ws.workspaces.find(w => w.id === workspaceId);
    if (!w) return;
    document.getElementById('workspace-id').value = w.id || '';
    document.getElementById('workspace-name').value = w.name || '';
    document.getElementById('workspace-path').value = w.path || '';
    document.getElementById('workspace-description').value = w.description || '';
};

OSA.deleteWorkspace = async function(workspaceId) {
    if (!workspaceId || workspaceId === 'default') {
        alert('Default workspace cannot be deleted.');
        return;
    }
    if (!confirm(`Delete workspace '${workspaceId}'?`)) return;
    try {
        const res = await fetch(`/api/workspaces/${encodeURIComponent(workspaceId)}`, {
            method: 'DELETE',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        await OSA.loadWorkspaces();
        OSA.setWorkspaceInlineStatus(`Deleted ${workspaceId}.`);
    } catch (error) {
        alert(`Failed to delete workspace: ${error.message}`);
    }
};

OSA.setActiveWorkspaceFromSettings = async function() {
    const select = document.getElementById('setting-active-workspace');
    if (!select) return;
    const workspaceId = select.value;
    try {
        const res = await fetch('/api/workspaces/active', {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({ workspace_id: workspaceId })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        const ws = OSA.getWorkspaceState();
        ws.activeWorkspace = data.id;
        OSA.setWorkspaceState(ws);
        OSA.renderWorkspaceSelect();
    } catch (error) {
        alert(`Failed to set active workspace: ${error.message}`);
    }
};

OSA.upsertWorkspaceFromForm = async function() {
    const id = document.getElementById('workspace-id').value.trim();
    const name = document.getElementById('workspace-name').value.trim();
    const path = document.getElementById('workspace-path').value.trim();
    const description = document.getElementById('workspace-description').value.trim();
    if (!id || !name || !path) {
        alert('Workspace id, name, and path are required.');
        return;
    }
    const ws = OSA.getWorkspaceState();
    const exists = ws.workspaces.some(w => w.id === id);
    const url = exists ? `/api/workspaces/${encodeURIComponent(id)}` : '/api/workspaces';
    try {
        const res = await fetch(url, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({ id, name, path, description: description || null })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        await OSA.loadWorkspaces();
        OSA.editWorkspaceInForm(data.id);
    } catch (error) {
        alert(`Failed to save workspace: ${error.message}`);
    }
};

OSA.resetWorkspaceForm = function() {
    ['workspace-id', 'workspace-name', 'workspace-path', 'workspace-description'].forEach(id => {
        const el = document.getElementById(id);
        if (el) el.value = '';
    });
};

window.toggleWorkspaceMenu = OSA.toggleWorkspaceMenu;
window.closeWorkspaceMenu = OSA.closeWorkspaceMenu;
window.togglePersonaMenu = OSA.togglePersonaMenu;
window.closePersonaMenu = OSA.closePersonaMenu;
window.onWorkspaceSelectionChange = OSA.onWorkspaceSelectionChange;
window.applySessionWorkspace = OSA.applySessionWorkspace;
window.browseWorkspacePath = OSA.browseWorkspacePath;
window.saveWorkspaceInline = OSA.saveWorkspaceInline;
window.applyInlineWorkspaceToSession = OSA.applyInlineWorkspaceToSession;
window.closeWorkspaceEditor = OSA.closeWorkspaceEditor;
window.openWorkspaceEditorForCreate = OSA.openWorkspaceEditorForCreate;
window.openWorkspaceEditorForEdit = OSA.openWorkspaceEditorForEdit;
window.resetWorkspaceForm = OSA.resetWorkspaceForm;
window.setActiveWorkspaceFromSettings = OSA.setActiveWorkspaceFromSettings;
window.upsertWorkspaceFromForm = OSA.upsertWorkspaceFromForm;
window.deleteWorkspace = OSA.deleteWorkspace;
window.editWorkspaceInForm = OSA.editWorkspaceInForm;

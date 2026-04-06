window.OSA = window.OSA || {};

OSA.updateWorkspaceChip = function(workspaceId, workspacePath) {
    const label = document.getElementById('workspace-trigger-label');
    const ws = OSA.getWorkspaceState();
    const workspace = ws.workspaces.find(w => w.id === (workspaceId || 'default'));
    const effectivePath = workspacePath || OSA.primaryWorkspacePath(workspace);
    if (label) {
        label.textContent = workspace?.name || workspaceId || 'default';
        label.title = effectivePath || workspace?.name || workspaceId || 'default';
    }
};

OSA.workspacePaths = function(workspace) {
    const explicitPaths = Array.isArray(workspace?.paths)
        ? workspace.paths.filter(wp => wp?.path && wp.path.trim())
        : [];

    if (explicitPaths.length) {
        return explicitPaths;
    }

    if (workspace?.path && workspace.path.trim()) {
        return [{
            path: workspace.path.trim(),
            permission: workspace.permission || 'read_write'
        }];
    }

    return [];
};

OSA.primaryWorkspacePath = function(workspace) {
    return OSA.workspacePaths(workspace)[0]?.path || '';
};

OSA.workspacePathSummary = function(workspace) {
    const paths = OSA.workspacePaths(workspace);
    if (!paths.length) {
        return workspace?.id || 'No path configured';
    }

    return paths.length > 1
        ? `${paths[0].path} (+${paths.length - 1} more)`
        : paths[0].path;
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
        const paths = OSA.workspacePaths(w);
        const primaryPath = paths[0]?.path || w.id;
        const pathCount = paths.length;
        return `
            <div class="menu-row ${isActive ? 'active' : ''}">
                <button class="menu-row-main" type="button" onclick="OSA.selectWorkspaceFromMenu('${OSA.escapeHtml(w.id)}')">
                    <span class="menu-row-copy">
                        <span class="menu-row-title">${OSA.escapeHtml(w.name || w.id)}</span>
                        <span class="menu-row-meta" title="${paths.map(p => p.path).join('\n')}">${OSA.escapeHtml(primaryPath)}${pathCount > 1 ? ` (+${pathCount - 1} more)` : ''}</span>
                    </span>
                </button>
                <span class="workspace-perm-subtext">${paths[0]?.permission === 'read_only' ? 'ro' : 'rw'}</span>
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
    document.getElementById('workspace-inline-description').value = workspace.description || '';
    document.getElementById('workspace-inline-id').readOnly = true;
    OSA.setEditingWorkspaceId(workspaceId);
    OSA.renderWorkspacePathsEditor(OSA.workspacePaths(workspace));
    document.getElementById('workspace-inline-editor').classList.remove('hidden');
    OSA.setWorkspaceInlineStatus(`Editing ${workspace.name || workspace.id}`);
};

OSA.openWorkspaceEditorForCreate = function() {
    document.getElementById('workspace-inline-id').value = '';
    document.getElementById('workspace-inline-name').value = '';
    document.getElementById('workspace-inline-description').value = '';
    document.getElementById('workspace-inline-id').readOnly = false;
    OSA.setEditingWorkspaceId(null);
    OSA.renderWorkspacePathsEditor([{ path: '', permission: 'read_write' }]);
    document.getElementById('workspace-inline-editor').classList.remove('hidden');
    OSA.setWorkspaceInlineStatus('Adding a new workspace.');
};

OSA.closeWorkspaceEditor = function() {
    document.getElementById('workspace-inline-editor').classList.add('hidden');
    OSA.setWorkspaceInlineStatus('');
};

OSA.renderWorkspacePathsEditor = function(paths) {
    const container = document.getElementById('workspace-paths-container');
    if (!container) return;
    
    if (!paths || paths.length === 0) {
        paths = [{ path: '', permission: 'read_write' }];
    }
    
    container.innerHTML = paths.map((wp, idx) => `
        <div class="workspace-path-row" data-index="${idx}">
            <input type="text" class="workspace-path-input" value="${OSA.escapeHtml(wp.path || '')}" placeholder="Choose a folder or enter path" />
            <select class="workspace-path-perm">
                <option value="read_write" ${wp.permission === 'read_write' ? 'selected' : ''}>Read + write</option>
                <option value="read_only" ${wp.permission === 'read_only' ? 'selected' : ''}>Read only</option>
            </select>
            <button class="workspace-path-remove" type="button" onclick="OSA.removeWorkspacePathRow(${idx})">-</button>
        </div>
    `).join('') + `<button class="workspace-path-add" type="button" onclick="OSA.browseWorkspacePath()">+ Add path</button>`;
    
    container.dataset.paths = JSON.stringify(paths);
};

OSA.removeWorkspacePathRow = function(index) {
    const container = document.getElementById('workspace-paths-container');
    let paths = JSON.parse(container.dataset.paths || '[]');
    if (paths.length <= 1) {
        OSA.setWorkspaceInlineStatus('A workspace must have at least one path', true);
        return;
    }
    paths.splice(index, 1);
    OSA.renderWorkspacePathsEditor(paths);
};

OSA.browseWorkspacePath = async function() {
    OSA.setWorkspaceInlineStatus('');
    try {
        const res = await fetch('/api/workspaces/browse', {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        
        const container = document.getElementById('workspace-paths-container');
        let paths = JSON.parse(container.dataset.paths || '[]');
        
        const parts = data.path.replace(/\\/g, '/').split('/').filter(Boolean);
        const nameInput = document.getElementById('workspace-inline-name');
        const idInput = document.getElementById('workspace-inline-id');
        if (nameInput && !nameInput.value.trim()) nameInput.value = parts[parts.length - 1] || '';
        if (idInput && !idInput.value.trim()) {
            const slug = (parts[parts.length - 1] || 'workspace').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
            idInput.value = slug || 'workspace';
        }
        
        if (paths.some(existing => existing.path === data.path)) {
            OSA.setWorkspaceInlineStatus('That path is already in this workspace.');
            return;
        }

        paths.push({ path: data.path, permission: 'read_write' });
        OSA.renderWorkspacePathsEditor(paths);
        OSA.setWorkspaceInlineStatus('Folder selected.');
    } catch (error) {
        if (error.message !== 'Folder selection was cancelled') {
            OSA.setWorkspaceInlineStatus(error.message, true);
        }
    }
};

OSA.getWorkspacePathsFromEditor = function() {
    const container = document.getElementById('workspace-paths-container');
    const rows = container.querySelectorAll('.workspace-path-row');
    const paths = [];
    rows.forEach(row => {
        const pathInput = row.querySelector('.workspace-path-input');
        const permSelect = row.querySelector('.workspace-path-perm');
        if (pathInput && pathInput.value.trim()) {
            paths.push({
                path: pathInput.value.trim(),
                permission: permSelect ? permSelect.value : 'read_write'
            });
        }
    });
    return paths;
};

OSA.onWorkspaceSelectionChange = function() {
    const id = OSA.selectedWorkspaceId();
    const ws = OSA.getWorkspaceState();
    const workspace = ws.workspaces.find(w => w.id === id);
    OSA.updateWorkspaceChip(id, OSA.primaryWorkspacePath(workspace));
    OSA.renderWorkspaceMenu();
};

OSA.applySessionWorkspace = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !currentSession.id) {
        alert('Select a session first.');
        return;
    }
    const workspaceId = OSA.selectedWorkspaceId();
    try {
        const url = '/api/sessions/' + encodeURIComponent(currentSession.id) + '/workspace';
        const res = await fetch(url, {
            method: 'POST',
            headers: { 'Authorization': 'Bearer ' + OSA.getToken(), 'Content-Type': 'application/json' },
            body: JSON.stringify({ workspace_id: workspaceId })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'HTTP ' + res.status);
        const ws = OSA.getWorkspaceState();
        ws.activeWorkspace = data.id;
        OSA.setWorkspaceState(ws);
        OSA.updateWorkspaceChip(data.id, OSA.primaryWorkspacePath(data));
        OSA.renderWorkspaceMenu();
        var nameOrId = data.name || data.id;
        OSA.setWorkspaceInlineStatus('Using ' + nameOrId + ' for this chat.');
    } catch (error) {
        OSA.setWorkspaceInlineStatus('Failed to set session workspace: ' + error.message, true);
    }
};

OSA.saveWorkspaceInline = async function() {
    const id = document.getElementById('workspace-inline-id').value.trim();
    const name = document.getElementById('workspace-inline-name').value.trim();
    const description = document.getElementById('workspace-inline-description').value.trim();
    const paths = OSA.getWorkspacePathsFromEditor();
    
    if (!id || !name) {
        OSA.setWorkspaceInlineStatus('Workspace id and name are required.', true);
        return null;
    }

    const dedupedPaths = [];
    const seen = new Set();
    paths.forEach(wp => {
        const normalized = wp.path.trim();
        if (!normalized || seen.has(normalized)) return;
        seen.add(normalized);
        dedupedPaths.push({
            path: normalized,
            permission: wp.permission || 'read_write'
        });
    });

    
    if (dedupedPaths.length === 0 || !dedupedPaths[0].path) {
        OSA.setWorkspaceInlineStatus('At least one workspace path is required.', true);
        return null;
    }
    
    const ws = OSA.getWorkspaceState();
    const exists = ws.workspaces.some(w => w.id === id);
    const url = exists ? `/api/workspaces/${encodeURIComponent(id)}` : '/api/workspaces';
    
    try {
        const res = await fetch(url, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({ id, name, paths: dedupedPaths, description: description || null })
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
    OSA.updateWorkspaceChip(ws.activeWorkspace, OSA.primaryWorkspacePath(active));
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
        const paths = OSA.workspacePaths(w);
        return `
            <div class="workspace-item">
                <div>
                    <div class="decision-key">${OSA.escapeHtml(w.name || w.id)} ${isActive ? '(active)' : ''}</div>
                    <div class="decision-value" title="${OSA.escapeHtml(paths.map(p => p.path).join('\n'))}">${OSA.escapeHtml(OSA.workspacePathSummary(w))}</div>
                    <div class="workspace-meta">id: ${OSA.escapeHtml(w.id)} · ${paths[0]?.permission === 'read_only' ? 'Read only' : 'Read + write'}${paths.length > 1 ? ` · ${paths.length} paths` : ''}</div>
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
        OSA.updateWorkspaceChip(ws.activeWorkspace, OSA.primaryWorkspacePath(active));
        OSA.renderWorkspaceMenu();
        return;
    }
    const sessionId = currentSession.id;
    try {
        const res = await fetch(`/api/sessions/${sessionId}/workspace`, { headers: { 'Authorization': `Bearer ${OSA.getToken()}` } });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        const activeSession = OSA.getCurrentSession();
        if (!activeSession || activeSession.id !== sessionId) return;
        const ws = OSA.getWorkspaceState();
        ws.activeWorkspace = data.id;
        OSA.setWorkspaceState(ws);
        OSA.updateWorkspaceChip(data.id, OSA.primaryWorkspacePath(data));
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
    document.getElementById('workspace-path').value = OSA.primaryWorkspacePath(w);
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

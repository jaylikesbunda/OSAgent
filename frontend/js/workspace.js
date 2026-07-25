window.OSA = window.OSA || {};

OSA.updateWorkspaceChip = function(workspaceId, workspacePath) {
    const label = document.getElementById('context-trigger-label');
    const wsLabel = document.getElementById('ctx-ws-active-label');
    const ws = OSA.getWorkspaceState();
    const workspace = ws.workspaces.find(w => w.id === (workspaceId || 'default'));
    const effectivePath = workspacePath || OSA.primaryWorkspacePath(workspace);
    const perm = OSA.workspacePaths(workspace)[0]?.permission;
    const permTag = perm === 'read_only' ? ' [RO]' : perm === 'read_write' ? ' [RW]' : '';
    const name = (workspace?.name || workspaceId || 'default') + permTag;
    if (label) {
        label.textContent = name;
        label.title = effectivePath || workspace?.name || workspaceId || 'default';
    }
    if (wsLabel) {
        wsLabel.textContent = name;
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

OSA.positionMenuForTrigger = function(menuEl, triggerEl) {
    if (!menuEl || !triggerEl) return;
    const host = menuEl.offsetParent || menuEl.parentElement;
    if (!host) return;

    const hostRect = host.getBoundingClientRect();
    const triggerRect = triggerEl.getBoundingClientRect();

    const wasHidden = menuEl.classList.contains('hidden');
    if (wasHidden) menuEl.classList.remove('hidden');
    const menuRect = menuEl.getBoundingClientRect();
    if (wasHidden) menuEl.classList.add('hidden');

    const gap = 6;
    let left = triggerRect.left - hostRect.left;
    const bottom = hostRect.bottom - triggerRect.top + gap;

    const maxLeft = hostRect.width - menuRect.width - 8;
    if (left > maxLeft) left = Math.max(8, maxLeft);
    if (left < 8) left = 8;

    menuEl.style.left = left + 'px';
    menuEl.style.right = 'auto';
    menuEl.style.bottom = bottom + 'px';
};

OSA._repositionOpenMenus = function() {
    const menu = document.getElementById('context-menu');
    const trigger = document.getElementById('context-trigger');
    if (menu && trigger && !menu.classList.contains('hidden')) {
        OSA.positionMenuForTrigger(menu, trigger);
    }
};

OSA.toggleContextMenu = function() {
    const menu = document.getElementById('context-menu');
    const trigger = document.getElementById('context-trigger');
    if (!menu || !trigger) return;
    menu.classList.toggle('hidden');
    trigger.classList.toggle('open');
    if (!menu.classList.contains('hidden')) {
        OSA.positionMenuForTrigger(menu, trigger);
        document.addEventListener('click', OSA._contextMenuOutsideClick);
    } else {
        document.removeEventListener('click', OSA._contextMenuOutsideClick);
        OSA.closeWorkspaceEditor();
    }
};

OSA.closeContextMenu = function() {
    const menu = document.getElementById('context-menu');
    const trigger = document.getElementById('context-trigger');
    if (menu) {
        menu.classList.add('hidden');
        menu.style.left = '';
        menu.style.right = '';
        menu.style.bottom = '';
    }
    if (trigger) trigger.classList.remove('open');
    document.removeEventListener('click', OSA._contextMenuOutsideClick);
    OSA.closeWorkspaceEditor();
};

OSA._contextMenuOutsideClick = function(e) {
    const menu = document.getElementById('context-menu');
    const trigger = document.getElementById('context-trigger');
    if (menu && !menu.contains(e.target) && trigger && !trigger.contains(e.target)) {
        OSA.closeContextMenu();
    }
};

OSA.switchContextTab = function(tab) {
    document.querySelectorAll('.ctx-tab').forEach(function(t) {
        t.classList.toggle('active', t.dataset.tab === tab);
    });
    document.getElementById('ctx-tab-workspace')?.classList.toggle('hidden', tab !== 'workspace');
    document.getElementById('ctx-tab-persona')?.classList.toggle('hidden', tab !== 'persona');
    if (tab === 'persona' && !OSA.getAvailablePersonas()?.length) {
        OSA.loadPersonaCatalog();
    }
};

OSA.toggleWorkspaceMenu = OSA.toggleContextMenu;
OSA.closeWorkspaceMenu = OSA.closeContextMenu;
OSA.togglePersonaMenu = function() { OSA.toggleContextMenu(); OSA.switchContextTab('persona'); };
OSA.closePersonaMenu = OSA.closeContextMenu;

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
        list.innerHTML = `
            <div class="menu-empty">
                <div class="menu-empty-title">No workspaces yet</div>
                <div class="menu-empty-text">Add a folder to scope what the agent can read and write.</div>
                <button class="control-btn primary menu-empty-cta" type="button" onclick="openWorkspaceEditorForCreate()">+ Add workspace</button>
            </div>
        `;
        return;
    }
    const activeId = OSA.selectedWorkspaceId();
    list.innerHTML = ws.workspaces.map(w => {
        const isActive = activeId === w.id;
        const paths = OSA.workspacePaths(w);
        const primaryPath = paths[0]?.path || w.id;
        const pathCount = paths.length;
        const perm = paths[0]?.permission === 'read_only' ? 'ro' : 'rw';
        const permClass = paths[0]?.permission === 'read_only' ? 'ro' : 'rw';
        return `
            <div class="menu-row ${isActive ? 'active' : ''}">
                <button class="menu-row-main" type="button" onclick="OSA.selectWorkspaceFromMenu('${OSA.escapeHtml(w.id)}')">
                    <span class="menu-row-check" aria-hidden="true">${isActive ? '&#10003;' : ''}</span>
                    <span class="menu-row-copy">
                        <span class="menu-row-title">${OSA.escapeHtml(w.name || w.id)}</span>
                        <span class="menu-row-meta" title="${paths.map(p => p.path).join('\n')}">${OSA.escapeHtml(primaryPath)}${pathCount > 1 ? ` (+${pathCount - 1} more)` : ''}</span>
                    </span>
                    <span class="menu-row-badge ${permClass}">${perm}</span>
                </button>
                <button class="menu-icon-btn" type="button" title="Edit workspace" onclick="event.stopPropagation(); OSA.openWorkspaceEditorForEdit('${OSA.escapeHtml(w.id)}')">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7"></path><path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z"></path></svg>
                </button>
            </div>
        `;
    }).join('');
};

OSA.selectWorkspaceFromMenu = async function(workspaceId) {
    const currentSession = OSA.getCurrentSession();
    if (currentSession?.id && OSA.isAgentProcessing()) {
        OSA.setWorkspaceInlineStatus('Stop or wait for the current turn before switching workspace.', true);
        OSA.closeWorkspaceMenu();
        return;
    }
    const ws = OSA.getWorkspaceState();

    try {
        const res = await OSA.fetchWithAuth('/api/workspaces/active', {
            method: 'POST',
            body: JSON.stringify({ workspace_id: workspaceId })
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        if (currentSession?.id) {
            const sessionWorkspace = await OSA.applySessionWorkspace(workspaceId);
            if (!sessionWorkspace) return;
        }
        ws.activeWorkspace = workspaceId;
        OSA.setWorkspaceState(ws);
        OSA.onWorkspaceSelectionChange();
    } catch (error) {
        OSA.setWorkspaceInlineStatus('Failed to switch workspace: ' + error.message, true);
    }
    OSA.closeWorkspaceMenu();
};

OSA._wsEditorPerm = 'read_write';

OSA.openWorkspaceEditorForEdit = function(workspaceId) {
    const ws = OSA.getWorkspaceState();
    const workspace = ws.workspaces.find(w => w.id === workspaceId);
    if (!workspace) return;
    const firstPath = (workspace.paths && workspace.paths[0]) || {};
    document.getElementById('workspace-inline-id').value = workspace.id || '';
    document.getElementById('workspace-inline-name').value = workspace.name || '';
    document.getElementById('workspace-inline-path').value = firstPath.path || '';
    OSA.setWorkspacePerm(firstPath.permission || 'read_write');
    document.getElementById('workspace-inline-id').readOnly = true;
    OSA.setEditingWorkspaceId(workspaceId);
    document.getElementById('workspace-inline-editor').classList.remove('hidden');
    OSA.setWorkspaceInlineStatus('Editing ' + (workspace.name || workspace.id));
};

OSA.openWorkspaceEditorForCreate = function() {
    document.getElementById('workspace-inline-id').value = '';
    document.getElementById('workspace-inline-name').value = '';
    document.getElementById('workspace-inline-path').value = '';
    document.getElementById('workspace-inline-id').readOnly = false;
    OSA.setWorkspacePerm('read_write');
    OSA.setEditingWorkspaceId(null);
    document.getElementById('workspace-inline-editor').classList.remove('hidden');
    OSA.setWorkspaceInlineStatus('');
    window.setTimeout(() => document.getElementById('workspace-inline-path')?.focus(), 0);
};

OSA.closeWorkspaceEditor = function() {
    document.getElementById('workspace-inline-editor').classList.add('hidden');
    OSA.setWorkspaceInlineStatus('');
};

OSA.setWorkspacePerm = function(perm) {
    OSA._wsEditorPerm = perm;
    const rw = document.getElementById('ws-perm-rw');
    const ro = document.getElementById('ws-perm-ro');
    if (rw) rw.classList.toggle('active', perm === 'read_write');
    if (ro) ro.classList.toggle('active', perm === 'read_only');
};

OSA.browseWorkspaceSimple = async function() {
    OSA.setWorkspaceInlineStatus('');
    try {
        const res = await OSA.fetchWithAuth('/api/workspaces/browse');
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'HTTP ' + res.status);
        document.getElementById('workspace-inline-path').value = data.path;
        const parts = data.path.replace(/\\/g, '/').split('/').filter(Boolean);
        const nameInput = document.getElementById('workspace-inline-name');
        const idInput = document.getElementById('workspace-inline-id');
        if (nameInput && !nameInput.value.trim()) nameInput.value = parts[parts.length - 1] || '';
        if (idInput && !idInput.value.trim()) {
            const slug = (parts[parts.length - 1] || 'workspace').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '');
            idInput.value = slug || 'workspace';
        }
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
    OSA.updateWorkspaceChip(id, OSA.primaryWorkspacePath(workspace));
    OSA.renderWorkspaceMenu();
};

OSA.applySessionWorkspace = async function(requestedWorkspaceId) {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession || !currentSession.id) {
        alert('Select a session first.');
        return null;
    }
    const workspaceId = requestedWorkspaceId || OSA.selectedWorkspaceId();
    try {
        const url = '/api/sessions/' + encodeURIComponent(currentSession.id) + '/workspace';
        const res = await OSA.fetchWithAuth(url, {
            method: 'POST',
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
        return data;
    } catch (error) {
        OSA.setWorkspaceInlineStatus('Failed to set session workspace: ' + error.message, true);
        return null;
    }
};

OSA.saveWorkspaceInline = async function() {
    const path = document.getElementById('workspace-inline-path').value.trim();
    if (!path) {
        OSA.setWorkspaceInlineStatus('Pick a folder path.', true);
        return null;
    }

    let id = document.getElementById('workspace-inline-id').value.trim();
    let name = document.getElementById('workspace-inline-name').value.trim();
    if (!id) {
        const parts = path.replace(/\\/g, '/').split('/').filter(Boolean);
        id = (parts[parts.length - 1] || 'workspace').toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-+|-+$/g, '') || 'workspace';
    }
    if (!name) {
        const parts = path.replace(/\\/g, '/').split('/').filter(Boolean);
        name = parts[parts.length - 1] || id;
    }

    const ws = OSA.getWorkspaceState();
    const exists = ws.workspaces.some(w => w.id === id);
    const url = exists ? '/api/workspaces/' + encodeURIComponent(id) : '/api/workspaces';

    try {
        const res = await OSA.fetchWithAuth(url, {
            method: 'POST',
            body: JSON.stringify({
                id: id,
                name: name,
                paths: [{ path: path, permission: OSA._wsEditorPerm || 'read_write' }],
                description: null
            })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'HTTP ' + res.status);
        await OSA.loadWorkspaces();
        OSA.setWorkspaceInlineStatus((exists ? 'Updated ' : 'Added ') + (data.name || data.id) + '.');
        return data;
    } catch (error) {
        OSA.setWorkspaceInlineStatus('Failed to save: ' + error.message, true);
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
        const res = await OSA.fetchWithAuth('/api/workspaces');
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
        const res = await OSA.fetchWithAuth(`/api/sessions/${sessionId}/workspace`);
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
        const res = await OSA.fetchWithAuth(`/api/workspaces/${encodeURIComponent(workspaceId)}`, {
            method: 'DELETE'
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
        const res = await OSA.fetchWithAuth('/api/workspaces/active', {
            method: 'POST',
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
        const res = await OSA.fetchWithAuth(url, {
            method: 'POST',
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
window.browseWorkspacePath = OSA.browseWorkspaceSimple;
window.saveWorkspaceInline = OSA.saveWorkspaceInline;
window.applyInlineWorkspaceToSession = OSA.applyInlineWorkspaceToSession;
window.closeWorkspaceEditor = OSA.closeWorkspaceEditor;
window.openWorkspaceEditorForCreate = OSA.openWorkspaceEditorForCreate;
window.openWorkspaceEditorForEdit = OSA.openWorkspaceEditorForEdit;
window.setActiveWorkspaceFromSettings = OSA.setActiveWorkspaceFromSettings;
window.upsertWorkspaceFromForm = OSA.upsertWorkspaceFromForm;
window.deleteWorkspace = OSA.deleteWorkspace;
window.editWorkspaceInForm = OSA.editWorkspaceInForm;
window.toggleContextMenu = OSA.toggleContextMenu;
window.closeContextMenu = OSA.closeContextMenu;

window.addEventListener('resize', () => {
    OSA.debounce('repositionMenus', OSA._repositionOpenMenus, 100);
});

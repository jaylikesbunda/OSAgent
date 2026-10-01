window.OSA = window.OSA || {};
OSA._previewState.width = 640;
OSA._fileBrowser = { generation: 0, tabs: new Map(), folders: new Map(), expanded: new Set(['']), root: 0, roots: [], mode: 'files', selected: null };
const closePreviewPane = OSA.closeFilePreview;
OSA.closeFilePreview = function() {
    OSA._fileBrowser.controller?.abort();
    OSA._fileBrowser.controller = null;
    closePreviewPane();
};

OSA.browserNode = function(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined) node.textContent = text;
    return node;
};

OSA.resetFileBrowser = function() {
    const old = OSA._fileBrowser;
    old.controller?.abort();
    OSA._fileBrowser = { generation: old.generation + 1, tabs: new Map(), folders: new Map(), expanded: new Set(['']), root: 0, roots: [], mode: 'files', selected: null };
    if (typeof document === 'undefined' || !document.getElementById('file-browser-tree')) return;
    OSA.renderFileBrowser();
    if (OSA._previewState.open) OSA.refreshFileBrowser();
};

OSA.showFileBrowser = function() {
    const panel = document.getElementById('file-preview-panel');
    const app = document.getElementById('app-view');
    if (!panel || !app) return;
    app.classList.add('with-preview');
    app.style.setProperty('--preview-width', `${OSA._previewState.width}px`);
    panel.classList.remove('hidden');
    document.getElementById('preview-resize-handle')?.classList.remove('hidden');
    OSA._previewState.open = true;
    if (!OSA._fileBrowser.folders.has('')) OSA.loadBrowserFolder('');
};

OSA.toggleFilePreview = function() {
    if (OSA._previewState.open) return OSA.closeFilePreview();
    OSA.showFileBrowser();
    OSA.renderFileBrowser();
};

OSA.openFilePreview = function(path, content, options = {}) {
    OSA.showFileBrowser();
    const state = OSA._fileBrowser;
    state.controller?.abort(); state.controller = null;
    const key = (options.root ?? state.root) + ':' + path;
    const previous = state.tabs.get(key);
    const tab = {
        key, path, root: options.root ?? state.root, content: typeof content === 'string' ? content : '',
        diff: options.mode === 'diff', oldContent: options.oldContent ?? '',
        newContent: options.newContent ?? content ?? '', source: options.source || 'tool',
        mode: options.mode === 'diff' ? 'diff' : previous?.mode === 'preview' ? 'preview' : 'source',
    };
    state.tabs.delete(key); state.tabs.set(key, tab);
    // Bound retained file contents without discarding the selected tab.
    if (state.tabs.size > 8) state.tabs.delete(state.tabs.keys().next().value);
    state.selected = key;
    OSA._previewState.path = path; OSA._previewState.mode = options.mode === 'diff' ? 'diff' : 'file';
    if (tab.diff) state.mode = 'changes';
    OSA.renderFileBrowser();
};

OSA.browserRequest = async function(path, root, signal) {
    const id = OSA.getCurrentSession()?.id;
    if (!id) throw new Error('Open a chat to browse its workspace.');
    const query = new URLSearchParams({ path, root: String(root) });
    const response = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(id)}/files?${query}`, { signal });
    const data = await response.json();
    if (!response.ok || data.error) throw new Error(data.error || 'Could not read file.');
    return data;
};

OSA.loadBrowserFolder = async function(path) {
    const state = OSA._fileBrowser;
    const key = state.root + ':' + path;
    state.loading ||= new Set();
    if (state.loading.has(key)) return;
    state.loading.add(key);
    state.folderErrors ||= new Map(); state.folderErrors.delete(path);
    OSA.renderFileTree();
    try {
        const data = await OSA.browserRequest(path, state.root);
        if (state !== OSA._fileBrowser) return;
        if (data.kind !== 'directory') throw new Error('This path is not a folder.');
        state.roots = data.roots; state.workspace = data.workspace;
        state.folders.set(path, data); state.error = '';
    } catch (error) {
        if (state !== OSA._fileBrowser) return;
        state.folderErrors.set(path, error.message);
    } finally {
        state.loading.delete(key);
        if (state === OSA._fileBrowser) { OSA.renderFileTree(); OSA.renderBrowserRoots(); }
    }
};

OSA.openBrowserFile = async function(path) {
    const state = OSA._fileBrowser;
    state.controller?.abort();
    const controller = state.controller = new AbortController();
    const root = state.root;
    const status = document.getElementById('file-preview-status');
    if (status) status.textContent = 'Loading…';
    try {
        const data = await OSA.browserRequest(path, root, controller.signal);
        if (state !== OSA._fileBrowser || controller !== state.controller) return;
        OSA.openFilePreview(data.path, data.content, { root, source: 'workspace' });
    } catch (error) {
        if (error.name === 'AbortError' || state !== OSA._fileBrowser || controller !== state.controller) return;
        if (status) status.textContent = error.message;
    }
};

OSA.refreshFileBrowser = function() {
    const state = OSA._fileBrowser;
    state.folders.clear(); state.folderErrors?.clear();
    state.expanded.forEach(path => OSA.loadBrowserFolder(path));
    OSA.renderFileBrowser();
};

OSA.selectFileBrowserRoot = function(root) {
    const state = OSA._fileBrowser;
    if (root === state.root) return;
    state.controller?.abort();
    OSA._fileBrowser = { ...state, generation: state.generation + 1, root, folders: new Map(), expanded: new Set(['']), loading: new Set(), folderErrors: new Map() };
    OSA.loadBrowserFolder(''); OSA.renderFileBrowser();
};

OSA.setFileBrowserMode = function(mode) {
    OSA._fileBrowser.mode = mode === 'changes' ? 'changes' : 'files';
    OSA.renderFileTree();
};

OSA.browserChanges = function() {
    const changes = new Map();
    const tools = [...(OSA.getSessionToolEvents?.() || []), ...(OSA.TModel?.items || []).filter(item => item.kind === 'tool').map(item => OSA.tmodelToolEventView(item))];
    const seen = new Set();
    tools.forEach(tool => {
        if (tool.success !== true || !['write_file', 'edit_file', 'apply_patch'].includes(tool.tool_name)) return;
        if (seen.has(tool.tool_call_id)) return;
        seen.add(tool.tool_call_id);
        (OSA.getToolDiffFiles?.(tool) || []).forEach(file => {
            if (!file.path) return;
            const prior = changes.get(file.path);
            changes.set(file.path, { ...file, old_content: prior?.old_content ?? file.old_content ?? '', new_content: file.new_content ?? '', path: file.path });
        });
    });
    return Array.from(changes.values());
};

OSA.renderBrowserRoots = function() {
    const select = document.getElementById('file-browser-root');
    if (!select) return;
    const state = OSA._fileBrowser;
    const signature = JSON.stringify([state.roots, state.root]);
    if (select.dataset.signature === signature) return;
    select.dataset.signature = signature;
    select.replaceChildren(...state.roots.map((root, index) => {
        const option = OSA.browserNode('option', '', root.split(/[\\/]/).filter(Boolean).pop() || root);
        option.value = index; option.title = root; return option;
    }));
    select.value = state.root; select.hidden = state.roots.length < 2;
    const heading = document.querySelector('.file-browser-heading');
    if (heading) heading.textContent = state.workspace || 'Workspace';
};

OSA.renderFileTree = function() {
    const tree = document.getElementById('file-browser-tree');
    if (!tree) return;
    const state = OSA._fileBrowser;
    const changes = OSA.browserChanges();
    const query = (document.getElementById('file-browser-filter')?.value || '').toLowerCase();
    document.getElementById('browser-change-count').textContent = changes.length || '';
    ['files', 'changes'].forEach(mode => document.getElementById('browser-' + mode + '-tab')?.setAttribute('aria-pressed', String(state.mode === mode)));
    const nodes = [];
    const active = state.tabs.get(state.selected);
    const row = (path, name, depth, folder, onClick, changed) => {
        const button = OSA.browserNode('button', 'file-tree-row' + (active?.path === path && active.root === state.root ? ' selected' : ''));
        button.type = 'button'; button.title = path; button.style.setProperty('--depth', depth);
        button.append(OSA.browserNode('span', 'file-tree-icon', folder ? state.expanded.has(path) ? '▾' : '▸' : changed ? 'M' : '·'), OSA.browserNode('span', 'file-tree-name', name));
        if (folder) button.setAttribute('aria-expanded', String(state.expanded.has(path)));
        button.addEventListener('click', onClick); nodes.push(button);
    };
    if (state.mode === 'changes') {
        changes.filter(file => file.path.toLowerCase().includes(query)).forEach(file => row(file.path, file.path.replace(/\\/g,'/').split('/').pop(), 0, false, () => OSA.showFilePreviewFromDiff(file), true));
        if (!nodes.length) nodes.push(OSA.browserNode('div', 'file-tree-note', changes.length ? 'No matching changes.' : 'No recorded edits in this chat.'));
    } else {
        const visit = (path, depth) => {
            const listing = state.folders.get(path);
            if (state.loading?.has(state.root + ':' + path)) nodes.push(OSA.browserNode('div', 'file-tree-note', 'Loading…'));
            const error = state.folderErrors?.get(path);
            if (error) {
                const retry = OSA.browserNode('button', 'file-tree-note', error + ' · Retry');
                retry.type = 'button'; retry.addEventListener('click', () => OSA.loadBrowserFolder(path)); nodes.push(retry);
            }
            (listing?.entries || []).forEach(entry => {
                if (query && !entry.directory && !entry.path.toLowerCase().includes(query)) return;
                row(entry.path, entry.name, depth, entry.directory, () => {
                    if (!entry.directory) return OSA.openBrowserFile(entry.path);
                    if (state.expanded.has(entry.path)) state.expanded.delete(entry.path);
                    else { state.expanded.add(entry.path); if (!state.folders.has(entry.path)) OSA.loadBrowserFolder(entry.path); }
                    OSA.renderFileTree();
                }, changes.some(file => file.path.replace(/\\/g,'/').endsWith('/' + entry.path) || file.path === entry.path));
                if (entry.directory && state.expanded.has(entry.path)) visit(entry.path, depth + 1);
            });
            if (listing?.truncated) nodes.push(OSA.browserNode('div', 'file-tree-note', 'Showing the first 2,000 entries.'));
            if (listing && !listing.entries.length) nodes.push(OSA.browserNode('div', 'file-tree-note', 'Empty folder'));
        };
        visit('', 0);
    }
    tree.replaceChildren(...nodes);
};

OSA.renderFileBrowser = function() {
    OSA.renderBrowserRoots(); OSA.renderFileTree();
    const state = OSA._fileBrowser;
    const tabs = document.getElementById('file-preview-tabs');
    const body = document.getElementById('file-preview-body');
    if (!tabs || !body) return;
    tabs.replaceChildren(...Array.from(state.tabs.values()).map(tab => {
        const item = OSA.browserNode('div', 'file-preview-tab' + (state.selected === tab.key ? ' selected' : ''));
        const select = OSA.browserNode('button', '', tab.path.replace(/\\/g, '/').split('/').pop() || tab.path);
        select.type = 'button'; select.title = tab.path; select.setAttribute('aria-pressed', String(state.selected === tab.key));
        select.addEventListener('click', () => { state.controller?.abort(); state.controller = null; state.selected = tab.key; OSA.renderFileBrowser(); });
        const close = OSA.browserNode('button', 'file-tab-close', '×'); close.type = 'button'; close.setAttribute('aria-label', 'Close ' + tab.path);
        close.addEventListener('click', () => { state.tabs.delete(tab.key); if (state.selected === tab.key) state.selected = Array.from(state.tabs.keys()).pop(); OSA.renderFileBrowser(); });
        item.append(select, close); return item;
    }));
    const tab = state.tabs.get(state.selected);
    const path = document.getElementById('file-preview-path');
    const modes = document.getElementById('file-preview-modes');
    path.textContent = tab?.path || 'Select a file'; path.title = tab?.path || '';
    modes.replaceChildren();
    if (!tab) { body._previewSignature = null; body.onscroll = null; body.replaceChildren(OSA.browserNode('div','file-browser-empty','Open a file from the tree or review a change from chat.')); document.getElementById('file-preview-status').textContent = ''; return; }
    const language = OSA.guessLanguageFromPath(tab.path);
    const available = ['source', ...(language === 'markdown' ? ['preview'] : []), ...(tab.diff ? ['diff'] : [])];
    available.forEach(mode => {
        const button = OSA.browserNode('button', '', mode[0].toUpperCase() + mode.slice(1)); button.type = 'button';
        button.setAttribute('aria-pressed', String(tab.mode === mode));
        button.addEventListener('click', () => { tab.mode = mode; OSA.renderFileBrowser(); }); modes.append(button);
    });
    const copy = OSA.browserNode('button', '', 'Copy'); copy.type = 'button'; copy.setAttribute('aria-label', 'Copy file contents');
    copy.addEventListener('click', async () => {
        try { await navigator.clipboard.writeText(tab.content); copy.textContent = 'Copied'; }
        catch (_) { document.getElementById('file-preview-status').textContent = 'Could not copy file contents.'; }
    });
    modes.append(copy);
    const signature = JSON.stringify([tab.key, tab.mode, tab.content, tab.oldContent, tab.newContent]);
    if (body._previewSignature !== signature) {
        body.onscroll = null;
        body._previewSignature = signature;
        if (tab.mode === 'diff') body.replaceChildren(OSA.renderDiffView(tab.oldContent, tab.newContent));
        else if (tab.mode === 'preview') body.innerHTML = OSA.renderFilePreviewBody(tab.path, tab.content, 'markdown');
        else {
            const source = OSA.browserNode('div', 'file-source');
            const gutter = OSA.browserNode('pre', 'file-source-gutter', tab.content.split('\n').map((_,i) => i + 1).join('\n')); gutter.setAttribute('aria-hidden', 'true');
            const pre = OSA.browserNode('pre', 'file-source-code'); const code = OSA.browserNode('code');
            if (language && OSA.highlightCode) code.innerHTML = OSA.highlightCode(tab.content, language); else code.textContent = tab.content;
            pre.append(code); source.append(gutter, pre); body.replaceChildren(source);
        }
        const scroll = tab.scroll?.[tab.mode] || {top: 0, left: 0};
        body.scrollTop = scroll.top; body.scrollLeft = scroll.left;
    }
    body.onscroll = () => {
        tab.scroll ||= {};
        tab.scroll[tab.mode] = {top: body.scrollTop, left: body.scrollLeft};
    };
    document.getElementById('file-preview-status').textContent = `${tab.source === 'tool' ? 'Tool snapshot' : 'Workspace file'} · ${tab.content.split('\n').length} lines${tab.diff ? ' · Recorded change' : ''}`;
};

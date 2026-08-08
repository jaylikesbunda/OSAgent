window.OSA = window.OSA || {};

// MCP settings pane.
//
// The point of the deferred catalog is that connecting a server costs
// almost no context, so adding one should feel cheap here too: paste a
// command, test it, save. Everything else (which tools exist, which are
// loaded) is read-only detail the user can look at but never has to.
OSA.McpUI = {
    status: null,
    editing: null,
    testResult: null,
    busy: false,

    // Escape for an HTML *attribute*.
    //
    // OSA.escapeHtml round-trips through textContent → innerHTML, which
    // escapes &, < and > but not quotes. Interpolating that into
    // value="…" lets a quoted argument close the attribute early, so the
    // field silently renders empty and the user's input is lost on the
    // next re-render.
    attr(value) {
        return String(value === null || value === undefined ? '' : value)
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#39;');
    },

    async init() {
        const existing = document.getElementById('pane-mcp');
        if (existing) return existing;

        // The pane is built on first open, which is after
        // switchSettingsTab has already toggled `.active` across the
        // panes that existed at the time. Mark it active here or the
        // tab appears blank until the user clicks away and back.
        const pane = document.createElement('div');
        pane.className = 'settings-pane';
        pane.id = 'pane-mcp';
        pane.innerHTML = `
            <div class="settings-pane-header">
                <h2>MCP Servers</h2>
                <p class="settings-pane-desc">
                    Connect Model Context Protocol servers to give the agent more tools.
                    Their schemas stay out of the conversation until the agent searches for
                    them, so adding servers costs almost no context.
                </p>
            </div>
            <div class="mcp-container">
                <div id="mcp-message" class="settings-error hidden"></div>
                <div id="mcp-summary" class="mcp-summary"></div>
                <div id="mcp-server-list"><div class="loading-placeholder">Loading…</div></div>
                <div id="mcp-editor"></div>
            </div>
        `;

        const main = document.querySelector('.settings-main');
        if (main) main.appendChild(pane);
        pane.classList.add('active');
        return pane;
    },

    async load() {
        await this.init();
        try {
            this.status = await OSA.getJson('/api/mcp/servers');
        } catch (error) {
            this.showMessage(error.message || 'Failed to load MCP servers', 'error');
            this.status = { servers: [] };
        }
        this.render();
    },

    render() {
        this.renderSummary();
        this.renderList();
        this.renderEditor();
    },

    renderSummary() {
        const el = document.getElementById('mcp-summary');
        if (!el || !this.status) return;

        const servers = this.status.servers || [];
        const connected = servers.filter(s => s.connected).length;
        const activated = (this.status.activated || []).length;

        el.innerHTML = `
            <div class="mcp-summary-stats">
                <span><strong>${connected}</strong>/${servers.length} connected</span>
                <span><strong>${this.status.catalog_size || 0}</strong> tools available</span>
                <span><strong>${activated}</strong> currently loaded into context</span>
            </div>
            <div class="mcp-summary-actions">
                <button class="btn-ghost" onclick="OSA.McpUI.reload()">Reconnect all</button>
                <button class="btn-action" onclick="OSA.McpUI.startAdd()">Add server</button>
            </div>
        `;
    },

    renderList() {
        const el = document.getElementById('mcp-server-list');
        if (!el || !this.status) return;

        const servers = this.status.servers || [];
        if (servers.length === 0) {
            el.innerHTML = `
                <div class="mcp-empty">
                    <p>No MCP servers configured.</p>
                    <p class="mcp-empty-hint">
                        Most servers run as a local command, for example
                        <code>npx -y @modelcontextprotocol/server-filesystem ~/notes</code>.
                    </p>
                </div>
            `;
            return;
        }

        el.innerHTML = servers.map(server => {
            const state = !server.enabled
                ? { cls: 'off', label: 'Disabled' }
                : server.connected
                    ? { cls: 'ok', label: `${server.tool_count} tools` }
                    : { cls: 'error', label: 'Not connected' };

            const target = server.transport === 'http'
                ? server.url
                : [server.command].concat(server.args || []).join(' ');

            return `
                <div class="mcp-server-card ${server.connected ? '' : 'is-inactive'}">
                    <div class="mcp-server-head">
                        <div>
                            <div class="mcp-server-name">
                                ${OSA.escapeHtml(server.name)}
                                <span class="mcp-badge mcp-badge-${state.cls}">${state.label}</span>
                            </div>
                            <div class="mcp-server-target"><code>${OSA.escapeHtml(target || '')}</code></div>
                            ${server.blurb ? `<div class="mcp-server-blurb">${OSA.escapeHtml(server.blurb)}</div>` : ''}
                        </div>
                        <div class="mcp-server-actions">
                            <label class="mcp-toggle">
                                <input type="checkbox" ${server.enabled ? 'checked' : ''}
                                       onchange="OSA.McpUI.toggle('${this.attr(server.name)}', this.checked)">
                                <span>Enabled</span>
                            </label>
                            <button class="btn-ghost" onclick="OSA.McpUI.startEdit('${this.attr(server.name)}')">Edit</button>
                            <button class="btn-ghost btn-danger" onclick="OSA.McpUI.remove('${this.attr(server.name)}')">Remove</button>
                        </div>
                    </div>
                    ${server.error ? `<div class="mcp-server-error">${OSA.escapeHtml(server.error)}</div>` : ''}
                    ${server.connected && server.tool_count > 0
                        ? `<button class="mcp-link" onclick="OSA.McpUI.showTools('${this.attr(server.name)}')">View tools</button>
                           <div id="mcp-tools-${this.attr(server.name)}" class="mcp-tool-list hidden"></div>`
                        : ''}
                </div>
            `;
        }).join('');
    },

    startAdd() {
        this.editing = {
            isNew: true,
            name: '',
            transport: 'stdio',
            command: '',
            args: '',
            url: '',
            env: '',
            headers: '',
            description: '',
            timeout_seconds: 60,
            always_active: ''
        };
        this.testResult = null;
        this.render();
    },

    startEdit(name) {
        const server = (this.status.servers || []).find(s => s.name === name);
        if (!server) return;

        this.editing = {
            isNew: false,
            name: server.name,
            transport: server.transport || 'stdio',
            command: server.command || '',
            args: (server.args || []).join(' '),
            url: server.url || '',
            env: Object.entries(server.env || {}).map(([k, v]) => `${k}=${v}`).join('\n'),
            // Header *values* are never sent to the browser (they carry
            // tokens); only the keys come back, so editing headers means
            // re-entering them.
            headers: (server.headers_keys || []).map(k => `${k}=`).join('\n'),
            description: server.description || '',
            timeout_seconds: server.timeout_seconds || 60,
            always_active: (server.always_active || []).join(', ')
        };
        this.testResult = null;
        this.render();
    },

    cancelEdit() {
        this.editing = null;
        this.testResult = null;
        this.render();
    },

    renderEditor() {
        const el = document.getElementById('mcp-editor');
        if (!el) return;

        if (!this.editing) {
            el.innerHTML = '';
            return;
        }

        const e = this.editing;
        const isHttp = e.transport === 'http';

        el.innerHTML = `
            <div class="mcp-editor-card">
                <h3>${e.isNew ? 'Add MCP server' : `Edit ${OSA.escapeHtml(e.name)}`}</h3>

                <label class="settings-label">Name</label>
                <input class="settings-input" id="mcp-f-name" value="${this.attr(e.name)}"
                       ${e.isNew ? '' : 'disabled'} placeholder="linear">
                <div class="settings-hint">Letters, numbers, dashes and underscores. Used to namespace its tools.</div>

                <label class="settings-label">Transport</label>
                <select class="settings-input" id="mcp-f-transport" onchange="OSA.McpUI.setField('transport', this.value)">
                    <option value="stdio" ${isHttp ? '' : 'selected'}>Local command (stdio)</option>
                    <option value="http" ${isHttp ? 'selected' : ''}>Remote (HTTP)</option>
                </select>

                ${isHttp ? `
                    <label class="settings-label">URL</label>
                    <input class="settings-input" id="mcp-f-url" value="${this.attr(e.url)}"
                           placeholder="https://example.com/mcp">

                    <label class="settings-label">Headers</label>
                    <textarea class="settings-input" id="mcp-f-headers" rows="3"
                              placeholder="Authorization=Bearer sk-...">${OSA.escapeHtml(e.headers)}</textarea>
                    <div class="settings-hint">One <code>Key=Value</code> per line. Stored in your config file.</div>
                ` : `
                    <label class="settings-label">Command</label>
                    <input class="settings-input" id="mcp-f-command" value="${this.attr(e.command)}"
                           placeholder="npx">

                    <label class="settings-label">Arguments</label>
                    <input class="settings-input" id="mcp-f-args" value="${this.attr(e.args)}"
                           placeholder="-y @modelcontextprotocol/server-filesystem ~/notes">
                    <div class="settings-hint">Space separated. Quote arguments containing spaces.</div>

                    <label class="settings-label">Environment</label>
                    <textarea class="settings-input" id="mcp-f-env" rows="3"
                              placeholder="API_KEY=...">${OSA.escapeHtml(e.env)}</textarea>
                    <div class="settings-hint">One <code>KEY=value</code> per line.</div>
                `}

                <label class="settings-label">Description <span class="settings-optional">(optional)</span></label>
                <input class="settings-input" id="mcp-f-description" value="${this.attr(e.description)}"
                       placeholder="Issue tracking for the core team">
                <div class="settings-hint">
                    One line telling the agent what this server is for. This is the only thing
                    about the server that is always in context, so it is worth writing.
                </div>

                <label class="settings-label">Always load these tools <span class="settings-optional">(optional)</span></label>
                <input class="settings-input" id="mcp-f-always" value="${this.attr(e.always_active)}"
                       placeholder="create_issue, list_issues">
                <div class="settings-hint">
                    Comma separated tool names, loaded at startup so the agent skips the search
                    step. Each one costs context in every request — keep the list short.
                </div>

                <label class="settings-label">Timeout (seconds)</label>
                <input class="settings-input" id="mcp-f-timeout" type="number" min="1" max="600"
                       value="${e.timeout_seconds}">

                ${this.testResult ? this.renderTestResult() : ''}

                <div class="mcp-editor-actions">
                    <button class="btn-ghost" onclick="OSA.McpUI.cancelEdit()">Cancel</button>
                    <button class="btn-ghost" onclick="OSA.McpUI.test()" ${this.busy ? 'disabled' : ''}>Test connection</button>
                    <button class="btn-action" onclick="OSA.McpUI.save()" ${this.busy ? 'disabled' : ''}>Save</button>
                </div>
            </div>
        `;
    },

    renderTestResult() {
        const result = this.testResult;
        if (result.connected) {
            const sample = (result.sample_tools || []).slice(0, 6).join(', ');
            return `
                <div class="mcp-test-result mcp-test-ok">
                    Connected — ${result.tool_count} tool(s) found.
                    ${sample ? `<br><span class="mcp-test-sample">${OSA.escapeHtml(sample)}</span>` : ''}
                </div>
            `;
        }
        return `
            <div class="mcp-test-result mcp-test-error">
                Could not connect: ${OSA.escapeHtml(result.error || 'unknown error')}
            </div>
        `;
    },

    setField(field, value) {
        this.collectForm();
        this.editing[field] = value;
        this.renderEditor();
    },

    // Read the DOM back into `editing` so a re-render (e.g. switching
    // transport) does not discard what was already typed.
    collectForm() {
        if (!this.editing) return;
        const read = (id, fallback) => {
            const el = document.getElementById(id);
            return el ? el.value : fallback;
        };
        const e = this.editing;
        e.name = read('mcp-f-name', e.name).trim();
        e.command = read('mcp-f-command', e.command);
        e.args = read('mcp-f-args', e.args);
        e.url = read('mcp-f-url', e.url);
        e.env = read('mcp-f-env', e.env);
        e.headers = read('mcp-f-headers', e.headers);
        e.description = read('mcp-f-description', e.description);
        e.always_active = read('mcp-f-always', e.always_active);
        e.timeout_seconds = parseInt(read('mcp-f-timeout', e.timeout_seconds), 10) || 60;
    },

    buildPayload() {
        this.collectForm();
        const e = this.editing || {};

        const payload = {
            name: e.name,
            enabled: true,
            transport: e.transport,
            description: e.description || null,
            timeout_seconds: e.timeout_seconds,
            always_active: this.splitList(e.always_active)
        };

        if (e.transport === 'http') {
            payload.url = e.url;
            payload.headers = this.parsePairs(e.headers);
        } else {
            payload.command = e.command;
            payload.args = this.splitArgs(e.args);
            payload.env = this.parsePairs(e.env);
        }
        return payload;
    },

    splitList(text) {
        return (text || '').split(',').map(s => s.trim()).filter(Boolean);
    },

    // Split on whitespace but keep quoted arguments intact, so a path
    // with spaces survives the round trip.
    splitArgs(text) {
        const matches = (text || '').match(/"[^"]*"|'[^']*'|\S+/g) || [];
        return matches.map(token => token.replace(/^["']|["']$/g, ''));
    },

    parsePairs(text) {
        const pairs = {};
        (text || '').split('\n').forEach(line => {
            const trimmed = line.trim();
            if (!trimmed) return;
            const index = trimmed.indexOf('=');
            if (index <= 0) return;
            const key = trimmed.slice(0, index).trim();
            const value = trimmed.slice(index + 1).trim();
            // An empty value means "keep whatever is already saved" for
            // an existing header, so don't overwrite it with "".
            if (key && value) pairs[key] = value;
        });
        return pairs;
    },

    async test() {
        if (this.busy || !this.editing) return;

        // Read the form before anything re-renders it: renderEditor
        // rebuilds the inputs from `this.editing`, so collecting after
        // it would send whatever was there before the user typed.
        const payload = this.buildPayload();

        // Everything after `busy = true` sits inside the try, including
        // the re-render: if any of it throws, the finally is the only
        // thing that clears the flag, and a latched flag leaves Test and
        // Save disabled until the page is reloaded.
        this.busy = true;
        try {
            this.renderEditor();
            this.testResult = await OSA.postJson('/api/mcp/test', payload);
            if (this.testResult.error && this.testResult.connected === undefined) {
                this.testResult = { connected: false, error: this.testResult.error };
            }
        } catch (error) {
            this.testResult = { connected: false, error: error.message || 'Request failed' };
        } finally {
            this.busy = false;
            this.renderEditor();
        }
    },

    async save() {
        if (this.busy || !this.editing) return;
        const payload = this.buildPayload();
        if (!payload.name) {
            this.showMessage('A server name is required', 'error');
            return;
        }

        this.busy = true;
        try {
            this.renderEditor();
            const response = await OSA.postJson('/api/mcp/servers', payload);
            if (response.error) throw new Error(response.error);

            this.editing = null;
            this.testResult = null;
            this.status = response.status || this.status;

            const saved = (this.status.servers || []).find(s => s.name === payload.name);
            if (saved && !saved.connected && saved.enabled) {
                this.showMessage(
                    `Saved, but '${payload.name}' did not connect: ${saved.error || 'unknown error'}`,
                    'error'
                );
            } else {
                this.showMessage(`Saved '${payload.name}'`, 'success');
            }
        } catch (error) {
            this.showMessage(error.message || 'Failed to save server', 'error');
        } finally {
            this.busy = false;
            this.render();
        }
    },

    async remove(name) {
        if (!confirm(`Remove MCP server '${name}'? Its tools become unavailable to the agent.`)) {
            return;
        }
        try {
            const response = await OSA.deleteJson(`/api/mcp/servers/${encodeURIComponent(name)}`);
            if (response.error) throw new Error(response.error);
            this.showMessage(`Removed '${name}'`, 'success');
            await this.load();
        } catch (error) {
            this.showMessage(error.message || 'Failed to remove server', 'error');
        }
    },

    async toggle(name, enabled) {
        try {
            const response = await OSA.postJson(
                `/api/mcp/servers/${encodeURIComponent(name)}/enabled`,
                { enabled }
            );
            if (response.error) throw new Error(response.error);
            this.status = response.status || this.status;
            this.render();
        } catch (error) {
            this.showMessage(error.message || 'Failed to update server', 'error');
            await this.load();
        }
    },

    async reload() {
        this.showMessage('Reconnecting…', 'info');
        try {
            const response = await OSA.postJson('/api/mcp/reload', {});
            if (response.error) throw new Error(response.error);
            this.status = response.status || this.status;
            this.render();
            this.showMessage('Reconnected', 'success');
        } catch (error) {
            this.showMessage(error.message || 'Reconnect failed', 'error');
        }
    },

    async showTools(name) {
        const el = document.getElementById(`mcp-tools-${name}`);
        if (!el) return;

        if (!el.classList.contains('hidden')) {
            el.classList.add('hidden');
            return;
        }

        el.classList.remove('hidden');
        el.innerHTML = '<div class="loading-placeholder">Loading…</div>';
        try {
            const data = await OSA.getJson(`/api/mcp/catalog?server=${encodeURIComponent(name)}`);
            const tools = data.tools || [];
            el.innerHTML = tools.map(tool => `
                <div class="mcp-tool-row">
                    <code>${OSA.escapeHtml(tool.tool)}</code>
                    ${tool.read_only ? '<span class="mcp-badge mcp-badge-ok">read-only</span>' : ''}
                    ${tool.activated ? '<span class="mcp-badge mcp-badge-loaded">loaded</span>' : ''}
                    <span class="mcp-tool-desc">${OSA.escapeHtml((tool.description || '').slice(0, 140))}</span>
                </div>
            `).join('') || '<div class="mcp-empty-hint">No tools reported.</div>';
        } catch (error) {
            el.innerHTML = `<div class="mcp-server-error">${OSA.escapeHtml(error.message || 'Failed to load tools')}</div>`;
        }
    },

    showMessage(text, kind) {
        const el = document.getElementById('mcp-message');
        if (!el) return;
        el.textContent = text;
        el.className = `settings-error mcp-message mcp-message-${kind || 'info'}`;
        el.classList.remove('hidden');
        if (kind === 'success' || kind === 'info') {
            setTimeout(() => el.classList.add('hidden'), 4000);
        }
    }
};

OSA.loadMcpUI = async function() {
    await OSA.McpUI.load();
};

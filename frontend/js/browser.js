window.OSA = window.OSA || {};

// Browser settings pane: how the agent's sandboxed browser behaves, and
// which of the user's own signed-in sessions it may borrow.
//
// Sessions are imported per domain. The scan step only shows domain names
// and cookie counts; cookie values stay on the server and are never sent to
// this page or to the model.
OSA.BrowserUI = {
    profiles: [],
    scan: null,
    jar: null,
    busy: false,

    esc(value) {
        return String(value === null || value === undefined ? '' : value)
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;')
            .replace(/'/g, '&#39;');
    },

    toggle(id, label, desc, checked) {
        return `
            <div class="toggle-field">
                <div class="toggle-info">
                    <label class="toggle-label">${label}</label>
                    <span class="toggle-desc">${desc}</span>
                </div>
                <label class="toggle-switch">
                    <input type="checkbox" id="${id}" ${checked ? 'checked' : ''} />
                    <span class="toggle-slider"></span>
                </label>
            </div>`;
    },

    init() {
        let pane = document.getElementById('pane-browser');
        if (pane) return pane;
        pane = document.createElement('div');
        pane.className = 'settings-pane';
        pane.id = 'pane-browser';
        const main = document.querySelector('.settings-main');
        if (main) main.appendChild(pane);
        pane.classList.add('active');
        return pane;
    },

    async load() {
        const pane = this.init();
        const cfg = (OSA.getCachedConfig && OSA.getCachedConfig()?.browser) || {};
        const v = (key, fallback) => (cfg[key] === undefined || cfg[key] === null ? fallback : cfg[key]);
        const lines = (list) => this.esc((list || []).join('\n'));

        pane.innerHTML = `
            <div class="settings-pane-header">
                <h2>Browser</h2>
                <p class="settings-pane-desc">
                    A sandboxed Chromium the agent can drive when a page needs JavaScript, clicking or logging in.
                    It runs in its own throwaway profile and cannot reach local or private network addresses.
                    Changes apply after OSA restarts.
                </p>
            </div>
            <div id="browser-message" class="settings-error hidden"></div>
            <div class="settings-group">
                <div class="settings-group-title">Behaviour</div>
                ${this.toggle('setting-browser-enabled', 'Enable browser tool', 'Lets the agent discover and use the browser through tool search', v('enabled', true))}
                ${this.toggle('setting-browser-block-private', 'Block local and private networks', 'Refuse requests to localhost, LAN and link-local addresses (recommended)', v('block_private_network', true))}
                ${this.toggle('setting-browser-eval', 'Allow JavaScript evaluation', 'Lets the agent run arbitrary scripts inside pages. Leave off unless you need it', v('allow_javascript_eval', false))}
                ${this.toggle('setting-browser-import-cookies', 'Use imported sessions', 'Start each agent browser signed in to the accounts imported below', v('import_cookies', true))}
                <div class="settings-field">
                    <label for="setting-browser-executable">Browser executable</label>
                    <input type="text" id="setting-browser-executable" value="${this.esc(v('executable_path', ''))}" placeholder="Auto-detect Chrome, Edge, Brave or Chromium" />
                </div>
                <div class="settings-field">
                    <label for="setting-browser-idle">Close idle browser after (seconds)</label>
                    <input type="number" min="30" id="setting-browser-idle" value="${this.esc(v('idle_timeout_seconds', 300))}" />
                </div>
            </div>
            <div class="settings-group">
                <div class="settings-group-title">Site rules</div>
                <div class="settings-field">
                    <label for="setting-browser-allowed">Allowed hosts (one per line, empty allows all public hosts)</label>
                    <textarea id="setting-browser-allowed" rows="3" placeholder="docs.rs&#10;*.github.com">${lines(cfg.allowed_hosts)}</textarea>
                </div>
                <div class="settings-field">
                    <label for="setting-browser-blocked">Blocked hosts (one per line)</label>
                    <textarea id="setting-browser-blocked" rows="3">${lines(cfg.blocked_hosts)}</textarea>
                </div>
            </div>
            <div class="settings-group">
                <div class="settings-group-title">Signed-in sessions</div>
                <p class="settings-pane-desc">
                    Copy logins from a browser installed on this computer so the agent can act as you on selected sites.
                    Anything the agent can reach while signed in, it can change: import only the sites you need, and
                    consider limiting Allowed hosts above. Close the source browser first.
                </p>
                <div class="settings-field">
                    <label for="browser-import-profile">Browser profile</label>
                    <select id="browser-import-profile"></select>
                </div>
                <div class="settings-actions">
                    <button class="btn-action" id="browser-scan-btn" onclick="OSA.BrowserUI.scanProfile()">Scan for sessions</button>
                </div>
                <div id="browser-scan-result"></div>
                <div class="settings-group-title" style="margin-top:16px">Imported</div>
                <div id="browser-jar"><div class="loading-placeholder">Loading…</div></div>
            </div>`;
        await this.refresh();
    },

    async refresh() {
        try {
            const data = await OSA.getJson('/api/browser/profiles');
            this.profiles = data.profiles || [];
            this.jar = data.jar || null;
        } catch (error) {
            this.message(error.message || 'Failed to load browser profiles');
            this.profiles = [];
        }
        const select = document.getElementById('browser-import-profile');
        if (select) {
            select.innerHTML = this.profiles.length
                ? this.profiles.map(p => `<option value="${this.esc(p.id)}">${this.esc(p.browser_label)} — ${this.esc(p.name)}</option>`).join('')
                : '<option value="">No supported browsers found</option>';
        }
        const scanBtn = document.getElementById('browser-scan-btn');
        if (scanBtn) scanBtn.disabled = this.profiles.length === 0;
        this.renderJar();
    },

    message(text, ok) {
        const el = document.getElementById('browser-message');
        if (!el) return;
        el.textContent = text || '';
        el.classList.toggle('hidden', !text);
        el.style.color = ok ? 'var(--success, #3fb950)' : '';
    },

    renderJar() {
        const el = document.getElementById('browser-jar');
        if (!el) return;
        const jar = this.jar;
        if (!jar || !jar.total) {
            el.innerHTML = '<div class="model-empty">No sessions imported.</div>';
            return;
        }
        const rows = (jar.domains || []).map(d => `
            <div class="toggle-field">
                <div class="toggle-info">
                    <label class="toggle-label">${this.esc(d.domain)}</label>
                    <span class="toggle-desc">${d.count} cookie${d.count === 1 ? '' : 's'}</span>
                </div>
                <button class="btn-ghost" data-domain="${this.esc(d.domain)}" onclick="OSA.BrowserUI.removeDomain(this.dataset.domain)">Remove</button>
            </div>`).join('');
        el.innerHTML = `
            <p class="settings-pane-desc">${jar.total} cookies from ${(jar.sources || []).map(s => this.esc(s)).join(', ')}</p>
            ${rows}
            <div class="settings-actions">
                <button class="btn-ghost" onclick="OSA.BrowserUI.clearAll()">Remove all imported sessions</button>
            </div>`;
    },

    async scanProfile() {
        const id = document.getElementById('browser-import-profile')?.value;
        if (!id || this.busy) return;
        this.busy = true;
        this.message('');
        const out = document.getElementById('browser-scan-result');
        out.innerHTML = '<div class="loading-placeholder">Reading cookies…</div>';
        try {
            this.scan = await OSA.postJson('/api/browser/import/scan', { profile_id: id });
            this.renderScan(id);
        } catch (error) {
            out.innerHTML = '';
            this.message(error.message || 'Scan failed');
        } finally {
            this.busy = false;
        }
    },

    renderScan(profileId) {
        const out = document.getElementById('browser-scan-result');
        const scan = this.scan;
        if (!scan) return;
        const skipped = Object.entries(scan.skipped || {})
            .map(([reason, n]) => `${n} skipped: ${this.esc(reason)}`).join('; ');
        if (!scan.domains.length) {
            out.innerHTML = `<div class="model-empty">No importable cookies found.${skipped ? ' ' + skipped : ''}</div>`;
            return;
        }
        const rows = scan.domains.map((d, i) => `
            <label class="browser-domain-row" style="display:flex;gap:8px;align-items:center;padding:3px 0">
                <input type="checkbox" class="browser-domain-pick" data-domain="${this.esc(d.domain)}" />
                <span style="flex:1">${this.esc(d.domain)}</span>
                <span class="toggle-desc">${d.count}</span>
            </label>`).join('');
        out.innerHTML = `
            <p class="settings-pane-desc">${scan.total} cookies on ${scan.domains.length} sites.${skipped ? ' ' + skipped + '.' : ''} Choose the sites to share with the agent:</p>
            <div class="settings-field">
                <input type="text" id="browser-domain-filter" placeholder="Filter sites…" oninput="OSA.BrowserUI.filterDomains(this.value)" />
            </div>
            <div id="browser-domain-list" style="max-height:260px;overflow:auto">${rows}</div>
            <div class="settings-actions">
                <button class="btn-action" onclick="OSA.BrowserUI.importSelected('${this.esc(profileId)}')">Import selected</button>
            </div>`;
    },

    filterDomains(query) {
        const q = (query || '').trim().toLowerCase();
        document.querySelectorAll('.browser-domain-row').forEach(row => {
            const domain = row.querySelector('input').dataset.domain;
            row.style.display = !q || domain.includes(q) ? 'flex' : 'none';
        });
    },

    async importSelected(profileId) {
        const domains = Array.from(document.querySelectorAll('.browser-domain-pick:checked'))
            .map(el => el.dataset.domain);
        if (!domains.length) {
            this.message('Select at least one site to import.');
            return;
        }
        if (this.busy) return;
        this.busy = true;
        try {
            const result = await OSA.postJson('/api/browser/import', { profile_id: profileId, domains });
            this.jar = result.jar;
            this.scan = null;
            document.getElementById('browser-scan-result').innerHTML = '';
            this.renderJar();
            const skipped = Object.entries(result.skipped || {})
                .map(([reason, n]) => `${n} skipped (${reason})`).join('; ');
            const note = result.imported
                ? `Imported ${result.imported} cookies${skipped ? ', ' + skipped : ''}. ${result.note || ''}`
                : `Nothing imported${skipped ? ': ' + skipped : ' — no cookies for the selected sites'}.`;
            this.message(note, !!result.imported);
        } catch (error) {
            this.message(error.message || 'Import failed');
        } finally {
            this.busy = false;
        }
    },

    async removeDomain(domain) {
        try {
            const result = await OSA.deleteJson(`/api/browser/cookies?domain=${encodeURIComponent(domain)}`);
            this.jar = result.jar;
            this.renderJar();
        } catch (error) {
            this.message(error.message || 'Remove failed');
        }
    },

    async clearAll() {
        if (!window.confirm('Remove every imported session from OSA? Your real browsers are not affected.')) return;
        try {
            const result = await OSA.deleteJson('/api/browser/cookies');
            this.jar = result.jar;
            this.renderJar();
        } catch (error) {
            this.message(error.message || 'Remove failed');
        }
    },

    // Merge the pane's fields into the config object being saved.
    collect(existing) {
        if (!document.getElementById('setting-browser-enabled')) return existing;
        const list = (id) => (document.getElementById(id).value || '')
            .split(/[\n,]/).map(s => s.trim()).filter(Boolean);
        return {
            ...(existing || {}),
            enabled: document.getElementById('setting-browser-enabled').checked,
            block_private_network: document.getElementById('setting-browser-block-private').checked,
            allow_javascript_eval: document.getElementById('setting-browser-eval').checked,
            import_cookies: document.getElementById('setting-browser-import-cookies').checked,
            executable_path: document.getElementById('setting-browser-executable').value.trim(),
            idle_timeout_seconds: parseInt(document.getElementById('setting-browser-idle').value) || 300,
            allowed_hosts: list('setting-browser-allowed'),
            blocked_hosts: list('setting-browser-blocked'),
        };
    },
};

OSA.loadBrowserUI = async function() {
    await OSA.BrowserUI.load();
};

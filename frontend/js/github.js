window.OSA = window.OSA || {};

// GitHub settings pane: connection (PAT or OAuth device flow), the
// CodeRabbit-style auto-reviewer, manual review runs, and history.
// Self-contained: talks only to /api/github/* so it never writes
// secrets through the global config save.
OSA.GithubUI = {
    initialized: false,
    watchRepos: [],
    allRepos: [],
    devicePollTimer: null,
    deviceFlowActive: false,
    devicePollInFlight: false,
    devicePollIntervalMs: 5000,
    devicePollFailures: 0,
    visibilityBound: false,
    reviewBusy: false,

    async init() {
        const existing = document.getElementById('pane-github');
        if (existing) return existing;
        const pane = document.createElement('div');
        pane.className = 'settings-pane';
        pane.id = 'pane-github';
        pane.innerHTML = this.buildPaneHTML();
        const settingsMain = document.querySelector('.settings-main');
        if (settingsMain) settingsMain.appendChild(pane);
        // The pane is built on first open, which is after switchSettingsTab
        // has already toggled `.active` across the panes that existed at the
        // time. Mark it active here or the tab stays blank until the user
        // clicks away and back.
        pane.classList.add('active');
        // The desktop shell cannot open external tabs from the webview
        // (window.open popups do not function there), so route GitHub
        // links through the server, which opens the system browser.
        pane.addEventListener('click', (event) => this.handlePaneLinkClick(event));
        await new Promise(resolve => setTimeout(resolve, 0));
        this.bindEvents();
        this.bindVisibilityPolling();
        this.initialized = true;
        return pane;
    },

    isNativeShell() {
        try {
            return typeof window.__TAURI__ !== 'undefined' && !!window.__TAURI__;
        } catch (error) {
            return false;
        }
    },

    async openExternal(url) {
        if (!url) return;
        if (this.isNativeShell()) {
            try {
                await this.fetchJson('/api/github/open', {
                    method: 'POST',
                    body: JSON.stringify({ url }),
                });
            } catch (error) {
                this.showMessage(error.message, 'error');
            }
            return;
        }
        window.open(url, '_blank', 'noopener');
    },

    handlePaneLinkClick(event) {
        if (!this.isNativeShell()) return;
        const anchor = event.target && event.target.closest ? event.target.closest('a[href]') : null;
        if (!anchor) return;
        const href = anchor.getAttribute('href') || '';
        if (!/^https:\/\/github\.com\//i.test(href)) return;
        event.preventDefault();
        this.openExternal(href);
    },

    buildPaneHTML() {
        return `
            <div class="settings-pane-header">
                <h2>GitHub</h2>
                <p class="settings-pane-desc">Automatic pull request reviews.</p>
            </div>
            <div class="github-container">
                <div class="github-message-slot"></div>

                <div class="github-card">
                    <div class="github-card-header">
                        <div>
                            <h3>Connection</h3>
                        </div>
                        <span class="github-status-badge" id="github-status-badge">Checking</span>
                    </div>

                    <div id="github-connected-view" class="github-connected hidden">
                        <div class="github-identity">
                            <img id="github-avatar" class="github-avatar" alt="" />
                            <div>
                                <div class="github-login" id="github-login"></div>
                                <div class="github-identity-meta" id="github-identity-meta"></div>
                            </div>
                        </div>
                        <button class="btn-ghost github-danger" id="github-disconnect-btn">Disconnect</button>
                    </div>

                    <div id="github-connect-view" class="github-connect hidden">
                        <div class="github-row">
                            <button class="btn-action" id="github-signin-btn">Sign in with GitHub</button>
                            <span class="github-hint" id="github-oauth-hint"></span>
                        </div>

                        <div id="github-device-flow" class="github-device-flow hidden">
                            <div>Authorize in the GitHub tab that just opened. The code is already filled in.</div>
                            <div class="github-device-code" id="github-device-code"></div>
                            <div class="github-row">
                                <a id="github-device-link" href="https://github.com/login/device" target="_blank" rel="noopener" class="btn-ghost">Open GitHub again</a>
                                <button class="btn-ghost" id="github-device-cancel">Cancel</button>
                            </div>
                            <div class="github-hint" id="github-device-status">Waiting for authorization</div>
                        </div>

                        <details class="github-details" id="github-token-details">
                            <summary>Other sign-in options</summary>
                            <div class="github-row">
                                <input type="password" id="github-token-input" class="github-input" placeholder="ghp_ or github_pat_" autocomplete="off" />
                                <button class="btn-ghost" id="github-save-token-btn">Save token</button>
                            </div>
                            <p class="github-hint">
                                Needs <code>repo</code> + <code>read:user</code>.
                                <a href="https://github.com/settings/tokens/new?scopes=repo%2Cread%3Auser&description=OSAgent" target="_blank" rel="noopener">Create a token</a>
                            </p>
                            <div class="github-row">
                                <input type="text" id="github-oauth-client-id" class="github-input" placeholder="Custom OAuth client ID (optional)" />
                                <button class="btn-ghost" id="github-save-oauth-btn">Save</button>
                            </div>
                        </details>
                    </div>

                    <div id="github-auth-error" class="github-auth-error hidden"></div>
                </div>

                <div class="github-card">
                    <div class="github-card-header">
                        <div>
                            <h3>Reviews</h3>
                        </div>
                    </div>

                    <div class="github-switch-list">
                        <label class="github-switch-row">
                            <input type="checkbox" id="github-auto-review" />
                            <span class="github-switch-label">On new commits</span>
                            <span class="github-hint">review open PRs when they change</span>
                        </label>
                        <label class="github-switch-row">
                            <input type="checkbox" id="github-trigger-mentions" />
                            <span class="github-switch-label">On @mention</span>
                            <input type="text" id="github-mention-handle" class="github-input github-input-inline" placeholder="osagent-review" />
                            <span class="github-hint">in a PR comment</span>
                        </label>
                    </div>

                    <div class="github-field">
                        <div class="github-field-label">
                            <span>Watched repositories</span>
                            <button class="github-link-btn" id="github-load-repos-btn">Browse my repositories</button>
                        </div>
                        <div class="github-chips" id="github-repo-chips"></div>
                        <div class="github-row">
                            <input type="text" id="github-repo-manual" class="github-input github-input-repo" placeholder="owner/repo" />
                            <button class="btn-ghost" id="github-add-manual-btn">Add</button>
                            <select id="github-repo-picker" class="github-input github-repo-picker hidden"></select>
                            <button class="btn-ghost hidden" id="github-add-repo-btn">Add selected</button>
                        </div>
                    </div>

                    <details class="github-details">
                        <summary>Advanced</summary>
                        <div class="github-grid">
                            <label>Poll interval (seconds)
                                <input type="number" id="github-poll-seconds" class="github-input" min="30" max="3600" step="10" />
                            </label>
                            <label>Review event
                                <select id="github-review-event" class="github-input">
                                    <option value="COMMENT">Comment</option>
                                    <option value="REQUEST_CHANGES">Request changes</option>
                                    <option value="APPROVE">Approve</option>
                                </select>
                            </label>
                            <label>Max inline comments
                                <input type="number" id="github-max-comments" class="github-input" min="1" max="100" />
                            </label>
                            <label>Review model
                                <select id="github-review-model" class="github-input">
                                    <option value="">Same as current model</option>
                                </select>
                            </label>
                            <label>Ignore authors
                                <input type="text" id="github-ignore-authors" class="github-input" placeholder="dependabot[bot], renovate[bot]" />
                            </label>
                            <label class="github-check">
                                <input type="checkbox" id="github-include-drafts" /> Review draft PRs
                            </label>
                        </div>
                        <label class="github-block">Custom instructions
                            <textarea id="github-review-instructions" class="github-input" rows="3" placeholder="e.g. We target Rust 1.78; never suggest rewriting tests."></textarea>
                        </label>
                        <p class="github-hint">
                            In a mention comment: <code>--dry-run</code> previews without posting,
                            <code>--approve</code> / <code>--request-changes</code> override the event for that run.
                        </p>
                    </details>

                    <div class="github-actions">
                        <span class="github-hint" id="github-poll-status"></span>
                        <button class="btn-action" id="github-save-config-btn">Save</button>
                    </div>
                </div>

                <details class="github-card github-collapse">
                    <summary><h3>Review a pull request now</h3></summary>
                    <div class="github-row">
                        <input type="text" id="github-run-repo" class="github-input github-input-repo" placeholder="owner/repo" />
                        <input type="number" id="github-run-number" class="github-input github-input-small" placeholder="PR #" min="1" />
                        <label class="github-check"><input type="checkbox" id="github-run-post" checked /> Post</label>
                        <button class="btn-action" id="github-run-review-btn">Review</button>
                    </div>
                    <div id="github-run-result" class="github-run-result hidden"></div>
                </details>

                <details class="github-card github-collapse">
                    <summary><h3>History</h3></summary>
                    <button class="btn-ghost" id="github-refresh-btn">Refresh</button>
                    <div id="github-history"><div class="github-hint">Loading</div></div>
                </details>
            </div>
        `;
    },

    bindEvents() {
        const $ = (id) => document.getElementById(id);
        $('github-save-token-btn')?.addEventListener('click', () => this.saveToken());
        $('github-token-input')?.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') this.saveToken();
        });
        $('github-disconnect-btn')?.addEventListener('click', () => this.disconnect());
        $('github-signin-btn')?.addEventListener('click', () => this.startDeviceFlow());
        $('github-device-cancel')?.addEventListener('click', () => this.cancelDeviceFlow(true));
        $('github-save-oauth-btn')?.addEventListener('click', () => this.saveOAuthClientId());
        $('github-save-config-btn')?.addEventListener('click', () => this.saveConfig());
        $('github-load-repos-btn')?.addEventListener('click', () => this.loadRepos());
        $('github-add-repo-btn')?.addEventListener('click', () => this.addSelectedRepo());
        $('github-add-manual-btn')?.addEventListener('click', () => this.addManualRepo());
        $('github-repo-manual')?.addEventListener('keydown', (e) => {
            if (e.key === 'Enter') this.addManualRepo();
        });
        $('github-run-review-btn')?.addEventListener('click', () => this.reviewNow());
        $('github-refresh-btn')?.addEventListener('click', () => {
            this.loadHistory();
            this.refreshStatus();
        });
    },

    async load() {
        await Promise.allSettled([this.refreshStatus(), this.loadConfig(), this.loadHistory()]);
    },

    // ── API helpers ─────────────────────────────────────────────────────

    async fetchJson(url, options = {}) {
        const response = await OSA.fetchWithAuth(url, options);
        const data = await response.json().catch(() => ({}));
        if (!response.ok) {
            throw new Error(data.error || `HTTP ${response.status}`);
        }
        return data;
    },

    showMessage(message, type) {
        const slot = document.querySelector('.github-message-slot');
        if (!slot) return;
        slot.innerHTML = '';
        const msg = document.createElement('div');
        msg.className = `github-message ${type}`;
        msg.textContent = message;
        slot.appendChild(msg);
        setTimeout(() => msg.remove(), 6000);
    },

    // ── Connection ──────────────────────────────────────────────────────

    async refreshStatus() {
        let status;
        try {
            status = await this.fetchJson('/api/github/status');
        } catch (error) {
            this.setBadge(false, 'Unreachable');
            return;
        }
        const connected = status.configured && !status.auth_error;
        this.setBadge(connected, connected ? (status.login || 'Connected') : (status.auth_error ? 'Auth failed' : 'Not connected'));

        const connectedView = document.getElementById('github-connected-view');
        const connectView = document.getElementById('github-connect-view');
        const authError = document.getElementById('github-auth-error');
        if (connected) {
            connectedView?.classList.remove('hidden');
            connectView?.classList.add('hidden');
            document.getElementById('github-login')?.replaceChildren(document.createTextNode(status.login || ''));
            const meta = [];
            if (status.name) meta.push(status.name);
            if (status.token_source && status.token_source !== 'config') meta.push(status.token_source);
            if (status.scopes && status.scopes.length) meta.push(status.scopes.join(', '));
            document.getElementById('github-identity-meta')?.replaceChildren(document.createTextNode(meta.join(' · ')));
            const avatar = document.getElementById('github-avatar');
            if (avatar && status.avatar_url) avatar.src = status.avatar_url;
        } else {
            connectedView?.classList.add('hidden');
            connectView?.classList.remove('hidden');
            if (status.auth_error) {
                authError?.classList.remove('hidden');
                if (authError) authError.textContent = status.auth_error;
            } else {
                authError?.classList.add('hidden');
            }
            // OAuth availability + hint. `oauth_app_source` is "shared"
            // when the build ships OSA's own app (zero setup), "custom"
            // when the user configured one, "none" otherwise.
            const signin = document.getElementById('github-signin-btn');
            const hint = document.getElementById('github-oauth-hint');
            const source = status.oauth_app_source || 'none';
            if (signin) signin.disabled = !status.oauth_client_id_set;
            if (hint) {
                hint.textContent = source === 'none'
                    ? 'Add an OAuth client ID under Other sign-in options, or use a token.'
                    : '';
            }
            const setup = document.getElementById('github-oauth-setup');
            if (setup) {
                const summary = setup.querySelector('summary');
                if (summary) {
                    summary.textContent = source === 'none'
                        ? 'OAuth app setup (needed for GitHub sign-in)'
                        : 'Your OAuth app (configured)';
                }
                if (source === 'none') setup.open = true;
                // The shared build app needs no decision from the user;
                // only surface the override when it is missing or custom.
                setup.classList.toggle('hidden', source === 'shared');
            }

            // A sign-in may still be in flight (reloaded page, second tab):
            // resume showing it instead of silently dropping it.
            if (status.device_flow && !this.deviceFlowActive) {
                this.showDeviceFlow(status.device_flow, { autoOpen: false });
            } else if (!status.device_flow && this.deviceFlowActive) {
                // Server side finished or was cancelled elsewhere.
                this.cancelDeviceFlow();
            }
        }

        const pollStatus = document.getElementById('github-poll-status');
        if (pollStatus) {
            const parts = [];
            if (status.configured) {
                if (!status.watch_repos || !status.watch_repos.length) {
                    parts.push('No repositories watched. Nothing runs automatically.');
                } else if (!status.auto_review && !status.trigger_mentions) {
                    parts.push('No triggers enabled. Reviews only run when you ask.');
                }
            }
            if (status.poll_running) parts.push('Sweep running');
            else if (status.last_poll_at) parts.push(`Last sweep: ${new Date(status.last_poll_at).toLocaleTimeString()}`);
            if (status.reviewing && status.reviewing.length) parts.push(`Reviewing: ${status.reviewing.join(', ')}`);
            if (status.last_poll_error) parts.push(`Last sweep error: ${status.last_poll_error}`);
            pollStatus.textContent = parts.join(' · ');
        }
    },

    setBadge(ok, text) {
        const badge = document.getElementById('github-status-badge');
        if (!badge) return;
        badge.textContent = text;
        badge.classList.toggle('ok', !!ok);
        badge.classList.toggle('off', !ok);
    },

    async saveToken() {
        const input = document.getElementById('github-token-input');
        const token = input?.value.trim();
        if (!token) {
            this.showMessage('Paste a token first.', 'error');
            return;
        }
        try {
            const data = await this.fetchJson('/api/github/token', {
                method: 'POST',
                body: JSON.stringify({ token }),
            });
            if (input) input.value = '';
            this.showMessage(`Connected as ${data.login}.`, 'success');
            await this.load();
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    async disconnect() {
        if (!confirm('Disconnect GitHub and remove the stored token?')) return;
        try {
            await this.fetchJson('/api/github/token', { method: 'DELETE' });
            this.showMessage('Disconnected.', 'success');
            await this.load();
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    async saveOAuthClientId() {
        const value = document.getElementById('github-oauth-client-id')?.value.trim() || '';
        try {
            await this.fetchJson('/api/github/config', {
                method: 'PUT',
                body: JSON.stringify({ oauth_client_id: value }),
            });
            this.showMessage('OAuth client ID saved.', 'success');
            await this.refreshStatus();
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    // ── Sign-in (OAuth device flow) ─────────────────────────────────────

    async startDeviceFlow() {
        if (this.deviceFlowActive) return;
        const native = this.isNativeShell();
        // Open the tab synchronously so the browser treats it as part of
        // the click; a window.open after an await gets popup-blocked. The
        // desktop shell has no working tabs, so it uses the system
        // browser via the server instead.
        let tab = null;
        if (!native) {
            try {
                tab = window.open('about:blank', '_blank');
            } catch (error) {
                tab = null;
            }
        }
        try {
            const data = await this.fetchJson('/api/github/oauth/start', { method: 'POST' });
            this.showDeviceFlow(data, { tab, autoOpen: true });
        } catch (error) {
            if (tab && !tab.closed) tab.close();
            this.showMessage(error.message, 'error');
        }
    },

    /// Chromium (and therefore WebView2 and any background browser tab)
    /// throttles or freezes timers while the page is hidden — exactly the
    /// state the window is in while the user authorizes in the system
    /// browser. Re-check the moment the page becomes visible or focused
    /// again so an approved sign-in is picked up immediately instead of
    /// waiting on a throttled timer.
    bindVisibilityPolling() {
        if (this.visibilityBound) return;
        this.visibilityBound = true;
        const wake = (event) => {
            if (!this.deviceFlowActive) return;
            // Only the visibilitychange event is gated on visibilityState: a
            // real focus/pageshow means the window is back in front even if
            // WebView2 reports the page as hidden.
            if (event && event.type === 'visibilitychange'
                && typeof document.visibilityState === 'string'
                && document.visibilityState === 'hidden') return;
            this.scheduleDevicePoll(0);
        };
        document.addEventListener('visibilitychange', wake);
        window.addEventListener('focus', wake);
        window.addEventListener('pageshow', wake);
    },

    /// Render the sign-in card for a flow snapshot and make sure polling
    /// is running. Used for fresh starts and when a reloaded pane finds a
    /// sign-in still in flight.
    showDeviceFlow(data, { tab = null, autoOpen = false } = {}) {
        this.deviceFlowActive = true;
        this.devicePollFailures = 0;
        this.devicePollIntervalMs = Math.max(5, data.interval || 5) * 1000;
        const flow = document.getElementById('github-device-flow');
        flow?.classList.remove('hidden');
        document.getElementById('github-device-code')?.replaceChildren(document.createTextNode(data.user_code));
        // Prefer the pre-filled URL (GitHub's or the one OSA constructs);
        // fall back to the bare page + clipboard copy.
        const complete = data.verification_uri_complete || '';
        const target = complete || data.verification_uri || 'https://github.com/login/device';
        const link = document.getElementById('github-device-link');
        if (link) link.href = target;
        if (autoOpen) {
            if (tab && !tab.closed) {
                try {
                    tab.location.href = target;
                } catch (error) {
                    this.openExternal(target);
                }
            } else if (this.isNativeShell()) {
                // Desktop shell: hand the URL to the server, which opens
                // the system browser.
                this.openExternal(target);
            } else {
                // Popup was blocked; the visible "Open GitHub again" link
                // is the fallback.
                window.open(target, '_blank', 'noopener');
            }
            if (!complete) {
                try {
                    navigator.clipboard?.writeText(data.user_code);
                } catch (error) {
                    // Clipboard access can be denied; the code stays visible.
                }
            }
        }
        this.showDeviceStatus(autoOpen
            ? 'Waiting for authorization. This pane connects on its own.'
            : 'Sign-in already in progress. Finish it on GitHub (code shown above).');
        this.scheduleDevicePoll();
    },

    /// A self-rescheduling timeout rather than `setInterval`: the next poll is
    /// only armed once the previous one settled, so slow requests cannot stack
    /// up, and a throttled timer can be re-armed from a visibility event.
    scheduleDevicePoll(delayMs) {
        if (!this.deviceFlowActive) return;
        if (this.devicePollTimer) {
            clearTimeout(this.devicePollTimer);
        }
        const delay = typeof delayMs === 'number'
            ? delayMs
            : (this.devicePollIntervalMs || 5000);
        this.devicePollTimer = setTimeout(() => {
            this.devicePollTimer = null;
            this.pollDeviceFlow();
        }, delay);
    },

    async pollDeviceFlow() {
        if (!this.deviceFlowActive || this.devicePollInFlight) return;
        this.devicePollInFlight = true;
        try {
            const data = await this.fetchJson('/api/github/oauth/poll', { method: 'POST' });
            if (data.status === 'complete') {
                this.cancelDeviceFlow();
                this.showMessage(`Signed in as ${data.login}.`, 'success');
                await this.load();
                return;
            }
            if (data.status === 'expired') {
                this.cancelDeviceFlow();
                this.showMessage('The code expired. Start sign-in again.', 'error');
                return;
            }
            // Still pending.
            this.devicePollFailures = 0;
            this.showDeviceStatus('Waiting for authorization. This pane connects on its own.');
        } catch (error) {
            const message = (error && error.message) || String(error || '');
            // The server genuinely has no sign-in to poll (finished elsewhere,
            // cancelled, or the server restarted). Stop and refresh quietly.
            if (/no github sign-in in progress/i.test(message)) {
                this.cancelDeviceFlow();
                this.refreshStatus();
                return;
            }
            // Transient failure. The webview is commonly suspended while the
            // system browser is in front, which can abort an in-flight poll.
            // That must not strand a sign-in the user already approved: keep
            // the flow alive and retry on the next tick.
            this.devicePollFailures += 1;
            if (this.devicePollFailures > 40) {
                this.cancelDeviceFlow();
                this.showMessage(message || 'GitHub sign-in could not be completed.', 'error');
                return;
            }
            this.showDeviceStatus('Waiting for authorization… (retrying)');
        } finally {
            this.devicePollInFlight = false;
            this.scheduleDevicePoll();
        }
    },

    /// Stop showing the sign-in card. `notifyServer` is true only for the
    /// user's Cancel button: completing a flow must not clear a session a
    /// second tab may still be polling.
    async cancelDeviceFlow(notifyServer = false) {
        this.deviceFlowActive = false;
        this.devicePollFailures = 0;
        if (this.devicePollTimer) {
            clearTimeout(this.devicePollTimer);
            this.devicePollTimer = null;
        }
        document.getElementById('github-device-flow')?.classList.add('hidden');
        if (notifyServer) {
            try {
                await this.fetchJson('/api/github/oauth/cancel', { method: 'POST' });
            } catch (error) {
                // Best effort; the code expires on its own anyway.
            }
        }
    },

    showDeviceStatus(text) {
        const status = document.getElementById('github-device-status');
        if (status) status.textContent = text;
    },

    // ── Auto-review config ──────────────────────────────────────────────

    async loadConfig() {
        let config;
        try {
            config = await this.fetchJson('/api/github/config');
        } catch (error) {
            this.showMessage(error.message, 'error');
            return;
        }
        const $ = (id) => document.getElementById(id);
        if ($('github-auto-review')) $('github-auto-review').checked = !!config.auto_review;
        if ($('github-trigger-mentions')) $('github-trigger-mentions').checked = !!config.trigger_mentions;
        if ($('github-mention-handle')) $('github-mention-handle').value = config.mention_handle || 'osagent-review';
        if ($('github-poll-seconds')) $('github-poll-seconds').value = config.poll_seconds || 120;
        if ($('github-review-event')) $('github-review-event').value = config.review_event || 'COMMENT';
        if ($('github-max-comments')) $('github-max-comments').value = config.max_inline_comments || 15;
        if (typeof OSA.populateModelSelect === 'function') {
            await OSA.populateModelSelect($('github-review-model'), config.review_model || '');
        } else if ($('github-review-model')) {
            $('github-review-model').value = config.review_model || '';
        }
        if ($('github-include-drafts')) $('github-include-drafts').checked = !!config.include_drafts;
        if ($('github-ignore-authors')) $('github-ignore-authors').value = (config.ignore_authors || []).join(', ');
        if ($('github-review-instructions')) $('github-review-instructions').value = config.review_instructions || '';
        if ($('github-oauth-client-id') && !$('github-oauth-client-id').value) {
            $('github-oauth-client-id').value = config.oauth_client_id || '';
        }
        this.watchRepos = Array.isArray(config.watch_repos) ? [...config.watch_repos] : [];
        this.renderChips();
    },

    renderChips() {
        const box = document.getElementById('github-repo-chips');
        if (!box) return;
        box.innerHTML = '';
        if (!this.watchRepos.length) {
            box.innerHTML = '<span class="github-hint">No repositories watched yet. Add one to enable automatic reviews.</span>';
            return;
        }
        for (const repo of this.watchRepos) {
            const chip = document.createElement('span');
            chip.className = 'github-chip';
            chip.textContent = repo;
            const remove = document.createElement('button');
            remove.textContent = '×';
            remove.title = 'Remove';
            remove.addEventListener('click', () => {
                this.watchRepos = this.watchRepos.filter((r) => r !== repo);
                this.renderChips();
            });
            chip.appendChild(remove);
            box.appendChild(chip);
        }
    },

    async loadRepos() {
        try {
            const data = await this.fetchJson('/api/github/repos');
            this.allRepos = data.repos || [];
            const picker = document.getElementById('github-repo-picker');
            const addBtn = document.getElementById('github-add-repo-btn');
            if (!picker || !addBtn) return;
            picker.innerHTML = '<option value="">Select a repository</option>';
            for (const repo of this.allRepos) {
                const option = document.createElement('option');
                option.value = repo.full_name;
                option.textContent = `${repo.full_name}${repo.private ? ' (private)' : ''}`;
                picker.appendChild(option);
            }
            picker.classList.remove('hidden');
            addBtn.classList.remove('hidden');
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    addSelectedRepo() {
        const picker = document.getElementById('github-repo-picker');
        const value = picker?.value;
        if (!value) {
            this.showMessage('Pick a repository first.', 'error');
            return;
        }
        this.addRepo(value);
    },

    addManualRepo() {
        const input = document.getElementById('github-repo-manual');
        const value = input?.value.trim();
        if (!value) return;
        this.addRepo(value);
        if (input) input.value = '';
    },

    addRepo(repo) {
        const candidate = repo.trim();
        if (!/^[\w.-]+\/[\w.-]+$/.test(candidate)) {
            this.showMessage(`"${candidate}" is not in owner/repo form.`, 'error');
            return;
        }
        if (!this.watchRepos.includes(candidate)) {
            this.watchRepos.push(candidate);
        }
        this.renderChips();
    },

    async saveConfig() {
        const $ = (id) => document.getElementById(id);
        const body = {
            auto_review: !!$('github-auto-review')?.checked,
            trigger_mentions: !!$('github-trigger-mentions')?.checked,
            mention_handle: ($('github-mention-handle')?.value || '').trim().replace(/^@/, ''),
            poll_seconds: Number($('github-poll-seconds')?.value) || 120,
            review_event: $('github-review-event')?.value || 'COMMENT',
            max_inline_comments: Number($('github-max-comments')?.value) || 15,
            review_model: $('github-review-model')?.value.trim() || '',
            include_drafts: !!$('github-include-drafts')?.checked,
            ignore_authors: ($('github-ignore-authors')?.value || '')
                .split(',')
                .map((s) => s.trim())
                .filter(Boolean),
            review_instructions: $('github-review-instructions')?.value.trim() || '',
            watch_repos: this.watchRepos,
        };
        if ($('github-oauth-client-id')?.value.trim()) {
            body.oauth_client_id = $('github-oauth-client-id').value.trim();
        }
        if (body.trigger_mentions && !body.mention_handle) {
            this.showMessage('Set a mention handle before enabling the @mention trigger.', 'error');
            return;
        }
        try {
            await this.fetchJson('/api/github/config', {
                method: 'PUT',
                body: JSON.stringify(body),
            });
            this.showMessage('GitHub settings saved.', 'success');
            await this.refreshStatus();
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    // ── Manual review + history ─────────────────────────────────────────

    async reviewNow() {
        if (this.reviewBusy) return;
        const repo = document.getElementById('github-run-repo')?.value.trim();
        const number = Number(document.getElementById('github-run-number')?.value);
        const post = !!document.getElementById('github-run-post')?.checked;
        if (!repo || !number) {
            this.showMessage('Enter a repository and PR number.', 'error');
            return;
        }
        const resultBox = document.getElementById('github-run-result');
        const button = document.getElementById('github-run-review-btn');
        this.reviewBusy = true;
        if (button) {
            button.disabled = true;
            button.textContent = 'Reviewing';
        }
        if (resultBox) {
            resultBox.classList.remove('hidden');
            resultBox.innerHTML = '<div class="github-hint">Fetching the diff and running the review model. This can take a minute.</div>';
        }
        try {
            const outcome = await this.fetchJson('/api/github/reviews/run', {
                method: 'POST',
                body: JSON.stringify({ repo, number, post }),
            });
            this.renderRunResult(outcome);
            await Promise.allSettled([this.loadHistory(), this.refreshStatus()]);
        } catch (error) {
            if (resultBox) resultBox.innerHTML = `<div class="github-error-text">${OSA.escapeHtml(error.message)}</div>`;
        } finally {
            this.reviewBusy = false;
            if (button) {
                button.disabled = false;
                button.textContent = 'Review';
            }
        }
    },

    renderRunResult(outcome) {
        const resultBox = document.getElementById('github-run-result');
        if (!resultBox) return;
        const header = outcome.posted
            ? `Posted review for <strong>${OSA.escapeHtml(outcome.repo)}#${outcome.pr_number}</strong> (${outcome.comments_posted} inline comment(s)).`
            : outcome.skipped
                ? `Skipped: ${OSA.escapeHtml(outcome.skipped)}.`
                : `Dry run for <strong>${OSA.escapeHtml(outcome.repo)}#${outcome.pr_number}</strong>, not posted (${outcome.comments_posted} inline finding(s)).`;
        const link = outcome.review_url
            ? ` <a href="${OSA.escapeAttr(outcome.review_url)}" target="_blank" rel="noopener">Open on GitHub</a>`
            : '';
        const summary = outcome.summary
            ? (typeof OSA.formatMessage === 'function'
                ? OSA.formatMessage(outcome.summary)
                : `<pre>${OSA.escapeHtml(outcome.summary)}</pre>`)
            : '';
        resultBox.innerHTML = `
            <div class="github-run-header">${header}${link}</div>
            <div class="github-run-meta">Model: ${OSA.escapeHtml(outcome.model || '-')} · took ${(outcome.duration_ms / 1000).toFixed(1)}s</div>
            <div class="github-run-summary">${summary}</div>
        `;
    },

    async loadHistory() {
        const box = document.getElementById('github-history');
        if (!box) return;
        let data;
        try {
            data = await this.fetchJson('/api/github/reviews?limit=50');
        } catch (error) {
            box.innerHTML = `<div class="github-error-text">${OSA.escapeHtml(error.message)}</div>`;
            return;
        }
        const reviews = data.reviews || [];
        if (!reviews.length) {
            box.innerHTML = '<div class="github-hint">No reviews yet.</div>';
            return;
        }
        const rows = reviews.map((review) => {
            const status = review.status === 'posted'
                ? '<span class="github-history-status ok">posted</span>'
                : review.status === 'failed'
                    ? '<span class="github-history-status off">failed</span>'
                    : `<span class="github-history-status">${OSA.escapeHtml(review.status)}</span>`;
            const link = review.review_url
                ? `<a href="${OSA.escapeAttr(review.review_url)}" target="_blank" rel="noopener">open</a>`
                : '-';
            const detailParts = [];
            if (review.error) {
                detailParts.push(`<div class="github-history-error">${OSA.escapeHtml(review.error)}</div>`);
            }
            if (review.summary) {
                const summary = typeof OSA.formatMessage === 'function'
                    ? OSA.formatMessage(review.summary)
                    : `<pre>${OSA.escapeHtml(review.summary)}</pre>`;
                detailParts.push(`<details class="github-history-summary"><summary>Summary</summary>${summary}</details>`);
            }
            const detailRow = detailParts.length
                ? `<tr class="github-history-detail-row"><td colspan="5">${detailParts.join('')}</td></tr>`
                : '';
            return `
                <tr>
                    <td>${OSA.escapeHtml(review.repo)}#${review.pr_number}</td>
                    <td>${status}</td>
                    <td>${review.comments_count}</td>
                    <td>${new Date(review.updated_at).toLocaleString()}</td>
                    <td>${link}</td>
                </tr>${detailRow}`;
        }).join('');
        box.innerHTML = `
            <table class="github-history-table">
                <thead>
                    <tr><th>Pull request</th><th>Status</th><th>Comments</th><th>Updated</th><th></th></tr>
                </thead>
                <tbody>${rows}</tbody>
            </table>`;
    },

    escapeHtml(text) {
        return OSA.escapeHtml(text || '');
    },
};

OSA.loadGithubUI = async function() {
    const pane = await OSA.GithubUI.init();
    if (pane) {
        await OSA.GithubUI.load();
    }
};

window.OSA.GithubUI = OSA.GithubUI;
window.loadGithubUI = OSA.loadGithubUI;

// Remote access: opt-in public tunnel with QR device pairing.
//
// The tunnel is managed by the server (cloudflared child process). This file
// only drives the settings UI and renders the pairing QR the phone scans.
OSA.Remote = {
    _countdownTimer: null,
    _lastStatus: null,

    async load() {
        const statusEl = document.getElementById('remote-access-status');
        try {
            const res = await OSA.fetchWithAuth('/api/remote/status');
            const data = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
            this._lastStatus = data;
            this.render(data);
        } catch (error) {
            if (statusEl) {
                statusEl.textContent = `Could not load remote access status: ${error.message || error}`;
            }
        }
    },

    render(data) {
        const toggle = document.getElementById('setting-remote-enabled');
        const warning = document.getElementById('remote-access-warning');
        const statusEl = document.getElementById('remote-access-status');
        const details = document.getElementById('remote-access-details');
        const urlEl = document.getElementById('remote-public-url');

        if (toggle) toggle.checked = !!data.enabled;

        const passwordReady = !!OSA._remotePasswordReady;
        if (warning) warning.classList.toggle('hidden', passwordReady);

        if (statusEl) {
            if (!data.cloudflared_available) {
                statusEl.textContent = 'cloudflared is not available for this platform. Install it manually and set remote_access.binary.';
            } else if (data.status === 'error') {
                statusEl.textContent = `Tunnel error: ${data.error || 'unknown error'}`;
            } else if (data.status === 'installing') {
                statusEl.textContent = 'Downloading cloudflared (one time)';
            } else if (data.enabled) {
                statusEl.textContent = data.status === 'running'
                    ? 'Tunnel running.'
                    : 'Starting tunnel';
            } else {
                statusEl.textContent = 'Disabled. Your instance is only reachable on this machine.';
            }
        }

        if (urlEl) urlEl.textContent = data.public_url || '-';
        if (details) details.classList.toggle('hidden', !data.enabled);
    },

    async onToggle() {
        const toggle = document.getElementById('setting-remote-enabled');
        const enable = !!(toggle && toggle.checked);
        toggle.disabled = true;
        try {
            const res = await OSA.fetchWithAuth(enable ? '/api/remote/enable' : '/api/remote/disable', {
                method: 'POST',
            });
            const data = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
            this.render(data);
            if (enable) {
                // The public URL only exists once cloudflared has dialed in; a
                // QR built before that would be empty.
                await this.waitForPublicUrl();
                await this.rotate();
            } else {
                this.clearQr();
            }
            if (OSA.showToast) {
                OSA.showToast(enable ? 'Remote access enabled' : 'Remote access disabled', 'info');
            }
        } catch (error) {
            if (toggle) toggle.checked = !enable;
            if (OSA.showToast) OSA.showToast(error.message || 'Remote access update failed', 'error');
            await this.load();
        } finally {
            toggle.disabled = false;
        }
    },

    async waitForPublicUrl(maxMs = 120000) {
        const deadline = Date.now() + maxMs;
        while (Date.now() < deadline) {
            if (this._lastStatus && (this._lastStatus.public_url || this._lastStatus.status === 'error')) {
                return;
            }
            await new Promise(resolve => setTimeout(resolve, 1000));
            await this.load();
        }
    },

    async rotate() {
        try {
            const res = await OSA.fetchWithAuth('/api/remote/pairing', { method: 'POST' });
            const data = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
            this.renderQr(data.qr_svg);
            this.startCountdown(data.expires_in_secs);
            if (data.public_url) {
                const urlEl = document.getElementById('remote-public-url');
                if (urlEl) urlEl.textContent = data.public_url;
            }
        } catch (error) {
            if (OSA.showToast) OSA.showToast(error.message || 'Could not create a pairing code', 'error');
        }
    },

    renderQr(svg) {
        const host = document.getElementById('remote-qr');
        if (!host) return;
        if (!svg) {
            this.clearQr();
            return;
        }
        // The SVG is produced by the server from the encoder matrix, not user
        // input, so it is safe to inject.
        host.innerHTML = svg;
        host.classList.remove('hidden');
    },

    clearQr() {
        const host = document.getElementById('remote-qr');
        if (host) {
            host.innerHTML = '';
            host.classList.add('hidden');
        }
        const expiry = document.getElementById('remote-pairing-expiry');
        if (expiry) expiry.textContent = '';
        if (this._countdownTimer) {
            clearInterval(this._countdownTimer);
            this._countdownTimer = null;
        }
    },

    /// Generate a code when the pane opens on a running tunnel and none
    /// is showing, so pairing needs no extra click.
    async ensureCode() {
        const host = document.getElementById('remote-qr');
        const showing = host && !host.classList.contains('hidden') && host.innerHTML.trim() !== '';
        if (showing) return;
        if (this._lastStatus && this._lastStatus.enabled && this._lastStatus.status === 'running') {
            await this.rotate();
        }
    },

    startCountdown(seconds) {
        const expiry = document.getElementById('remote-pairing-expiry');
        if (this._countdownTimer) clearInterval(this._countdownTimer);
        let remaining = Number(seconds) || 0;
        const tick = () => {
            if (!expiry) return;
            if (remaining <= 0) {
                expiry.textContent = 'Code expired. Generate a new one.';
                clearInterval(this._countdownTimer);
                this._countdownTimer = null;
                return;
            }
            expiry.textContent = `Code expires in ${remaining}s`;
            remaining -= 1;
        };
        tick();
        this._countdownTimer = setInterval(tick, 1000);
    },
};

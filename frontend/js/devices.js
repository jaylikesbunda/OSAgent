// Paired devices: list devices that joined by QR pairing and revoke them.
window.OSA = window.OSA || {};

OSA.Devices = {
    async load() {
        const host = document.getElementById('paired-devices-list');
        if (!host) return;
        try {
            const res = await OSA.fetchWithAuth('/api/devices');
            const data = await res.json().catch(() => ({}));
            if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
            this.render(Array.isArray(data) ? data : []);
        } catch (error) {
            host.innerHTML = `<div class="settings-help">Could not load devices: ${OSA.escapeHtml(error.message || String(error))}</div>`;
        }
    },

    render(devices) {
        const host = document.getElementById('paired-devices-list');
        if (!host) return;
        const revokeAll = document.getElementById('revoke-all-devices');
        const hint = document.getElementById('paired-devices-hint');
        if (revokeAll) revokeAll.classList.toggle('hidden', !devices.length);
        if (hint) hint.classList.toggle('hidden', !devices.length);
        if (!devices.length) {
            host.innerHTML = '<div class="settings-help">No devices paired yet. Pair one by scanning a code above.</div>';
            return;
        }
        host.innerHTML = devices.map(d => {
            const created = this.formatTime(d.created_at);
            const seen = d.last_seen_at ? this.formatTime(d.last_seen_at) : 'never';
            const label = d.label || 'Device';
            return `<div class="paired-device-row" data-device-id="${OSA.escapeAttr(d.id)}">`
                + `<div class="paired-device-main">`
                + `<div class="paired-device-label">${OSA.escapeHtml(label)}</div>`
                + `<div class="paired-device-meta">Paired ${OSA.escapeHtml(created)} &middot; Last seen ${OSA.escapeHtml(seen)}</div>`
                + `</div>`
                + `<button type="button" class="device-revoke-btn" onclick="OSA.Devices.revoke(${OSA.jsArg(d.id)})">Revoke</button>`
                + `</div>`;
        }).join('');
    },

    formatTime(value) {
        if (!value) return '';
        if (typeof OSA.formatRelativeDateTime === 'function') {
            const relative = OSA.formatRelativeDateTime(value);
            if (relative) return relative;
        }
        const date = new Date(value);
        return Number.isNaN(date.getTime()) ? '' : date.toLocaleString();
    },

    async revoke(id) {
        try {
            const res = await OSA.fetchWithAuth(`/api/devices/${encodeURIComponent(id)}`, {
                method: 'DELETE',
            });
            if (!res.ok && res.status !== 404) {
                const data = await res.json().catch(() => ({}));
                throw new Error(data.error || `HTTP ${res.status}`);
            }
            if (OSA.showToast) OSA.showToast('Device revoked', 'info');
            await this.load();
        } catch (error) {
            if (OSA.showToast) OSA.showToast(error.message || 'Could not revoke device', 'error');
        }
    },

    async revokeAll() {
        const host = document.getElementById('paired-devices-list');
        const count = host ? host.querySelectorAll('.paired-device-row').length : 0;
        if (count === 0) return;
        if (!window.confirm('Revoke all paired devices? They will have to scan a new QR code.')) return;
        try {
            const res = await OSA.fetchWithAuth('/api/devices', { method: 'DELETE' });
            if (!res.ok) {
                const data = await res.json().catch(() => ({}));
                throw new Error(data.error || `HTTP ${res.status}`);
            }
            if (OSA.showToast) OSA.showToast('All devices revoked', 'info');
            await this.load();
        } catch (error) {
            if (OSA.showToast) OSA.showToast(error.message || 'Could not revoke devices', 'error');
        }
    },
};

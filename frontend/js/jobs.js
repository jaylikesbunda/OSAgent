window.OSA = window.OSA || {};

OSA.Jobs = {
    panel: null,
    refreshTimer: null,
    pendingDeleteId: null,

    init() {
        if (Notification.permission === 'default') {
            Notification.requestPermission();
        }
        const discordCb = document.getElementById('notify-discord');
        if (discordCb) {
            discordCb.addEventListener('change', () => this.toggleDiscordField());
        }
    },

    DEFAULT_BRIEFING_PROMPT: 'Get today\'s weather forecast for my location, check for any world news highlights, and provide a brief summary of the day ahead.',

    onTypeChange() {
        const type = document.getElementById('job-type').value;
        const msg = document.getElementById('job-message');
        if (type === 'daily_briefing' && !msg.value.trim()) {
            msg.value = this.DEFAULT_BRIEFING_PROMPT;
        }
    },

    toggleDiscordField() {
        const field = document.getElementById('discord-channel-field');
        const cb = document.getElementById('notify-discord');
        if (field && cb) {
            field.classList.toggle('hidden', !cb.checked);
        }
    },

    async show() {
        const modal = document.getElementById('jobs-modal');
        if (!modal) return;
        modal.classList.remove('hidden');
        await this.load();
        this.startAutoRefresh();
    },

    hide() {
        const modal = document.getElementById('jobs-modal');
        if (modal) modal.classList.add('hidden');
        this.stopAutoRefresh();
    },

    startAutoRefresh() {
        this.stopAutoRefresh();
        this.refreshTimer = setInterval(() => this.load(), 30000);
    },

    stopAutoRefresh() {
        if (this.refreshTimer) {
            clearInterval(this.refreshTimer);
            this.refreshTimer = null;
        }
    },

    async load() {
        try {
            const jobs = await OSA.getScheduledJobs();
            this.renderJobs(jobs);
        } catch (e) {
            console.error('Failed to load jobs:', e);
            this.renderError();
        }
    },

    renderError() {
        const list = document.querySelector('#jobs-modal .jobs-list');
        if (!list) return;
        list.innerHTML = `
            <div class="jobs-empty">
                <div class="jobs-empty-icon">!</div>
                <div class="jobs-empty-title">Failed to load jobs</div>
                <div class="jobs-empty-text">Check your connection and try again</div>
            </div>`;
    },

    renderJobs(jobs) {
        const list = document.querySelector('#jobs-modal .jobs-list');
        if (!list) return;

        if (!jobs || jobs.length === 0) {
            list.innerHTML = `
                <div class="jobs-empty">
                    <div class="jobs-empty-icon">
                        <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
                    </div>
                    <div class="jobs-empty-title">No scheduled jobs</div>
                    <div class="jobs-empty-text">Create a job to schedule reminders, prompts, or daily briefings</div>
                </div>`;
            return;
        }

        list.innerHTML = jobs.map(job => this.renderJobCard(job)).join('');
    },

    renderJobCard(job) {
        const enabled = job.enabled;
        const hasFailed = job.failure_count > 0;
        const statusClass = !enabled ? 'paused' : hasFailed ? 'error' : 'active';
        const statusLabel = !enabled ? 'Paused' : hasFailed ? `${job.failure_count} failure${job.failure_count > 1 ? 's' : ''}` : 'Active';
        const typeLabel = job.job_type.replace(/_/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
        const channels = (job.notify_channels || ['web']).map(c =>
            `<span class="job-channel-tag job-channel-${c}">${c}</span>`
        ).join('');

        return `
        <div class="job-card ${enabled ? '' : 'job-disabled'}" data-id="${job.id}">
            <div class="job-card-main">
                <div class="job-card-top">
                    <div class="job-type-badge job-type-${job.job_type}">
                        ${this.typeIcon(job.job_type)}
                        ${typeLabel}
                    </div>
                    <span class="job-status-dot job-status-${statusClass}" title="${statusLabel}"></span>
                </div>
                <div class="job-message">${this.escapeHtml(job.message)}</div>
                <div class="job-card-meta">
                    <span class="job-schedule-tag" title="${this.escapeHtml(job.cron_expr)}">
                        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><polyline points="12 6 12 12 16 14"/></svg>
                        ${this.escapeHtml(job.cron_expr)}
                    </span>
                    ${channels}
                </div>
                <div class="job-card-times">
                    ${this.renderNextRun(job.next_run_at)}
                    ${job.last_run_at ? this.renderLastRun(job.last_run_at) : ''}
                    ${hasFailed ? `<span class="job-time-failures">${job.failure_count} failure${job.failure_count > 1 ? 's' : ''}</span>` : ''}
                </div>
            </div>
            <div class="job-card-actions">
                <button class="job-action-btn job-toggle-btn ${enabled ? 'is-on' : 'is-off'}" onclick="OSA.Jobs.toggle('${job.id}')" title="${enabled ? 'Pause' : 'Resume'}">
                    ${enabled
                        ? '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="4" width="4" height="16"/><rect x="14" y="4" width="4" height="16"/></svg>'
                        : '<svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polygon points="5 3 19 12 5 21 5 3"/></svg>'
                    }
                </button>
                <button class="job-action-btn job-delete-btn" onclick="OSA.Jobs.confirmDelete('${job.id}')" title="Delete">
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="3 6 5 6 21 6"/><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2"/></svg>
                </button>
            </div>
        </div>`;
    },

    renderNextRun(ts) {
        const rel = this.relativeTime(ts);
        const abs = this.formatAbsolute(ts);
        return `<span class="job-time-next" title="${abs}">Next: ${rel}</span>`;
    },

    renderLastRun(ts) {
        const rel = this.relativeTime(ts);
        const abs = this.formatAbsolute(ts);
        return `<span class="job-time-last" title="${abs}">Last: ${rel}</span>`;
    },

    typeIcon(type) {
        const icons = {
            reminder: '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.73 21a2 2 0 0 1-3.46 0"/></svg>',
            run_prompt: '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><polyline points="4 17 10 11 4 5"/><line x1="12" y1="19" x2="20" y2="19"/></svg>',
            daily_briefing: '<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="18" height="18" rx="2" ry="2"/><line x1="16" y1="2" x2="16" y2="6"/><line x1="8" y1="2" x2="8" y2="6"/><line x1="3" y1="10" x2="21" y2="10"/></svg>',
        };
        return icons[type] || icons.reminder;
    },

    showCreateForm() {
        const modal = document.getElementById('jobs-create-modal');
        if (modal) modal.classList.remove('hidden');
    },

    hideCreateForm() {
        const modal = document.getElementById('jobs-create-modal');
        if (modal) modal.classList.add('hidden');
    },

    async createJob() {
        const when = document.getElementById('job-when').value.trim();
        const message = document.getElementById('job-message').value.trim();
        const type = document.getElementById('job-type').value;

        if (!when || !message) return;

        const notify_via = [];
        if (document.getElementById('notify-web').checked) notify_via.push('web');
        if (document.getElementById('notify-discord').checked) notify_via.push('discord');
        if (notify_via.length === 0) notify_via.push('web');

        const payload = { when, message, job_type: type, notify_via };

        const discordChannel = document.getElementById('discord-channel-id');
        if (discordChannel && discordChannel.value.trim()) {
            payload.discord_channel_id = discordChannel.value.trim();
        }

        try {
            await OSA.createScheduledJob(payload);
            document.getElementById('job-when').value = '';
            document.getElementById('job-message').value = '';
            if (discordChannel) discordChannel.value = '';
            this.hideCreateForm();
            await this.load();
        } catch (e) {
            console.error('Failed to create job:', e);
            this.showToast('Failed to create job: ' + (e.message || 'Unknown error'), 'error');
        }
    },

    confirmDelete(id) {
        this.pendingDeleteId = id;
        const modal = document.getElementById('jobs-delete-modal');
        if (modal) modal.classList.remove('hidden');
        const btn = document.getElementById('jobs-delete-confirm-btn');
        if (btn) {
            btn.onclick = () => {
                this.remove(this.pendingDeleteId);
                this.hideDeleteConfirm();
            };
        }
    },

    hideDeleteConfirm() {
        const modal = document.getElementById('jobs-delete-modal');
        if (modal) modal.classList.add('hidden');
        this.pendingDeleteId = null;
    },

    async remove(id) {
        try {
            await OSA.deleteScheduledJob(id);
            await this.load();
        } catch (e) {
            console.error('Failed to delete job:', e);
            this.showToast('Failed to delete job', 'error');
        }
    },

    async toggle(id) {
        try {
            await OSA.toggleScheduledJob(id);
            await this.load();
        } catch (e) {
            console.error('Failed to toggle job:', e);
            this.showToast('Failed to update job', 'error');
        }
    },

    showNotification(message, type, job_id) {
        if (document.hidden && Notification.permission === 'granted') {
            new Notification('OSAgent', { body: message });
        }
        this.showToast(message, type || 'info');
    },

    showToast(message, type) {
        const existing = document.querySelectorAll('.jobs-toast');
        existing.forEach(t => t.remove());

        const toast = document.createElement('div');
        toast.className = `jobs-toast jobs-toast-${type || 'info'}`;

        const iconMap = {
            info: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>',
            error: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="15" y1="9" x2="9" y2="15"/><line x1="9" y1="9" x2="15" y2="15"/></svg>',
            reminder: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.73 21a2 2 0 0 1-3.46 0"/></svg>',
            success: '<svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M22 11.08V12a10 10 0 1 1-5.93-9.14"/><polyline points="22 4 12 14.01 9 11.01"/></svg>',
        };

        toast.innerHTML = `
            <span class="jobs-toast-icon">${iconMap[type] || iconMap.info}</span>
            <span class="jobs-toast-msg">${this.escapeHtml(message)}</span>
            <button class="jobs-toast-dismiss" onclick="this.parentElement.remove()">&times;</button>
        `;

        document.body.appendChild(toast);
        setTimeout(() => { if (toast.parentNode) toast.remove(); }, 6000);
    },

    relativeTime(ts) {
        if (!ts) return '-';
        const d = new Date(ts);
        const now = new Date();
        const diffMs = d - now;
        const absMs = Math.abs(diffMs);
        const suffix = diffMs < 0 ? 'ago' : 'from now';

        if (absMs < 60000) return 'now';
        if (absMs < 3600000) {
            const mins = Math.round(absMs / 60000);
            return `${mins}m ${suffix}`;
        }
        if (absMs < 86400000) {
            const hrs = Math.round(absMs / 3600000);
            return `${hrs}h ${suffix}`;
        }
        const days = Math.round(absMs / 86400000);
        if (days < 7) return `${days}d ${suffix}`;
        return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
    },

    formatAbsolute(ts) {
        if (!ts) return '';
        const d = new Date(ts);
        return d.toLocaleString(undefined, {
            weekday: 'short',
            month: 'short',
            day: 'numeric',
            hour: '2-digit',
            minute: '2-digit',
        });
    },

    escapeHtml(str) {
        const div = document.createElement('div');
        div.textContent = str;
        return div.innerHTML;
    },
};

document.addEventListener('DOMContentLoaded', () => OSA.Jobs.init());

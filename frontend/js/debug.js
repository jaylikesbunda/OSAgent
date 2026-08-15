window.OSA = window.OSA || {};

(function () {
    const MAX_ENTRIES = 500;
    const MAX_RENDERED = 200;

    let entries = [];
    let counters = {};
    let enabled = localStorage.getItem('osa.debug.enabled') === '1';
    let overlay = null;
    let listEl = null;
    let chipsEl = null;
    let renderPending = false;
    let warnSeen = {};

    function sessionId() {
        try {
            const s = OSA.getCurrentSession && OSA.getCurrentSession();
            return s && s.id ? String(s.id).slice(0, 8) : '';
        } catch (e) {
            return '';
        }
    }

    function syncCheckbox() {
        const checkbox = document.getElementById('setting-debug-overlay');
        if (checkbox) checkbox.checked = enabled;
    }

    function setEnabled(value) {
        enabled = !!value;
        localStorage.setItem('osa.debug.enabled', enabled ? '1' : '0');
        syncCheckbox();
        if (overlay) overlay.style.display = enabled ? '' : 'none';
        if (enabled) scheduleRender();
    }

    function detailText(detail) {
        try {
            return typeof detail === 'string' ? detail : JSON.stringify(detail);
        } catch (e) {
            return String(detail);
        }
    }

    function log(kind, detail) {
        entries.push({
            at: Date.now(),
            kind,
            session: sessionId(),
            detail: detailText(detail),
        });
        if (entries.length > MAX_ENTRIES) entries.splice(0, entries.length - MAX_ENTRIES);
        counters[kind] = (counters[kind] || 0) + 1;
        if (enabled && typeof console.debug === 'function') {
            console.debug('[OSA debug]', kind, detail);
        }
        if (enabled) scheduleRender();
    }

    function warn(kind, key, detail) {
        const k = `${kind}:${key || ''}`;
        warnSeen[k] = (warnSeen[k] || 0) + 1;
        if (warnSeen[k] === 1) {
            console.warn('[OSA debug]', kind, detail);
        }
        log(`warn.${kind}`, { key, detail, count: warnSeen[k] });
    }

    function scheduleRender() {
        if (renderPending) return;
        renderPending = true;
        requestAnimationFrame(() => {
            renderPending = false;
            renderOverlay();
        });
    }

    function escapeHtml(text) {
        return String(text)
            .replace(/&/g, '&amp;')
            .replace(/</g, '&lt;')
            .replace(/>/g, '&gt;')
            .replace(/"/g, '&quot;');
    }

    function kindClass(kind) {
        if (kind.indexOf('warn.') === 0) return 'dbg-warn';
        if (kind.indexOf('anchor.recovered') === 0) return 'dbg-recover';
        if (kind.indexOf('anchor.hidden') === 0 || kind.indexOf('anchor.orphan') === 0) return 'dbg-bad';
        if (kind.indexOf('anchor.') === 0) return 'dbg-anchor';
        if (kind.indexOf('event.') === 0) return 'dbg-event';
        if (kind.indexOf('tool.') === 0) return 'dbg-tool';
        return '';
    }

    function renderOverlay() {
        if (!overlay || !listEl || !chipsEl) return;
        const visible = entries.slice(-MAX_RENDERED).reverse();
        let html = '';
        for (const entry of visible) {
            html += `<div class="dbg-row ${kindClass(entry.kind)}"><span class="dbg-time">${new Date(entry.at).toISOString().slice(11, 23)}</span><span class="dbg-kind">${escapeHtml(entry.kind)}</span><span class="dbg-session">${escapeHtml(entry.session)}</span><span class="dbg-detail">${escapeHtml(entry.detail)}</span></div>`;
        }
        listEl.innerHTML = html;
        const top = Object.keys(counters)
            .map(k => ({ k, n: counters[k] }))
            .sort((a, b) => b.n - a.n)
            .slice(0, 8);
        chipsEl.innerHTML = top.map(c => `<span class="dbg-chip">${escapeHtml(c.k)}:${c.n}</span>`).join('');
        listEl.scrollTop = 0;
    }

    function snapshot() {
        const state = {
            enabled,
            session: {
                id: sessionId(),
                messageCount: 0,
                taskStatus: '',
            },
            transcript: {
                descriptorCount: 0,
                windowStart: 0,
                windowEnd: 0,
                maxWindowSize: 0,
            },
            anchoredNodes: [],
            sessionToolEvents: 0,
            queue: 0,
        };
        try {
            const s = OSA.getCurrentSession && OSA.getCurrentSession();
            if (s) {
                state.session.id = s.id;
                state.session.messageCount = Array.isArray(s.messages) ? s.messages.length : 0;
                state.session.taskStatus = s.task_status || '';
            }
        } catch (e) {}
        try {
            const view = OSA.getTranscriptView && OSA.getTranscriptView();
            if (view) {
                state.transcript.descriptorCount = Array.isArray(view.descriptors) ? view.descriptors.length : 0;
                state.transcript.windowStart = view.windowStart;
                state.transcript.windowEnd = view.windowEnd;
                state.transcript.maxWindowSize = view.maxWindowSize;
                for (const [index, nodes] of view.anchoredNodesByIndex.entries()) {
                    state.anchoredNodes.push({
                        index,
                        nodes: nodes.map(n => ({
                            id: n.id || String(n.className),
                            connected: n.isConnected,
                        })),
                    });
                }
            }
        } catch (e) {}
        try {
            state.sessionToolEvents = Array.isArray(OSA.sessionToolEvents) ? OSA.sessionToolEvents.length : 0;
        } catch (e) {}
        try {
            state.queue = (OSA.getSessionQueue && OSA.getSessionQueue()).length;
        } catch (e) {}
        return state;
    }

    function copyDiagnostics() {
        const payload = {
            generatedAt: new Date().toISOString(),
            snapshot: snapshot(),
            entries: entries.slice(-MAX_ENTRIES),
        };
        const text = JSON.stringify(payload, null, 2);
        if (navigator.clipboard && navigator.clipboard.writeText) {
            navigator.clipboard.writeText(text).catch(() => fallbackCopy(text));
        } else {
            fallbackCopy(text);
        }
    }

    function fallbackCopy(text) {
        try {
            const ta = document.createElement('textarea');
            ta.value = text;
            ta.style.position = 'fixed';
            ta.style.opacity = '0';
            document.body.appendChild(ta);
            ta.select();
            document.execCommand('copy');
            ta.remove();
        } catch (e) {}
    }

    function buildOverlay() {
        overlay = document.createElement('div');
        overlay.id = 'osa-debug-overlay';
        overlay.innerHTML = `
            <div class="dbg-header">
                <span class="dbg-title">OSA debug</span>
                <button type="button" id="dbg-toggle" title="Enable/disable live console output (Shift+D)">${enabled ? 'live on' : 'live off'}</button>
                <button type="button" id="dbg-copy" title="Copy diagnostics to clipboard">copy</button>
                <button type="button" id="dbg-clear" title="Clear log">clear</button>
            </div>
            <div class="dbg-chips" id="dbg-chips"></div>
            <div class="dbg-list" id="dbg-list"></div>
        `;
        listEl = overlay.querySelector('#dbg-list');
        chipsEl = overlay.querySelector('#dbg-chips');
        overlay.querySelector('#dbg-toggle').addEventListener('click', () => {
            setEnabled(!enabled);
            overlay.querySelector('#dbg-toggle').textContent = enabled ? 'live on' : 'live off';
            if (enabled) scheduleRender();
        });
        overlay.querySelector('#dbg-copy').addEventListener('click', copyDiagnostics);
        overlay.querySelector('#dbg-clear').addEventListener('click', () => {
            entries = [];
            counters = {};
            renderOverlay();
        });
        document.body.appendChild(overlay);
        overlay.style.display = enabled ? '' : 'none';
        renderOverlay();
    }

    function init() {
        const style = document.createElement('style');
        style.textContent = `
            #osa-debug-overlay {
                position: fixed;
                top: 56px;
                right: 12px;
                width: min(460px, calc(100vw - 24px));
                max-height: 60vh;
                display: flex;
                flex-direction: column;
                background: #16171d;
                color: #d7d9e0;
                border: 1px solid #2c2e3a;
                border-radius: 8px;
                box-shadow: 0 8px 32px rgba(0,0,0,.5);
                z-index: 2000;
                font: 11px/1.45 ui-monospace, SFMono-Regular, Consolas, monospace;
                overflow: hidden;
            }
            #osa-debug-overlay .dbg-header {
                display: flex;
                align-items: center;
                gap: 6px;
                padding: 6px 8px;
                border-bottom: 1px solid #2c2e3a;
            }
            #osa-debug-overlay .dbg-title { font-weight: 600; margin-right: auto; }
            #osa-debug-overlay button {
                background: #23242e;
                color: #b8bac4;
                border: 1px solid #2c2e3a;
                border-radius: 4px;
                padding: 2px 8px;
                font: inherit;
                cursor: pointer;
            }
            #osa-debug-overlay button:hover { background: #2e3040; }
            #osa-debug-overlay .dbg-chips {
                display: flex;
                flex-wrap: wrap;
                gap: 4px;
                padding: 6px 8px;
                border-bottom: 1px solid #2c2e3a;
                max-height: 72px;
                overflow-y: auto;
            }
            #osa-debug-overlay .dbg-chip {
                background: #23242e;
                border-radius: 4px;
                padding: 1px 6px;
                color: #8f93a3;
                white-space: nowrap;
            }
            #osa-debug-overlay .dbg-list { overflow-y: auto; padding: 4px 8px; }
            #osa-debug-overlay .dbg-row {
                display: grid;
                grid-template-columns: 74px 130px 60px 1fr;
                gap: 6px;
                padding: 1px 0;
                white-space: nowrap;
            }
            #osa-debug-overlay .dbg-time { color: #6f7382; }
            #osa-debug-overlay .dbg-kind { color: #8f93a3; overflow: hidden; text-overflow: ellipsis; }
            #osa-debug-overlay .dbg-session { color: #5c6080; overflow: hidden; text-overflow: ellipsis; }
            #osa-debug-overlay .dbg-detail { color: #d7d9e0; overflow: hidden; text-overflow: ellipsis; }
            #osa-debug-overlay .dbg-row.dbg-warn .dbg-kind { color: #f0b429; }
            #osa-debug-overlay .dbg-row.dbg-bad .dbg-kind { color: #f26d6d; }
            #osa-debug-overlay .dbg-row.dbg-recover .dbg-kind { color: #4cc38a; }
            #osa-debug-overlay .dbg-row.dbg-anchor .dbg-kind { color: #6aa9ff; }
            #osa-debug-overlay .dbg-row.dbg-tool .dbg-kind { color: #c58aff; }
        `;
        document.head.appendChild(style);
        buildOverlay();

        document.addEventListener('keydown', (e) => {
            if (e.key !== 'D' && e.key !== 'd') return;
            if (!e.shiftKey || e.ctrlKey || e.metaKey || e.altKey) return;
            const target = e.target;
            if (target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.tagName === 'SELECT' || target.isContentEditable)) return;
            setEnabled(!enabled);
            if (overlay) {
                overlay.querySelector('#dbg-toggle').textContent = enabled ? 'live on' : 'live off';
            }
            if (enabled) {
                log('debug.enabled', { at: new Date().toISOString() });
            }
            scheduleRender();
        });

        if (/[?&]debug=1/.test(window.location.search)) {
            setEnabled(true);
        }
        syncCheckbox();
        log('debug.init', { enabled });

        window.addEventListener('error', event => {
            warn('window.error', event.filename || 'unknown', {
                message: event.message,
                line: event.lineno,
                column: event.colno,
            });
        });
        window.addEventListener('unhandledrejection', event => {
            const reason = event.reason;
            warn(
                'window.unhandledrejection',
                reason?.name || 'promise',
                reason?.stack || reason?.message || String(reason)
            );
        });
    }

    OSA.debug = {
        get enabled() { return enabled; },
        set enabled(v) {
            setEnabled(v);
        },
        log,
        warn,
        snapshot,
        copy: copyDiagnostics,
        clear() {
            entries = [];
            counters = {};
            renderOverlay();
        },
    };

    OSA.onDebugOverlayToggleChange = function() {
        const checkbox = document.getElementById('setting-debug-overlay');
        OSA.debug.enabled = !!(checkbox && checkbox.checked);
    };

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', init);
    } else {
        init();
    }
})();

window.OSA = window.OSA || {};

OSA.parseGoalCommand = function(text) {
    if (!/^\/goal(?:\s|$)/i.test(text)) return null;
    let rest = text.slice(5).trim();
    let maxRounds;
    const actionMatch = /^(status|pause|resume|clear)(?:\s|$)/i.exec(rest);
    const action = actionMatch ? actionMatch[1].toLowerCase() : (rest ? 'create' : 'status');
    if (actionMatch) rest = rest.slice(actionMatch[0].length).trim();
    if (/^--rounds(?:\s|$)/.test(rest)) {
        const budget = /^--rounds\s+(\d+)(?:\s|$)/.exec(rest);
        if (!budget || Number(budget[1]) < 1 || Number(budget[1]) > 100) {
            throw new Error('Use --rounds with a number from 1 to 100.');
        }
        maxRounds = Number(budget[1]);
        rest = rest.slice(budget[0].length).trim();
    }
    if (action === 'create' && !rest) throw new Error('Use /goal [--rounds N] <objective>.');
    if (action !== 'create' && (rest || (maxRounds !== undefined && action !== 'resume'))) {
        throw new Error('Use /goal status, pause, clear, or resume [--rounds N].');
    }
    return { action, ...(action === 'create' ? { objective: rest } : {}), ...(maxRounds === undefined ? {} : { max_rounds: maxRounds }) };
};

OSA.handleGoalCommand = async function(text) {
    try {
        const command = OSA.parseGoalCommand(text);
        if (!command) return false;
        let session = OSA.getCurrentSession();
        if (!session?.id && command.action === 'create') session = await OSA.createSession();
        if (!session?.id) throw new Error('Open a chat first.');
        const sessionId = session.id;
        const url = `/api/sessions/${encodeURIComponent(sessionId)}/goal`;
        // Subscribe before starting work so early tool events are not missed.
        if (command.action === 'create' || command.action === 'resume') {
            OSA.connectEventSource?.(sessionId);
        }
        const options = command.action === 'status' ? undefined
            : command.action === 'clear' ? { method: 'DELETE' }
            : { method: 'POST', body: JSON.stringify(command) };
        const response = await OSA.fetchWithAuth(url, options);
        const data = await response.json();
        if (!response.ok || data.error) throw new Error(data.error || `HTTP ${response.status}`);
        if (OSA.getCurrentSession()?.id === sessionId) {
            OSA._goalSnapshot = data.goal ? { ...data, armed: data.armed ?? data.goal.phase === 'active' } : { goal: null };
            OSA.renderGoalPanel?.();
            OSA.refreshGoalPanel?.();
            if (data.goal) {
                const goal = data.goal;
                const reason = goal.blocked_reason || (goal.policy_code === 'round_budget_exhausted' ? 'Round budget reached; use /goal resume to continue.' : '');
                OSA.showToast?.(`Goal ${goal.phase}: ${goal.objective} (${goal.rounds_started}/${goal.max_rounds} rounds). ${reason}`.trim());
            } else {
                OSA.showToast?.(command.action === 'clear' ? 'Goal cleared. The current turn, if running, can finish.' : 'No goal set. Use /goal <objective> to start one.');
            }
            OSA.refreshCurrentSessionQueue?.();
        }
        return true;
    } catch (error) {
        OSA.showToast?.(error.message || 'Goal command failed.');
        return false;
    }
};

OSA.renderGoalPanel = function() {
    if (typeof document === 'undefined') return;
    const panel = document.getElementById('goal-panel');
    if (!panel) return;
    const goal = OSA._goalSnapshot?.goal;
    panel.classList.toggle('hidden', !goal);
    const trigger = document.getElementById('goal-trigger');
    if (trigger) {
        trigger.title = goal ? 'View goal' : 'Set a goal';
        trigger.setAttribute('aria-label', trigger.title);
    }
    if (!goal) { panel.replaceChildren(); panel._goalSignature = null; return; }
    const signature = JSON.stringify([goal, OSA._goalSnapshot.armed, !!OSA._goalActionPending]);
    if (panel._goalSignature === signature) return;
    panel._goalSignature = signature;
    const moreOpen = panel.querySelector('details')?.open || false;
    const previousRounds = panel.dataset.goalId === String(goal.id) ? panel.querySelector('#goal-resume-rounds')?.value : null;
    const restoreBudgetFocus = document.activeElement?.id === 'goal-resume-rounds';
    panel.dataset.goalId = goal.id;
    const node = (tag, cls, text) => {
        const el = document.createElement(tag);
        if (cls) el.className = cls;
        if (text !== undefined) el.textContent = text;
        return el;
    };
    const running = goal.phase === 'active' && OSA._goalSnapshot.armed !== false;
    const label = goal.phase === 'complete' ? 'Complete' : running ? 'Active' : goal.phase === 'blocked' ? 'Blocked' : 'Paused';
    panel.dataset.phase = label.toLowerCase();
    const row = node('div', 'goal-row');
    const copy = node('div', 'goal-copy');
    copy.append(node('div', 'goal-eyebrow', `Goal · ${label}`), node('div', 'goal-objective', goal.objective));
    row.append(copy);
    const action = (text, handler) => {
        const button = node('button', 'goal-action', text);
        button.type = 'button';
        button.disabled = !!OSA._goalActionPending;
        button.addEventListener('click', handler);
        return button;
    };
    if (goal.phase !== 'complete') row.append(action(running ? 'Pause' : 'Resume', () => OSA.runGoalPanelAction(running ? 'pause' : 'resume')));
    const meta = node('div', 'goal-meta');
    // This meter describes budget consumption, never estimated completion.
    meta.append(node('span', '', `${goal.rounds_started} / ${goal.max_rounds} rounds`));
    const more = node('details', 'goal-options');
    more.open = moreOpen;
    more.append(node('summary', '', 'Options'));
    const options = node('div', 'goal-options-body');
    const reason = goal.blocked_reason || (goal.policy_code === 'round_budget_exhausted' ? 'Round budget reached. Resume to continue.' : !OSA._goalSnapshot.armed && goal.phase === 'active' ? 'Resume to continue after restart.' : '');
    if (reason) options.append(node('p', 'goal-note', reason));
    if (goal.phase !== 'complete') {
        const label = node('label', '', 'Next budget ');
        const rounds = node('input');
        rounds.type = 'number'; rounds.min = '1'; rounds.max = '100'; rounds.value = previousRounds ?? goal.max_rounds;
        rounds.setAttribute('aria-label', 'Round budget for resume');
        rounds.id = 'goal-resume-rounds';
        label.append(rounds); options.append(label);
    }
    options.append(action('Clear goal', event => {
        const button = event.currentTarget;
        if (button.dataset.confirm !== 'yes') { button.dataset.confirm = 'yes'; button.textContent = 'Confirm clear'; return; }
        OSA.runGoalPanelAction('clear');
    }));
    more.append(options); meta.append(more);
    panel.replaceChildren(row, meta);
    if (restoreBudgetFocus) panel.querySelector('#goal-resume-rounds')?.focus();
};

OSA.refreshGoalPanel = async function() {
    if (typeof document === 'undefined' || !document.getElementById('goal-panel')) return;
    const id = OSA.getCurrentSession()?.id;
    if (!id) return;
    const generation = OSA._goalViewGeneration;
    const request = OSA._goalRequest = (OSA._goalRequest || 0) + 1;
    clearTimeout(OSA._goalPoll);
    try {
        const response = await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(id)}/goal`);
        if (!response.ok) throw new Error('Could not load goal.');
        const snapshot = await response.json();
        if (generation !== OSA._goalViewGeneration || id !== OSA.getCurrentSession()?.id || request !== OSA._goalRequest) return;
        if (snapshot.error) throw new Error(snapshot.error);
        OSA._goalSnapshot = snapshot;
        OSA.renderGoalPanel();
    } catch (_) {
        // Retain the last confirmed state; retry while this chat remains open.
    } finally {
        if (generation === OSA._goalViewGeneration && id === OSA.getCurrentSession()?.id && request === OSA._goalRequest) {
            OSA._goalPoll = setTimeout(OSA.refreshGoalPanel, OSA._goalSnapshot?.goal?.phase === 'active' ? 3000 : 10000);
        }
    }
};

OSA.watchSessionGoal = function() {
    OSA._goalViewGeneration = (OSA._goalViewGeneration || 0) + 1;
    clearTimeout(OSA._goalPoll);
    OSA._goalSnapshot = null;
    OSA._goalActionPending = false;
    OSA.closeGoalEditor?.();
    OSA.renderGoalPanel();
    OSA.refreshGoalPanel();
};

OSA.openGoalEditor = function() {
    if (OSA._goalSnapshot?.goal) {
        const options = document.querySelector('#goal-panel details');
        if (options) options.open = !options.open;
        return;
    }
    document.getElementById('goal-editor')?.classList.remove('hidden');
    document.getElementById('goal-trigger')?.setAttribute('aria-expanded', 'true');
    document.getElementById('goal-objective')?.focus();
};

OSA.closeGoalEditor = function() {
    if (typeof document === 'undefined') return;
    document.getElementById('goal-editor')?.classList.add('hidden');
    document.getElementById('goal-trigger')?.setAttribute('aria-expanded', 'false');
};

OSA.runGoalPanelAction = async function(action) {
    if (OSA._goalActionPending) return;
    const sessionId = OSA.getCurrentSession()?.id;
    const rounds = document.getElementById('goal-resume-rounds');
    if (action === 'resume' && rounds && !rounds.reportValidity()) return;
    const budget = action === 'resume' && rounds ? ` --rounds ${rounds.value}` : '';
    OSA._goalActionPending = true;
    OSA.renderGoalPanel();
    try { await OSA.handleGoalCommand(`/goal ${action}${budget}`); }
    finally {
        if (sessionId === OSA.getCurrentSession()?.id) { OSA._goalActionPending = false; OSA.renderGoalPanel(); }
    }
};

OSA.submitGoalEditor = async function(event) {
    event.preventDefault();
    const form = document.getElementById('goal-editor');
    const button = document.getElementById('goal-start');
    if (button.disabled || !form.reportValidity()) return;
    const objective = document.getElementById('goal-objective');
    const rounds = document.getElementById('goal-rounds');
    const sessionId = OSA.getCurrentSession()?.id;
    const draft = objective.value;
    if (!draft.trim()) { objective.focus(); return; }
    button.disabled = true;
    try {
        if (await OSA.handleGoalCommand(`/goal --rounds ${rounds.value} ${draft.trim()}`)) {
            if ((sessionId && sessionId === OSA.getCurrentSession()?.id) || (!sessionId && OSA._goalSnapshot?.goal?.objective === draft.trim())) {
                if (objective.value === draft) objective.value = '';
                OSA.closeGoalEditor();
            }
        }
    } finally { button.disabled = false; }
};

window.OSA = window.OSA || {};

OSA.renderSessionTodos = function() {
    const list = document.getElementById('session-todos-list');
    const meta = document.getElementById('session-todos-meta');
    if (!list || !meta) return;
    const todos = OSA.getSessionTodos() || [];
    meta.textContent = `${todos.length} item${todos.length === 1 ? '' : 's'}`;
    if (!todos.length) {
        list.innerHTML = '<div class="inspector-empty">Todo items will appear here when the agent uses todowrite.</div>';
        OSA.updateInspectorCount();
        return;
    }
    const byStatus = { in_progress: [], pending: [], completed: [], cancelled: [] };
    todos.forEach(t => {
        const status = (t.status || 'pending').toLowerCase();
        if (byStatus[status]) byStatus[status].push(t);
    });
    const renderItems = (items, sectionLabel) => {
        if (!items.length) return '';
        let result = `<div class="todo-section-label">${sectionLabel}</div>`;
        items.forEach((t, idx) => {
            const status = t.status || 'pending';
            const priority = t.priority || 'medium';
            result += `
                <div class="todo-item ${status}" onclick="OSA.toggleTodoItem(${idx})">
                    <div class="todo-checkbox"></div>
                    <div class="todo-content">
                        <div class="todo-text">${OSA.escapeHtml(t.content || '')}</div>
                        <div class="todo-meta">
                            <span class="todo-priority ${priority}">${priority}</span>
                        </div>
                    </div>
                </div>
            `;
        });
        return result;
    };
    let html = '<div class="todo-panel"><div class="todo-list">';
    html += renderItems(byStatus.in_progress, 'In Progress');
    html += renderItems(byStatus.pending, 'Pending');
    html += renderItems(byStatus.completed, 'Completed');
    html += renderItems(byStatus.cancelled, 'Cancelled');
    html += '</div></div>';
    list.innerHTML = html;
    OSA.updateInspectorCount();
};

OSA.toggleTodoItem = function(index) {
    const todos = OSA.getSessionTodos();
    if (!todos[index]) return;
    const current = todos[index].status || 'pending';
    const next = current === 'pending' ? 'in_progress' : current === 'in_progress' ? 'completed' : 'pending';
    todos[index].status = next;
    OSA.setSessionTodos(todos);
    OSA.renderSessionTodos();
};

OSA.fetchAndRenderTodos = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id) {
        OSA.setSessionTodos([]);
        OSA.renderSessionTodos();
        OSA.updateTodoDock();
        return;
    }
    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/todos`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        OSA.setSessionTodos(Array.isArray(data) ? data : []);
        OSA.renderSessionTodos();
        OSA.updateTodoDock();
    } catch (error) {
        console.error('Failed to fetch todos:', error);
        OSA.setSessionTodos([]);
        OSA.updateTodoDock();
    }
};

window.toggleTodoItem = OSA.toggleTodoItem;

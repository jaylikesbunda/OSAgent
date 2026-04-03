window.OSA = window.OSA || {};

OSA.loadPersonaCatalog = async function() {
    try {
        const res = await fetch('/api/personas', {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        OSA.setAvailablePersonas(data.personas || []);
        if (!OSA.getActivePersona()) {
            OSA.updatePersonaTrigger(null);
        }
        OSA.renderPersonaMenu();
        OSA.onPersonaSelectionChange();
    } catch (error) {
        console.error('Failed to load persona catalog:', error);
    }
};

OSA.updatePersonaTrigger = function(active) {
    const label = document.getElementById('persona-trigger-label');
    if (!label) return;
    if (!active) {
        label.textContent = 'Default';
        label.title = 'Default';
        return;
    }
    const personas = OSA.getAvailablePersonas();
    const name = active.id === 'default' ? 'Default' : (personas.find(p => p.id === active.id)?.name || active.id);
    label.textContent = name;
    label.title = name;
};

OSA.renderPersonaMenu = function() {
    const list = document.getElementById('persona-menu-list');
    if (!list) return;
    const personas = OSA.getAvailablePersonas();
    const selectedId = OSA.getSelectedPersonaId() || 'default';
    const allPersonas = [{ id: 'default', name: 'Default', summary: 'Balanced, product-ready engineering help.' }, ...personas.filter(p => p.id !== 'default')];
    list.innerHTML = allPersonas.map(p => {
        const isActive = selectedId === p.id;
        return `
            <div class="menu-row ${isActive ? 'active' : ''}">
                <button class="menu-row-main" type="button" onclick="OSA.selectPersonaFromMenu('${OSA.escapeHtml(p.id)}')">
                    <span class="menu-row-copy">
                        <span class="menu-row-title">${OSA.escapeHtml(p.name || p.id)}</span>
                    </span>
                </button>
            </div>
        `;
    }).join('');
};

OSA.onPersonaSelectionChange = function() {
    const customWrap = document.getElementById('persona-custom-wrap');
    const customInput = document.getElementById('persona-character');
    if (!customWrap || !customInput) return;
    const selectedId = OSA.getSelectedPersonaId() || 'default';
    if (selectedId === 'custom') {
        customWrap.classList.remove('hidden');
    } else {
        customWrap.classList.add('hidden');
        customInput.value = '';
    }
};

OSA.selectPersonaFromMenu = function(personaId) {
    OSA.setSelectedPersonaId(personaId || 'default');
    OSA.renderPersonaMenu();
    OSA.onPersonaSelectionChange();
    if (OSA.getSelectedPersonaId() !== 'custom') {
        OSA.applyPersona();
        OSA.closePersonaMenu();
    }
};

OSA.loadSessionPersona = async function() {
    const customInput = document.getElementById('persona-character');
    if (!customInput) return;
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id) {
        OSA.setActivePersona(null);
        OSA.setSelectedPersonaId('default');
        OSA.updatePersonaTrigger(null);
        customInput.value = '';
        OSA.renderPersonaMenu();
        OSA.onPersonaSelectionChange();
        return;
    }
    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/persona`, {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        OSA.setActivePersona(data.active || null);
        OSA.setSelectedPersonaId(data.active?.id || 'default');
        OSA.updatePersonaTrigger(data.active || null);
        OSA.renderPersonaMenu();
        if (data.active?.id === 'custom') {
            customInput.value = data.active.roleplay_character || '';
        } else {
            customInput.value = '';
        }
        OSA.onPersonaSelectionChange();
    } catch (error) {
        console.error('Failed to load session persona:', error);
    }
};

OSA.applyPersona = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id) {
        alert('Select a session first.');
        return;
    }
    const customInput = document.getElementById('persona-character');
    if (!customInput) return;
    const personaId = OSA.getSelectedPersonaId() || 'default';
    if (personaId === 'default') {
        await OSA.resetPersona();
        return;
    }
    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/persona`, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({
                persona_id: personaId,
                roleplay_character: personaId === 'custom' ? (customInput.value.trim() || null) : null
            })
        });
        const data = await res.json();
        if (!res.ok) throw new Error(data.error || `HTTP ${res.status}`);
        OSA.setActivePersona(data.active || null);
        OSA.setSelectedPersonaId(data.active?.id || 'default');
        OSA.updatePersonaTrigger(data.active || null);
        OSA.renderPersonaMenu();
        if (OSA.getSelectedPersonaId() !== 'custom') {
            OSA.closePersonaMenu();
        }
    } catch (error) {
        alert(`Failed to set persona: ${error.message}`);
    }
};

OSA.resetPersona = async function() {
    const currentSession = OSA.getCurrentSession();
    if (!currentSession?.id) {
        alert('Select a session first.');
        return;
    }
    try {
        const res = await fetch(`/api/sessions/${currentSession.id}/persona`, {
            method: 'DELETE',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });
        if (!res.ok) {
            const data = await res.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${res.status}`);
        }
        OSA.setActivePersona(null);
        OSA.setSelectedPersonaId('default');
        OSA.updatePersonaTrigger(null);
        const customInput = document.getElementById('persona-character');
        if (customInput) customInput.value = '';
        OSA.renderPersonaMenu();
        OSA.onPersonaSelectionChange();
        OSA.closePersonaMenu();
    } catch (error) {
        alert(`Failed to reset persona: ${error.message}`);
    }
};

window.selectPersonaFromMenu = OSA.selectPersonaFromMenu;
window.applyPersona = OSA.applyPersona;
window.resetPersona = OSA.resetPersona;

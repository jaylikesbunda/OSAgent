window.OSA = window.OSA || {};

OSA.SkillsUI = {
    skills: [],
    expandedSkill: null,
    uploadZone: null,

    async init() {
        const existingPane = document.getElementById('pane-skills');
        if (existingPane) {
            return existingPane;
        }
        const pane = await this.createSkillsPane();
        pane.classList.add('active');
        return pane;
    },

    async createSkillsPane() {
        const pane = document.createElement('div');
        pane.className = 'settings-pane';
        pane.id = 'pane-skills';
        pane.innerHTML = `
            <div class="settings-pane-header">
                <h2>Skills</h2>
                <p class="settings-pane-desc">Install and manage skills for specialized tasks</p>
            </div>
            <div class="skills-container">
                <div class="skills-upload-zone" id="skills-upload-zone">
                    <input type="file" id="skills-file-input" accept=".oskill">
                    <div class="skills-upload-icon">+</div>
                    <div class="skills-upload-text">
                        <strong>Drop .oskill file</strong> or click to browse
                    </div>
                    <div class="skills-upload-hint">Install skills from .oskill bundle files</div>
                </div>
                <div id="skills-list-container">
                    <div class="skill-loading">
                        <div class="skill-spinner"></div>
                    </div>
                </div>
            </div>
        `;
        
        const settingsMain = document.querySelector('.settings-main');
        if (settingsMain) {
            settingsMain.appendChild(pane);
        }
        
        await new Promise(resolve => setTimeout(resolve, 0));
        this.bindEvents();
        return pane;
    },

    bindEvents() {
        const uploadZone = document.getElementById('skills-upload-zone');
        const fileInput = document.getElementById('skills-file-input');
        
        if (uploadZone) {
            uploadZone.addEventListener('click', () => fileInput?.click());
            uploadZone.addEventListener('dragover', (e) => {
                e.preventDefault();
                uploadZone.classList.add('dragover');
            });
            uploadZone.addEventListener('dragleave', () => {
                uploadZone.classList.remove('dragover');
            });
            uploadZone.addEventListener('drop', (e) => {
                e.preventDefault();
                uploadZone.classList.remove('dragover');
                const file = e.dataTransfer?.files[0];
                if (file) this.handleFileUpload(file);
            });
        }
        
        if (fileInput) {
            fileInput.addEventListener('change', (e) => {
                const file = e.target.files?.[0];
                if (file) this.handleFileUpload(file);
            });
        }
    },

    async handleFileUpload(file) {
        if (!file.name.endsWith('.oskill')) {
            this.showMessage('Please select a .oskill file', 'error');
            return;
        }

        if (file.size > 50 * 1024 * 1024) {
            this.showMessage('File too large (max 50MB)', 'error');
            return;
        }

        const formData = new FormData();
        formData.append('bundle', file);

        try {
            const response = await fetch('/api/skills/install', {
                method: 'POST',
                headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
                body: formData
            });

            const data = await response.json();
            
            if (!response.ok) {
                throw new Error(data.error || 'Install failed');
            }

            this.showMessage(`Successfully installed ${data.name}`, 'success');
            await this.loadSkills();
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    async loadSkills() {
        let container = document.getElementById('skills-list-container');
        if (!container) {
            console.warn('Skills container not found, initializing...');
            await this.init();
            container = document.getElementById('skills-list-container');
        }
        if (!container) {
            console.error('Failed to create skills container');
            return;
        }

        container.innerHTML = '<div class="skill-loading"><div class="skill-spinner"></div></div>';

        try {
            const response = await OSA.getJson('/api/skills');
            this.skills = response.skills || [];
            this.renderSkills();
        } catch (error) {
            container.innerHTML = `<div class="skills-empty">
                <div class="skills-empty-icon">!</div>
                <h3>Failed to load skills</h3>
                <p>${this.escapeHtml(error.message)}</p>
            </div>`;
        }
    },

    renderSkills() {
        const container = document.getElementById('skills-list-container');
        if (!container) return;

        if (this.skills.length === 0) {
            container.innerHTML = `
                <div class="skills-empty">
                    <div class="skills-empty-icon">+</div>
                    <h3>No skills installed</h3>
                    <p>Install skill bundles (.oskill files) to add new capabilities</p>
                    <button class="btn-action" onclick="document.getElementById('skills-file-input')?.click()">
                        Install Skill
                    </button>
                </div>
            `;
            return;
        }

        container.innerHTML = `
            <div class="skills-list">
                ${this.skills.map(skill => this.renderSkillCard(skill)).join('')}
            </div>
        `;

        container.querySelectorAll('.skill-card-header').forEach(header => {
            header.addEventListener('click', () => {
                const name = header.dataset.skill;
                this.toggleSkillExpand(name);
            });
        });

        container.querySelectorAll('.skill-toggle input').forEach(toggle => {
            toggle.addEventListener('change', (e) => {
                e.stopPropagation();
                this.handleToggle(e.target.dataset.skill, e.target.checked);
            });
        });

        container.querySelectorAll('.btn-test-skill').forEach(btn => {
            btn.addEventListener('click', (e) => {
                e.stopPropagation();
                this.testSkill(btn.dataset.skill);
            });
        });

        container.querySelectorAll('.btn-export-skill').forEach(btn => {
            btn.addEventListener('click', (e) => {
                e.stopPropagation();
                this.exportSkill(btn.dataset.skill);
            });
        });

        container.querySelectorAll('.btn-uninstall-skill').forEach(btn => {
            btn.addEventListener('click', (e) => {
                e.stopPropagation();
                if (confirm(`Are you sure you want to uninstall ${btn.dataset.skill}?`)) {
                    this.uninstallSkill(btn.dataset.skill);
                }
            });
        });
    },

    renderSkillCard(skill) {
        const isExpanded = this.expandedSkill === skill.name;
        const iconHtml = skill.icon_url
            ? `<img class="skill-icon-img" src="${this.escapeHtml(skill.icon_url)}" alt="${this.escapeHtml(skill.name)}">`
            : `<div class="skill-icon">${this.escapeHtml(skill.emoji || '+')}</div>`;
        
        return `
            <div class="skill-card ${isExpanded ? 'expanded' : ''}" data-skill="${this.escapeHtml(skill.name)}">
                <div class="skill-card-header" data-skill="${this.escapeHtml(skill.name)}">
                    ${iconHtml}
                    <div class="skill-info">
                        <div class="skill-name">
                            ${this.escapeHtml(skill.name)}
                            ${skill.version ? `<span class="version">v${this.escapeHtml(skill.version)}</span>` : ''}
                        </div>
                        <div class="skill-description">${this.escapeHtml(skill.description || 'No description')}</div>
                        <div class="skill-meta">
                            ${skill.has_config ? '<span class="skill-badge configured">Configured</span>' : ''}
                            ${skill.has_icon ? '<span class="skill-badge">Icon</span>' : ''}
                        </div>
                    </div>
                    <div class="skill-actions">
                        <label class="skill-toggle">
                            <input type="checkbox" data-skill="${this.escapeHtml(skill.name)}" ${skill.enabled ? 'checked' : ''}>
                            <span class="skill-toggle-slider"></span>
                        </label>
                        <button class="skill-expand-btn">
                            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                                <polyline points="6 9 12 15 18 9"></polyline>
                            </svg>
                        </button>
                    </div>
                </div>
                <div class="skill-card-details">
                    <div class="skill-content-preview" id="content-${this.escapeHtml(skill.name)}">Loading...</div>
                    <div class="skill-config-section" id="config-${this.escapeHtml(skill.name)}">
                        <h4>Configuration</h4>
                        <div class="skill-config-grid">
                            <div class="skill-loading"><div class="skill-spinner"></div></div>
                        </div>
                    </div>
                    <div class="skill-card-actions">
                        <button class="btn-action btn-test-skill" data-skill="${this.escapeHtml(skill.name)}">Test</button>
                        <button class="btn-ghost btn-export-skill" data-skill="${this.escapeHtml(skill.name)}">Export</button>
                        <button class="btn-danger btn-uninstall-skill" data-skill="${this.escapeHtml(skill.name)}">Uninstall</button>
                    </div>
                </div>
            </div>
        `;
    },

    async toggleSkillExpand(name) {
        if (this.expandedSkill === name) {
            this.expandedSkill = null;
            this.renderSkills();
        } else {
            this.expandedSkill = name;
            this.renderSkills();
            await this.loadSkillDetails(name);
        }
    },

    async loadSkillDetails(name) {
        const contentEl = document.getElementById(`content-${name}`);
        const configEl = document.getElementById(`config-${name}`);

        try {
            const response = await OSA.getJson(`/api/skills/${encodeURIComponent(name)}`);

            if (contentEl) {
                contentEl.textContent = response.content || 'No content';
            }

            if (configEl) {
                const schema = response.config_schema || [];
                const savedValues = response.config || {};

                if (schema.length === 0) {
                    configEl.innerHTML = `
                        <h4>Configuration</h4>
                        <p class="field-hint">This skill has no configuration options</p>
                    `;
                } else {
                    const fieldsHtml = schema.map(field => {
                        const saved = savedValues[field.name];
                        const currentValue = saved ? saved.value : (field.default || '');
                        const isSensitive = field.field_type === 'api_key' || field.field_type === 'password';
                        const inputType = isSensitive ? 'password'
                            : field.field_type === 'number' ? 'number'
                            : 'text';
                        const requiredHtml = field.required
                            ? '<span class="field-required">Required</span>'
                            : '<span class="field-optional">Optional</span>';
                        const hintHtml = field.description
                            ? `<span class="field-hint">${this.escapeHtml(field.description)}</span>`
                            : '';
                        return `
                            <div class="skill-config-field">
                                <label for="config-${this.escapeHtml(name)}-${this.escapeHtml(field.name)}">
                                    ${this.escapeHtml(field.name)} ${requiredHtml}
                                </label>
                                <input type="${inputType}"
                                       id="config-${this.escapeHtml(name)}-${this.escapeHtml(field.name)}"
                                       data-skill="${this.escapeHtml(name)}"
                                       data-key="${this.escapeHtml(field.name)}"
                                       value="${this.escapeHtml(currentValue)}"
                                       placeholder="${this.escapeHtml(field.description || '')}">
                                ${hintHtml}
                            </div>
                        `;
                    }).join('');

                    configEl.innerHTML = `
                        <h4>Configuration</h4>
                        <div class="skill-config-grid">${fieldsHtml}</div>
                        ${response.has_authorize ? `<button class="btn-action" style="margin-top:12px" onclick="OSA.SkillsUI.authorizeSkill('${this.escapeHtml(name)}')">Authorize</button>` : ''}
                        <button class="btn-action" style="margin-top:12px" onclick="OSA.SkillsUI.saveConfig('${this.escapeHtml(name)}')">Save Configuration</button>
                    `;

                    configEl.querySelectorAll('input').forEach(input => {
                        input.addEventListener('input', () => input.classList.add('modified'));
                    });
                }
            }
        } catch (error) {
            if (contentEl) {
                contentEl.textContent = `Error: ${error.message}`;
            }
        }
    },

    async saveConfig(skillName) {
        const configEl = document.getElementById(`config-${skillName}`);
        if (!configEl) return;

        const settings = {};
        configEl.querySelectorAll('input[data-key]').forEach(input => {
            const key = input.dataset.key;
            const value = input.value.trim();
            // Always include the key — backend removes it if value is empty
            settings[key] = value;
        });

        if (Object.keys(settings).length === 0) {
            this.showMessage('No configuration fields to save', 'error');
            return;
        }

        try {
            const response = await fetch(`/api/skills/${encodeURIComponent(skillName)}/config`, {
                method: 'PUT',
                headers: { 
                    'Authorization': `Bearer ${OSA.getToken()}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({ settings })
            });

            if (!response.ok) {
                const data = await response.json();
                throw new Error(data.error || 'Save failed');
            }

            this.showMessage('Configuration saved', 'success');
            configEl.querySelectorAll('input[data-key]').forEach(input => input.classList.remove('modified'));
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    async authorizeSkill(skillName) {
        try {
            const response = await fetch(`/api/skills/${encodeURIComponent(skillName)}/authorize`, {
                method: 'POST',
                headers: { 
                    'Authorization': `Bearer ${OSA.getToken()}`
                }
            });

            const data = await response.json();

            if (!response.ok) {
                throw new Error(data.error || 'Authorization failed');
            }

            this.showMessage(data.message || 'Authorization completed', 'success');
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    async handleToggle(name, enabled) {
        try {
            const response = await fetch(`/api/skills/${encodeURIComponent(name)}/enabled`, {
                method: 'PUT',
                headers: { 
                    'Authorization': `Bearer ${OSA.getToken()}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({ enabled })
            });

            if (!response.ok) {
                const data = await response.json();
                throw new Error(data.error || 'Toggle failed');
            }
        } catch (error) {
            this.showMessage(error.message, 'error');
            await this.loadSkills();
        }
    },

    async testSkill(name) {
        try {
            const response = await OSA.postJson(`/api/skills/${encodeURIComponent(name)}/test`, {});
            this.showMessage(response.message || 'Test complete', 'success');
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    async exportSkill(name) {
        try {
            const response = await fetch(`/api/skills/${encodeURIComponent(name)}/export`, {
                headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
            });

            if (!response.ok) {
                throw new Error('Export failed');
            }

            const blob = await response.blob();
            const url = URL.createObjectURL(blob);
            const a = document.createElement('a');
            a.href = url;
            a.download = `${name}.oskill`;
            document.body.appendChild(a);
            a.click();
            document.body.removeChild(a);
            URL.revokeObjectURL(url);

            this.showMessage(`Exported ${name}.oskill`, 'success');
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    async uninstallSkill(name) {
        try {
            const response = await fetch('/api/skills/uninstall', {
                method: 'POST',
                headers: { 
                    'Authorization': `Bearer ${OSA.getToken()}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({ name })
            });

            if (!response.ok) {
                const data = await response.json();
                throw new Error(data.error || 'Uninstall failed');
            }

            this.showMessage(`Uninstalled ${name}`, 'success');
            if (this.expandedSkill === name) {
                this.expandedSkill = null;
            }
            await this.loadSkills();
        } catch (error) {
            this.showMessage(error.message, 'error');
        }
    },

    showMessage(message, type) {
        const existing = document.querySelector('.skill-message');
        if (existing) existing.remove();

        const container = document.querySelector('.skills-container');
        if (!container) return;

        const msg = document.createElement('div');
        msg.className = `skill-message ${type}`;
        msg.textContent = message;
        container.insertBefore(msg, container.firstChild);

        setTimeout(() => msg.remove(), 5000);
    },

    escapeHtml(text) {
        if (!text) return '';
        const div = document.createElement('div');
        div.textContent = text;
        return div.innerHTML;
    }
};

OSA.loadSkillsUI = async function() {
    const pane = await OSA.SkillsUI.init();
    if (pane) {
        await OSA.SkillsUI.loadSkills();
    }
};

window.OSA.SkillsUI = OSA.SkillsUI;
window.loadSkillsUI = OSA.loadSkillsUI;

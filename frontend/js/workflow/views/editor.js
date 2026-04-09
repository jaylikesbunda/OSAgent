class WorkflowEditor {
  constructor(container, api) {
    this.container = container;
    this.api = api;
    this.state = new WorkflowState();
    this.executor = null;
    this.adapter = null;
    this.selectedNode = null;
    this.isDragging = false;
    this.dragNodeType = null;
    this.initialized = false;
    this.runAttachments = [];
    this.runAttachmentCounter = 0;
  }

  async init() {
    this.api.setToken?.((window.OSA && typeof OSA.getToken === 'function' && OSA.getToken()) || '');
    if (this.initialized) {
      await this.loadWorkflows();
      return;
    }

    this.render();
    await this.loadWorkflows();
    this.setupEventListeners();
    this.initialized = true;
  }

  render() {
    this.container.innerHTML = `
      <div class="workflow-editor">
        <div class="workflow-toolbar">
          <button class="btn-back" title="Back to Chat">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="15 18 9 12 15 6"></polyline></svg>
          </button>
          <span class="toolbar-title">Workflow Editor</span>
          <div class="toolbar-actions">
            <div class="toolbar-lock">
              <label for="workflow-lock-select">Locked Workspace</label>
              <select id="workflow-lock-select" class="workflow-lock-select">
                <option value="">No lock</option>
              </select>
            </div>
            <button class="btn-save" title="Save Workflow">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z"></path><polyline points="17 21 17 13 7 13 7 21"></polyline><polyline points="7 3 7 8 15 8"></polyline></svg>
              Save
            </button>
            <button class="btn-run" title="Run Workflow">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>
              Run
            </button>
            <button class="btn-stop hidden" title="Stop Workflow">
              <svg width="14" height="14" viewBox="0 0 24 24" fill="currentColor"><rect x="6" y="6" width="12" height="12" rx="2"></rect></svg>
              Stop
            </button>
          </div>
        </div>
        <div class="workflow-content">
          <div class="workflow-sidebar">
            <div class="workflow-list-section">
              <h3>Workflows</h3>
              <button class="btn-new-workflow">+ New</button>
              <div class="workflow-list"></div>
            </div>
            <div class="workflow-templates-section">
              <h3>Templates</h3>
              <div class="workflow-template-list"></div>
            </div>
            <div class="node-palette-section">
              <h3>Nodes</h3>
              <div class="node-palette">
                <div class="palette-item" data-node-type="osa/trigger" draggable="true">
                  <span class="palette-icon trigger">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>
                  </span>
                  <span>Start</span>
                </div>
                <div class="palette-item" data-node-type="osa/agent" draggable="true">
                  <span class="palette-icon agent">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="11" width="18" height="10" rx="2"></rect><circle cx="12" cy="5" r="2"></circle><path d="M12 7v4"></path></svg>
                  </span>
                  <span>AI Task</span>
                </div>
                <div class="palette-item" data-node-type="osa/condition" draggable="true">
                  <span class="palette-icon condition">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><path d="M12 16v-4"></path><path d="M12 8h.01"></path></svg>
                  </span>
                  <span>If / Else</span>
                </div>
                <div class="palette-item" data-node-type="osa/transform" draggable="true">
                  <span class="palette-icon transform">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 3 21 3 21 8"></polyline><line x1="4" y1="20" x2="21" y2="3"></line><polyline points="21 16 21 21 16 21"></polyline><line x1="15" y1="15" x2="21" y2="21"></line><line x1="4" y1="4" x2="9" y2="9"></line></svg>
                  </span>
                  <span>Format Text</span>
                </div>
                <div class="palette-item" data-node-type="osa/delay" draggable="true">
                  <span class="palette-icon delay">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><polyline points="12 6 12 12 16 14"></polyline></svg>
                  </span>
                  <span>Delay</span>
                </div>
                <div class="palette-item" data-node-type="osa/output" draggable="true">
                  <span class="palette-icon output">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"></path><polyline points="17 8 12 3 7 8"></polyline><line x1="12" y1="3" x2="12" y2="15"></line></svg>
                  </span>
                  <span>Show Result</span>
                </div>
                <div class="palette-item" data-node-type="osa/file_input" draggable="true">
                  <span class="palette-icon" style="background:#2a6f54;color:#fff;">F</span>
                  <span>Load File</span>
                </div>
                <div class="palette-item" data-node-type="osa/file_output" draggable="true">
                  <span class="palette-icon" style="background:#2a6f54;color:#fff;">W</span>
                  <span>Save File</span>
                </div>
                <div class="palette-item" data-node-type="osa/approval" draggable="true">
                  <span class="palette-icon" style="background:#7a4b20;color:#fff;">?</span>
                  <span>Ask Human</span>
                </div>
                <div class="palette-item" data-node-type="osa/foreach" draggable="true">
                  <span class="palette-icon" style="background:#4c4f7a;color:#fff;">#</span>
                  <span>Repeat List</span>
                </div>
              </div>
            </div>
          </div>
          <div class="workflow-canvas-container">
            <div class="canvas-placeholder">
              <p>Select a workflow or create a new one</p>
            </div>
          </div>
          <div class="workflow-properties-panel">
            <div class="properties-placeholder">
              <p>Select a node to edit its properties</p>
            </div>
            <div class="properties-content hidden"></div>
          </div>
        </div>
        <div class="workflow-execution-log hidden">
          <h3>Execution Log</h3>
          <div class="log-content"></div>
        </div>
        <div class="workflow-run-modal hidden" aria-hidden="true">
          <div class="workflow-run-modal-backdrop"></div>
          <div class="workflow-run-dialog" role="dialog" aria-modal="true" aria-label="Run workflow">
            <div class="workflow-run-header">
              <h3>Run Workflow</h3>
              <button class="workflow-run-close" type="button" aria-label="Close run dialog">&times;</button>
            </div>
            <div class="workflow-run-body">
              <div class="workflow-run-field">
                <label for="workflow-run-workspace">Workspace</label>
                <select id="workflow-run-workspace" class="workflow-run-workspace"></select>
                <div class="property-note">This run uses its own workflow session in the selected workspace.</div>
                <div class="workflow-run-workspace-lock-hint hidden"></div>
              </div>
              <div class="workflow-run-field">
                <label for="workflow-run-input">Trigger Input (optional)</label>
                <textarea id="workflow-run-input" class="workflow-run-input" placeholder="Add input text available as {{trigger_input}}"></textarea>
              </div>
              <div class="workflow-run-field">
                <div class="workflow-run-field-head">
                  <label for="workflow-run-file-input">Attachments (optional)</label>
                  <button class="workflow-run-pick" type="button">Choose files</button>
                </div>
                <input id="workflow-run-file-input" class="workflow-run-file-input" type="file" multiple>
                <div class="workflow-run-dropzone">Drop files here or use Choose files</div>
                <div class="workflow-run-status hidden"></div>
                <div class="workflow-run-file-list"></div>
              </div>
            </div>
            <div class="workflow-run-footer">
              <button class="workflow-run-cancel" type="button">Cancel</button>
              <button class="workflow-run-start" type="button">Start Run</button>
            </div>
          </div>
        </div>
      </div>
    `;

    this.setupDragAndDrop();
  }

  setupDragAndDrop() {
    const palette = this.container.querySelector('.node-palette');
    const canvasContainer = this.container.querySelector('.workflow-canvas-container');

    if (!palette || !canvasContainer) return;

    palette.querySelectorAll('.palette-item').forEach(item => {
      item.addEventListener('dragstart', (e) => {
        this.dragNodeType = e.target.closest('.palette-item').dataset.nodeType;
        e.dataTransfer.effectAllowed = 'copy';
      });
    });

    canvasContainer.addEventListener('dragover', (e) => {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'copy';
    });

    canvasContainer.addEventListener('drop', (e) => {
      e.preventDefault();
      if (this.dragNodeType && this.adapter) {
        const rect = canvasContainer.getBoundingClientRect();
        const x = e.clientX - rect.left;
        const y = e.clientY - rect.top;
        this.adapter.addNode(this.dragNodeType, x, y);
      }
      this.dragNodeType = null;
    });
  }

  setupEventListeners() {
    const btnBack = this.container.querySelector('.btn-back');
    const btnSave = this.container.querySelector('.btn-save');
    const btnRun = this.container.querySelector('.btn-run');
    const btnStop = this.container.querySelector('.btn-stop');
    const btnNew = this.container.querySelector('.btn-new-workflow');
    const lockSelect = this.container.querySelector('.workflow-lock-select');
    const runModal = this.container.querySelector('.workflow-run-modal');
    const runClose = this.container.querySelector('.workflow-run-close');
    const runCancel = this.container.querySelector('.workflow-run-cancel');
    const runStart = this.container.querySelector('.workflow-run-start');
    const runPick = this.container.querySelector('.workflow-run-pick');
    const runFileInput = this.container.querySelector('.workflow-run-file-input');
    const runDropzone = this.container.querySelector('.workflow-run-dropzone');
    const runFileList = this.container.querySelector('.workflow-run-file-list');

    btnBack?.addEventListener('click', () => this.goBack());
    btnSave?.addEventListener('click', () => this.saveWorkflow());
    btnRun?.addEventListener('click', () => this.openRunDialog());
    btnStop?.addEventListener('click', () => this.stopWorkflow());
    btnNew?.addEventListener('click', () => this.createNewWorkflow());

    lockSelect?.addEventListener('change', () => {
      if (!this.state.currentWorkflow) return;
      const value = lockSelect.value || null;
      this.state.currentWorkflow.default_workspace_id = value;
      this.markUnsaved();
    });

    this.renderTemplateLibrary();
    this.container.querySelectorAll('.workflow-template-item').forEach((button) => {
      button.addEventListener('click', () => {
        const templateId = button.dataset.templateId;
        this.createWorkflowFromTemplate(templateId);
      });
    });

    runClose?.addEventListener('click', () => this.closeRunDialog());
    runCancel?.addEventListener('click', () => this.closeRunDialog());
    runStart?.addEventListener('click', () => this.submitRunDialog());
    runPick?.addEventListener('click', () => runFileInput?.click());
    runModal?.querySelector('.workflow-run-modal-backdrop')?.addEventListener('click', () => this.closeRunDialog());

    runFileInput?.addEventListener('change', async (e) => {
      const files = Array.from(e.target.files || []);
      await this.addRunFiles(files);
      e.target.value = '';
    });

    runDropzone?.addEventListener('dragover', (e) => {
      e.preventDefault();
      runDropzone.classList.add('drag-over');
    });

    runDropzone?.addEventListener('dragleave', (e) => {
      e.preventDefault();
      runDropzone.classList.remove('drag-over');
    });

    runDropzone?.addEventListener('drop', async (e) => {
      e.preventDefault();
      runDropzone.classList.remove('drag-over');
      const files = Array.from(e.dataTransfer?.files || []);
      await this.addRunFiles(files);
    });

    runFileList?.addEventListener('click', (e) => {
      const removeBtn = e.target.closest('[data-remove-run-file]');
      if (!removeBtn) return;
      const attachmentId = removeBtn.dataset.removeRunFile;
      this.removeRunFile(attachmentId);
    });

    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && runModal && !runModal.classList.contains('hidden')) {
        this.closeRunDialog();
      }
    });

    this.state.on('currentWorkflowChanged', (workflow) => this.onWorkflowSelected(workflow));
    this.state.on('nodeStatesChanged', (states) => this.updateNodeVisuals(states));
  }

  async loadWorkflows() {
    try {
      const workflows = await this.api.listWorkflows();
      this.state.setWorkflows(workflows);
      this.renderWorkflowList(workflows);
      this.populateWorkflowLockSelect();
    } catch (error) {
      console.error('Failed to load workflows:', error);
    }
  }

  renderWorkflowList(workflows) {
    const list = this.container.querySelector('.workflow-list');
    if (!list) return;

    if (workflows.length === 0) {
      list.innerHTML = '<p class="empty-message">No workflows yet</p>';
      return;
    }

    list.innerHTML = workflows.map(w => {
      const safeName = this.escapeHtml(`${w.name || 'Untitled'}${w.default_workspace_id ? ' [locked]' : ''}`);
      const safeId = this.escapeAttribute(w.id || '');
      return `
      <div class="workflow-item${this.state.currentWorkflow?.id === w.id ? ' active' : ''}" data-id="${safeId}">
        <span class="workflow-name">${safeName}</span>
        <span class="workflow-version">v${Number(w.current_version || 1)}</span>
      </div>
    `;
    }).join('');

    list.querySelectorAll('.workflow-item').forEach(item => {
      item.addEventListener('click', () => {
        const id = item.dataset.id;
        const workflow = workflows.find(w => w.id === id);
        if (workflow) {
          this.state.setCurrentWorkflow(workflow);
        }
      });
    });
  }

  getWorkflowTemplates() {
    return [
      {
        id: 'summarize_file',
        name: 'Summarize File',
        description: 'Upload a file, get a concise summary',
      },
      {
        id: 'review_code_file',
        name: 'Review Code File',
        description: 'Upload code and get issues + fixes',
      },
      {
        id: 'extract_action_items',
        name: 'Extract Action Items',
        description: 'Turn docs into task lists',
      },
      {
        id: 'approval_before_save',
        name: 'Approval Before Save',
        description: 'Ask for approval before writing output file',
      },
      {
        id: 'batch_process_files',
        name: 'Batch Process Files',
        description: 'Summarize all uploaded files in one run',
      },
    ];
  }

  renderTemplateLibrary() {
    const container = this.container.querySelector('.workflow-template-list');
    if (!container) return;

    const templates = this.getWorkflowTemplates();
    container.innerHTML = templates.map((template) => {
      const safeName = window.OSA?.escapeHtml ? OSA.escapeHtml(template.name) : template.name;
      const safeDescription = window.OSA?.escapeHtml ? OSA.escapeHtml(template.description) : template.description;
      return `
        <button class="workflow-template-item" type="button" data-template-id="${template.id}">
          <span class="workflow-template-name">${safeName}</span>
          <span class="workflow-template-description">${safeDescription}</span>
        </button>
      `;
    }).join('');
  }

  escapeHtml(value) {
    return String(value ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  escapeAttribute(value) {
    return this.escapeHtml(value);
  }

  fieldValue(value, fallback = '') {
    return this.escapeHtml(value ?? fallback);
  }

  getPrimaryInputToken(selectedNode) {
    if (!selectedNode || !this.adapter?.graph) {
      return '{{input}}';
    }

    const selectedLiteId = Number(selectedNode.id);
    const rawLinks = this.adapter.graph.links;
    const links = Array.isArray(rawLinks) ? rawLinks : Object.values(rawLinks || {});

    for (const entry of links) {
      let sourceNodeLiteId;
      let targetNodeLiteId;

      if (Array.isArray(entry)) {
        if (entry.length < 5) continue;
        sourceNodeLiteId = Number(entry[1]);
        targetNodeLiteId = Number(entry[3]);
      } else if (entry && typeof entry === 'object') {
        sourceNodeLiteId = Number(entry.origin_id);
        targetNodeLiteId = Number(entry.target_id);
      } else {
        continue;
      }

      if (targetNodeLiteId !== selectedLiteId) continue;

      const sourceNode = this.adapter.getNode(sourceNodeLiteId);
      const sourceType = sourceNode?.constructor?.type || sourceNode?.type;
      if (sourceType === 'osa/file_input') return '{{input.content}}';
      if (sourceType === 'osa/agent') return '{{input.result}}';
      if (sourceType === 'osa/output') return '{{input.output}}';
    }

    return '{{input}}';
  }

  populateWorkflowLockSelect() {
    const select = this.container.querySelector('.workflow-lock-select');
    if (!select) return;

    const workspaces = (window.OSA && typeof OSA.getWorkspaceState === 'function')
      ? (OSA.getWorkspaceState()?.workspaces || [])
      : [];

    const options = ['<option value="">No lock</option>'];
    workspaces.forEach((workspace) => {
      const id = workspace.id || 'default';
      const name = workspace.name || id;
      const safeId = window.OSA?.escapeHtml ? OSA.escapeHtml(id) : id;
      const safeName = window.OSA?.escapeHtml ? OSA.escapeHtml(name) : name;
      options.push(`<option value="${safeId}">${safeName}</option>`);
    });
    select.innerHTML = options.join('');
  }

  applyWorkflowLockToToolbar() {
    const select = this.container.querySelector('.workflow-lock-select');
    if (!select) return;
    const lockedWorkspaceId = this.state.currentWorkflow?.default_workspace_id || '';

    const hasOption = Array.from(select.options).some((option) => option.value === lockedWorkspaceId);
    if (lockedWorkspaceId && !hasOption) {
      const option = document.createElement('option');
      option.value = lockedWorkspaceId;
      option.textContent = `${lockedWorkspaceId} (missing)`;
      select.appendChild(option);
    }

    select.value = lockedWorkspaceId;
  }

  createTemplateNode(id, type, pos, properties = {}, size = [180, 80]) {
    return {
      id,
      type,
      pos,
      size,
      properties: {
        node_id: `template_node_${id}`,
        ...properties,
      },
      flags: {}
    };
  }

  buildTemplateGraph(templateId) {
    const link = (id, sourceNode, sourceSlot, targetNode, targetSlot) => [
      id,
      sourceNode,
      sourceSlot,
      targetNode,
      targetSlot,
      'flow',
    ];

    if (templateId === 'summarize_file') {
      return {
        nodes: [
          this.createTemplateNode(1, 'osa/trigger', [80, 180], { trigger_type: 'manual' }, [140, 60]),
          this.createTemplateNode(2, 'osa/file_input', [300, 170], { use_attachment: true, attachment_index: 0 }, [170, 70]),
          this.createTemplateNode(3, 'osa/agent', [540, 150], {
            agent_id: 'main',
            task_template: 'Summarize this file in 5 clear bullet points:\n\n{{input.content}}',
          }, [220, 110]),
          this.createTemplateNode(4, 'osa/output', [820, 180], {
            format: 'text',
            template: '{{input.result}}',
          }, [160, 70]),
        ],
        links: [
          link(1, 1, 0, 2, 0),
          link(2, 2, 0, 3, 0),
          link(3, 3, 0, 4, 0),
        ],
      };
    }

    if (templateId === 'review_code_file') {
      return {
        nodes: [
          this.createTemplateNode(1, 'osa/trigger', [80, 180], { trigger_type: 'manual' }, [140, 60]),
          this.createTemplateNode(2, 'osa/file_input', [300, 170], { use_attachment: true, attachment_index: 0 }, [170, 70]),
          this.createTemplateNode(3, 'osa/agent', [540, 140], {
            agent_id: 'main',
            task_template: 'Review this code file for bugs, risky patterns, and maintainability issues. Return:\n1) top issues\n2) why\n3) concrete fix suggestions\n\nCode:\n{{input.content}}',
          }, [240, 130]),
          this.createTemplateNode(4, 'osa/output', [840, 180], {
            format: 'text',
            template: '{{input.result}}',
          }, [160, 70]),
        ],
        links: [
          link(1, 1, 0, 2, 0),
          link(2, 2, 0, 3, 0),
          link(3, 3, 0, 4, 0),
        ],
      };
    }

    if (templateId === 'extract_action_items') {
      return {
        nodes: [
          this.createTemplateNode(1, 'osa/trigger', [80, 180], { trigger_type: 'manual' }, [140, 60]),
          this.createTemplateNode(2, 'osa/file_input', [300, 170], { use_attachment: true, attachment_index: 0 }, [170, 70]),
          this.createTemplateNode(3, 'osa/agent', [540, 140], {
            agent_id: 'main',
            task_template: 'Extract action items from this file. For each item include: owner (if known), due date (if known), and priority.\n\nFile:\n{{input.content}}',
          }, [240, 120]),
          this.createTemplateNode(4, 'osa/output', [840, 180], {
            format: 'text',
            template: '{{input.result}}',
          }, [160, 70]),
        ],
        links: [
          link(1, 1, 0, 2, 0),
          link(2, 2, 0, 3, 0),
          link(3, 3, 0, 4, 0),
        ],
      };
    }

    if (templateId === 'approval_before_save') {
      return {
        nodes: [
          this.createTemplateNode(1, 'osa/trigger', [60, 220], { trigger_type: 'manual' }, [140, 60]),
          this.createTemplateNode(2, 'osa/file_input', [260, 210], { use_attachment: true, attachment_index: 0 }, [170, 70]),
          this.createTemplateNode(3, 'osa/agent', [500, 180], {
            agent_id: 'main',
            task_template: 'Create a cleaned summary from this file:\n\n{{input.content}}',
          }, [220, 100]),
          this.createTemplateNode(4, 'osa/approval', [760, 200], {
            prompt: 'Approve saving this generated output to a file?',
            approve_label: 'Save',
            reject_label: 'Cancel',
          }, [200, 90]),
          this.createTemplateNode(5, 'osa/file_output', [1020, 130], {
            path: 'output/approved_result.txt',
            content_template: '{{input.input.result}}',
            create_dirs: true,
          }, [220, 90]),
          this.createTemplateNode(6, 'osa/output', [1020, 290], {
            format: 'text',
            template: 'Not saved (approval denied).',
          }, [180, 70]),
        ],
        links: [
          link(1, 1, 0, 2, 0),
          link(2, 2, 0, 3, 0),
          link(3, 3, 0, 4, 0),
          link(4, 4, 0, 5, 0),
          link(5, 4, 1, 6, 0),
        ],
      };
    }

    return {
      nodes: [
        this.createTemplateNode(1, 'osa/trigger', [80, 180], { trigger_type: 'manual' }, [140, 60]),
        this.createTemplateNode(2, 'osa/agent', [320, 130], {
          agent_id: 'main',
          task_template: 'For each uploaded file in {{trigger_attachment_contents}}, provide a short summary and key risks.',
        }, [260, 110]),
        this.createTemplateNode(3, 'osa/output', [660, 180], {
          format: 'text',
          template: '{{input.result}}',
        }, [160, 70]),
      ],
      links: [
        link(1, 1, 0, 2, 0),
        link(2, 2, 0, 3, 0),
      ],
    };
  }

  async createWorkflowFromTemplate(templateId) {
    const templates = this.getWorkflowTemplates();
    const template = templates.find((item) => item.id === templateId);
    if (!template) return;

    const defaultName = template.name;
    const name = prompt('Name this workflow:', defaultName);
    if (!name) return;

    const graph = this.buildTemplateGraph(templateId);
    const graphJson = JSON.stringify(graph);
    const defaultWorkspaceId = this.getActiveWorkspaceId();

    try {
      const workflow = await this.api.createWorkflow(name, template.description, {
        graphJson,
        defaultWorkspaceId,
      });
      this.state.setCurrentWorkflow(workflow);
      await this.loadWorkflows();
      this.state.setCurrentWorkflow(workflow);
    } catch (error) {
      console.error('Failed to create workflow template:', error);
      alert('Failed to create workflow template: ' + error.message);
    }
  }

  async onWorkflowSelected(workflow) {
    if (!workflow) return;

    try {
      if (this.adapter) {
        this.adapter.destroy();
        this.adapter = null;
      }

      this.renderWorkflowList(this.state.workflows || []);

      const version = await this.api.getVersion(workflow.id, workflow.current_version);
      this.state.setCurrentVersion(version);

      const canvasContainer = this.container.querySelector('.workflow-canvas-container');
      canvasContainer.innerHTML = '<div class="workflow-canvas"></div>';
      canvasContainer.querySelector('.workflow-canvas').style.width = '100%';
      canvasContainer.querySelector('.workflow-canvas').style.height = '500px';

      this.adapter = new LitegraphAdapter(canvasContainer.querySelector('.workflow-canvas'));
      
      await this.adapter.init();
      this.adapter.registerNodes();

      this.executor = new ExecutionManager(this.adapter, this.state);
      this.executor.on('nodeStatesChanged', (states) => this.state.emit('nodeStatesChanged', states));
      this.executor.on('executionCompleted', () => this.onExecutionFinished());
      this.executor.on('executionFailed', (data) => this.onExecutionFailed(data));
      this.executor.on('executionError', (data) => this.onExecutionFailed(data));
      this.executor.on('executionStopped', () => this.onExecutionFinished());
      this.executor.on('executionCancelled', () => this.onExecutionFinished());
      
      this.adapter.onNodeSelect((node) => {
        this.selectedNode = node;
        this.showNodeProperties(node);
      });

      this.adapter.onGraphChange(() => {
        this.markUnsaved();
      });

      if (version && version.graph_json) {
        this.adapter.deserialize(version.graph_json);
      }

      this.populateWorkflowLockSelect();
      this.applyWorkflowLockToToolbar();

      this.showCanvas();
    } catch (error) {
      console.error('Failed to load workflow:', error);
    }
  }

  showNodeProperties(node) {
    const placeholder = this.container.querySelector('.properties-placeholder');
    const content = this.container.querySelector('.properties-content');

    if (!node) {
      placeholder?.classList.remove('hidden');
      content?.classList.add('hidden');
      return;
    }

    placeholder?.classList.add('hidden');
    content?.classList.remove('hidden');

    const type = node.constructor.type || node.type;
    const helpText = this.getNodeHelp(type);
    let propertiesHtml = `<h4>${node.constructor.title || 'Node'}</h4>`;
    if (helpText) {
      propertiesHtml += `<p class="property-help">${this.escapeHtml(helpText)}</p>`;
    }

    switch (type) {
      case 'osa/trigger':
        propertiesHtml += `
          <div class="property-group">
            <label>What this does</label>
            <div class="property-note">This starts the workflow when you click Run (or trigger it from Discord).</div>
          </div>
        `;
        break;

      case 'osa/agent':
        propertiesHtml += `
          <div class="property-group">
            <label>What should this step do?</label>
            <textarea data-prop="task_template" placeholder="Example: Summarize the input in 5 bullet points.">${this.fieldValue(node.properties?.task_template, '{{input}}')}</textarea>
            ${this.renderDataPicker('task_template', node)}
          </div>
          <div class="property-group">
            <label>Quick Start</label>
            <div class="property-inline-actions property-preset-actions">
              <button type="button" class="property-mini-btn" data-ui="agent_preset_btn" data-preset="summarize">Summarize</button>
              <button type="button" class="property-mini-btn" data-ui="agent_preset_btn" data-preset="extract">Extract</button>
              <button type="button" class="property-mini-btn" data-ui="agent_preset_btn" data-preset="rewrite">Rewrite</button>
              <button type="button" class="property-mini-btn" data-ui="agent_preset_btn" data-preset="review">Review</button>
              <button type="button" class="property-mini-btn" data-insert-template="{{input}}">Insert input</button>
              <button type="button" class="property-mini-btn" data-insert-template="{{trigger_input}}">Insert trigger input</button>
            </div>
          </div>
          <details class="property-advanced">
            <summary>Advanced options</summary>
            <div class="property-group">
              <label>Agent ID</label>
              <input type="text" data-prop="agent_id" value="${this.fieldValue(node.properties?.agent_id, 'main')}" placeholder="main">
            </div>
            <div class="property-group">
              <label>System Prompt (optional)</label>
              <textarea data-prop="system_prompt" placeholder="Optional custom behavior instructions">${this.fieldValue(node.properties?.system_prompt, '')}</textarea>
            </div>
            <div class="property-group">
              <label>File Context (optional)</label>
              <textarea data-prop="file_context" placeholder="Example: {{input.content}}">${this.fieldValue(node.properties?.file_context, '')}</textarea>
              ${this.renderDataPicker('file_context', node)}
            </div>
          </details>
        `;
        break;

      case 'osa/condition': {
        const conditionState = this.parseConditionExpression(node.properties?.expression || 'true');
        propertiesHtml += `
          <div class="property-group">
            <label>Decision Type</label>
            <select data-ui="condition_mode">
              <option value="always_true"${conditionState.mode === 'always_true' ? ' selected' : ''}>Always true</option>
              <option value="always_false"${conditionState.mode === 'always_false' ? ' selected' : ''}>Always false</option>
              <option value="equals"${conditionState.mode === 'equals' ? ' selected' : ''}>Text equals</option>
              <option value="contains"${conditionState.mode === 'contains' ? ' selected' : ''}>Text contains</option>
              <option value="custom"${conditionState.mode === 'custom' ? ' selected' : ''}>Custom expression</option>
            </select>
          </div>
          <div class="property-group condition-pair-inputs${conditionState.mode === 'equals' || conditionState.mode === 'contains' ? '' : ' hidden'}" data-ui="condition_pair_group">
            <label>Left side (usually input text)</label>
            <input type="text" data-ui="condition_left" value="${this.fieldValue(conditionState.left, '{{input}}')}" placeholder="{{input}}">
            <label>Right side (text to match)</label>
            <input type="text" data-ui="condition_right" value="${this.fieldValue(conditionState.right, '')}" placeholder="keyword">
          </div>
          <div class="property-group condition-custom-input${conditionState.mode === 'custom' ? '' : ' hidden'}" data-ui="condition_custom_group">
            <label>Custom expression</label>
            <textarea data-ui="condition_custom" placeholder="Examples: true, false, {{trigger_input}} = deploy, contains({{input}}, error)">${this.fieldValue(conditionState.custom, '')}</textarea>
          </div>
          <div class="property-group">
            <label>Generated Expression</label>
            <input type="text" data-prop="expression" readonly value="${this.fieldValue(node.properties?.expression || conditionState.generated, 'true')}">
          </div>
        `;
        break;
      }

      case 'osa/transform':
        propertiesHtml += `
          <div class="property-group">
            <label>Output Template</label>
            <textarea data-prop="script" placeholder="Example: # Summary\n{{input}}">${this.fieldValue(node.properties?.script, '{{input}}')}</textarea>
            ${this.renderDataPicker('script', node)}
            <div class="property-inline-actions">
              <button type="button" class="property-mini-btn" data-insert-template="{{input}}">Insert input</button>
            </div>
          </div>
        `;
        break;

      case 'osa/delay':
        propertiesHtml += `
          <div class="property-group">
            <label>Wait Time (milliseconds)</label>
            <input type="number" min="0" step="100" data-prop="milliseconds" data-value-type="number" value="${Number(node.properties?.milliseconds || 1000)}">
            <div class="property-note">1000 ms = 1 second.</div>
          </div>
        `;
        break;

      case 'osa/output':
        propertiesHtml += `
          <div class="property-group">
            <label>Output Format</label>
            <select data-prop="format">
              <option value="text"${(node.properties?.format || 'text') === 'text' ? ' selected' : ''}>Readable text</option>
              <option value="json"${node.properties?.format === 'json' ? ' selected' : ''}>Raw JSON</option>
            </select>
          </div>
          <div class="property-group">
            <label>What should the final result show?</label>
            <textarea data-prop="template" placeholder="Example: Done!\n\n{{input.result}}">${this.fieldValue(node.properties?.template, '{{input}}')}</textarea>
            ${this.renderDataPicker('template', node)}
            <div class="property-inline-actions">
              <button type="button" class="property-mini-btn" data-insert-template="{{input}}">Insert previous step</button>
              <button type="button" class="property-mini-btn" data-insert-template="{{trigger_input}}">Insert trigger input</button>
            </div>
          </div>
        `;
        break;

      case 'osa/file_input': {
        const fileNumber = Math.max(1, Number(node.properties?.attachment_index || 0) + 1);
        propertiesHtml += `
          <div class="property-group">
            <label>Which uploaded file should this step read?</label>
            <input type="number" min="1" step="1" data-ui="file_input_attachment_number" value="${fileNumber}">
            <div class="property-note">Pick files in the Run dialog. 1 = first uploaded file, 2 = second, etc.</div>
            <div class="property-inline-actions">
              <button type="button" class="property-mini-btn" data-ui="file_input_open_run_dialog">Open file picker</button>
            </div>
          </div>
          <input type="hidden" data-prop="use_attachment" data-value-type="boolean" value="true">
          <input type="hidden" data-prop="attachment_index" data-value-type="number" value="${Number(node.properties?.attachment_index || 0)}">
          <input type="hidden" data-prop="path" value="">
        `;
        break;
      }

      case 'osa/file_output':
        propertiesHtml += `
          <div class="property-group">
            <label>Save result to path</label>
            <input type="text" data-prop="path" value="${this.fieldValue(node.properties?.path, '')}" placeholder="Example: output/summary.md">
          </div>
          <div class="property-group">
            <label>Content to write</label>
            <textarea data-prop="content_template" placeholder="Example: {{input.result}}">${this.fieldValue(node.properties?.content_template, '{{input}}')}</textarea>
            ${this.renderDataPicker('content_template', node)}
            <div class="property-inline-actions">
              <button type="button" class="property-mini-btn" data-insert-template="{{input}}">Insert previous step</button>
              <button type="button" class="property-mini-btn" data-insert-template="{{input.content}}">Insert file content</button>
            </div>
          </div>
          <div class="property-group property-check">
            <label>
              <input type="checkbox" data-prop="create_dirs" data-value-type="boolean" ${node.properties?.create_dirs !== false ? 'checked' : ''}>
              Auto-create missing folders
            </label>
          </div>
        `;
        break;

      case 'osa/approval':
        propertiesHtml += `
          <div class="property-group">
            <label>Approval prompt</label>
            <textarea data-prop="prompt" placeholder="Example: Ready to publish this result?">${this.fieldValue(node.properties?.prompt, 'Approve workflow step?')}</textarea>
          </div>
          <div class="property-group property-split">
            <div>
              <label>Approve button text</label>
              <input type="text" data-prop="approve_label" value="${this.fieldValue(node.properties?.approve_label, 'Approve')}">
            </div>
            <div>
              <label>Reject button text</label>
              <input type="text" data-prop="reject_label" value="${this.fieldValue(node.properties?.reject_label, 'Reject')}">
            </div>
          </div>
        `;
        break;

      case 'osa/foreach':
        propertiesHtml += `
          <div class="property-group">
            <label>List to loop through</label>
            <textarea data-prop="items_template" placeholder='Example: ["a", "b", "c"] or {{input.items}}'>${this.fieldValue(node.properties?.items_template, '{{input}}')}</textarea>
            ${this.renderDataPicker('items_template', node)}
            <div class="property-note">Use a JSON array when possible for predictable results.</div>
          </div>
          <div class="property-group">
            <label>Name for each item</label>
            <input type="text" data-prop="item_variable" value="${this.fieldValue(node.properties?.item_variable, 'item')}" placeholder="item">
          </div>
        `;
        break;

      default:
        propertiesHtml += '<p>No configurable properties</p>';
    }

    propertiesHtml += `
      <button class="btn-delete-node" data-node-id="${node.id}">Delete Node</button>
    `;

    content.innerHTML = propertiesHtml;

    content.querySelectorAll('[data-prop]').forEach(input => {
      const eventName = input.tagName === 'TEXTAREA' || input.type === 'text' ? 'input' : 'change';
      input.addEventListener(eventName, (e) => {
        const propName = e.target.dataset.prop;
        const valueType = e.target.dataset.valueType;
        let value;

        if (e.target.type === 'checkbox') {
          value = !!e.target.checked;
        } else {
          value = e.target.value;
        }

        if (valueType === 'number') {
          value = Number(value || 0);
        }

        if (valueType === 'boolean' && e.target.type !== 'checkbox') {
          value = String(value).toLowerCase() === 'true';
        }

        this.adapter.setNodeProperty(node.id, propName, value);
      });
    });

    this.bindTemplateInsertButtons(content, node.id);

    if (type === 'osa/condition') {
      this.bindConditionEditor(content, node.id);
    }

    if (type === 'osa/file_input') {
      this.bindFileInputEditor(content, node.id);
    }

    if (type === 'osa/agent') {
      this.bindAgentPresetPicker(content, node.id);
    }

    const deleteBtn = content.querySelector('.btn-delete-node');
    deleteBtn?.addEventListener('click', () => {
      this.adapter.removeNode(node.id);
      this.selectedNode = null;
      placeholder?.classList.remove('hidden');
      content?.classList.add('hidden');
    });
  }

  getNodeHelp(type) {
    const helps = {
      'osa/trigger': 'Start point for the workflow.',
      'osa/agent': 'Use AI to process text or file content from previous steps.',
      'osa/condition': 'Send the flow to True or False path based on a rule.',
      'osa/transform': 'Reformat or reshape text without creating a new AI call.',
      'osa/delay': 'Pause before continuing to the next step.',
      'osa/output': 'Show the final result.',
      'osa/file_input': 'Use a file uploaded specifically for this workflow run.',
      'osa/file_output': 'Write workflow output to a file.',
      'osa/approval': 'Pause and ask a human to approve or reject.',
      'osa/foreach': 'Loop through each item in a list.',
    };
    return helps[type] || '';
  }

  parseConditionExpression(expression) {
    const raw = String(expression || '').trim();
    if (!raw || raw.toLowerCase() === 'true') {
      return { mode: 'always_true', left: '{{input}}', right: '', custom: '', generated: 'true' };
    }
    if (raw.toLowerCase() === 'false') {
      return { mode: 'always_false', left: '{{input}}', right: '', custom: '', generated: 'false' };
    }

    const containsMatch = raw.match(/^contains\((.*?),(.*)\)$/i);
    if (containsMatch) {
      return {
        mode: 'contains',
        left: containsMatch[1].trim(),
        right: containsMatch[2].trim(),
        custom: raw,
        generated: raw,
      };
    }

    const eqMatch = raw.match(/^(.*?)\s*=\s*(.*)$/);
    if (eqMatch) {
      return {
        mode: 'equals',
        left: eqMatch[1].trim(),
        right: eqMatch[2].trim(),
        custom: raw,
        generated: raw,
      };
    }

    return {
      mode: 'custom',
      left: '{{input}}',
      right: '',
      custom: raw,
      generated: raw,
    };
  }

  buildConditionExpression(mode, left, right, custom) {
    if (mode === 'always_true') return 'true';
    if (mode === 'always_false') return 'false';
    if (mode === 'contains') return `contains(${(left || '{{input}}').trim()}, ${(right || '').trim()})`;
    if (mode === 'equals') return `${(left || '{{input}}').trim()} = ${(right || '').trim()}`;
    return (custom || 'true').trim();
  }

  bindConditionEditor(content, nodeId) {
    const modeInput = content.querySelector('[data-ui="condition_mode"]');
    const leftInput = content.querySelector('[data-ui="condition_left"]');
    const rightInput = content.querySelector('[data-ui="condition_right"]');
    const customInput = content.querySelector('[data-ui="condition_custom"]');
    const pairGroup = content.querySelector('[data-ui="condition_pair_group"]');
    const customGroup = content.querySelector('[data-ui="condition_custom_group"]');
    const expressionPreview = content.querySelector('[data-prop="expression"]');

    const sync = () => {
      const mode = modeInput?.value || 'always_true';
      const expression = this.buildConditionExpression(
        mode,
        leftInput?.value || '{{input}}',
        rightInput?.value || '',
        customInput?.value || ''
      );

      this.adapter.setNodeProperty(nodeId, 'expression', expression);
      if (expressionPreview) {
        expressionPreview.value = expression;
      }

      if (pairGroup) {
        pairGroup.classList.toggle('hidden', !(mode === 'equals' || mode === 'contains'));
      }
      if (customGroup) {
        customGroup.classList.toggle('hidden', mode !== 'custom');
      }
    };

    modeInput?.addEventListener('change', sync);
    leftInput?.addEventListener('input', sync);
    rightInput?.addEventListener('input', sync);
    customInput?.addEventListener('input', sync);
    sync();
  }

  bindFileInputEditor(content, nodeId) {
    const attachmentNumber = content.querySelector('[data-ui="file_input_attachment_number"]');
    const openRunDialogBtn = content.querySelector('[data-ui="file_input_open_run_dialog"]');

    const sync = () => {
      const attachmentIndex = Math.max(0, Number(attachmentNumber?.value || 1) - 1);

      this.adapter.setNodeProperty(nodeId, 'use_attachment', true);
      this.adapter.setNodeProperty(nodeId, 'attachment_index', attachmentIndex);
      this.adapter.setNodeProperty(nodeId, 'path', '');

      const hiddenUseAttachment = content.querySelector('[data-prop="use_attachment"]');
      const hiddenAttachmentIndex = content.querySelector('[data-prop="attachment_index"]');
      const hiddenPath = content.querySelector('[data-prop="path"]');
      if (hiddenUseAttachment) hiddenUseAttachment.value = 'true';
      if (hiddenAttachmentIndex) hiddenAttachmentIndex.value = String(attachmentIndex);
      if (hiddenPath) hiddenPath.value = '';
    };

    attachmentNumber?.addEventListener('input', sync);
    openRunDialogBtn?.addEventListener('click', () => this.openRunDialog({ preserveState: true, pickFiles: true }));
    sync();
  }

  bindAgentPresetPicker(content, nodeId) {
    const presetButtons = content.querySelectorAll('[data-ui="agent_preset_btn"]');
    const taskTemplate = content.querySelector('[data-prop="task_template"]');
    if (!presetButtons.length || !taskTemplate) return;

    const inputToken = this.getPrimaryInputToken(this.selectedNode);
    const presets = {
      summarize: `Summarize this in 5 clear bullet points:\n\n${inputToken}`,
      extract: `Extract the key facts and action items from this:\n\n${inputToken}`,
      rewrite: `Rewrite this to be clearer and easier to read:\n\n${inputToken}`,
      review: `Review this for problems and suggest fixes:\n\n${inputToken}`,
    };

    presetButtons.forEach((button) => {
      button.addEventListener('click', () => {
        const value = button.dataset.preset;
        if (!value || !presets[value]) return;
        taskTemplate.value = presets[value];
        this.adapter.setNodeProperty(nodeId, 'task_template', taskTemplate.value);
      });
    });
  }

  bindTemplateInsertButtons(content, nodeId) {
    const buttons = content.querySelectorAll('[data-insert-template], [data-picker-token]');
    if (!buttons.length) return;

    buttons.forEach((button) => {
      button.addEventListener('click', () => {
        const token = button.dataset.insertTemplate || button.dataset.pickerToken || '';
        const targetProp = button.dataset.targetProp || null;
        const targetSelector = targetProp
          ? `textarea[data-prop="${targetProp}"]`
          : 'textarea[data-prop="task_template"], textarea[data-prop="template"], textarea[data-prop="content_template"], textarea[data-prop="script"]';
        const target = content.querySelector(targetSelector);
        if (!target) return;

        const start = target.selectionStart ?? target.value.length;
        const end = target.selectionEnd ?? target.value.length;
        const before = target.value.slice(0, start);
        const after = target.value.slice(end);
        const spacer = before.endsWith(' ') || before.endsWith('\n') || before.length === 0 ? '' : ' ';
        target.value = `${before}${spacer}${token}${after}`;
        target.focus();

        const propName = target.dataset.prop;
        if (propName) {
          this.adapter.setNodeProperty(nodeId, propName, target.value);
        }
      });
    });
  }

  getDataPickerTokens(selectedNode) {
    const tokens = [
      { label: 'Previous step', token: '{{input}}' },
      { label: 'Previous result text', token: '{{input.result}}' },
      { label: 'Loaded file content', token: '{{input.content}}' },
      { label: 'Run input', token: '{{trigger_input}}' },
      { label: 'All uploaded file text', token: '{{trigger_attachment_contents}}' },
      { label: 'Uploaded files metadata', token: '{{trigger_attachments}}' },
      { label: 'Loop item', token: '{{item}}' },
    ];

    const nodes = this.adapter?.getNodes?.() || [];
    nodes.forEach((node) => {
      const type = node?.constructor?.type || node?.type;
      const nodeLabel = node?.title || node?.properties?.node_id || `Node ${node?.id}`;
      const nodeKey = node?.properties?.node_id || String(node?.id || '');
      if (!nodeKey) return;

      if (type === 'osa/agent') {
        tokens.push({ label: `${nodeLabel} result`, token: `{{agent_${nodeKey}_result.result}}` });
      }
      if (type === 'osa/file_input') {
        tokens.push({ label: `${nodeLabel} content`, token: `{{input.by_node.${nodeKey}.content}}` });
      }
    });

    if (this.adapter?.graph?.links && selectedNode) {
      const selectedLiteId = Number(selectedNode.id);
      const rawLinks = this.adapter.graph.links;
      const links = Array.isArray(rawLinks)
        ? rawLinks
        : Object.values(rawLinks || {});
      links.forEach((entry) => {
        let sourceNodeLiteId;
        let targetNodeLiteId;

        if (Array.isArray(entry)) {
          if (entry.length < 5) return;
          sourceNodeLiteId = Number(entry[1]);
          targetNodeLiteId = Number(entry[3]);
        } else if (entry && typeof entry === 'object') {
          sourceNodeLiteId = Number(entry.origin_id);
          targetNodeLiteId = Number(entry.target_id);
        } else {
          return;
        }

        if (targetNodeLiteId !== selectedLiteId) return;

        const sourceNode = this.adapter.getNode(sourceNodeLiteId);
        const sourceKey = sourceNode?.properties?.node_id || String(sourceNode?.id || '');
        if (!sourceKey) return;

        tokens.push({
          label: `From ${sourceNode?.title || sourceKey}`,
          token: `{{input.by_node.${sourceKey}}}`,
        });
        tokens.push({
          label: `From ${sourceNode?.title || sourceKey} result`,
          token: `{{input.by_node.${sourceKey}.result}}`,
        });
      });
    }

    const deduped = new Map();
    tokens.forEach((item) => {
      if (!item?.token) return;
      if (!deduped.has(item.token)) {
        deduped.set(item.token, item);
      }
    });
    return Array.from(deduped.values());
  }

  renderDataPicker(targetProp, selectedNode) {
    const tokens = this.getDataPickerTokens(selectedNode);
    const chips = tokens.map((item) => {
      const safeLabel = window.OSA?.escapeHtml ? OSA.escapeHtml(item.label) : item.label;
      const safeToken = window.OSA?.escapeHtml ? OSA.escapeHtml(item.token) : item.token;
      return `<button type="button" class="data-picker-chip" data-target-prop="${targetProp}" data-picker-token="${safeToken}">${safeLabel}</button>`;
    }).join('');

    return `
      <div class="data-picker">
        <div class="data-picker-title">Insert data</div>
        <div class="data-picker-chips">${chips}</div>
      </div>
    `;
  }

  updateNodeVisuals(states) {
    if (!this.adapter) return;

    this.adapter.getNodes().forEach(node => {
      const state = states.get(node.id) || states.get(String(node.id));
      if (state) {
        node.executionStatus = state;
      }
    });
  }

  showCanvas() {
    const canvasContainer = this.container.querySelector('.workflow-canvas-container');
    const placeholder = canvasContainer.querySelector('.canvas-placeholder');
    if (placeholder) {
      placeholder.style.display = 'none';
    }
  }

  markUnsaved() {
    const title = this.container.querySelector('.toolbar-title');
    if (title && !title.textContent.includes('*')) {
      title.textContent = title.textContent + ' *';
    }
  }

  async saveWorkflow() {
    if (!this.state.currentWorkflow) {
      await this.createNewWorkflow();
      return !!this.state.currentWorkflow;
    }

    if (!this.adapter) return false;

    const graphJson = this.adapter.serialize();
    const lockSelect = this.container.querySelector('.workflow-lock-select');
    const defaultWorkspaceId = lockSelect?.value || null;

    try {
      await this.api.updateWorkflow(this.state.currentWorkflow.id, {
        graphJson,
        defaultWorkspaceId,
      });
      this.state.currentWorkflow.default_workspace_id = defaultWorkspaceId;
      const title = this.container.querySelector('.toolbar-title');
      title.textContent = title.textContent.replace(' *', '');
      await this.loadWorkflows();
      return true;
    } catch (error) {
      console.error('Failed to save workflow:', error);
      alert('Failed to save workflow: ' + error.message);
      return false;
    }
  }

  openRunDialog(options = {}) {
    if (!this.state.currentWorkflow) return;
    const modal = this.container.querySelector('.workflow-run-modal');
    if (!modal) return;

    this.populateRunWorkspaceSelect();

    if (!options.preserveState) {
      this.runAttachments = [];
      this.renderRunAttachmentList();
      this.setRunStatus('');

      const input = this.container.querySelector('.workflow-run-input');
      if (input) input.value = '';
    }

    modal.classList.remove('hidden');
    modal.setAttribute('aria-hidden', 'false');

    if (options.pickFiles) {
      const runFileInput = this.container.querySelector('.workflow-run-file-input');
      setTimeout(() => runFileInput?.click(), 0);
    }
  }

  closeRunDialog(options = {}) {
    const modal = this.container.querySelector('.workflow-run-modal');
    if (!modal) return;

    const shouldClear = options.clearState !== false;

    modal.classList.add('hidden');
    modal.setAttribute('aria-hidden', 'true');

    if (shouldClear) {
      const input = this.container.querySelector('.workflow-run-input');
      if (input) input.value = '';

      this.runAttachments = [];
      this.renderRunAttachmentList();
      this.setRunStatus('');
    }
  }

  setRunStatus(message, tone = 'info') {
    const status = this.container.querySelector('.workflow-run-status');
    if (!status) return;

    if (!message) {
      status.textContent = '';
      status.classList.add('hidden');
      status.dataset.state = '';
      return;
    }

    status.textContent = message;
    status.classList.remove('hidden');
    status.dataset.state = tone;
  }

  async submitRunDialog() {
    const startBtn = this.container.querySelector('.workflow-run-start');
    if (startBtn) startBtn.disabled = true;

    const input = this.container.querySelector('.workflow-run-input');
    const workspaceSelect = this.container.querySelector('.workflow-run-workspace');
    const triggerInput = input?.value?.trim() || '';
    const workspaceId = workspaceSelect?.value || this.getActiveWorkspaceId();

    const parameters = {};
    if (triggerInput) {
      parameters.trigger_input = triggerInput;
    }

    const attachments = this.runAttachments
      .filter((att) => att.kind !== 'image')
      .map((att) => ({
        filename: att.filename,
        mime: att.mime,
        data_url: att.dataUrl,
      }));

    const images = this.runAttachments
      .filter((att) => att.kind === 'image')
      .map((att) => ({
        filename: att.filename,
        mime: att.mime,
        data_url: att.dataUrl,
      }));

    this.closeRunDialog({ clearState: true });

    try {
      await this.runWorkflow({
        parameters,
        attachments,
        images,
        workspaceId,
        sessionMode: 'workflow',
      });
    } finally {
      if (startBtn) startBtn.disabled = false;
    }
  }

  getMaxAttachmentSize() {
    return (window.OSA && Number(OSA.MAX_ATTACHMENT_SIZE)) || (12 * 1024 * 1024);
  }

  getActiveWorkspaceId() {
    if (window.OSA && typeof OSA.getWorkspaceState === 'function') {
      const ws = OSA.getWorkspaceState();
      return ws?.activeWorkspace || 'default';
    }
    return 'default';
  }

  populateRunWorkspaceSelect() {
    const select = this.container.querySelector('.workflow-run-workspace');
    if (!select) return;

    const lockHint = this.container.querySelector('.workflow-run-workspace-lock-hint');

    const activeId = this.getActiveWorkspaceId();
    const lockedWorkspaceId = this.state.currentWorkflow?.default_workspace_id || null;
    const workspaces = (window.OSA && typeof OSA.getWorkspaceState === 'function')
      ? (OSA.getWorkspaceState()?.workspaces || [])
      : [];

    if (!workspaces.length) {
      select.innerHTML = '<option value="default">default</option>';
      select.value = lockedWorkspaceId || 'default';
      select.disabled = !!lockedWorkspaceId;
      if (lockHint) {
        lockHint.textContent = lockedWorkspaceId
          ? `Workflow locked to workspace: ${lockedWorkspaceId}`
          : '';
        lockHint.classList.toggle('hidden', !lockedWorkspaceId);
      }
      return;
    }

    const options = workspaces.map((workspace) => {
      const id = workspace.id || 'default';
      const label = workspace.name || id;
      const escapedId = window.OSA && typeof OSA.escapeHtml === 'function' ? OSA.escapeHtml(id) : id;
      const escapedLabel = window.OSA && typeof OSA.escapeHtml === 'function' ? OSA.escapeHtml(label) : label;
      return `<option value="${escapedId}">${escapedLabel}</option>`;
    });
    select.innerHTML = options.join('');
    const fallbackValue = workspaces.some((workspace) => workspace.id === activeId)
      ? activeId
      : (workspaces[0].id || 'default');

    const lockExists = lockedWorkspaceId && workspaces.some((workspace) => workspace.id === lockedWorkspaceId);
    if (lockedWorkspaceId && !lockExists) {
      const option = document.createElement('option');
      option.value = lockedWorkspaceId;
      option.textContent = `${lockedWorkspaceId} (missing)`;
      select.appendChild(option);
    }

    select.value = lockedWorkspaceId || fallbackValue;
    select.disabled = !!lockedWorkspaceId;

    if (lockHint) {
      lockHint.textContent = lockedWorkspaceId
        ? `Workflow locked to workspace: ${lockedWorkspaceId}`
        : '';
      lockHint.classList.toggle('hidden', !lockedWorkspaceId);
    }
  }

  isSupportedRunFile(file) {
    if (window.OSA && typeof OSA.isSupportedAttachmentFile === 'function') {
      return OSA.isSupportedAttachmentFile(file);
    }

    const imageTypes = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];
    const acceptedExtensions = ['pdf', 'txt', 'md', 'markdown', 'json', 'csv', 'js', 'jsx', 'ts', 'tsx', 'rs', 'py', 'html', 'css', 'toml', 'yaml', 'yml', 'xml', 'sql', 'sh', 'ps1', 'bat', 'ini', 'log'];
    if (imageTypes.includes(file.type)) return true;
    const parts = String(file.name || '').split('.');
    const ext = parts.length > 1 ? parts[parts.length - 1].toLowerCase() : '';
    return acceptedExtensions.includes(ext);
  }

  readFileAsDataUrl(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result);
      reader.onerror = () => reject(new Error(`Failed to read ${file.name}`));
      reader.readAsDataURL(file);
    });
  }

  async addRunFiles(files) {
    if (!Array.isArray(files) || files.length === 0) return;

    const maxSize = this.getMaxAttachmentSize();
    const maxSizeMb = Math.round(maxSize / (1024 * 1024));
    const accepted = [];
    const rejected = [];

    for (const file of files) {
      if (!this.isSupportedRunFile(file)) {
        rejected.push(`Unsupported type: ${file.name}`);
        continue;
      }
      if (file.size > maxSize) {
        rejected.push(`Too large: ${file.name} (max ${maxSizeMb} MB)`);
        continue;
      }
      accepted.push(file);
    }

    if (rejected.length > 0) {
      this.setRunStatus(rejected[0], 'error');
    } else {
      this.setRunStatus('');
    }

    for (const file of accepted) {
      try {
        const dataUrl = await this.readFileAsDataUrl(file);
        this.runAttachmentCounter += 1;
        this.runAttachments.push({
          id: `wf-run-att-${Date.now()}-${this.runAttachmentCounter}`,
          filename: file.name,
          mime: file.type || 'application/octet-stream',
          sizeBytes: file.size,
          kind: (file.type || '').startsWith('image/') ? 'image' : 'document',
          dataUrl,
        });
      } catch (error) {
        this.setRunStatus(error.message || 'Failed to load attachment', 'error');
      }
    }

    this.renderRunAttachmentList();
  }

  removeRunFile(id) {
    this.runAttachments = this.runAttachments.filter((att) => att.id !== id);
    this.renderRunAttachmentList();
  }

  renderRunAttachmentList() {
    const list = this.container.querySelector('.workflow-run-file-list');
    if (!list) return;

    if (this.runAttachments.length === 0) {
      list.innerHTML = '<div class="workflow-run-file-empty">No files selected</div>';
      return;
    }

    list.innerHTML = this.runAttachments.map((att) => {
      const kb = Math.max(1, Math.round((att.sizeBytes || 0) / 1024));
      const tag = att.kind === 'image' ? 'image' : 'file';
      const safeFilename = (window.OSA && typeof OSA.escapeHtml === 'function')
        ? OSA.escapeHtml(att.filename)
        : att.filename;
      return `
        <div class="workflow-run-file-item">
          <div class="workflow-run-file-main">
            <span class="workflow-run-file-tag">${tag}</span>
            <span class="workflow-run-file-name" title="${safeFilename}">${safeFilename}</span>
            <span class="workflow-run-file-size">${kb} KB</span>
          </div>
          <button type="button" class="workflow-run-file-remove" data-remove-run-file="${att.id}" aria-label="Remove ${safeFilename}">&times;</button>
        </div>
      `;
    }).join('');
  }

  async runWorkflow(runOptions = {}) {
    if (!this.state.currentWorkflow) return;
    if (!this.executor) return;

    const saved = await this.saveWorkflow();
    if (!saved || !this.state.currentWorkflow) return;

    const btnRun = this.container.querySelector('.btn-run');
    const btnStop = this.container.querySelector('.btn-stop');

    btnRun?.classList.add('hidden');
    btnStop?.classList.remove('hidden');

    try {
      await this.executor.startExecution(this.state.currentWorkflow.id, runOptions);
    } catch (error) {
      this.onExecutionFailed({ error: error.message });
    }
  }

  async stopWorkflow() {
    if (!this.executor) return;
    await this.executor.stopExecution();
    this.onExecutionFinished();
  }

  async createNewWorkflow() {
    const name = prompt('Enter workflow name:');
    if (!name) return;

    const lockSelect = this.container.querySelector('.workflow-lock-select');
    const defaultWorkspaceId = lockSelect ? (lockSelect.value || null) : null;

    try {
      const workflow = await this.api.createWorkflow(name, null, { defaultWorkspaceId });
      this.state.setCurrentWorkflow(workflow);
      await this.loadWorkflows();
    } catch (error) {
      console.error('Failed to create workflow:', error);
      alert('Failed to create workflow: ' + error.message);
    }
  }

  goBack() {
    this.closeRunDialog();

    if (this.adapter) {
      this.adapter.destroy();
      this.adapter = null;
    }
    
    const appView = document.getElementById('app-view');
    const workflowEditor = document.getElementById('workflow-editor');
    
    if (workflowEditor) {
      workflowEditor.classList.add('hidden');
      workflowEditor.style.display = 'none';
    }
    
    if (appView) {
      appView.classList.remove('hidden');
      appView.style.display = 'flex';
    }
  }

  onExecutionFinished() {
    const btnRun = this.container.querySelector('.btn-run');
    const btnStop = this.container.querySelector('.btn-stop');
    btnRun?.classList.remove('hidden');
    btnStop?.classList.add('hidden');
  }

  onExecutionFailed(data) {
    this.onExecutionFinished();
    if (data?.error) {
      alert('Workflow execution failed: ' + data.error);
    }
  }
}

window.WorkflowEditor = WorkflowEditor;

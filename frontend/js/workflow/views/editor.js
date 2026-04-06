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
            <div class="node-palette-section">
              <h3>Nodes</h3>
              <div class="node-palette">
                <div class="palette-item" data-node-type="osa/trigger" draggable="true">
                  <span class="palette-icon trigger">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polygon points="5 3 19 12 5 21 5 3"></polygon></svg>
                  </span>
                  <span>Trigger</span>
                </div>
                <div class="palette-item" data-node-type="osa/agent" draggable="true">
                  <span class="palette-icon agent">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><rect x="3" y="11" width="18" height="10" rx="2"></rect><circle cx="12" cy="5" r="2"></circle><path d="M12 7v4"></path></svg>
                  </span>
                  <span>Agent</span>
                </div>
                <div class="palette-item" data-node-type="osa/condition" draggable="true">
                  <span class="palette-icon condition">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><circle cx="12" cy="12" r="10"></circle><path d="M12 16v-4"></path><path d="M12 8h.01"></path></svg>
                  </span>
                  <span>Condition</span>
                </div>
                <div class="palette-item" data-node-type="osa/transform" draggable="true">
                  <span class="palette-icon transform">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><polyline points="16 3 21 3 21 8"></polyline><line x1="4" y1="20" x2="21" y2="3"></line><polyline points="21 16 21 21 16 21"></polyline><line x1="15" y1="15" x2="21" y2="21"></line><line x1="4" y1="4" x2="9" y2="9"></line></svg>
                  </span>
                  <span>Transform</span>
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
                  <span>Output</span>
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

    btnBack?.addEventListener('click', () => this.goBack());
    btnSave?.addEventListener('click', () => this.saveWorkflow());
    btnRun?.addEventListener('click', () => this.runWorkflow());
    btnStop?.addEventListener('click', () => this.stopWorkflow());
    btnNew?.addEventListener('click', () => this.createNewWorkflow());

    this.state.on('currentWorkflowChanged', (workflow) => this.onWorkflowSelected(workflow));
    this.state.on('nodeStatesChanged', (states) => this.updateNodeVisuals(states));
  }

  async loadWorkflows() {
    try {
      const workflows = await this.api.listWorkflows();
      this.state.setWorkflows(workflows);
      this.renderWorkflowList(workflows);
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

    list.innerHTML = workflows.map(w => `
      <div class="workflow-item${this.state.currentWorkflow?.id === w.id ? ' active' : ''}" data-id="${w.id}">
        <span class="workflow-name">${w.name}</span>
        <span class="workflow-version">v${w.current_version}</span>
      </div>
    `).join('');

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
    let propertiesHtml = `<h4>${node.constructor.title || 'Node'}</h4>`;

    switch (type) {
      case 'osa/agent':
        propertiesHtml += `
          <div class="property-group">
            <label>Agent ID</label>
            <input type="text" data-prop="agent_id" value="${node.properties?.agent_id || 'main'}">
          </div>
          <div class="property-group">
            <label>System Prompt</label>
            <textarea data-prop="system_prompt">${node.properties?.system_prompt || ''}</textarea>
          </div>
          <div class="property-group">
            <label>Task Template</label>
            <textarea data-prop="task_template">${node.properties?.task_template || '{{input}}'}</textarea>
          </div>
        `;
        break;
      case 'osa/condition':
        propertiesHtml += `
          <div class="property-group">
            <label>Expression</label>
            <input type="text" data-prop="expression" value="${node.properties?.expression || ''}">
          </div>
        `;
        break;
      case 'osa/transform':
        propertiesHtml += `
          <div class="property-group">
            <label>Script</label>
            <textarea data-prop="script">${node.properties?.script || '{{input}}'}</textarea>
          </div>
        `;
        break;
      case 'osa/delay':
        propertiesHtml += `
          <div class="property-group">
            <label>Milliseconds</label>
            <input type="number" data-prop="milliseconds" value="${node.properties?.milliseconds || 1000}">
          </div>
        `;
        break;
      case 'osa/output':
        propertiesHtml += `
          <div class="property-group">
            <label>Format</label>
            <select data-prop="format">
              <option value="text"${(node.properties?.format || 'text') === 'text' ? ' selected' : ''}>Text</option>
              <option value="json"${node.properties?.format === 'json' ? ' selected' : ''}>JSON</option>
            </select>
          </div>
          <div class="property-group">
            <label>Template</label>
            <textarea data-prop="template">${node.properties?.template || '{{input}}'}</textarea>
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
      input.addEventListener('change', (e) => {
        const propName = e.target.dataset.prop;
        this.adapter.setNodeProperty(node.id, propName, e.target.value);
      });
    });

    const deleteBtn = content.querySelector('.btn-delete-node');
    deleteBtn?.addEventListener('click', () => {
      this.adapter.removeNode(node.id);
      this.selectedNode = null;
      placeholder?.classList.remove('hidden');
      content?.classList.add('hidden');
    });
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

    try {
      await this.api.updateWorkflow(this.state.currentWorkflow.id, graphJson);
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

  async runWorkflow() {
    if (!this.state.currentWorkflow) return;
    if (!this.executor) return;

    const saved = await this.saveWorkflow();
    if (!saved || !this.state.currentWorkflow) return;

    const btnRun = this.container.querySelector('.btn-run');
    const btnStop = this.container.querySelector('.btn-stop');

    btnRun?.classList.add('hidden');
    btnStop?.classList.remove('hidden');

    try {
      await this.executor.startExecution(this.state.currentWorkflow.id);
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

    try {
      const workflow = await this.api.createWorkflow(name);
      this.state.setCurrentWorkflow(workflow);
      await this.loadWorkflows();
    } catch (error) {
      console.error('Failed to create workflow:', error);
      alert('Failed to create workflow: ' + error.message);
    }
  }

  goBack() {
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

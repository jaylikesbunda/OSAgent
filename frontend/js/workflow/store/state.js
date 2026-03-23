class WorkflowState {
  constructor() {
    this.workflows = [];
    this.currentWorkflow = null;
    this.currentVersion = null;
    this.runs = [];
    this.listeners = {};
  }

  on(event, callback) {
    if (!this.listeners[event]) {
      this.listeners[event] = [];
    }
    this.listeners[event].push(callback);
  }

  emit(event, data) {
    if (this.listeners[event]) {
      this.listeners[event].forEach(cb => cb(data));
    }
  }

  setWorkflows(workflows) {
    this.workflows = workflows;
    this.emit('workflowsChanged', workflows);
  }

  setCurrentWorkflow(workflow) {
    this.currentWorkflow = workflow;
    this.emit('currentWorkflowChanged', workflow);
  }

  setCurrentVersion(version) {
    this.currentVersion = version;
    this.emit('currentVersionChanged', version);
  }

  setRuns(runs) {
    this.runs = runs;
    this.emit('runsChanged', runs);
  }

  addRun(run) {
    this.runs.unshift(run);
    this.emit('runsChanged', this.runs);
  }

  updateRun(runId, updates) {
    const index = this.runs.findIndex(r => r.id === runId);
    if (index !== -1) {
      this.runs[index] = { ...this.runs[index], ...updates };
      this.emit('runUpdated', this.runs[index]);
    }
  }
}

window.WorkflowState = WorkflowState;

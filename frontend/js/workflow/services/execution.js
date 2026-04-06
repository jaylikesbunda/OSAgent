class ExecutionManager {
  constructor(adapter, state) {
    this.adapter = adapter;
    this.state = state;
    this.isRunning = false;
    this.currentRunId = null;
    this.eventSource = null;
    this.pollTimer = null;
    this.nodeStates = new Map();
  }

  async startExecution(workflowId) {
    if (this.isRunning) {
      console.warn('Execution already in progress');
      return;
    }

    this.isRunning = true;
    this.emit('executionStarted', { workflowId });

    this.clearNodeStates();
    this.setAllNodesRunning();

    try {
      const api = new WorkflowAPI();
      const result = await api.executeWorkflow(workflowId);
      
      this.currentRunId = result.run_id || result.id;
      this.state.addRun({
        id: this.currentRunId,
        workflow_id: workflowId,
        status: 'running',
        started_at: new Date().toISOString()
      });

      this.startPolling(workflowId);
    } catch (error) {
      console.error('Failed to start workflow:', error);
      this.isRunning = false;
      this.emit('executionError', { error: error.message });
      throw error;
    }
  }

  startPolling(workflowId) {
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
    }

    const api = new WorkflowAPI();
    api.setToken?.((window.OSA && typeof OSA.getToken === 'function' && OSA.getToken()) || '');

    const poll = async () => {
      try {
        const [run, logs] = await Promise.all([
          api.getRun(workflowId, this.currentRunId),
          api.getRunLogs(workflowId, this.currentRunId)
        ]);

        this.applyLogs(logs || []);

        if (!run) {
          return;
        }

        if (run.status === 'completed') {
          this.isRunning = false;
          this.stopPolling();
          this.state.updateRun(this.currentRunId, {
            status: run.status,
            completed_at: run.completed_at || new Date().toISOString(),
            error_message: run.error_message || null
          });
          this.emit('executionCompleted', run);
          return;
        }

        if (run.status === 'failed') {
          this.isRunning = false;
          this.stopPolling();
          this.state.updateRun(this.currentRunId, {
            status: 'failed',
            completed_at: run.completed_at || new Date().toISOString(),
            error_message: run.error_message || null
          });
          this.emit('executionFailed', { error: run.error_message || 'Workflow failed' });
          return;
        }

        if (run.status === 'cancelled') {
          this.isRunning = false;
          this.stopPolling();
          this.state.updateRun(this.currentRunId, {
            status: 'cancelled',
            completed_at: run.completed_at || new Date().toISOString()
          });
          this.emit('executionCancelled', {});
        }
      } catch (error) {
        console.error('Workflow polling failed:', error);
        this.isRunning = false;
        this.stopPolling();
        this.emit('executionError', { error: error.message || 'Connection lost' });
      }
    };

    poll();
    this.pollTimer = setInterval(poll, 1000);
  }

  applyLogs(logs) {
    const seenNodeIds = new Set();
    logs.forEach(log => {
      seenNodeIds.add(log.node_id);
      if (log.status === 'started') {
        this.setNodeState(log.node_id, 'running');
      } else if (log.status === 'completed') {
        this.setNodeState(log.node_id, 'completed');
      } else if (log.status === 'failed') {
        this.setNodeState(log.node_id, 'failed');
      }
    });

    this.adapter.getNodes().forEach(node => {
      if (!seenNodeIds.has(String(node.id)) && !this.nodeStates.has(node.id)) {
        this.nodeStates.set(node.id, 'idle');
      }
    });

    this.emit('nodeStatesChanged', this.nodeStates);
  }

  async stopExecution() {
    if (!this.isRunning || !this.currentRunId) {
      return;
    }

    try {
      const api = new WorkflowAPI();
      await api.cancelRun(this.state.currentWorkflow?.id, this.currentRunId);
      this.isRunning = false;
      this.stopPolling();
      this.emit('executionStopped', {});
    } catch (error) {
      console.error('Failed to stop execution:', error);
    }
  }

  stopPolling() {
    if (this.pollTimer) {
      clearInterval(this.pollTimer);
      this.pollTimer = null;
    }
  }

  clearNodeStates() {
    this.nodeStates.clear();
    this.emit('nodeStatesChanged', this.nodeStates);
  }

  setAllNodesRunning() {
    const nodes = this.adapter.getNodes();
    nodes.forEach(node => {
      this.nodeStates.set(node.id, 'running');
    });
    this.emit('nodeStatesChanged', this.nodeStates);
  }

  setNodeState(nodeId, status, error = null) {
    this.nodeStates.set(nodeId, status);
    this.emit('nodeStatesChanged', this.nodeStates);
    
    const node = this.adapter.getNode(nodeId);
    if (node) {
      node.executionStatus = status;
      node.executionError = error;
    }
  }

  getNodeState(nodeId) {
    return this.nodeStates.get(nodeId);
  }

  emit(event, data) {
    const callback = this[`on${event.charAt(0).toUpperCase() + event.slice(1)}`];
    if (callback) {
      callback(data);
    }
  }

  on(event, callback) {
    const upperEvent = event.charAt(0).toUpperCase() + event.slice(1);
    const name = `on${upperEvent}`;
    this[name] = callback;
  }
}

window.ExecutionManager = ExecutionManager;

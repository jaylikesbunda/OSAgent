class ExecutionManager {
  constructor(adapter, state) {
    this.adapter = adapter;
    this.state = state;
    this.isRunning = false;
    this.currentRunId = null;
    this.eventSource = null;
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

      this.subscribeToEvents(workflowId);
    } catch (error) {
      console.error('Failed to start workflow:', error);
      this.isRunning = false;
      this.emit('executionError', { error: error.message });
    }
  }

  subscribeToEvents(workflowId) {
    if (this.eventSource) {
      this.eventSource.close();
    }

    this.eventSource = new EventSource(`/api/workflows/${workflowId}/runs/${this.currentRunId}/logs`);

    this.eventSource.onmessage = (event) => {
      try {
        const data = JSON.parse(event.data);
        this.handleEvent(data);
      } catch (e) {
        console.warn('Failed to parse event:', e);
      }
    };

    this.eventSource.onerror = () => {
      this.isRunning = false;
      this.emit('executionError', { error: 'Connection lost' });
    };
  }

  handleEvent(event) {
    if (!event.event_type) return;

    switch (event.event_type.type) {
      case 'node_started':
        this.setNodeState(event.event_type.node_id, 'running');
        break;
      case 'node_completed':
        this.setNodeState(event.event_type.node_id, 'completed');
        break;
      case 'node_failed':
        this.setNodeState(event.event_type.node_id, 'failed', event.event_type.error);
        break;
      case 'workflow_run_completed':
        this.isRunning = false;
        this.state.updateRun(this.currentRunId, {
          status: event.event_type.status,
          completed_at: new Date().toISOString()
        });
        this.emit('executionCompleted', event.event_type);
        break;
      case 'workflow_run_failed':
        this.isRunning = false;
        this.state.updateRun(this.currentRunId, {
          status: 'failed',
          error_message: event.event_type.error,
          completed_at: new Date().toISOString()
        });
        this.emit('executionFailed', event.event_type);
        break;
      case 'workflow_run_cancelled':
        this.isRunning = false;
        this.state.updateRun(this.currentRunId, {
          status: 'cancelled',
          completed_at: new Date().toISOString()
        });
        this.emit('executionCancelled', {});
        break;
    }
  }

  async stopExecution() {
    if (!this.isRunning || !this.currentRunId) {
      return;
    }

    try {
      const api = new WorkflowAPI();
      await api.cancelRun(this.state.currentWorkflow?.id, this.currentRunId);
      this.isRunning = false;
      this.emit('executionStopped', {});
    } catch (error) {
      console.error('Failed to stop execution:', error);
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

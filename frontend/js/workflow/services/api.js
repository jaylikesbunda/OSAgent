class WorkflowAPI {
  constructor(baseUrl = '/api') {
    this.baseUrl = baseUrl;
    this.token = localStorage.getItem('osagent_token');
  }

  setToken(token) {
    this.token = token;
  }

  async request(method, path, body = null) {
    const headers = {
      'Content-Type': 'application/json'
    };

    if (this.token) {
      headers['Authorization'] = `Bearer ${this.token}`;
    }

    const options = { method, headers };

    if (body) {
      options.body = JSON.stringify(body);
    }

    const response = await fetch(`${this.baseUrl}${path}`, options);
    
    if (!response.ok) {
      const error = await response.json().catch(() => ({ error: 'Unknown error' }));
      throw new Error(error.error || `HTTP ${response.status}`);
    }

    const data = await response.json();
    
    if (!data.success) {
      throw new Error(data.error || 'API request failed');
    }

    return data.data;
  }

  async createWorkflow(name, description = null) {
    return this.request('POST', '/workflows', { name, description });
  }

  async listWorkflows(limit = 100, offset = 0) {
    return this.request('GET', `/workflows?limit=${limit}&offset=${offset}`);
  }

  async getWorkflow(id) {
    return this.request('GET', `/workflows/${id}`);
  }

  async updateWorkflow(id, graphJson) {
    return this.request('PUT', `/workflows/${id}`, { graph_json: graphJson });
  }

  async deleteWorkflow(id) {
    return this.request('DELETE', `/workflows/${id}`);
  }

  async getVersions(id) {
    return this.request('GET', `/workflows/${id}/versions`);
  }

  async getVersion(id, version) {
    return this.request('GET', `/workflows/${id}/versions/${version}`);
  }

  async rollback(id, version) {
    return this.request('POST', `/workflows/${id}/rollback/${version}`);
  }

  async executeWorkflow(id, initialContext = null, parameters = {}) {
    return this.request('POST', `/workflows/${id}/execute`, {
      initial_context: initialContext,
      parameters
    });
  }

  async listRuns(id) {
    return this.request('GET', `/workflows/${id}/runs`);
  }

  async getRun(id, runId) {
    return this.request('GET', `/workflows/${id}/runs/${runId}`);
  }

  async cancelRun(id, runId) {
    return this.request('DELETE', `/workflows/${id}/runs/${runId}`);
  }

  async getRunLogs(id, runId) {
    return this.request('GET', `/workflows/${id}/runs/${runId}/logs`);
  }
}

window.WorkflowAPI = WorkflowAPI;

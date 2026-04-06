class WorkflowAPI {
  constructor(baseUrl = '/api') {
    this.baseUrl = baseUrl;
    this.token = (window.OSA && typeof OSA.getToken === 'function' && OSA.getToken())
      || localStorage.getItem('token')
      || localStorage.getItem('osagent_token')
      || '';
  }

  setToken(token) {
    this.token = token;
  }

  async request(method, path, body = null) {
    if (!this.token && window.OSA && typeof OSA.getToken === 'function') {
      this.token = OSA.getToken() || '';
    }

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
    const raw = await response.text();
    let payload = null;
    if (raw) {
      try {
        payload = JSON.parse(raw);
      } catch {
        payload = null;
      }
    }
    
    if (!response.ok) {
      throw new Error(payload?.error || payload?.message || raw || `HTTP ${response.status}`);
    }

    if (!payload) {
      return null;
    }
    
    if (payload.success === false) {
      throw new Error(payload.error || 'API request failed');
    }

    return Object.prototype.hasOwnProperty.call(payload, 'data') ? payload.data : payload;
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

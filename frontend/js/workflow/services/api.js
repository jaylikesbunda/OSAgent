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

  async createWorkflow(name, description = null, options = {}) {
    return this.request('POST', '/workflows', {
      name,
      description,
      graph_json: options.graphJson || null,
      default_workspace_id: Object.prototype.hasOwnProperty.call(options, 'defaultWorkspaceId')
        ? options.defaultWorkspaceId
        : null,
    });
  }

  async listWorkflows(limit = 100, offset = 0) {
    return this.request('GET', `/workflows?limit=${limit}&offset=${offset}`);
  }

  async getWorkflow(id) {
    return this.request('GET', `/workflows/${id}`);
  }

  async updateWorkflow(id, options = {}) {
    const payload = {};
    if (Object.prototype.hasOwnProperty.call(options, 'graphJson')) {
      payload.graph_json = options.graphJson;
    }
    if (Object.prototype.hasOwnProperty.call(options, 'defaultWorkspaceId')) {
      payload.default_workspace_id = options.defaultWorkspaceId;
    }
    return this.request('PUT', `/workflows/${id}`, payload);
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

  async executeWorkflow(id, options = {}) {
    return this.request('POST', `/workflows/${id}/execute`, {
      initial_context: options.initialContext || null,
      parameters: options.parameters || {},
      parent_session_id: options.parentSessionId || null,
      attachments: options.attachments || [],
      images: options.images || [],
      source: options.source || null,
      notify_channels: options.notifyChannels || [],
      discord_channel_id: options.discordChannelId || null
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

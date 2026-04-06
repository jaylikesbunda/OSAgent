(function() {
  function createWorkflowEditor() {
    const container = document.getElementById('workflow-editor');
    if (!container) {
      console.warn('Workflow editor container not found');
      return null;
    }

    const api = new WorkflowAPI();
    api.setToken((window.OSA && typeof OSA.getToken === 'function' && OSA.getToken()) || '');
    const editor = new WorkflowEditor(container, api);
    window.workflowEditor = editor;
    return editor;
  }

  window.ensureWorkflowEditor = function() {
    if (window.workflowEditor) {
      window.workflowEditor.api?.setToken?.((window.OSA && typeof OSA.getToken === 'function' && OSA.getToken()) || '');
      return window.workflowEditor;
    }

    return createWorkflowEditor();
  };
})();

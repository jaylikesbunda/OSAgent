(function() {
  function initWorkflowModule() {
    const container = document.getElementById('workflow-editor');
    if (!container) {
      console.warn('Workflow editor container not found');
      return;
    }

    const api = new WorkflowAPI();
    const editor = new WorkflowEditor(container, api);
    
    editor.init().catch(err => {
      console.error('Failed to initialize workflow editor:', err);
    });

    window.workflowEditor = editor;
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initWorkflowModule);
  } else {
    initWorkflowModule();
  }
})();

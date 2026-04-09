class OSABaseNode {
  static type = 'osa_base';
  static title = 'Node';

  constructor() {
    this.properties = {
      nodeId: this.id || `node_${Date.now()}`
    };
  }

  onExecute() {
    throw new Error('Subclass must implement onExecute()');
  }
}

class OSATriggerNode extends OSABaseNode {
  static type = 'osa/trigger';
  static title = 'Start';

  constructor() {
    super();
    this.color = '#1a6b3a';
    this.bgcolor = '#1e3a2f';
    this.addOutput('output', 'object');
    this.properties = {
      node_id: this.id || `trigger_${Date.now()}`,
      trigger_type: 'manual'
    };
    this.size = [140, 60];
  }

  onExecute() {
    this.setOutputData(0, {
      triggered: true,
      node_id: this.properties.node_id,
      timestamp: new Date().toISOString()
    });
  }
}

class OSAAgentNode extends OSABaseNode {
  static type = 'osa/agent';
  static title = 'AI Task';

  constructor() {
    super();
    this.color = '#2a5a7a';
    this.bgcolor = '#1e2e3a';
    this.addOutput('result', 'object');
    this.addInput('input', 'object');
    this.properties = {
      node_id: this.id || `agent_${Date.now()}`,
      agent_id: 'main',
      system_prompt: '',
      task_template: '{{input}}'
    };
    this.size = [180, 80];
  }

  onExecute() {
    const input = this.getInputData(0) || {};
    const { agent_id, system_prompt, task_template } = this.properties;
    
    this.setOutputData(0, {
      agent_id,
      task: this.renderTemplate(task_template, input),
      system_prompt
    });
  }

  renderTemplate(template, input) {
    let result = template;
    const regex = /\{\{([^}]+)\}\}/g;
    result = result.replace(regex, (match, key) => {
      const keys = key.trim().split('.');
      let val = input;
      for (const k of keys) {
        val = val?.[k];
      }
      return val !== undefined ? String(val) : match;
    });
    return result;
  }

  onPropertyChanged(name, value) {
    this.properties[name] = value;
    return true;
  }
}

class OSAConditionNode extends OSABaseNode {
  static type = 'osa/condition';
  static title = 'If / Else';

  constructor() {
    super();
    this.color = '#8a6a20';
    this.bgcolor = '#2a2518';
    this.addInput('input', 'object');
    this.addOutput('true', 'object');
    this.addOutput('false', 'object');
    this.properties = {
      node_id: this.id || `condition_${Date.now()}`,
      expression: 'true'
    };
    this.size = [160, 70];
  }

  onExecute() {
    const input = this.getInputData(0) || {};
    const result = this.evaluateExpression(this.properties.expression, input);
    
    if (result) {
      this.setOutputData(0, input);
      this.setOutputData(1, null);
    } else {
      this.setOutputData(0, null);
      this.setOutputData(1, input);
    }
  }

  evaluateExpression(expression, context) {
    try {
      const exprLower = expression.toLowerCase().trim();
      if (exprLower === 'true') return true;
      if (exprLower === 'false') return false;

      let evalContext = { ...context };
      let evalExpr = expression;

      const regex = /\{\{([^}]+)\}\}/g;
      evalExpr = evalExpr.replace(regex, (match, key) => {
        const keys = key.trim().split('.');
        let val = evalContext;
        for (const k of keys) {
          val = val?.[k];
        }
        return JSON.stringify(val);
      });

      return Function(`"use strict"; return (${evalExpr})`)(context);
    } catch (e) {
      console.warn('Condition evaluation error:', e);
      return false;
    }
  }
}

class OSATransformNode extends OSABaseNode {
  static type = 'osa/transform';
  static title = 'Format Text';

  constructor() {
    super();
    this.color = '#5a5a20';
    this.bgcolor = '#252520';
    this.addInput('input', 'object');
    this.addOutput('output', 'object');
    this.properties = {
      node_id: this.id || `transform_${Date.now()}`,
      script: '{{input}}'
    };
    this.size = [160, 70];
  }

  onExecute() {
    const input = this.getInputData(0);
    const output = this.renderTemplate(this.properties.script, input);
    
    try {
      this.setOutputData(0, JSON.parse(output));
    } catch {
      this.setOutputData(0, output);
    }
  }

  renderTemplate(template, input) {
    let result = template;
    const regex = /\{\{([^}]+)\}\}/g;
    result = result.replace(regex, (match, key) => {
      const keys = key.trim().split('.');
      let val = input;
      for (const k of keys) {
        val = val?.[k];
      }
      return val !== undefined ? String(val) : match;
    });
    return result;
  }
}

class OSADelayNode extends OSABaseNode {
  static type = 'osa/delay';
  static title = 'Wait';

  constructor() {
    super();
    this.color = '#4a4a6a';
    this.bgcolor = '#22222e';
    this.addInput('input', 'object');
    this.addOutput('output', 'object');
    this.properties = {
      node_id: this.id || `delay_${Date.now()}`,
      milliseconds: 1000
    };
    this.size = [120, 60];
  }

  async onExecute() {
    const ms = parseInt(this.properties.milliseconds) || 1000;
    const input = this.getInputData(0);
    await new Promise(resolve => setTimeout(resolve, ms));
    this.setOutputData(0, input || { delayed: true, milliseconds: ms });
  }
}

class OSAOutputNode extends OSABaseNode {
  static type = 'osa/output';
  static title = 'Show Result';

  constructor() {
    super();
    this.color = '#3a5a3a';
    this.bgcolor = '#1e2e1e';
    this.addInput('input', 'object');
    this.properties = {
      node_id: this.id || `output_${Date.now()}`,
      format: 'text',
      template: '{{input}}',
      destination: 'chat'
    };
    this.size = [140, 60];
  }

  onExecute() {
    const input = this.getInputData(0);
    const output = this.renderTemplate(this.properties.template, input);
    
    this.output = {
      format: this.properties.format,
      template: this.properties.template,
      destination: this.properties.destination,
      result: output
    };
  }

  renderTemplate(template, input) {
    let result = template;
    const regex = /\{\{([^}]+)\}\}/g;
    result = result.replace(regex, (match, key) => {
      const keys = key.trim().split('.');
      let val = input;
      for (const k of keys) {
        val = val?.[k];
      }
      return val !== undefined ? String(val) : match;
    });
    return result;
  }
}

class OSAFileInputNode extends OSABaseNode {
  static type = 'osa/file_input';
  static title = 'Load File';

  constructor() {
    super();
    this.color = '#2a6f54';
    this.bgcolor = '#1d3228';
    this.addOutput('output', 'object');
    this.properties = {
      node_id: this.id || `file_input_${Date.now()}`,
      path: '',
      use_attachment: true,
      attachment_index: 0
    };
    this.size = [170, 70];
  }

  onExecute() {
    this.setOutputData(0, {
      path: this.properties.path,
      use_attachment: !!this.properties.use_attachment,
      attachment_index: Number(this.properties.attachment_index || 0)
    });
  }
}

class OSAFileOutputNode extends OSABaseNode {
  static type = 'osa/file_output';
  static title = 'Save File';

  constructor() {
    super();
    this.color = '#2a6f54';
    this.bgcolor = '#1d3228';
    this.addInput('input', 'object');
    this.addOutput('output', 'object');
    this.properties = {
      node_id: this.id || `file_output_${Date.now()}`,
      path: '',
      content_template: '{{input}}',
      create_dirs: true
    };
    this.size = [180, 80];
  }

  onExecute() {
    const input = this.getInputData(0);
    this.setOutputData(0, {
      path: this.properties.path,
      content_template: this.properties.content_template,
      create_dirs: !!this.properties.create_dirs,
      input
    });
  }
}

class OSAApprovalNode extends OSABaseNode {
  static type = 'osa/approval';
  static title = 'Ask Human';

  constructor() {
    super();
    this.color = '#7a4b20';
    this.bgcolor = '#2c2118';
    this.addInput('input', 'object');
    this.addOutput('approved', 'object');
    this.addOutput('rejected', 'object');
    this.properties = {
      node_id: this.id || `approval_${Date.now()}`,
      prompt: 'Approve workflow step?',
      approve_label: 'Approve',
      reject_label: 'Reject'
    };
    this.size = [180, 80];
  }

  onExecute() {
    const input = this.getInputData(0) || {};
    this.setOutputData(0, input);
    this.setOutputData(1, input);
  }
}

class OSAForEachNode extends OSABaseNode {
  static type = 'osa/foreach';
  static title = 'Repeat List';

  constructor() {
    super();
    this.color = '#4c4f7a';
    this.bgcolor = '#22243a';
    this.addInput('input', 'object');
    this.addOutput('output', 'object');
    this.properties = {
      node_id: this.id || `foreach_${Date.now()}`,
      items_template: '{{input}}',
      item_variable: 'item'
    };
    this.size = [170, 80];
  }

  onExecute() {
    const input = this.getInputData(0);
    this.setOutputData(0, {
      items_template: this.properties.items_template,
      item_variable: this.properties.item_variable,
      input
    });
  }
}

function registerOSANodes(LiteGraph) {
  LiteGraph.registerNodeType('osa/trigger', OSATriggerNode);
  LiteGraph.registerNodeType('osa/agent', OSAAgentNode);
  LiteGraph.registerNodeType('osa/condition', OSAConditionNode);
  LiteGraph.registerNodeType('osa/transform', OSATransformNode);
  LiteGraph.registerNodeType('osa/delay', OSADelayNode);
  LiteGraph.registerNodeType('osa/output', OSAOutputNode);
  LiteGraph.registerNodeType('osa/file_input', OSAFileInputNode);
  LiteGraph.registerNodeType('osa/file_output', OSAFileOutputNode);
  LiteGraph.registerNodeType('osa/approval', OSAApprovalNode);
  LiteGraph.registerNodeType('osa/foreach', OSAForEachNode);
}

window.registerOSANodes = registerOSANodes;

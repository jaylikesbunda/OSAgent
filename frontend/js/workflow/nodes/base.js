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
  static title = 'Trigger';

  constructor() {
    super();
    this.color = '#1a6b3a';
    this.bgcolor = '#1e3a2f';
    this.addOutput('trigger', 'trigger');
    this.properties = {
      nodeId: this.id || `trigger_${Date.now()}`,
      triggerType: 'manual'
    };
    this.size = [140, 60];
  }

  onExecute() {
    this.setOutputData(0, {
      triggered: true,
      nodeId: this.properties.nodeId,
      timestamp: new Date().toISOString()
    });
  }
}

class OSAAgentNode extends OSABaseNode {
  static type = 'osa/agent';
  static title = 'Agent';

  constructor() {
    super();
    this.color = '#2a5a7a';
    this.bgcolor = '#1e2e3a';
    this.addInput('context', 'context');
    this.addOutput('context', 'context');
    this.addOutput('result', 'object');
    this.properties = {
      nodeId: this.id || `agent_${Date.now()}`,
      agentId: 'main',
      systemPrompt: '',
      taskTemplate: '{{input}}'
    };
    this.size = [180, 80];
  }

  onExecute() {
    const input = this.getInputData(0) || {};
    const { agentId, systemPrompt, taskTemplate } = this.properties;
    
    this.setOutputData(0, input);
    this.setOutputData(1, {
      agentId,
      task: this.renderTemplate(taskTemplate, input),
      systemPrompt
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
    if (name === 'agentId') {
      this.properties.agentId = value;
    } else if (name === 'systemPrompt') {
      this.properties.systemPrompt = value;
    } else if (name === 'taskTemplate') {
      this.properties.taskTemplate = value;
    }
    return true;
  }
}

class OSAConditionNode extends OSABaseNode {
  static type = 'osa/condition';
  static title = 'Condition';

  constructor() {
    super();
    this.color = '#8a6a20';
    this.bgcolor = '#2a2518';
    this.addOutput('true', 'flow');
    this.addOutput('false', 'flow');
    this.properties = {
      nodeId: this.id || `condition_${Date.now()}`,
      expression: 'input.result === "success"'
    };
    this.size = [160, 70];
  }

  onExecute() {
    const input = this.getInputData(0) || {};
    const result = this.evaluateExpression(this.properties.expression, input);
    
    if (result) {
      this.triggerSlot(0, input);
    } else {
      this.triggerSlot(1, input);
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
  static title = 'Transform';

  constructor() {
    super();
    this.color = '#5a5a20';
    this.bgcolor = '#252520';
    this.addInput('input', 'object');
    this.addOutput('output', 'object');
    this.properties = {
      nodeId: this.id || `transform_${Date.now()}`,
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
  static title = 'Delay';

  constructor() {
    super();
    this.color = '#4a4a6a';
    this.bgcolor = '#22222e';
    this.addInput('trigger', 'flow');
    this.addOutput('trigger', 'flow');
    this.properties = {
      nodeId: this.id || `delay_${Date.now()}`,
      milliseconds: 1000
    };
    this.size = [120, 60];
  }

  async onExecute() {
    const ms = parseInt(this.properties.milliseconds) || 1000;
    await new Promise(resolve => setTimeout(resolve, ms));
    this.setOutputData(0, { delayed: true, milliseconds: ms });
  }
}

class OSAOutputNode extends OSABaseNode {
  static type = 'osa/output';
  static title = 'Output';

  constructor() {
    super();
    this.color = '#3a5a3a';
    this.bgcolor = '#1e2e1e';
    this.addInput('input', 'object');
    this.properties = {
      nodeId: this.id || `output_${Date.now()}`,
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

function registerOSANodes(LiteGraph) {
  LiteGraph.registerNodeType('osa/trigger', OSATriggerNode);
  LiteGraph.registerNodeType('osa/agent', OSAAgentNode);
  LiteGraph.registerNodeType('osa/condition', OSAConditionNode);
  LiteGraph.registerNodeType('osa/transform', OSATransformNode);
  LiteGraph.registerNodeType('osa/delay', OSADelayNode);
  LiteGraph.registerNodeType('osa/output', OSAOutputNode);
}

window.registerOSANodes = registerOSANodes;

class LitegraphAdapter {
  constructor(container) {
    this.container = container;
    this.graph = null;
    this.graphContainer = null;
    this.canvas = null;
    this.selectedNode = null;
    this.onNodeSelectCallback = null;
    this.onGraphChangeCallback = null;
    this.litegraphLoaded = false;
  }

  async waitForLitegraph() {
    if (this.litegraphLoaded) return;
    
    return new Promise((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error('Litegraph.js failed to load'));
      }, 10000);
      
      const check = () => {
        if (typeof LGraph !== 'undefined' && typeof LGraphCanvas !== 'undefined') {
          clearTimeout(timeout);
          this.litegraphLoaded = true;
          resolve();
        } else {
          setTimeout(check, 100);
        }
      };
      check();
    });
  }

  async init() {
    await this.waitForLitegraph();
    
    this.graph = new LGraph();
    
    this.canvas = document.createElement('canvas');
    this.canvas.style.width = '100%';
    this.canvas.style.height = '100%';
    this.canvas.style.display = 'block';
    
    this.container.style.height = '100%';
    this.container.style.minHeight = '0';
    this.container.appendChild(this.canvas);
    
    const rect = this.container.getBoundingClientRect();
    console.log('Canvas container rect:', rect.width, rect.height);
    this.canvas.width = rect.width || 800;
    this.canvas.height = rect.height || 600;
    console.log('Canvas actual size:', this.canvas.width, this.canvas.height);
    
    this.canvasWidget = new LGraphCanvas(this.canvas, this.graph);
    
    this.graph.onNodeSelected = (node) => {
      if (node) {
        this.selectedNode = node;
        if (this.onNodeSelectCallback) {
          this.onNodeSelectCallback(node);
        }
      }
    };
    
    this.graph.onNodeDeselected = () => {
      this.selectedNode = null;
      if (this.onNodeSelectCallback) {
        this.onNodeSelectCallback(null);
      }
    };
    
    if (this.canvasWidget.processNodeSelected) {
      const original = this.canvasWidget.processNodeSelected.bind(this.canvasWidget);
      this.canvasWidget.processNodeSelected = (node, evt) => {
        console.log('processNodeSelected:', node ? node.type : 'null');
        if (node && this.onNodeSelectCallback) {
          this.selectedNode = node;
          this.onNodeSelectCallback(node);
        }
        return original(node, evt);
      };
    }

    this.graph.onAfterChange = () => {
      if (this.onGraphChangeCallback) {
        this.onGraphChangeCallback(this.graph);
      }
    };

    const resizeCanvas = () => {
      const rect = this.container.getBoundingClientRect();
      console.log('resizeCanvas:', rect.width, rect.height);
      this.canvas.width = rect.width;
      this.canvas.height = rect.height;
      if (this.canvasWidget) {
        this.canvasWidget.resize(rect.width, rect.height);
      }
    };

    this.graph.start();

    setInterval(() => {
      document.querySelectorAll('.litecontextmenu, .litegdialog, .graphdialog, .litemenubar, .dialog').forEach(el => {
        if (el.style.display !== 'none') {
          el.remove();
        }
      });
    }, 100);

    window.addEventListener('resize', resizeCanvas);
    
    setTimeout(resizeCanvas, 100);
  }

  registerNodes() {
    if (typeof registerOSANodes !== 'undefined') {
      registerOSANodes(LiteGraph);
    }
  }

  createNode(type, x, y) {
    if (!this.graph) return null;
    const node = LiteGraph.createNode(type);
    if (node) {
      node.pos = [x, y];
      this.graph.add(node);
      return node;
    }
    return null;
  }

  addNode(type, x, y) {
    return this.createNode(type, x, y);
  }

  removeNode(nodeId) {
    if (!this.graph) return;
    const node = this.graph.getNodeById(nodeId);
    if (node) {
      this.graph.remove(node);
    }
  }

  connect(sourceNodeId, sourceSlot, targetNodeId, targetSlot) {
    if (!this.graph) return false;
    const sourceNode = this.graph.getNodeById(sourceNodeId);
    const targetNode = this.graph.getNodeById(targetNodeId);
    
    if (sourceNode && targetNode) {
      return this.graph.connect(sourceNode, sourceSlot, targetNode, targetSlot);
    }
    return false;
  }

  disconnect(sourceNodeId, sourceSlot, targetNodeId, targetSlot) {
    if (!this.graph) return;
    const sourceNode = this.graph.getNodeById(sourceNodeId);
    const targetNode = this.graph.getNodeById(targetNodeId);
    
    if (sourceNode && targetNode) {
      sourceNode.disconnect(sourceSlot, targetNode, targetSlot);
    }
  }

  serialize() {
    if (!this.graph) return '{}';
    const data = this.graph.serialize();
    return JSON.stringify(data);
  }

  deserialize(jsonString) {
    if (!this.graph) return false;
    try {
      const data = JSON.parse(jsonString);
      this.graph.configure(data);
      return true;
    } catch (e) {
      console.error('Failed to deserialize graph:', e);
      return false;
    }
  }

  clear() {
    if (!this.graph) return;
    this.graph.clear();
  }

  getNodes() {
    if (!this.graph) return [];
    return this.graph.nodes;
  }

  getNode(nodeId) {
    if (!this.graph) return null;
    return this.graph.getNodeById(nodeId)
      || this.graph.getNodeById(parseInt(nodeId, 10))
      || this.graph.nodes.find(node => String(node.id) === String(nodeId))
      || null;
  }

  onNodeSelect(callback) {
    this.onNodeSelectCallback = callback;
  }

  onGraphChange(callback) {
    this.onGraphChangeCallback = callback;
  }

  setNodeProperty(nodeId, propertyName, value) {
    if (!this.graph) return;
    const node = this.getNode(nodeId);
    if (node) {
      node.properties[propertyName] = value;
      if (typeof node.onPropertyChanged === 'function') {
        node.onPropertyChanged(propertyName, value);
      }
      this.graph.change();
    }
  }

  getNodeProperty(nodeId, propertyName) {
    if (!this.graph) return null;
    const node = this.getNode(nodeId);
    if (node) {
      return node.properties[propertyName];
    }
    return null;
  }

  resize(width, height) {
    if (this.canvas) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    if (this.canvasWidget) {
      this.canvasWidget.resize(width, height);
    }
  }

  destroy() {
    if (this.graph) {
      this.graph.stop();
      this.graph.clear();
    }
    if (this.canvas && this.canvas.parentNode) {
      this.canvas.parentNode.removeChild(this.canvas);
    }
  }
}

window.LitegraphAdapter = LitegraphAdapter;

use crate::error::{OSAgentError, Result};
use crate::workflow::types::*;
use std::collections::{HashMap, HashSet};

pub struct GraphValidator {
    errors: Vec<ValidationError>,
}

#[derive(Debug)]
pub struct ValidationError {
    pub node_id: Option<String>,
    pub field: Option<String>,
    pub message: String,
}

impl GraphValidator {
    pub fn new() -> Self {
        Self { errors: Vec::new() }
    }

    pub fn validate(&mut self, graph: &WorkflowGraph) -> bool {
        self.errors.clear();

        self.check_trigger_exists(graph);
        self.check_output_exists(graph);
        self.check_no_cycles(graph);
        self.check_agent_ids(graph);
        self.check_condition_expressions(graph);

        self.errors.is_empty()
    }

    pub fn get_errors(&self) -> &[ValidationError] {
        &self.errors
    }

    fn check_trigger_exists(&mut self, graph: &WorkflowGraph) {
        let has_trigger = graph.nodes.iter().any(|n| n.node_type == NodeType::Trigger);
        if !has_trigger {
            self.errors.push(ValidationError {
                node_id: None,
                field: Some("nodes".to_string()),
                message: "Workflow must have at least one Trigger node".to_string(),
            });
        }
    }

    fn check_output_exists(&mut self, graph: &WorkflowGraph) {
        let has_output = graph.nodes.iter().any(|n| n.node_type == NodeType::Output);
        if !has_output {
            self.errors.push(ValidationError {
                node_id: None,
                field: Some("nodes".to_string()),
                message: "Workflow must have at least one Output node".to_string(),
            });
        }
    }

    fn check_no_cycles(&mut self, graph: &WorkflowGraph) {
        let node_ids: HashSet<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

        for node_id in &node_ids {
            adjacency.insert(node_id, Vec::new());
        }

        for edge in &graph.edges {
            if let Some(neighbors) = adjacency.get_mut(edge.source_node_id.as_str()) {
                neighbors.push(edge.target_node_id.as_str());
            }
        }

        if let Err(cycle_node) = topological_sort_no_cycles(&adjacency, node_ids) {
            self.errors.push(ValidationError {
                node_id: Some(cycle_node),
                field: Some("edges".to_string()),
                message: "Workflow contains a cycle".to_string(),
            });
        }
    }

    fn check_agent_ids(&mut self, graph: &WorkflowGraph) {
        for node in &graph.nodes {
            if node.node_type == NodeType::Agent {
                if let Some(config) = node.config.get("agent_id") {
                    if config.is_string() {
                        let agent_id = config.as_str().unwrap_or("");
                        if agent_id.trim().is_empty() {
                            self.errors.push(ValidationError {
                                node_id: Some(node.id.clone()),
                                field: Some("agent_id".to_string()),
                                message: "Agent ID cannot be empty".to_string(),
                            });
                        }
                    }
                } else {
                    self.errors.push(ValidationError {
                        node_id: Some(node.id.clone()),
                        field: Some("agent_id".to_string()),
                        message: "Agent node must have an agent_id".to_string(),
                    });
                }
            }
        }
    }

    fn check_condition_expressions(&mut self, graph: &WorkflowGraph) {
        for node in &graph.nodes {
            if node.node_type == NodeType::Condition {
                if let Some(config) = node.config.get("expression") {
                    if !config.is_string() || config.as_str().unwrap_or("").trim().is_empty() {
                        self.errors.push(ValidationError {
                            node_id: Some(node.id.clone()),
                            field: Some("expression".to_string()),
                            message: "Condition node must have a non-empty expression".to_string(),
                        });
                    }
                }
            }
        }
    }
}

fn topological_sort_no_cycles<'a>(
    adjacency: &HashMap<&'a str, Vec<&'a str>>,
    node_ids: HashSet<&'a str>,
) -> std::result::Result<Vec<&'a str>, String> {
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for node_id in &node_ids {
        in_degree.insert(node_id, 0);
    }

    for neighbors in adjacency.values() {
        for neighbor in neighbors {
            if let Some(deg) = in_degree.get_mut(neighbor) {
                *deg += 1;
            }
        }
    }

    let mut queue: Vec<&str> = in_degree
        .iter()
        .filter(|(_, &degree)| degree == 0)
        .map(|(node, _)| *node)
        .collect();

    let mut result: Vec<&str> = Vec::new();

    while let Some(node) = queue.pop() {
        result.push(node);

        if let Some(neighbors) = adjacency.get(node) {
            for neighbor in neighbors {
                if let Some(deg) = in_degree.get_mut(neighbor) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(neighbor);
                    }
                }
            }
        }
    }

    if result.len() != node_ids.len() {
        for node in &node_ids {
            if !result.contains(node) {
                return Err(node.to_string());
            }
        }
        unreachable!();
    }

    Ok(result)
}

pub fn topological_sort(graph: &WorkflowGraph) -> Result<Vec<String>> {
    let node_ids: Vec<&str> = graph.nodes.iter().map(|n| n.id.as_str()).collect();
    let node_id_set: HashSet<&str> = node_ids.iter().copied().collect();

    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for node_id in &node_ids {
        adjacency.insert(node_id, Vec::new());
    }

    for edge in &graph.edges {
        if let Some(neighbors) = adjacency.get_mut(edge.source_node_id.as_str()) {
            neighbors.push(edge.target_node_id.as_str());
        }
    }

    let sorted = topological_sort_no_cycles(&adjacency, node_id_set).map_err(|cycle_node| {
        OSAgentError::Workflow(format!("Cycle detected at node: {}", cycle_node))
    })?;

    Ok(sorted.iter().map(|s| s.to_string()).collect())
}

pub fn parse_litegraph_json(json_str: &str) -> Result<WorkflowGraph> {
    let data: serde_json::Value = serde_json::from_str(json_str)
        .map_err(|e| OSAgentError::Workflow(format!("Invalid JSON: {}", e)))?;

    let nodes_array = data
        .get("nodes")
        .and_then(|n| n.as_array())
        .ok_or_else(|| OSAgentError::Workflow("Missing or invalid 'nodes' field".to_string()))?;

    let links_array = data
        .get("links")
        .and_then(|l| l.as_array())
        .ok_or_else(|| OSAgentError::Workflow("Missing or invalid 'links' field".to_string()))?;

    let mut nodes = Vec::new();
    let mut node_id_map: HashMap<usize, String> = HashMap::new();

    for (idx, node_val) in nodes_array.iter().enumerate() {
        let id = node_val
            .get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("node_{}", idx));

        let type_str = node_val.get("type").and_then(|v| v.as_str()).unwrap_or("");

        let node_type = parse_node_type(type_str);

        let pos = node_val.get("pos");
        let position = if let Some(pos_arr) = pos.and_then(|p| p.as_array()) {
            Position {
                x: pos_arr.get(0).and_then(|v| v.as_f64()).unwrap_or(0.0),
                y: pos_arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0),
            }
        } else {
            Position { x: 0.0, y: 0.0 }
        };

        let config = node_val
            .get("properties")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        node_id_map.insert(idx, id.clone());

        nodes.push(WorkflowNode {
            id,
            node_type,
            position,
            config,
        });
    }

    let mut edges = Vec::new();

    for (link_idx, link_val) in links_array.iter().enumerate() {
        if let Some(link_arr) = link_val.as_array() {
            if link_arr.len() >= 4 {
                let source_idx = link_arr
                    .get(1)
                    .and_then(|v| v.as_u64())
                    .and_then(|v| Some(v as usize));
                let target_idx = link_arr
                    .get(3)
                    .and_then(|v| v.as_u64())
                    .and_then(|v| Some(v as usize));

                if let (Some(src_idx), Some(tgt_idx)) = (source_idx, target_idx) {
                    if let (Some(source_id), Some(target_id)) =
                        (node_id_map.get(&src_idx), node_id_map.get(&tgt_idx))
                    {
                        edges.push(WorkflowEdge {
                            id: format!("edge_{}", link_idx),
                            source_node_id: source_id.clone(),
                            source_port: "output".to_string(),
                            target_node_id: target_id.clone(),
                            target_port: "input".to_string(),
                        });
                    }
                }
            }
        }
    }

    Ok(WorkflowGraph { nodes, edges })
}

fn parse_node_type(type_str: &str) -> NodeType {
    if type_str.starts_with("osa/") {
        match type_str.strip_prefix("osa/") {
            Some("trigger") => return NodeType::Trigger,
            Some("agent") => return NodeType::Agent,
            Some("condition") => return NodeType::Condition,
            Some("transform") => return NodeType::Transform,
            Some("delay") => return NodeType::Delay,
            Some("output") => return NodeType::Output,
            _ => {}
        }
    }

    match type_str {
        "osa_trigger" | "trigger" => NodeType::Trigger,
        "osa_agent" | "agent" => NodeType::Agent,
        "osa_condition" | "condition" => NodeType::Condition,
        "osa_transform" | "transform" => NodeType::Transform,
        "osa_delay" | "delay" => NodeType::Delay,
        "osa_output" | "output" => NodeType::Output,
        _ => NodeType::Agent,
    }
}

pub fn to_litegraph_json(graph: &WorkflowGraph) -> Result<String> {
    let mut nodes: Vec<serde_json::Value> = Vec::new();
    let mut node_id_to_idx: HashMap<&str, usize> = HashMap::new();

    for (idx, node) in graph.nodes.iter().enumerate() {
        node_id_to_idx.insert(node.id.as_str(), idx);

        let type_str = match node.node_type {
            NodeType::Trigger => "osa/trigger",
            NodeType::Agent => "osa/agent",
            NodeType::Condition => "osa/condition",
            NodeType::Transform => "osa/transform",
            NodeType::Delay => "osa/delay",
            NodeType::Output => "osa/output",
        };

        nodes.push(serde_json::json!({
            "id": node.id,
            "type": type_str,
            "pos": [node.position.x, node.position.y],
            "size": [120, 60],
            "properties": node.config,
            "flags": {}
        }));
    }

    let mut links: Vec<serde_json::Value> = Vec::new();

    for (edge_idx, edge) in graph.edges.iter().enumerate() {
        if let (Some(&source_idx), Some(&target_idx)) = (
            node_id_to_idx.get(edge.source_node_id.as_str()),
            node_id_to_idx.get(edge.target_node_id.as_str()),
        ) {
            links.push(serde_json::json!([edge_idx, source_idx, target_idx, null]));
        }
    }

    let litegraph_data = serde_json::json!({
        "nodes": nodes,
        "links": links
    });

    serde_json::to_string_pretty(&litegraph_data)
        .map_err(|e| OSAgentError::Workflow(format!("Failed to serialize: {}", e)))
}

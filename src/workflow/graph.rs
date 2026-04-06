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

impl Default for GraphValidator {
    fn default() -> Self {
        Self::new()
    }
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
    fn value_to_string(value: &serde_json::Value) -> Option<String> {
        if let Some(string) = value.as_str() {
            return Some(string.to_string());
        }
        if let Some(number) = value.as_i64() {
            return Some(number.to_string());
        }
        if let Some(number) = value.as_u64() {
            return Some(number.to_string());
        }
        None
    }

    fn resolve_node_id(
        value: &serde_json::Value,
        litegraph_id_map: &HashMap<String, String>,
        index_map: &HashMap<usize, String>,
    ) -> Option<String> {
        if let Some(raw) = value_to_string(value) {
            if let Some(node_id) = litegraph_id_map.get(&raw) {
                return Some(node_id.clone());
            }
        }

        value
            .as_u64()
            .and_then(|index| index_map.get(&(index as usize)).cloned())
    }

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
    let mut litegraph_id_map: HashMap<String, String> = HashMap::new();
    let mut index_map: HashMap<usize, String> = HashMap::new();

    for (idx, node_val) in nodes_array.iter().enumerate() {
        let litegraph_id = node_val
            .get("id")
            .and_then(value_to_string)
            .unwrap_or_else(|| idx.to_string());

        let type_str = node_val.get("type").and_then(|v| v.as_str()).unwrap_or("");

        let node_type = parse_node_type(type_str);

        let pos = node_val.get("pos");
        let position = if let Some(pos_arr) = pos.and_then(|p| p.as_array()) {
            Position {
                x: pos_arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0),
                y: pos_arr.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0),
            }
        } else {
            Position { x: 0.0, y: 0.0 }
        };

        let mut config = node_val
            .get("properties")
            .cloned()
            .unwrap_or(serde_json::json!({}));

        let workflow_id = config
            .get("node_id")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .unwrap_or_else(|| litegraph_id.clone());

        if let Some(properties) = config.as_object_mut() {
            properties.insert(
                "node_id".to_string(),
                serde_json::Value::String(workflow_id.clone()),
            );
        }

        litegraph_id_map.insert(litegraph_id, workflow_id.clone());
        index_map.insert(idx, workflow_id.clone());

        nodes.push(WorkflowNode {
            id: workflow_id,
            node_type,
            position,
            config,
        });
    }

    let mut edges = Vec::new();

    for (link_idx, link_val) in links_array.iter().enumerate() {
        if let Some(link_arr) = link_val.as_array() {
            let parsed = if link_arr.len() >= 5 {
                let source_id = link_arr
                    .get(1)
                    .and_then(|value| resolve_node_id(value, &litegraph_id_map, &index_map));
                let target_id = link_arr
                    .get(3)
                    .and_then(|value| resolve_node_id(value, &litegraph_id_map, &index_map));
                let source_port = link_arr
                    .get(2)
                    .and_then(value_to_string)
                    .unwrap_or_else(|| "0".to_string());
                let target_port = link_arr
                    .get(4)
                    .and_then(value_to_string)
                    .unwrap_or_else(|| "0".to_string());

                source_id
                    .zip(target_id)
                    .map(|(source_node_id, target_node_id)| WorkflowEdge {
                        id: format!("edge_{}", link_idx),
                        source_node_id,
                        source_port,
                        target_node_id,
                        target_port,
                    })
            } else if link_arr.len() >= 3 {
                let source_id = link_arr
                    .get(1)
                    .and_then(|value| resolve_node_id(value, &litegraph_id_map, &index_map));
                let target_id = link_arr
                    .get(2)
                    .and_then(|value| resolve_node_id(value, &litegraph_id_map, &index_map));

                source_id
                    .zip(target_id)
                    .map(|(source_node_id, target_node_id)| WorkflowEdge {
                        id: format!("edge_{}", link_idx),
                        source_node_id,
                        source_port: "0".to_string(),
                        target_node_id,
                        target_port: "0".to_string(),
                    })
            } else {
                None
            };

            if let Some(edge) = parsed {
                edges.push(edge);
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
    let mut node_id_to_litegraph_id: HashMap<&str, usize> = HashMap::new();

    for (idx, node) in graph.nodes.iter().enumerate() {
        let litegraph_id = idx + 1;
        node_id_to_litegraph_id.insert(node.id.as_str(), litegraph_id);

        let type_str = match node.node_type {
            NodeType::Trigger => "osa/trigger",
            NodeType::Agent => "osa/agent",
            NodeType::Condition => "osa/condition",
            NodeType::Transform => "osa/transform",
            NodeType::Delay => "osa/delay",
            NodeType::Output => "osa/output",
        };

        let mut properties = node.config.clone();
        if let Some(object) = properties.as_object_mut() {
            object.insert(
                "node_id".to_string(),
                serde_json::Value::String(node.id.clone()),
            );
        }

        nodes.push(serde_json::json!({
            "id": litegraph_id,
            "type": type_str,
            "pos": [node.position.x, node.position.y],
            "size": [120, 60],
            "properties": properties,
            "flags": {}
        }));
    }

    let mut links: Vec<serde_json::Value> = Vec::new();

    for (edge_idx, edge) in graph.edges.iter().enumerate() {
        if let (Some(&source_id), Some(&target_id)) = (
            node_id_to_litegraph_id.get(edge.source_node_id.as_str()),
            node_id_to_litegraph_id.get(edge.target_node_id.as_str()),
        ) {
            let source_slot = edge.source_port.parse::<usize>().unwrap_or(0);
            let target_slot = edge.target_port.parse::<usize>().unwrap_or(0);
            links.push(serde_json::json!([
                edge_idx + 1,
                source_id,
                source_slot,
                target_id,
                target_slot,
                "flow"
            ]));
        }
    }

    let litegraph_data = serde_json::json!({
        "nodes": nodes,
        "links": links
    });

    serde_json::to_string_pretty(&litegraph_data)
        .map_err(|e| OSAgentError::Workflow(format!("Failed to serialize: {}", e)))
}

//! `draw_diagram` — renders an inline, pan/zoomable SVG diagram in chat.
//!
//! Two authoring modes are supported:
//!
//! * **structured** (default): the model submits a validated node/edge spec
//!   and the browser lays it out deterministically, which is what keeps
//!   diagrams consistent and "nice" instead of depending on the model to
//!   hand-tune coordinates.
//! * **raw**: an explicit escape hatch for hand-authored SVG. The source is
//!   sanitized here *and* again in the browser before it is ever mounted, so
//!   neither side of the wire has to trust the other.
//!
//! The SVG itself is returned in `ToolResult::metadata` rather than in
//! `output`: the transcript persists tool metadata verbatim, so a diagram
//! survives a session reload without bloating the text the model reads back.

use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample, ToolOutcome, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Map, Value};

const MAX_TITLE_CHARS: usize = 120;
const MAX_NODES: usize = 120;
const MAX_EDGES: usize = 240;
const MAX_GROUPS: usize = 24;
const MAX_LABEL_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 800;
const MAX_RAW_SVG_BYTES: usize = 256 * 1024;

/// Substrings that make a raw SVG an active document rather than a picture.
/// Matched case-insensitively against the source before it is stored.
const RAW_SVG_FORBIDDEN: &[&str] = &[
    "<script",
    "<foreignobject",
    "<iframe",
    "<embed",
    "<object",
    "<audio",
    "<video",
    "<animate",
    "<animatetransform",
    "<animatemotion",
    "<set",
    "<handler",
    "<listener",
    "<use",
    "<image",
    "<pattern",
    "<a",
    "javascript:",
    "vbscript:",
    "data:text/html",
    "@import",
    "expression(",
    "<!--<![endif",
];

/// Only fragment references are allowed (`url(#gradient)`, `href="#node"`).
fn has_external_reference(source: &str) -> bool {
    let lower = source.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if lower[index..].starts_with("url(") {
            let rest_start = index + 4;
            let rest = lower[rest_start..].trim_start();
            if !rest.starts_with('#') {
                return true;
            }
            index = rest_start;
            continue;
        }
        if lower[index..].starts_with("href=") {
            let rest = lower[index + 5..].trim_start();
            // Allow `href="#id"` and `href='#id'` only.
            if !(rest.starts_with("\"#") || rest.starts_with("'#")) {
                return true;
            }
            index += 5;
            continue;
        }
        // A quoted attribute value may contain newlines, so step over the
        // current byte rather than slicing on a char boundary blindly.
        index += 1;
    }
    false
}

fn sanitize_raw_svg(source: &str) -> Result<String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "raw_svg is empty. Pass a full <svg>...</svg> document, or use the structured spec instead."
                .to_string(),
        ));
    }
    if trimmed.len() > MAX_RAW_SVG_BYTES {
        return Err(OSAgentError::ToolExecution(format!(
            "raw_svg is {} bytes (max {}). Simplify the SVG or use the structured spec.",
            trimmed.len(),
            MAX_RAW_SVG_BYTES
        )));
    }
    if !trimmed.to_ascii_lowercase().contains("<svg") {
        return Err(OSAgentError::ToolExecution(
            "raw_svg must contain an <svg> root element.".to_string(),
        ));
    }
    let lower = trimmed.to_ascii_lowercase();
    for needle in RAW_SVG_FORBIDDEN {
        if lower.contains(needle) {
            return Err(OSAgentError::ToolExecution(format!(
                "raw_svg contains a forbidden construct ({needle}). Scripts, embedded media, \
                 external references, and event-driven animation are not allowed; use the \
                 structured spec instead."
            )));
        }
    }
    if lower.contains(" on") && has_event_handler_attribute(&lower) {
        return Err(OSAgentError::ToolExecution(
            "raw_svg contains an event handler attribute (on*). Interactive behaviour is \
             provided by the diagram viewer, not by inline handlers."
                .to_string(),
        ));
    }
    if has_external_reference(&lower) {
        return Err(OSAgentError::ToolExecution(
            "raw_svg references an external resource. Only fragment references (url(#id), \
             href=\"#id\") are allowed."
                .to_string(),
        ));
    }
    Ok(trimmed.to_string())
}

/// Cheap `on*=` attribute check. We are not parsing XML here (the browser does
/// the authoritative parse), we only need to reject obvious active content
/// before it is stored.
fn has_event_handler_attribute(lower: &str) -> bool {
    let bytes = lower.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        let starts_attr = index == 0 || bytes[index - 1].is_ascii_whitespace();
        if starts_attr
            && bytes[index] == b'o'
            && index + 2 < bytes.len()
            && bytes[index + 1] == b'n'
        {
            // Attribute names are short and hyphen-free; walk forward over the
            // identifier and require an `=` so `one=`-style values cannot trip
            // the check.
            let mut cursor = index + 2;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric()
                    || bytes[cursor] == b'-'
                    || bytes[cursor] == b':')
            {
                cursor += 1;
            }
            if cursor < bytes.len() && bytes[cursor] == b'=' {
                return true;
            }
        }
        index += 1;
    }
    false
}

fn trimmed_string(value: &Value, field: &str, max_chars: usize) -> Result<String> {
    let raw = value
        .as_str()
        .ok_or_else(|| OSAgentError::ToolExecution(format!("'{field}' must be a string")))?;
    let text = raw.trim().to_string();
    if text.is_empty() {
        return Err(OSAgentError::ToolExecution(format!(
            "'{field}' must not be empty"
        )));
    }
    if text.chars().count() > max_chars {
        return Err(OSAgentError::ToolExecution(format!(
            "'{field}' is {} characters (max {max_chars})",
            text.chars().count()
        )));
    }
    Ok(text)
}

fn optional_string(value: Option<&Value>, field: &str, max_chars: usize) -> Result<Option<String>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => {
            let text = raw.trim().to_string();
            if text.is_empty() {
                return Ok(None);
            }
            if text.chars().count() > max_chars {
                return Err(OSAgentError::ToolExecution(format!(
                    "'{field}' is {} characters (max {max_chars})",
                    text.chars().count()
                )));
            }
            Ok(Some(text))
        }
        Some(_) => Err(OSAgentError::ToolExecution(format!(
            "'{field}' must be a string when present"
        ))),
    }
}

fn optional_number(value: Option<&Value>, field: &str) -> Result<Option<f64>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => match number.as_f64() {
            Some(value) if value.is_finite() => Ok(Some(value)),
            _ => Err(OSAgentError::ToolExecution(format!(
                "'{field}' must be a finite number"
            ))),
        },
        Some(_) => Err(OSAgentError::ToolExecution(format!(
            "'{field}' must be a number when present"
        ))),
    }
}

fn validated_direction(value: Option<&Value>) -> Result<String> {
    match value
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_ascii_lowercase())
    {
        None => Ok("TB".to_string()),
        Some(direction) if direction == "tb" || direction == "td" => Ok("TB".to_string()),
        Some(direction) if direction == "lr" || direction == "rl" => Ok("LR".to_string()),
        Some(other) => Err(OSAgentError::ToolExecution(format!(
            "spec.direction must be 'TB' or 'LR' (got '{other}')"
        ))),
    }
}

fn validated_theme(value: Option<&Value>) -> String {
    match value
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_ascii_lowercase())
    {
        Some(theme) if theme == "light" || theme == "dark" => theme,
        _ => "auto".to_string(),
    }
}

/// Rebuild the node/edge spec from known fields only. Unknown keys are
/// dropped so nothing unexpected can reach the renderer or the transcript.
fn build_structured_spec(spec: &Value) -> Result<Value> {
    let object = spec
        .as_object()
        .ok_or_else(|| OSAgentError::ToolExecution("spec must be an object".to_string()))?;

    let nodes_input = object
        .get("nodes")
        .ok_or_else(|| OSAgentError::ToolExecution("spec.nodes is required".to_string()))?
        .as_array()
        .ok_or_else(|| OSAgentError::ToolExecution("spec.nodes must be an array".to_string()))?;

    if nodes_input.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "spec.nodes must contain at least one node".to_string(),
        ));
    }
    if nodes_input.len() > MAX_NODES {
        return Err(OSAgentError::ToolExecution(format!(
            "spec.nodes has {} entries (max {MAX_NODES})",
            nodes_input.len()
        )));
    }

    let mut nodes: Vec<Value> = Vec::with_capacity(nodes_input.len());
    let mut node_ids: Vec<String> = Vec::with_capacity(nodes_input.len());
    for (index, node) in nodes_input.iter().enumerate() {
        let node_object = node.as_object().ok_or_else(|| {
            OSAgentError::ToolExecution(format!("spec.nodes[{index}] must be an object"))
        })?;
        let id = trimmed_string(
            node_object.get("id").ok_or_else(|| {
                OSAgentError::ToolExecution(format!("spec.nodes[{index}].id is required"))
            })?,
            &format!("spec.nodes[{index}].id"),
            80,
        )?;
        if node_ids.contains(&id) {
            return Err(OSAgentError::ToolExecution(format!(
                "duplicate node id '{id}' in spec.nodes"
            )));
        }
        let label = trimmed_string(
            node_object.get("label").ok_or_else(|| {
                OSAgentError::ToolExecution(format!("spec.nodes[{index}].label is required"))
            })?,
            &format!("spec.nodes[{index}].label"),
            MAX_LABEL_CHARS,
        )?;
        node_ids.push(id.clone());

        let mut clean = Map::new();
        clean.insert("id".to_string(), Value::String(id));
        clean.insert("label".to_string(), Value::String(label));
        for key in ["description", "kind", "group", "icon"] {
            if let Some(text) = optional_string(node_object.get(key), key, MAX_DESCRIPTION_CHARS)? {
                clean.insert(key.to_string(), Value::String(text));
            }
        }
        for key in ["x", "y", "width", "height"] {
            if let Some(number) = optional_number(node_object.get(key), key)? {
                clean.insert(key.to_string(), json!(number));
            }
        }
        nodes.push(Value::Object(clean));
    }

    let mut edges: Vec<Value> = Vec::new();
    if let Some(edges_input) = object.get("edges") {
        if !edges_input.is_null() {
            let edges_array = edges_input.as_array().ok_or_else(|| {
                OSAgentError::ToolExecution("spec.edges must be an array".to_string())
            })?;
            if edges_array.len() > MAX_EDGES {
                return Err(OSAgentError::ToolExecution(format!(
                    "spec.edges has {} entries (max {MAX_EDGES})",
                    edges_array.len()
                )));
            }
            for (index, edge) in edges_array.iter().enumerate() {
                let edge_object = edge.as_object().ok_or_else(|| {
                    OSAgentError::ToolExecution(format!("spec.edges[{index}] must be an object"))
                })?;
                let from = trimmed_string(
                    edge_object.get("from").ok_or_else(|| {
                        OSAgentError::ToolExecution(format!("spec.edges[{index}].from is required"))
                    })?,
                    &format!("spec.edges[{index}].from"),
                    80,
                )?;
                let to = trimmed_string(
                    edge_object.get("to").ok_or_else(|| {
                        OSAgentError::ToolExecution(format!("spec.edges[{index}].to is required"))
                    })?,
                    &format!("spec.edges[{index}].to"),
                    80,
                )?;
                if !node_ids.contains(&from) {
                    return Err(OSAgentError::ToolExecution(format!(
                        "spec.edges[{index}].from references unknown node '{from}'"
                    )));
                }
                if !node_ids.contains(&to) {
                    return Err(OSAgentError::ToolExecution(format!(
                        "spec.edges[{index}].to references unknown node '{to}'"
                    )));
                }
                let mut clean = Map::new();
                clean.insert("from".to_string(), Value::String(from));
                clean.insert("to".to_string(), Value::String(to));
                if let Some(text) =
                    optional_string(edge_object.get("label"), "label", MAX_LABEL_CHARS)?
                {
                    clean.insert("label".to_string(), Value::String(text));
                }
                if let Some(text) = optional_string(edge_object.get("style"), "style", 32)? {
                    clean.insert("style".to_string(), Value::String(text));
                }
                edges.push(Value::Object(clean));
            }
        }
    }

    let mut groups: Vec<Value> = Vec::new();
    if let Some(groups_input) = object.get("groups") {
        if !groups_input.is_null() {
            let groups_array = groups_input.as_array().ok_or_else(|| {
                OSAgentError::ToolExecution("spec.groups must be an array".to_string())
            })?;
            if groups_array.len() > MAX_GROUPS {
                return Err(OSAgentError::ToolExecution(format!(
                    "spec.groups has {} entries (max {MAX_GROUPS})",
                    groups_array.len()
                )));
            }
            for (index, group) in groups_array.iter().enumerate() {
                let group_object = group.as_object().ok_or_else(|| {
                    OSAgentError::ToolExecution(format!("spec.groups[{index}] must be an object"))
                })?;
                let id = trimmed_string(
                    group_object.get("id").ok_or_else(|| {
                        OSAgentError::ToolExecution(format!("spec.groups[{index}].id is required"))
                    })?,
                    &format!("spec.groups[{index}].id"),
                    80,
                )?;
                let label = trimmed_string(
                    group_object.get("label").ok_or_else(|| {
                        OSAgentError::ToolExecution(format!(
                            "spec.groups[{index}].label is required"
                        ))
                    })?,
                    &format!("spec.groups[{index}].label"),
                    MAX_LABEL_CHARS,
                )?;
                let mut clean = Map::new();
                clean.insert("id".to_string(), Value::String(id));
                clean.insert("label".to_string(), Value::String(label));
                if let Some(text) = optional_string(group_object.get("color"), "color", 32)? {
                    clean.insert("color".to_string(), Value::String(text));
                }
                groups.push(Value::Object(clean));
            }
        }
    }

    let mut spec_out = Map::new();
    spec_out.insert("nodes".to_string(), Value::Array(nodes));
    spec_out.insert("edges".to_string(), Value::Array(edges));
    spec_out.insert("groups".to_string(), Value::Array(groups));
    spec_out.insert(
        "direction".to_string(),
        Value::String(validated_direction(object.get("direction"))?),
    );
    Ok(Value::Object(spec_out))
}

pub struct DrawDiagramTool;

#[async_trait]
impl Tool for DrawDiagramTool {
    fn name(&self) -> &str {
        "draw_diagram"
    }

    fn description(&self) -> &str {
        "Draw a diagram that renders inline in the chat as a pan/zoomable, inspectable SVG the \
user can save as a .svg file. Use the structured `spec` form for flowcharts, architecture maps, \
sequence-style chains, state machines, and concept maps — the layout is generated for you and \
always looks consistent. Use `raw_svg` only when the user explicitly asks for hand-authored SVG \
markup; scripts, embedded media, and external references are rejected."
    }

    fn when_to_use(&self) -> &str {
        "Use whenever a visual would explain something better than prose or ASCII art: system or \
architecture diagrams, request/response flows, state transitions, decision trees, layered \
plans, org/module maps, timelines, and comparison maps"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for simple lists, for numeric tables, or when the user asked for a file in a \
specific project format — use write_file for that"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Layered architecture diagram".to_string(),
                input: json!({
                    "title": "OSAgent architecture",
                    "spec": {
                        "direction": "TB",
                        "nodes": [
                            {"id": "ui", "label": "Web UI", "group": "front"},
                            {"id": "runtime", "label": "Agent runtime", "group": "core"},
                            {"id": "tools", "label": "Tool registry", "group": "core"},
                            {"id": "db", "label": "SQLite", "group": "data"}
                        ],
                        "edges": [
                            {"from": "ui", "to": "runtime"},
                            {"from": "runtime", "to": "tools"},
                            {"from": "runtime", "to": "db"}
                        ],
                        "groups": [
                            {"id": "front", "label": "Frontend"},
                            {"id": "core", "label": "Core"},
                            {"id": "data", "label": "Storage"}
                        ]
                    }
                }),
            },
            ToolExample {
                description: "A short decision flow with descriptions".to_string(),
                input: json!({
                    "title": "Publish decision",
                    "spec": {
                        "direction": "LR",
                        "nodes": [
                            {"id": "review", "label": "Review", "description": "Two approvals needed"},
                            {"id": "staging", "label": "Staging", "description": "Automated smoke tests"},
                            {"id": "ship", "label": "Ship", "description": "Signed release"}
                        ],
                        "edges": [
                            {"from": "review", "to": "staging", "label": "approved"},
                            {"from": "staging", "to": "ship", "label": "green"}
                        ]
                    }
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short diagram title shown in the card header"
                },
                "spec": {
                    "type": "object",
                    "description": "Structured diagram. Preferred for every normal diagram request; layout is generated for you.",
                    "properties": {
                        "direction": {
                            "type": "string",
                            "enum": ["TB", "LR"],
                            "description": "TB = top-to-bottom layers, LR = left-to-right flow"
                        },
                        "nodes": {
                            "type": "array",
                            "minItems": 1,
                            "maxItems": 120,
                            "description": "Every node needs a unique `id` and a short `label`.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Unique id referenced by edges" },
                                    "label": { "type": "string", "description": "Short visible label" },
                                    "description": { "type": "string", "description": "Shown in the inspector when the node is clicked" },
                                    "kind": { "type": "string", "description": "Optional role hint, e.g. 'primary', 'warning', 'success', 'muted'" },
                                    "group": { "type": "string", "description": "Optional group id, matching an entry in `groups`" },
                                    "icon": { "type": "string", "description": "Optional short 1-2 character glyph" }
                                },
                                "required": ["id", "label"]
                            }
                        },
                        "edges": {
                            "type": "array",
                            "maxItems": 240,
                            "description": "Directed connections between node ids.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "from": { "type": "string" },
                                    "to": { "type": "string" },
                                    "label": { "type": "string" },
                                    "style": { "type": "string", "enum": ["solid", "dashed"], "description": "Use 'dashed' for optional/async flows" }
                                },
                                "required": ["from", "to"]
                            }
                        },
                        "groups": {
                            "type": "array",
                            "maxItems": 24,
                            "description": "Optional labelled regions that visually cluster nodes.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "label": { "type": "string" },
                                    "color": { "type": "string", "description": "Optional hue hint such as 'blue' or 'violet'" }
                                },
                                "required": ["id", "label"]
                            }
                        }
                    },
                    "required": ["nodes"]
                },
                "raw_svg": {
                    "type": "string",
                    "description": "Escape hatch: hand-authored SVG markup. Only when the user explicitly asks for raw SVG. Scripts, event handlers, embedded media, and external references are rejected."
                },
                "theme": {
                    "type": "string",
                    "enum": ["auto", "dark", "light"],
                    "description": "auto follows the app theme; dark/light pin the diagram palette"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_result(args).await.map(|result| result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let title = optional_string(args.get("title"), "title", MAX_TITLE_CHARS)?;
        let theme = validated_theme(args.get("theme"));
        let raw_svg = optional_string(args.get("raw_svg"), "raw_svg", MAX_RAW_SVG_BYTES)?;
        let has_spec = matches!(args.get("spec"), Some(Value::Object(_)));

        if raw_svg.is_some() && has_spec {
            return Err(OSAgentError::ToolExecution(
                "Pass either 'spec' or 'raw_svg', not both.".to_string(),
            ));
        }

        let (source, title, spec, svg, node_count, edge_count) = if let Some(svg) = raw_svg {
            let svg = sanitize_raw_svg(&svg)?;
            let node_count = svg.matches("<g").count() + svg.matches("<rect").count();
            let edge_count = svg.matches("<path").count() + svg.matches("<line").count();
            let title = title.unwrap_or_else(|| "Diagram".to_string());
            ("raw", title, Value::Null, Some(svg), node_count, edge_count)
        } else if has_spec {
            let spec = build_structured_spec(args.get("spec").unwrap_or(&Value::Null))?;
            let node_count = spec["nodes"].as_array().map(|a| a.len()).unwrap_or(0);
            let edge_count = spec["edges"].as_array().map(|a| a.len()).unwrap_or(0);
            let title = title.unwrap_or_else(|| "Diagram".to_string());
            ("structured", title, spec, None, node_count, edge_count)
        } else {
            return Err(OSAgentError::ToolExecution(
                "draw_diagram requires a 'spec' object (preferred) or a 'raw_svg' string."
                    .to_string(),
            ));
        };

        let mut metadata = Map::new();
        metadata.insert("kind".to_string(), Value::String("diagram".to_string()));
        metadata.insert("version".to_string(), json!(1));
        metadata.insert("title".to_string(), Value::String(title.clone()));
        metadata.insert("source".to_string(), Value::String(source.to_string()));
        metadata.insert("theme".to_string(), Value::String(theme));
        metadata.insert("node_count".to_string(), json!(node_count));
        metadata.insert("edge_count".to_string(), json!(edge_count));
        if !spec.is_null() {
            metadata.insert("spec".to_string(), spec);
        }
        if let Some(svg) = svg {
            metadata.insert("svg".to_string(), Value::String(svg));
        }

        let output = format!(
            "Diagram \"{}\" drawn ({} source, {node_count} shape group(s), {edge_count} connector(s)). \
The user can pan, zoom, click shapes for detail, and download the SVG. Tell them what the diagram \
shows; do not paste the SVG source into the chat.",
            title, source
        );

        Ok(ToolResult {
            output,
            outcome: ToolOutcome::Success,
            title: Some(title),
            metadata: Value::Object(metadata),
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json() -> Value {
        json!({
            "title": "Flow",
            "spec": {
                "direction": "tb",
                "nodes": [
                    {"id": "a", "label": "A", "description": "first", "bogus": "drop me"},
                    {"id": "b", "label": "B", "group": "core", "width": 220}
                ],
                "edges": [{"from": "a", "to": "b", "label": "next"}],
                "groups": [{"id": "core", "label": "Core"}]
            },
            "session_id": "s1"
        })
    }

    #[test]
    fn structured_spec_is_validated_and_reduced_to_known_fields() {
        let result =
            tokio_test::block_on(DrawDiagramTool.execute_result(spec_json())).expect("result");
        assert_eq!(result.metadata["kind"], "diagram");
        assert_eq!(result.metadata["version"], 1);
        assert_eq!(result.metadata["source"], "structured");
        assert_eq!(result.metadata["title"], "Flow");
        assert_eq!(result.metadata["node_count"], 2);
        assert_eq!(result.metadata["edge_count"], 1);
        assert_eq!(result.metadata["spec"]["direction"], "TB");
        assert!(result.metadata["spec"]["nodes"][0].get("bogus").is_none());
        assert!(result.metadata.get("svg").is_none());
    }

    #[test]
    fn duplicate_node_ids_are_rejected() {
        let args = json!({
            "spec": {
                "nodes": [{"id": "a", "label": "A"}, {"id": "a", "label": "Again"}]
            }
        });
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("duplicate node id"));
    }

    #[test]
    fn edges_must_reference_known_nodes() {
        let args = json!({
            "spec": {
                "nodes": [{"id": "a", "label": "A"}],
                "edges": [{"from": "a", "to": "ghost"}]
            }
        });
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("unknown node 'ghost'"));
    }

    #[test]
    fn missing_spec_and_svg_is_an_error() {
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(json!({"title": "x"})))
            .expect_err("error");
        assert!(error.to_string().contains("requires a 'spec'"));
    }

    #[test]
    fn spec_and_raw_svg_together_is_an_error() {
        let args = json!({
            "spec": {"nodes": [{"id": "a", "label": "A"}]},
            "raw_svg": "<svg viewBox=\"0 0 10 10\"></svg>"
        });
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("not both"));
    }

    #[test]
    fn raw_svg_without_svg_root_is_rejected() {
        let args = json!({"raw_svg": "<div>not svg</div>"});
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("<svg> root"));
    }

    #[test]
    fn raw_svg_with_script_is_rejected() {
        let args = json!({"raw_svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>"});
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("forbidden construct"));
    }

    #[test]
    fn raw_svg_with_event_handler_is_rejected() {
        let args = json!({
            "raw_svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"10\" height=\"10\" onclick=\"alert(1)\"/></svg>"
        });
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("event handler"));
    }

    #[test]
    fn raw_svg_with_external_reference_is_rejected() {
        let args = json!({
            "raw_svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"><image href=\"https://example.com/x.png\"/></svg>"
        });
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("forbidden construct"));
    }

    #[test]
    fn fragment_references_are_allowed_in_raw_svg() {
        let args = json!({
            "raw_svg": "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><defs><linearGradient id=\"g\"><stop offset=\"0\"/></linearGradient></defs><rect width=\"10\" height=\"10\" fill=\"url(#g)\"/></svg>"
        });
        let result = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect("result");
        assert_eq!(result.metadata["source"], "raw");
        assert!(result.metadata["svg"].as_str().unwrap().contains("url(#g)"));
    }

    #[test]
    fn raw_svg_links_are_rejected_even_to_fragments() {
        let args = json!({
            "raw_svg": "<svg xmlns=\"http://www.w3.org/2000/svg\"><a href=\"#x\"><rect width=\"1\" height=\"1\"/></a></svg>"
        });
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("forbidden construct"));
    }

    #[test]
    fn node_limit_is_enforced() {
        let nodes: Vec<Value> = (0..(MAX_NODES + 1))
            .map(|index| json!({"id": format!("n{index}"), "label": "N"}))
            .collect();
        let args = json!({"spec": {"nodes": nodes}});
        let error = tokio_test::block_on(DrawDiagramTool.execute_result(args)).expect_err("error");
        assert!(error.to_string().contains("max 120"));
    }
}

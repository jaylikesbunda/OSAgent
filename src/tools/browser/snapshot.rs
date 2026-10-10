//! Accessibility-tree snapshots with stable element refs.
//!
//! The model never sees raw DOM. It sees a compact text outline of the
//! page's accessibility tree where every actionable element carries a
//! `[ref=eN]` handle. Actions take that handle and the browser resolves it
//! back to a DOM node, so the model does not need to guess selectors.

use serde_json::Value;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const MAX_NAME_CHARS: usize = 160;
const MAX_URL_CHARS: usize = 80;

#[derive(Debug, Clone)]
pub struct RefEntry {
    pub backend_node_id: i64,
    pub role: String,
    pub name: String,
}

/// Hard stop so a pathological page cannot produce an unbounded outline.
const MAX_LINES: usize = 6000;

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub lines: Vec<String>,
    /// Every actionable element on the page, including ones on pages of the
    /// outline that were not shown. A ref is valid whichever page it is on.
    pub refs: Vec<(String, RefEntry)>,
}

impl Snapshot {
    pub fn text(&self) -> String {
        self.lines.join("
")
    }

    pub fn fingerprint(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.lines.hash(&mut hasher);
        hasher.finish()
    }

    /// Append another frame's outline under a heading line.
    pub fn extend_frame(&mut self, heading: String, other: Snapshot) {
        if other.lines.is_empty() {
            return;
        }
        self.lines.push(heading);
        self.lines.extend(other.lines.into_iter().map(|line| format!("  {line}")));
        self.refs.extend(other.refs);
    }

    /// 1-based page of the outline holding at most `max_chars` of text
    /// (a single longer line still gets a page to itself). Returns the page
    /// text and the total number of pages.
    pub fn page(&self, page: usize, max_chars: usize) -> (String, usize) {
        let mut pages: Vec<Vec<&str>> = vec![Vec::new()];
        let mut used = 0;
        for line in &self.lines {
            let cost = line.len() + 1;
            if used + cost > max_chars && !pages.last().is_some_and(Vec::is_empty) {
                pages.push(Vec::new());
                used = 0;
            }
            pages.last_mut().expect("page exists").push(line);
            used += cost;
        }
        let total = pages.len();
        let index = page.clamp(1, total) - 1;
        (pages[index].join("
"), total)
    }
}

const INTERACTIVE_ROLES: &[&str] = &[
    "link",
    "button",
    "textbox",
    "searchbox",
    "combobox",
    "checkbox",
    "radio",
    "switch",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "slider",
    "spinbutton",
    "option",
    "listbox",
    "treeitem",
    "PopUpButton",
    "ComboBoxSelect",
    "DisclosureTriangle",
];

/// Roles whose children carry no structure worth indenting.
const PASS_THROUGH_ROLES: &[&str] = &[
    "none",
    "presentation",
    "generic",
    "RootWebArea",
    "WebArea",
    "paragraph",
    "Section",
    "div",
    "InlineTextBox",
    "LineBreak",
    "ListMarker",
];

pub fn is_interactive(role: &str) -> bool {
    INTERACTIVE_ROLES.contains(&role)
}

fn field(node: &Value, key: &str) -> String {
    match node.get(key).and_then(|v| v.get("value")) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

fn tidy(text: &str, limit: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= limit {
        collapsed
    } else {
        let cut: String = collapsed.chars().take(limit).collect();
        format!("{cut}…")
    }
}

fn properties(node: &Value) -> Vec<(String, String)> {
    node.get("properties")
        .and_then(Value::as_array)
        .map(|props| {
            props
                .iter()
                .filter_map(|prop| {
                    let name = prop.get("name")?.as_str()?.to_string();
                    let value = match prop.get("value")?.get("value")? {
                        Value::String(s) => s.clone(),
                        Value::Number(n) => n.to_string(),
                        Value::Bool(b) => b.to_string(),
                        _ => return None,
                    };
                    Some((name, value))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn describe_properties(role: &str, props: &[(String, String)]) -> String {
    let mut out = String::new();
    for (name, value) in props {
        match name.as_str() {
            "checked" | "pressed" if value != "false" => {
                out.push_str(&format!(" [{name}={value}]"));
            }
            "expanded" | "selected" => out.push_str(&format!(" [{name}={value}]")),
            "disabled" | "required" | "readonly" | "invalid" if value == "true" => {
                out.push_str(&format!(" [{name}]"));
            }
            "level" if role == "heading" => out.push_str(&format!(" [level={value}]")),
            "url" if role == "link" => {
                out.push_str(&format!(" [href={}]", tidy(value, MAX_URL_CHARS)));
            }
            _ => {}
        }
    }
    out
}

struct Renderer<'a> {
    nodes: &'a [Value],
    children: HashMap<&'a str, Vec<usize>>,
    interactive_only: bool,
    ref_start: usize,
    lines: Vec<String>,
    refs: Vec<(String, RefEntry)>,
}

impl<'a> Renderer<'a> {
    fn walk(&mut self, index: usize, depth: usize, ancestor_text: &str) {
        if self.lines.len() >= MAX_LINES {
            return;
        }
        let nodes = self.nodes;
        let node = &nodes[index];
        let ignored = node.get("ignored").and_then(Value::as_bool).unwrap_or(false);
        let role = field(node, "role");
        let name = tidy(&field(node, "name"), MAX_NAME_CHARS);
        let value = tidy(&field(node, "value"), MAX_NAME_CHARS);

        let mut child_depth = depth;
        let mut child_ancestor_text = ancestor_text.to_string();

        if !ignored {
            let interactive = is_interactive(&role);
            let backend = node.get("backendDOMNodeId").and_then(Value::as_i64);
            let mut line = None;

            if role == "StaticText" {
                if !self.interactive_only && !name.is_empty() && name != ancestor_text {
                    line = Some(format!("text \"{name}\""));
                }
            } else if interactive {
                let props = describe_properties(&role, &properties(node));
                let mut text = role.clone();
                if !name.is_empty() {
                    text.push_str(&format!(" \"{name}\""));
                }
                if !value.is_empty() && value != name {
                    text.push_str(&format!(" value=\"{value}\""));
                }
                text.push_str(&props);
                if let Some(backend) = backend {
                    let id = format!("e{}", self.ref_start + self.refs.len() + 1);
                    text.push_str(&format!(" [ref={id}]"));
                    self.refs.push((
                        id,
                        RefEntry {
                            backend_node_id: backend,
                            role: role.clone(),
                            name: name.clone(),
                        },
                    ));
                }
                line = Some(text);
                child_ancestor_text = name.clone();
            } else if !self.interactive_only
                && !PASS_THROUGH_ROLES.contains(&role.as_str())
                && !role.is_empty()
                && !name.is_empty()
            {
                let props = describe_properties(&role, &properties(node));
                line = Some(format!("{role} \"{name}\"{props}"));
                child_ancestor_text = name.clone();
            }

            if let Some(line) = line {
                let rendered = format!("{}- {}", "  ".repeat(depth.min(12)), line);
                self.lines.push(rendered);
                child_depth = depth + 1;
            }
        }

        let node_id = node.get("nodeId").and_then(Value::as_str).unwrap_or("");
        let kids = self
            .children
            .get(node_id)
            .cloned()
            .unwrap_or_default();
        for child in kids {
            self.walk(child, child_depth, &child_ancestor_text);
        }
    }
}

/// Render `Accessibility.getFullAXTree` nodes into an outline with refs.
/// Refs are numbered from `ref_start + 1` so several frames can share one
/// numbering.
pub fn render(nodes: &[Value], interactive_only: bool, ref_start: usize) -> Snapshot {
    if nodes.is_empty() {
        return Snapshot::default();
    }

    // The tree arrives flat. `childIds` is authoritative for ordering.
    let mut index_by_id: HashMap<&str, usize> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        if let Some(id) = node.get("nodeId").and_then(Value::as_str) {
            index_by_id.insert(id, index);
        }
    }
    let mut children: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut has_parent = vec![false; nodes.len()];
    for node in nodes {
        let Some(id) = node.get("nodeId").and_then(Value::as_str) else {
            continue;
        };
        if let Some(ids) = node.get("childIds").and_then(Value::as_array) {
            let list: Vec<usize> = ids
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|child| index_by_id.get(child).copied())
                .collect();
            for &child in &list {
                has_parent[child] = true;
            }
            children.insert(id, list);
        }
    }

    let mut renderer = Renderer {
        nodes,
        children,
        interactive_only,
        ref_start,
        lines: Vec::new(),
        refs: Vec::new(),
    };
    for index in 0..nodes.len() {
        if !has_parent[index] {
            renderer.walk(index, 0, "");
        }
    }

    Snapshot {
        lines: renderer.lines,
        refs: renderer.refs,
    }
}

/// Pick the ref whose accessible name best matches `needle`: exact name
/// match first, then substring. Case-insensitive.
pub fn find_ref<'a>(snapshot: &'a Snapshot, needle: &str) -> Vec<&'a (String, RefEntry)> {
    let needle = needle.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let exact: Vec<_> = snapshot
        .refs
        .iter()
        .filter(|(_, entry)| entry.name.to_lowercase() == needle)
        .collect();
    if !exact.is_empty() {
        return exact;
    }
    snapshot
        .refs
        .iter()
        .filter(|(_, entry)| entry.name.to_lowercase().contains(&needle))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> Vec<Value> {
        vec![
            json!({"nodeId":"1","role":{"value":"RootWebArea"},"name":{"value":"Demo"},
                   "childIds":["2","3","6","9"]}),
            json!({"nodeId":"2","role":{"value":"heading"},"name":{"value":"Welcome"},
                   "properties":[{"name":"level","value":{"value":1}}],
                   "childIds":["10"],"backendDOMNodeId":11}),
            json!({"nodeId":"3","role":{"value":"link"},"name":{"value":"Docs"},
                   "properties":[{"name":"url","value":{"value":"https://example.com/docs"}}],
                   "childIds":["4"],"backendDOMNodeId":21}),
            json!({"nodeId":"4","role":{"value":"StaticText"},"name":{"value":"Docs"},"childIds":[]}),
            json!({"nodeId":"6","role":{"value":"textbox"},"name":{"value":"Search"},
                   "value":{"value":"rust"},"backendDOMNodeId":31,"childIds":[]}),
            json!({"nodeId":"9","role":{"value":"button"},"name":{"value":"Go"},
                   "properties":[{"name":"disabled","value":{"value":true}}],
                   "backendDOMNodeId":41,"childIds":[]}),
            json!({"nodeId":"10","role":{"value":"StaticText"},"name":{"value":"Welcome"},"childIds":[]}),
            json!({"nodeId":"99","ignored":true,"role":{"value":"generic"},"childIds":[]}),
        ]
    }

    #[test]
    fn assigns_refs_to_interactive_nodes_in_document_order() {
        let snap = render(&fixture(), false, 0);
        let ids: Vec<_> = snap.refs.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["e1", "e2", "e3"]);
        assert_eq!(snap.refs[0].1.backend_node_id, 21);
        assert_eq!(snap.refs[1].1.role, "textbox");
        assert!(snap.text().contains("link \"Docs\" [href=https://example.com/docs] [ref=e1]"));
        assert!(snap.text().contains("textbox \"Search\" value=\"rust\" [ref=e2]"));
        assert!(snap.text().contains("button \"Go\" [disabled] [ref=e3]"));
    }

    #[test]
    fn suppresses_text_that_repeats_its_ancestor_name() {
        let snap = render(&fixture(), false, 0);
        assert!(!snap.text().contains("text \"Docs\""));
        assert!(!snap.text().contains("text \"Welcome\""));
        assert!(snap.text().contains("heading \"Welcome\" [level=1]"));
    }

    #[test]
    fn interactive_filter_drops_structure() {
        let snap = render(&fixture(), true, 0);
        assert!(!snap.text().contains("heading"));
        assert!(snap.text().contains("[ref=e1]"));
        assert_eq!(snap.refs.len(), 3);
    }

    #[test]
    fn pages_split_the_outline_but_keep_every_ref() {
        let snap = render(&fixture(), true, 0);
        let (first, total) = snap.page(1, 60);
        assert!(total > 1);
        assert!(!first.is_empty());
        assert_eq!(snap.refs.len(), 3, "refs on later pages stay usable");
        let (last, _) = snap.page(99, 60);
        assert!(last.contains("[ref=e3]"));
        let (_, one_page) = snap.page(1, 100_000);
        assert_eq!(one_page, 1);
    }

    #[test]
    fn frames_share_one_ref_numbering() {
        let mut main = render(&fixture(), true, 0);
        let inner = render(&fixture(), true, main.refs.len());
        main.extend_frame("iframe \"login\":".to_string(), inner);
        let ids: Vec<_> = main.refs.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["e1", "e2", "e3", "e4", "e5", "e6"]);
        assert!(main.text().contains("iframe \"login\":"));
    }

    #[test]
    fn find_ref_prefers_exact_names() {
        let snap = render(&fixture(), false, 0);
        assert_eq!(find_ref(&snap, "docs")[0].0, "e1");
        assert_eq!(find_ref(&snap, "sear")[0].0, "e2");
        assert!(find_ref(&snap, "missing").is_empty());
    }

    #[test]
    fn fingerprint_tracks_content() {
        let a = render(&fixture(), false, 0);
        let mut changed = fixture();
        changed[4]["value"] = json!({"value":"other"});
        let b = render(&changed, false, 0);
        assert_ne!(a.fingerprint(), b.fingerprint());
        assert_eq!(a.fingerprint(), render(&fixture(), false, 0).fingerprint());
    }
}

//! Keyboard parsing, box geometry and the annotation overlay.

use serde_json::Value;

pub const MOD_ALT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_META: u32 = 4;
pub const MOD_SHIFT: u32 = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeySpec {
    pub key: String,
    pub code: String,
    pub vk: u32,
    /// Text a key press inserts. `None` for non-printing keys and for
    /// chords with Ctrl/Alt/Meta, which are shortcuts rather than typing.
    pub text: Option<String>,
    pub modifiers: u32,
}

/// Parse `Enter`, `Tab`, `ArrowDown`, `a`, `Control+a`, `Shift+Tab`, ...
pub fn parse_key(spec: &str) -> Result<KeySpec, String> {
    let mut modifiers = 0;
    let mut parts: Vec<&str> = spec.split('+').map(str::trim).collect();
    // A literal "+" key splits into two empty parts.
    if spec.trim() == "+" {
        parts = vec!["+"];
    } else if spec.trim().ends_with("++") {
        let head = spec.trim().trim_end_matches("++");
        parts = head.split('+').map(str::trim).collect();
        parts.push("+");
    }
    let key_name = parts.pop().filter(|k| !k.is_empty()).ok_or("empty key")?;
    for modifier in parts {
        modifiers |= match modifier.to_ascii_lowercase().as_str() {
            "alt" | "option" => MOD_ALT,
            "ctrl" | "control" => MOD_CTRL,
            "meta" | "cmd" | "command" | "win" | "super" => MOD_META,
            "shift" => MOD_SHIFT,
            other => return Err(format!("unknown modifier `{other}`")),
        };
    }

    let named = |key: &str, code: &str, vk: u32, text: Option<&str>| KeySpec {
        key: key.to_string(),
        code: code.to_string(),
        vk,
        text: text.map(str::to_string),
        modifiers,
    };

    let spec = match key_name.to_ascii_lowercase().as_str() {
        "enter" | "return" => named("Enter", "Enter", 13, Some("\r")),
        "tab" => named("Tab", "Tab", 9, None),
        "escape" | "esc" => named("Escape", "Escape", 27, None),
        "backspace" => named("Backspace", "Backspace", 8, None),
        "delete" | "del" => named("Delete", "Delete", 46, None),
        "space" | " " => named(" ", "Space", 32, Some(" ")),
        "arrowup" | "up" => named("ArrowUp", "ArrowUp", 38, None),
        "arrowdown" | "down" => named("ArrowDown", "ArrowDown", 40, None),
        "arrowleft" | "left" => named("ArrowLeft", "ArrowLeft", 37, None),
        "arrowright" | "right" => named("ArrowRight", "ArrowRight", 39, None),
        "home" => named("Home", "Home", 36, None),
        "end" => named("End", "End", 35, None),
        "pageup" => named("PageUp", "PageUp", 33, None),
        "pagedown" => named("PageDown", "PageDown", 34, None),
        _ => {
            let mut chars = key_name.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                return Err(format!("unsupported key `{key_name}`"));
            };
            let upper = ch.to_ascii_uppercase();
            let (code, vk) = if ch.is_ascii_alphabetic() {
                (format!("Key{upper}"), upper as u32)
            } else if ch.is_ascii_digit() {
                (format!("Digit{ch}"), ch as u32)
            } else {
                (String::new(), 0)
            };
            let shifted = modifiers & MOD_SHIFT != 0;
            let shortcut = modifiers & (MOD_CTRL | MOD_ALT | MOD_META) != 0;
            let shown = if shifted { ch.to_ascii_uppercase() } else { ch };
            KeySpec {
                key: shown.to_string(),
                code,
                vk,
                text: (!shortcut).then(|| shown.to_string()),
                modifiers,
            }
        }
    };
    Ok(spec)
}

/// Bounding rect `(x, y, width, height)` of a CDP quad `[x1,y1,...,x4,y4]`.
pub fn quad_rect(quad: &[f64]) -> Option<(f64, f64, f64, f64)> {
    if quad.len() < 8 {
        return None;
    }
    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];
    let min_x = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max_x = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some((min_x, min_y, max_x - min_x, max_y - min_y))
}

pub fn rect_center(rect: (f64, f64, f64, f64)) -> (f64, f64) {
    (rect.0 + rect.2 / 2.0, rect.1 + rect.3 / 2.0)
}

/// Quad from a `DOM.getBoxModel` response (`model.content`).
pub fn box_model_quad(response: &Value) -> Option<Vec<f64>> {
    let quad = response
        .get("model")?
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(Value::as_f64)
        .collect::<Vec<_>>();
    (quad.len() >= 8).then_some(quad)
}

pub const REMOVE_OVERLAY_JS: &str =
    "(()=>{const o=document.getElementById('__osa_overlay');if(o)o.remove();})()";

/// JavaScript that draws numbered boxes over `(label, x, y, w, h)` rects in
/// viewport coordinates. Labels are the refs so the screenshot and the
/// text snapshot refer to the same handles.
pub fn overlay_script(boxes: &[(String, f64, f64, f64, f64)]) -> String {
    let data = serde_json::to_string(
        &boxes
            .iter()
            .map(|(label, x, y, w, h)| serde_json::json!([label, x, y, w, h]))
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    format!(
        r#"(()=>{{
const old=document.getElementById('__osa_overlay');if(old)old.remove();
const root=document.createElement('div');root.id='__osa_overlay';
root.style.cssText='position:fixed;left:0;top:0;width:100vw;height:100vh;pointer-events:none;z-index:2147483647';
for(const [label,x,y,w,h] of {data}){{
const box=document.createElement('div');
box.style.cssText='position:fixed;box-sizing:border-box;border:2px solid #e11d48;left:'+x+'px;top:'+y+'px;width:'+w+'px;height:'+h+'px';
const tag=document.createElement('span');tag.textContent=label;
tag.style.cssText='position:absolute;left:-2px;top:-16px;background:#e11d48;color:#fff;font:bold 11px monospace;padding:0 3px;line-height:14px';
box.appendChild(tag);root.appendChild(box);}}
document.documentElement.appendChild(root);}})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_named_and_character_keys() {
        let enter = parse_key("Enter").unwrap();
        assert_eq!((enter.vk, enter.text.as_deref()), (13, Some("\r")));

        let a = parse_key("a").unwrap();
        assert_eq!((a.code.as_str(), a.vk, a.text.as_deref()), ("KeyA", 65, Some("a")));

        let digit = parse_key("7").unwrap();
        assert_eq!(digit.code, "Digit7");
    }

    #[test]
    fn chords_carry_modifiers_and_suppress_text() {
        let select_all = parse_key("Control+a").unwrap();
        assert_eq!(select_all.modifiers, MOD_CTRL);
        assert_eq!(select_all.text, None);

        let shift_tab = parse_key("Shift+Tab").unwrap();
        assert_eq!(shift_tab.modifiers, MOD_SHIFT);
        assert_eq!(shift_tab.key, "Tab");

        let upper = parse_key("Shift+a").unwrap();
        assert_eq!(upper.text.as_deref(), Some("A"));
    }

    #[test]
    fn rejects_unknown_input() {
        assert!(parse_key("Hyper+x").is_err());
        assert!(parse_key("F13x").is_err());
        assert!(parse_key("").is_err());
    }

    #[test]
    fn plus_key_is_literal() {
        assert_eq!(parse_key("+").unwrap().key, "+");
        let chord = parse_key("Control++").unwrap();
        assert_eq!((chord.key.as_str(), chord.modifiers), ("+", MOD_CTRL));
    }

    #[test]
    fn quad_geometry() {
        let rect = quad_rect(&[10.0, 20.0, 110.0, 20.0, 110.0, 60.0, 10.0, 60.0]).unwrap();
        assert_eq!(rect, (10.0, 20.0, 100.0, 40.0));
        assert_eq!(rect_center(rect), (60.0, 40.0));
        assert!(quad_rect(&[1.0, 2.0]).is_none());
    }

    #[test]
    fn extracts_box_model_quad() {
        let response = json!({"model": {"content": [0,0,4,0,4,2,0,2]}});
        assert_eq!(box_model_quad(&response).unwrap().len(), 8);
        assert!(box_model_quad(&json!({})).is_none());
    }

    #[test]
    fn overlay_embeds_labels_as_json() {
        let script = overlay_script(&[("e1".to_string(), 1.0, 2.0, 30.0, 40.0)]);
        assert!(script.contains(r#"["e1",1.0,2.0,30.0,40.0]"#));
        assert!(script.contains("__osa_overlay"));
    }
}

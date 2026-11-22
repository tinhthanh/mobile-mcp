//! The compact screen for iOS, from WebDriverAgent's `/source?format=json`,
//! in the same format as Android's: `<mark><label>|x,y [flags]`. Coordinates
//! are points, which is what WebDriverAgent taps take.

use serde_json::Value;

use crate::util::truncate;

const MAX_LABEL: usize = 60;
const CONTAINER_AREA: f64 = 0.4;

const TAPPABLE: [&str; 20] = [
    "Button",
    "Cell",
    "Link",
    "Switch",
    "Toggle",
    "Slider",
    "Stepper",
    "Tab",
    "MenuItem",
    "MenuBarItem",
    "Icon",
    "PickerWheel",
    "CheckBox",
    "RadioButton",
    "PopUpButton",
    "ComboBox",
    "MenuButton",
    "DisclosureTriangle",
    "SegmentedControl",
    "PageIndicator",
];
const INPUT: [&str; 4] = ["TextField", "SecureTextField", "SearchField", "TextView"];
const SCROLLABLE: [&str; 3] = ["ScrollView", "Table", "CollectionView"];
/// Plain text an agent reads.
const TEXT: [&str; 2] = ["StaticText", "Image"];

#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn ty(n: &Value) -> &str {
    let t = n.get("type").and_then(Value::as_str).unwrap_or("");
    t.strip_prefix("XCUIElementType").unwrap_or(t)
}

fn truthy(v: Option<&Value>) -> Option<bool> {
    match v? {
        Value::Bool(b) => Some(*b),
        Value::String(s) => Some(s == "1" || s == "true"),
        Value::Number(n) => Some(n.as_i64() == Some(1)),
        _ => None,
    }
}

fn rect(n: &Value) -> Option<Rect> {
    let r = n.get("rect").or_else(|| n.get("frame"))?;
    let f = |k: &str| r.get(k).and_then(Value::as_f64);
    Some(Rect {
        x: f("x")?,
        y: f("y")?,
        w: f("width")?,
        h: f("height")?,
    })
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn str_of<'a>(n: &'a Value, k: &str) -> &'a str {
    n.get(k).and_then(Value::as_str).unwrap_or("")
}

fn own_label(n: &Value) -> String {
    // a field shows what is typed in it (or its placeholder), like Android's text
    if INPUT.contains(&ty(n)) {
        let v = clean(str_of(n, "value"));
        if !v.is_empty() {
            return v;
        }
    }
    let label = clean(str_of(n, "label"));
    if !label.is_empty() {
        return label;
    }
    if TEXT.contains(&ty(n)) {
        return clean(str_of(n, "value"));
    }
    String::new()
}

fn short_id(n: &Value) -> String {
    let id = str_of(n, "rawIdentifier");
    let id = if id.is_empty() { str_of(n, "name") } else { id };
    id.rsplit('.').next().unwrap_or("").to_string()
}

fn children(n: &Value) -> impl Iterator<Item = &Value> {
    n.get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn is_tappable(n: &Value) -> bool {
    TAPPABLE.contains(&ty(n))
}

fn is_input(n: &Value) -> bool {
    INPUT.contains(&ty(n))
}

/// Adds `s` unless a part already says it ("Wi-Fi, Cafe" covers "Wi-Fi").
fn push_part(out: &mut Vec<String>, s: String) {
    if s.is_empty() || out.iter().any(|p| p.contains(&s)) {
        return;
    }
    out.retain(|p| !s.contains(p.as_str()));
    out.push(s);
}

fn collect_texts(n: &Value, out: &mut Vec<String>) {
    for c in children(n) {
        if is_tappable(c) || is_input(c) || ty(c) == "Keyboard" {
            continue;
        }
        if TEXT.contains(&ty(c)) {
            push_part(out, own_label(c));
        }
        collect_texts(c, out);
    }
}

struct Entry {
    line: String,
    point: String,
    fallback: bool,
}

struct Walker {
    screen: Option<Rect>,
    entries: Vec<Entry>,
}

impl Walker {
    fn visible(&self, n: &Value, r: &Rect) -> bool {
        if truthy(n.get("isVisible")) == Some(false) || r.w <= 0.0 || r.h <= 0.0 {
            return false;
        }
        let Some(s) = self.screen else { return true };
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        cx >= s.x && cx <= s.x + s.w && cy >= s.y && cy <= s.y + s.h
    }

    fn actionable(&self, n: &Value) -> bool {
        if is_input(n) {
            return true;
        }
        if !is_tappable(n) {
            return false;
        }
        match (rect(n), self.screen) {
            (Some(r), Some(s)) if s.w * s.h > 0.0 => r.w * r.h < s.w * s.h * CONTAINER_AREA,
            _ => true,
        }
    }

    fn emit(&mut self, mark: char, label: &str, fallback: bool, r: &Rect, n: &Value) {
        let mut flags = Vec::new();
        let v = str_of(n, "value");
        if matches!(ty(n), "Switch" | "Toggle" | "CheckBox")
            && (v == "1" || v.eq_ignore_ascii_case("on"))
        {
            flags.push("on");
        }
        if str_of(n, "traits").contains("Selected") || truthy(n.get("isSelected")) == Some(true) {
            flags.push("sel");
        }
        if truthy(n.get("isEnabled")) == Some(false) {
            flags.push("off");
        }
        if is_input(n) && truthy(n.get("hasFocus")) == Some(true) {
            flags.push("focus");
        }
        let point = format!("{},{}", (r.x + r.w / 2.0) as i64, (r.y + r.h / 2.0) as i64);
        let mut line = format!("{mark}{}|{point}", truncate(label, MAX_LABEL));
        if !flags.is_empty() {
            line.push(' ');
            line.push_str(&flags.join(" "));
        }
        if !self.entries.iter().any(|e| e.line == line) {
            self.entries.push(Entry {
                line,
                point,
                fallback,
            });
        }
    }

    fn walk(&mut self, n: &Value, under_tappable: bool) {
        if ty(n) == "Keyboard" {
            return;
        }
        let actionable = self.actionable(n);
        if let Some(r) = rect(n)
            && self.visible(n, &r)
        {
            let own = own_label(n);
            if actionable {
                let mut parts = Vec::new();
                push_part(&mut parts, own.clone());
                collect_texts(n, &mut parts);
                let label = parts.join(" · ");
                let shown = if !label.is_empty() {
                    label.clone()
                } else if !short_id(n).is_empty() {
                    short_id(n)
                } else {
                    ty(n).to_string()
                };
                self.emit(
                    if is_input(n) { '#' } else { '*' },
                    &shown,
                    label.is_empty(),
                    &r,
                    n,
                );
            } else if SCROLLABLE.contains(&ty(n)) {
                let shown = if own.is_empty() {
                    ty(n).to_string()
                } else {
                    own.clone()
                };
                self.emit('~', &shown, own.is_empty(), &r, n);
            } else if TEXT.contains(&ty(n)) && !own.is_empty() && !under_tappable {
                self.emit(' ', &own, false, &r, n);
            }
        }
        for c in children(n) {
            self.walk(c, actionable || under_tappable);
        }
    }
}

/// Lines of the compact screen (without the `app=` line).
pub fn compact(source: &Value) -> Vec<String> {
    let root = source.get("value").unwrap_or(source);
    let mut w = Walker {
        screen: rect(root),
        entries: Vec::new(),
    };
    w.walk(root, false);
    let labelled: Vec<&str> = w
        .entries
        .iter()
        .filter(|e| !e.fallback)
        .map(|e| e.point.as_str())
        .collect();
    let mut seen: Vec<&str> = Vec::new();
    let mut lines = Vec::new();
    for e in &w.entries {
        if e.fallback {
            let keep = !labelled.contains(&e.point.as_str()) && !seen.contains(&e.point.as_str());
            seen.push(&e.point);
            if !keep {
                continue;
            }
        }
        lines.push(e.line.clone());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(ty: &str, label: &str, x: i64, y: i64, w: i64, h: i64, children: Vec<Value>) -> Value {
        json!({"type": ty, "label": label, "rect": {"x": x, "y": y, "width": w, "height": h}, "isVisible": "1", "isEnabled": "1", "children": children})
    }

    fn app(children: Vec<Value>) -> Value {
        json!({"value": node("Application", "Settings", 0, 0, 375, 812, children)})
    }

    #[test]
    fn merges_cell_texts_without_repeating_the_cell_label() {
        let cell = node(
            "Cell",
            "Wi-Fi, Cafe",
            16,
            388,
            343,
            53,
            vec![
                node("StaticText", "Wi-Fi", 60, 400, 50, 20, vec![]),
                node("StaticText", "Cafe", 250, 400, 60, 20, vec![]),
            ],
        );
        let plain = node(
            "Cell",
            "",
            16,
            440,
            343,
            53,
            vec![node("StaticText", "Pin", 60, 450, 40, 20, vec![])],
        );
        assert_eq!(
            compact(&app(vec![cell, plain])),
            ["*Wi-Fi, Cafe|187,414", "*Pin|187,466"]
        );
    }

    #[test]
    fn marks_inputs_switches_disabled_and_skips_the_keyboard() {
        let mut field = node("TextField", "", 16, 100, 343, 40, vec![]);
        field["value"] = json!("Tìm kiếm");
        let mut sw = node("Switch", "Chế độ máy bay", 300, 200, 51, 31, vec![]);
        sw["value"] = json!("1");
        let mut off = node("Button", "Gửi", 0, 300, 100, 40, vec![]);
        off["isEnabled"] = json!("0");
        let kb = node(
            "Keyboard",
            "",
            0,
            500,
            375,
            300,
            vec![node("Key", "q", 0, 500, 30, 40, vec![])],
        );
        assert_eq!(
            compact(&app(vec![field, sw, off, kb])),
            [
                "#Tìm kiếm|187,120",
                "*Chế độ máy bay|325,215 on",
                "*Gửi|50,320 off"
            ]
        );
    }

    #[test]
    fn hides_invisible_and_offscreen_and_container_merges() {
        let mut hidden = node("StaticText", "hidden", 0, 0, 100, 20, vec![]);
        hidden["isVisible"] = json!("0");
        let below = node("StaticText", "below", 0, 900, 100, 20, vec![]);
        let container = node(
            "Button",
            "",
            0,
            0,
            375,
            700,
            vec![node("StaticText", "Tin nhắn", 10, 100, 100, 20, vec![])],
        );
        assert_eq!(
            compact(&app(vec![hidden, below, container])),
            [" Tin nhắn|60,110"]
        );
    }
}

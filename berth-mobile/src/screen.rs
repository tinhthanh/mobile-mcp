//! The compact screen: one line per element an agent can read or tap.
//!
//! `<mark><label>|<x>,<y>[ flags]`, x,y is the tap point (center). Marks: `*`
//! tappable, `#` text input, `~` scrollable, ` ` plain text.

use unicode_normalization::UnicodeNormalization;

use crate::util::{Error, Result, truncate};

pub const SCREEN_LEGEND: &str = "Lines are \"<mark><label>|x,y [flags]\" where x,y is the center to tap. \
Marks: * tappable, # text input, ~ scrollable, space = plain text. Flags: on (checked), sel (selected), off (disabled), focus.";

const MAX_LABEL: usize = 60;

/// A "tappable" view covering this much of the screen is a container (a
/// clickable root FrameLayout in Zalo); merging its texts would collapse the
/// whole screen into one line.
const CONTAINER_AREA: f64 = 0.4;

#[derive(Clone, Copy)]
struct Rect {
    left: i64,
    top: i64,
    right: i64,
    bottom: i64,
}

impl Rect {
    fn area(&self) -> i64 {
        (self.right - self.left) * (self.bottom - self.top)
    }
}

type Node<'a, 'i> = roxmltree::Node<'a, 'i>;

fn parse_bounds(b: Option<&str>) -> Option<Rect> {
    // "[l,t][r,b]"
    let s = b?.strip_prefix('[')?.strip_suffix(']')?;
    let (a, c) = s.split_once("][")?;
    let (l, t) = a.split_once(',')?;
    let (r, bo) = c.split_once(',')?;
    Some(Rect {
        left: l.parse().ok()?,
        top: t.parse().ok()?,
        right: r.parse().ok()?,
        bottom: bo.parse().ok()?,
    })
}

fn attr<'a>(n: &Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attribute(name)
}

fn is_true(n: &Node, name: &str) -> bool {
    attr(n, name) == Some("true")
}

fn clean(v: Option<&str>) -> String {
    v.unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn own_label(n: &Node) -> String {
    for key in ["text", "content-desc", "hint"] {
        let s = clean(attr(n, key));
        if !s.is_empty() {
            return s;
        }
    }
    String::new()
}

fn short_id(n: &Node) -> String {
    let id = attr(n, "resource-id").unwrap_or("");
    id.rsplit('/').next().unwrap_or("").to_string()
}

fn short_class(n: &Node) -> String {
    // appium names the element after its class; uiautomator dump has class=
    let cls = attr(n, "class")
        .filter(|c| !c.is_empty() && *c != "hierarchy")
        .unwrap_or_else(|| {
            let tag = n.tag_name().name();
            if tag == "node" { "View" } else { tag }
        });
    cls.rsplit('.').next().unwrap_or("View").to_string()
}

fn class_of<'a>(n: &Node<'a, '_>) -> &'a str {
    attr(n, "class").unwrap_or("")
}

fn is_input(n: &Node) -> bool {
    let c = class_of(n);
    let c = if c.is_empty() { n.tag_name().name() } else { c };
    c.contains("EditText") || c.contains("AutoCompleteTextView")
}

fn is_tappable(n: &Node) -> bool {
    is_true(n, "clickable") || is_true(n, "long-clickable") || is_true(n, "checkable")
}

fn children<'a, 'i>(n: &Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

/// Texts of non-interactive descendants, which belong to the tappable ancestor.
fn collect_texts(n: &Node, out: &mut Vec<String>) {
    for c in children(n) {
        if is_tappable(&c) || is_input(&c) {
            continue;
        }
        let label = own_label(&c);
        if !label.is_empty() && !out.contains(&label) {
            out.push(label);
        }
        collect_texts(&c, out);
    }
}

pub struct CompactScreen {
    pub app: String,
    pub lines: Vec<String>,
}

struct Entry {
    line: String,
    point: String,
    /// The label is a resource id or class name, not something a person reads.
    fallback: bool,
}

struct Walker {
    screen: Option<Rect>,
    entries: Vec<Entry>,
}

impl Walker {
    fn visible(&self, r: &Rect) -> bool {
        if r.right <= r.left || r.bottom <= r.top {
            return false;
        }
        let Some(s) = self.screen else { return true };
        let cx = (r.left + r.right) as f64 / 2.0;
        let cy = (r.top + r.bottom) as f64 / 2.0;
        cx >= s.left as f64 && cx <= s.right as f64 && cy >= s.top as f64 && cy <= s.bottom as f64
    }

    fn actionable(&self, n: &Node) -> bool {
        if is_input(n) {
            return true;
        }
        if !is_tappable(n) {
            return false;
        }
        let screen_area = self.screen.map(|s| s.area()).unwrap_or(0);
        match parse_bounds(attr(n, "bounds")) {
            Some(r) if screen_area > 0 => (r.area() as f64) < screen_area as f64 * CONTAINER_AREA,
            _ => true,
        }
    }

    fn emit(&mut self, mark: char, label: &str, fallback: bool, r: &Rect, n: &Node) {
        let mut flags = Vec::new();
        if is_true(n, "checked") {
            flags.push("on");
        }
        if is_true(n, "selected") {
            flags.push("sel");
        }
        if attr(n, "enabled").is_some() && !is_true(n, "enabled") {
            flags.push("off");
        }
        if is_input(n) && is_true(n, "focused") {
            flags.push("focus");
        }
        let point = format!("{},{}", (r.left + r.right) >> 1, (r.top + r.bottom) >> 1);
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

    fn walk(&mut self, n: &Node, under_tappable: bool) {
        let actionable = self.actionable(n);
        if let Some(r) = parse_bounds(attr(n, "bounds"))
            && self.visible(&r)
            && attr(n, "displayed") != Some("false")
        {
            let own = own_label(n);
            if actionable {
                let mut texts = Vec::new();
                if !own.is_empty() {
                    texts.push(own.clone());
                }
                collect_texts(n, &mut texts);
                let label = texts.join(" · ");
                let shown = if !label.is_empty() {
                    label.clone()
                } else if !short_id(n).is_empty() {
                    short_id(n)
                } else {
                    short_class(n)
                };
                self.emit(
                    if is_input(n) { '#' } else { '*' },
                    &shown,
                    label.is_empty(),
                    &r,
                    n,
                );
            } else if is_true(n, "scrollable") {
                let shown = if !own.is_empty() {
                    own.clone()
                } else if !short_id(n).is_empty() {
                    short_id(n)
                } else {
                    short_class(n)
                };
                self.emit('~', &shown, own.is_empty(), &r, n);
            } else if !own.is_empty() && !under_tappable {
                self.emit(' ', &own, false, &r, n);
            }
        }
        for c in children(n) {
            self.walk(&c, actionable || under_tappable);
        }
    }
}

/// Turns a uiautomator dump or appium page source into the compact screen.
pub fn compact_screen(dump: &str) -> Result<CompactScreen> {
    let xml = &dump[dump.find("<?xml").unwrap_or(0)..];
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| Error::other(format!("couldn't read the screen: {e}")))?;
    let root = doc.root_element();
    let roots: Vec<Node> = if root.tag_name().name() == "hierarchy" {
        children(&root).collect()
    } else {
        vec![root]
    };
    let screen = roots.iter().find_map(|r| parse_bounds(attr(r, "bounds")));
    let mut w = Walker {
        screen,
        entries: Vec::new(),
    };
    for r in &roots {
        w.walk(r, false);
    }

    // widgets often stack an id-only view on the labelled one ("search_widget_ghost_lens" over
    // "Camera search"): keep the readable line for that tap point
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
            // several unlabelled views on one point need only one line
            let keep = !labelled.contains(&e.point.as_str()) && !seen.contains(&e.point.as_str());
            seen.push(&e.point);
            if !keep {
                continue;
            }
        }
        lines.push(e.line.clone());
    }
    let app = roots
        .iter()
        .find_map(|r| attr(r, "package"))
        .unwrap_or("unknown")
        .to_string();
    Ok(CompactScreen { app, lines })
}

/// Typographic quotes and dashes ("Don’t allow") match what an agent types.
fn normalize(s: &str) -> String {
    let nfc: String = s.nfc().collect();
    let mapped: String = nfc
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{2032}' | '\u{00B4}' | '`' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{2033}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            c => c,
        })
        .collect();
    mapped
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, PartialEq)]
pub struct Found {
    pub line: String,
    pub x: i64,
    pub y: i64,
    pub matches: usize,
}

/// The line to tap for `text`: an exact label part on a tappable line wins,
/// then exact on any line, then a substring.
pub fn find_line(screen: &str, text: &str) -> Option<Found> {
    let wanted = normalize(text);
    let mut rows: Vec<(u8, String, i64, i64)> = Vec::new();
    for line in screen.lines().skip(1) {
        let Some(bar) = line.rfind('|') else { continue };
        let point = line[bar + 1..].split(' ').next().unwrap_or("");
        let mut xy = point.split(',').map(|n| n.parse::<i64>());
        let (Some(Ok(x)), Some(Ok(y))) = (xy.next(), xy.next()) else {
            continue;
        };
        let mark = line.chars().next().unwrap_or(' ');
        let label = &line[mark.len_utf8().min(bar)..bar];
        let parts: Vec<String> = label.split(" · ").map(normalize).collect();
        let tappable = mark == '*' || mark == '#';
        let exact = parts.contains(&wanted);
        let partial = parts.iter().any(|p| p.contains(&wanted));
        let rank = match (exact, partial, tappable) {
            (true, _, true) => 0,
            (true, _, false) => 1,
            (_, true, true) => 2,
            (_, true, false) => 3,
            _ => continue,
        };
        rows.push((rank, line.to_string(), x, y));
    }
    let best = rows.iter().map(|r| r.0).min()?;
    let top: Vec<_> = rows.into_iter().filter(|r| r.0 == best).collect();
    let matches = top.len();
    let (_, line, x, y) = top.into_iter().next()?;
    Some(Found {
        line,
        x,
        y,
        matches,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(attrs: &str, inner: &str) -> String {
        format!("<node {attrs}>{inner}</node>")
    }

    fn dump(inner: &str) -> String {
        format!(
            "junk<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy rotation=\"0\">{}</hierarchy>",
            node("package=\"com.example\" bounds=\"[0,0][720,1600]\"", inner)
        )
    }

    fn lines(xml: &str) -> Vec<String> {
        compact_screen(xml).unwrap().lines
    }

    #[test]
    fn merges_child_texts_and_drops_layout_noise() {
        let xml = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../test/fixtures/settings.xml"
        ))
        .unwrap();
        let s = compact_screen(&xml).unwrap();
        assert_eq!(s.app, "com.android.settings");
        for want in [
            "*Wi‑Fi · Example WiFi|360,375",
            "*Bluetooth · Off|360,495",
            "*Display|360,1159",
            "#Search settings|371,267",
        ] {
            assert!(
                s.lines.iter().any(|l| l == want),
                "missing {want}: {:#?}",
                s.lines
            );
        }
        assert!(!s.lines.iter().any(|l| l.contains("dashboard_tile")));
        assert!(!s.lines.iter().any(|l| l.starts_with(" Display")));
        // the raw dump is ~9k tokens, mobile-mcp's element list ~2.3k
        assert!(s.lines.join("\n").chars().count() / 4 < 150);
    }

    #[test]
    fn keeps_nested_tappables_separate() {
        let subtitle = node("text=\"subtitle\" bounds=\"[0,50][300,100]\"", "");
        let toggle = node(
            "clickable=\"true\" checkable=\"true\" checked=\"true\" resource-id=\"a:id/switch\" bounds=\"[600,0][720,100]\"",
            "",
        );
        let xml = dump(&node(
            "clickable=\"true\" text=\"Row\" bounds=\"[0,0][720,100]\"",
            &(subtitle + &toggle),
        ));
        assert_eq!(lines(&xml), ["*Row · subtitle|360,50", "*switch|660,50 on"]);
    }

    #[test]
    fn reads_appium_page_source() {
        let xml = "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><hierarchy index=\"0\" class=\"hierarchy\" rotation=\"0\" width=\"720\" height=\"1522\">\
            <android.widget.FrameLayout package=\"com.example\" bounds=\"[0,0][720,1600]\" displayed=\"true\">\
            <android.widget.LinearLayout clickable=\"true\" bounds=\"[0,0][720,100]\" displayed=\"true\">\
            <android.widget.TextView text=\"Hiển thị\" bounds=\"[0,0][300,100]\" displayed=\"true\"/>\
            </android.widget.LinearLayout>\
            <android.widget.TextView text=\"hidden\" bounds=\"[0,100][300,200]\" displayed=\"false\"/>\
            </android.widget.FrameLayout></hierarchy>";
        let s = compact_screen(xml).unwrap();
        assert_eq!(s.app, "com.example");
        assert_eq!(s.lines, ["*Hiển thị|360,50"]);
    }

    #[test]
    fn skips_zero_size_and_offscreen() {
        let xml = dump(
            &(node("text=\"gone\" bounds=\"[0,0][0,0]\"", "")
                + &node("text=\"below\" bounds=\"[0,1700][720,1800]\"", "")
                + &node("text=\"here\" bounds=\"[0,0][100,100]\"", "")),
        );
        assert_eq!(lines(&xml), [" here|50,50"]);
    }

    #[test]
    fn flags_disabled_and_focused() {
        let xml = dump(
            &(node(
                "clickable=\"true\" enabled=\"false\" text=\"Next\" bounds=\"[0,0][100,100]\"",
                "",
            ) + &node(
                "class=\"android.widget.EditText\" focused=\"true\" text=\"\" hint=\"Email\" bounds=\"[0,100][720,200]\"",
                "",
            )),
        );
        assert_eq!(lines(&xml), ["*Next|50,50 off", "#Email|360,150 focus"]);
    }

    #[test]
    fn no_merge_into_screen_sized_tappable() {
        let texts = node("text=\"Messages\" bounds=\"[0,300][200,350]\"", "")
            + &node("text=\"Links&#10;Files\" bounds=\"[0,400][720,450]\"", "");
        let xml = dump(&node(
            "clickable=\"true\" bounds=\"[0,0][720,1500]\"",
            &texts,
        ));
        assert_eq!(lines(&xml), [" Messages|100,325", " Links Files|360,425"]);
    }

    #[test]
    fn drops_id_only_line_stacked_on_labelled() {
        let xml = dump(
            &(node(
                "clickable=\"true\" resource-id=\"g:id/search_widget_ghost_lens\" bounds=\"[600,400][660,500]\"",
                "",
            ) + &node(
                "clickable=\"true\" content-desc=\"Camera search\" bounds=\"[600,400][660,500]\"",
                "",
            ) + &node(
                "clickable=\"true\" resource-id=\"g:id/only_id\" bounds=\"[0,400][100,500]\"",
                "",
            )),
        );
        assert_eq!(lines(&xml), ["*Camera search|630,450", "*only_id|50,450"]);
    }

    const SCREEN: &str = "app=com.android.settings\n Display|412,159\n*Display size · Default|360,1170\n*Display|360,1159\n*Hiển thị|360,200";

    #[test]
    fn prefers_exact_tappable() {
        let f = find_line(SCREEN, "display").unwrap();
        assert_eq!((f.x, f.y, f.matches), (360, 1159, 1));
    }

    #[test]
    fn matches_merged_part_and_unicode() {
        assert_eq!(
            find_line(SCREEN, "Default").map(|f| (f.x, f.y)),
            Some((360, 1170))
        );
        assert_eq!(
            find_line(SCREEN, "hiển thị").map(|f| (f.x, f.y)),
            Some((360, 200))
        );
    }

    #[test]
    fn typographic_quotes_and_dashes() {
        let dialog = "app=com.google.android.permissioncontroller\n*Don’t allow|360,1036\n*Wi‑Fi – settings|360,400";
        assert_eq!(find_line(dialog, "Don't allow").map(|f| f.y), Some(1036));
        assert_eq!(
            find_line(dialog, "wi-fi - settings").map(|f| f.y),
            Some(400)
        );
    }

    #[test]
    fn null_when_nothing_matches() {
        assert!(find_line(SCREEN, "Bluetooth").is_none());
    }
}

//! Switches the person sets in Shipyard (Settings → Plugins). Shipyard keeps
//! the values in the file named by $BERTH_PLUGIN_SETTINGS; this binary
//! enforces them.

use serde_json::{Value, json};

pub struct Setting {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub default: bool,
    pub warning: Option<&'static str>,
}

pub const NET: &str = "net";
pub const AUTO_UIA2: &str = "auto-uia2";
pub const AUTO_WDA: &str = "auto-wda";

pub const SETTINGS: [Setting; 3] = [
    Setting {
        key: AUTO_UIA2,
        label: "Tự cài UiAutomator2 lên điện thoại",
        description: "Cài APK chính thức của Appium (đã ghim SHA-256) khi gặp điện thoại chưa có: đọc màn hình nhanh hơn và gõ được tiếng Việt.",
        default: true,
        warning: None,
    },
    Setting {
        key: AUTO_WDA,
        label: "Tự cài WebDriverAgent lên iPhone",
        description: "Ký lại WebDriverAgent chính thức của Appium (đã ghim SHA-256) bằng chứng chỉ Apple Development của anh và cài lên iPhone có trong provisioning profile. Chỉ nghe trên loopback của iPhone, tự tắt khi không dùng.",
        default: true,
        warning: None,
    },
    Setting {
        key: NET,
        label: "Cho agent bắt lưu lượng mạng (net)",
        description: "Agent được đặt proxy HTTP của điện thoại qua mitmproxy trên máy này, hoặc cài PCAPdroid để ghi kết nối của một app.",
        default: false,
        warning: Some(
            "Agent sẽ thấy địa chỉ, header và nội dung request của các app trên điện thoại (header bí mật được che), và có thể cài PCAPdroid. Chỉ bật trên máy test.",
        ),
    },
];

fn stored() -> Value {
    std::env::var_os("BERTH_PLUGIN_SETTINGS")
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("values").cloned())
        .unwrap_or(Value::Null)
}

/// The value of a switch: the stored one, else its default.
pub fn get(key: &str) -> bool {
    let def = SETTINGS
        .iter()
        .find(|s| s.key == key)
        .map(|s| s.default)
        .unwrap_or(false);
    stored().get(key).and_then(Value::as_bool).unwrap_or(def)
}

/// `settings --json`: what Shipyard shows.
pub fn list_json() -> Value {
    let list: Vec<Value> = SETTINGS
        .iter()
        .map(|s| {
            let mut v = json!({"key": s.key, "label": s.label, "description": s.description, "value": get(s.key)});
            if let Some(w) = s.warning {
                v["warning"] = json!(w);
            }
            v
        })
        .collect();
    json!({ "settings": list })
}

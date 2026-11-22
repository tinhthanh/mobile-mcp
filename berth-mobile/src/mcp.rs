//! MCP over stdio: newline-delimited JSON-RPC 2.0. Only what a tools-only
//! server needs: initialize, ping, tools/list and tools/call.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::tools::{self, Shared};
use crate::util::log;

const PROTOCOL_VERSIONS: [&str; 5] = [
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
    "2024-10-07",
];

fn send(out: &mut impl Write, msg: &Value) {
    let mut line = serde_json::to_vec(msg).unwrap_or_default();
    line.push(b'\n');
    let _ = out.write_all(&line);
    let _ = out.flush();
}

fn handle(state: &Shared, method: &str, params: &Value) -> Result<Value, (i64, String)> {
    match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = if PROTOCOL_VERSIONS.contains(&asked) {
                asked
            } else {
                PROTOCOL_VERSIONS[0]
            };
            Ok(json!({
                "protocolVersion": version,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "berth-mobile", "version": env!("CARGO_PKG_VERSION")},
                "instructions": tools::instructions(),
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({"tools": tools::definitions()})),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            Ok(tools::call(state, name, &args))
        }
        _ => Err((-32601, format!("Method not found: {method}"))),
    }
}

/// Serves until stdin closes.
pub fn serve(state: Shared) {
    log(&format!(
        "berth-mobile {} serving MCP on stdio",
        env!("CARGO_PKG_VERSION")
    ));
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                send(
                    &mut out,
                    &json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": format!("Parse error: {e}")}}),
                );
                continue;
            }
        };
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            continue;
        };
        // notifications (no id) need no answer
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let reply = match handle(&state, method, &params) {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        };
        send(&mut out, &reply);
    }
}

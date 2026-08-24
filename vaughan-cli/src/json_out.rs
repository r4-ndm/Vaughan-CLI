//! JSON envelope helpers for scriptable CLI output.

use serde_json::Value;

pub fn print_json_value(json_mode: bool, value: &Value, human: impl FnOnce()) {
    if json_mode {
        let envelope = serde_json::json!({ "ok": true, "data": value });
        println!(
            "{}",
            serde_json::to_string_pretty(&envelope).unwrap_or_default()
        );
    } else {
        human();
    }
}

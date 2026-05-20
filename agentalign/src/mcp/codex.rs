//! Codex CLI MCP format strategy (TOML).
//!
//! Uses `toml` for comment-preserving TOML parsing.
//!
//! Format:
//! ```toml
//! [mcp_servers.server_name]
//! command = "npx"
//! args = ["package"]
//!
//! [mcp_servers.server_name.env]
//! KEY = "value"
//! ```

use agentalign_shared::error::{AdapterError, Result};
use agentalign_shared::models::{
    CanonicalWorkspaceState, ClientCapabilities, PlaceholderStyle,
};
use agentalign_shared::traits::{ConfigurationAdapter, McpFormatStrategy};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::path::Path;

pub struct CodexStrategy;

impl ConfigurationAdapter for CodexStrategy {
    fn target_name(&self) -> &'static str {
        "codex"
    }

    fn deserialize_to_canonical(&self, raw: &str) -> Result<JsonValue> {
        // Parse TOML using tomli for reading
        let parsed: toml_edit::DocumentMut = raw
            .parse()
            .map_err(|e| AdapterError::TomlParse(e.to_string()))?;

        let mut canonical_servers = serde_json::Map::new();

        // Walk all tables matching mcp_servers.<name>
        for (key, table) in parsed.iter() {
            if key != "mcp_servers" {
                continue;
            }
            let tbl = table
                .as_table()
                .ok_or_else(|| AdapterError::Other("mcp_servers is not a table".into()))?;

            for (name, server_tbl) in tbl.iter() {
                let server = server_tbl
                    .as_table()
                    .ok_or_else(|| {
                        AdapterError::Other(format!(
                            "Server '{}' is not a table",
                            name
                        ))
                    })?;

                let mut entry = serde_json::Map::new();
                entry.insert("type".into(), json!("local"));

                let mut cmd_vec: Vec<String> = Vec::new();
                if let Some(cmd) = server.get("command").and_then(|v| v.as_str()) {
                    cmd_vec.push(cmd.to_string());
                }
                if let Some(args) = server.get("args").and_then(|v| v.as_array()) {
                    for arg in args {
                        if let Some(s) = arg.as_str() {
                            cmd_vec.push(s.to_string());
                        }
                    }
                }
                if !cmd_vec.is_empty() {
                    entry.insert("command".into(), json!(cmd_vec));
                }

                // Extract env sub-table
                if let Some(env_tbl) = server.get("env").and_then(|v| v.as_table()) {
                    let mut env_map = serde_json::Map::new();
                    for (env_k, env_v) in env_tbl.iter() {
                        if let Some(s) = env_v.as_str() {
                            env_map.insert(env_k.to_string(), json!(s));
                        }
                    }
                    if !env_map.is_empty() {
                        entry.insert("env".into(), JsonValue::Object(env_map));
                    }
                }

                // Collect extras
                let known = ["command", "args", "env", "type"];
                for (k, v) in server.iter() {
                    if !known.contains(&k) {
                        if let Some(s) = v.as_str() {
                            entry.insert(k.to_string(), json!(s));
                        } else if let Some(i) = v.as_integer() {
                            entry.insert(k.to_string(), json!(i));
                        } else if let Some(b) = v.as_bool() {
                            entry.insert(k.to_string(), json!(b));
                        } else if let Some(arr) = v.as_array() {
                            let jarr: Vec<JsonValue> = arr
                                .iter()
                                .map(|x| {
                                    if let Some(s) = x.as_str() {
                                        json!(s)
                                    } else if let Some(i) = x.as_integer() {
                                        json!(i)
                                    } else {
                                        json!(null)
                                    }
                                })
                                .collect();
                            entry.insert(k.to_string(), JsonValue::Array(jarr));
                        }
                    }
                }

                canonical_servers.insert(name.to_string(), JsonValue::Object(entry));
            }
        }

        let mut root = serde_json::Map::new();
        root.insert("mcp".into(), JsonValue::Object(canonical_servers));
        Ok(JsonValue::Object(root))
    }

    fn serialize_from_canonical(&self, canonical: &JsonValue) -> Result<String> {
        let mcp = canonical
            .get("mcp")
            .and_then(|v| v.as_object())
            .ok_or_else(|| AdapterError::Other("Missing 'mcp' key in canonical".into()))?;

        let mut doc = toml_edit::DocumentMut::new();

        for (name, entry) in mcp {
            let entry_obj = entry.as_object().ok_or_else(|| {
                AdapterError::Other(format!("Server '{}' is not an object", name))
            })?;

            let transport = entry_obj
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("local");

            // Create table: mcp_servers.<name>
            let table_path = format!("mcp_servers.{}", name);
            let table = doc
                .entry("mcp_servers")
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_of_tables()
                .ok_or_else(|| {
                    AdapterError::Other("Failed to create mcp_servers table".into())
                })?;

            let mut server_table = toml_edit::InlineTable::new();
            server_table.set_implicit(true);

            if transport == "remote" {
                if let Some(url) = entry_obj.get("url").and_then(|v| v.as_str()) {
                    server_table.insert("url", url.into());
                }
            } else if let Some(cmd_arr) = entry_obj.get("command").and_then(|v| v.as_array()) {
                if let Some(first) = cmd_arr.first().and_then(|v| v.as_str()) {
                    server_table.insert("command", first.into());
                    if cmd_arr.len() > 1 {
                        let rest: Vec<String> = cmd_arr[1..]
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        let args_toml: Vec<toml_edit::Value> = rest
                            .iter()
                            .map(|s| s.to_string().into())
                            .collect();
                        server_table.insert(
                            "args",
                            toml_edit::Value::Array(
                                args_toml.into_iter().collect::<toml_edit::Array>(),
                            ),
                        );
                    }
                }
            }

            // Add env sub-table
            if let Some(env) = entry_obj.get("env").and_then(|v| v.as_object()) {
                let mut env_table = toml_edit::InlineTable::new();
                for (ek, ev) in env {
                    if let Some(s) = ev.as_str() {
                        env_table.insert(ek, s.into());
                    }
                }
                // We need to insert env as a sub-table, not inline
                // Use table-like syntax via dotted keys
                let env_path = format!("{}.env", name);
                let env_tbl = doc
                    .entry("mcp_servers")
                    .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                    .as_table_of_tables()
                    .ok_or_else(|| {
                        AdapterError::Other("Failed to access mcp_servers".into())
                    })?;

                // Insert env values as dotted keys under mcp_servers.<name>.env.<key>
                for (ek, ev) in env {
                    let full_path = format!("mcp_servers.{}.env.{}", name, ek);
                    let keys: Vec<&str> = full_path.split('.').collect();
                    let mut current: &mut toml_edit::Table = doc.as_table_mut();
                    for &key in &keys[..keys.len() - 1] {
                        current = current
                            .entry(key)
                            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                            .as_table_mut()
                            .ok_or_else(|| {
                                AdapterError::Other(format!(
                                    "Failed to create table for {}",
                                    key
                                ))
                            })?;
                    }
                    let last_key = keys[keys.len() - 1];
                    if let Some(s) = ev.as_str() {
                        current[last_key] = toml_edit::value(s);
                    }
                }
            }

            // Add extras
            let known = ["type", "command", "url", "headers", "env", "enabled"];
            for (k, v) in entry_obj {
                if known.contains(&k.as_str()) {
                    continue;
                }
                if let Some(s) = v.as_str() {
                    server_table.insert(k, s.into());
                } else if let Some(b) = v.as_bool() {
                    server_table.insert(k, b.into());
                } else if let Some(i) = v.as_i64() {
                    server_table.insert(k, i.into());
                }
            }

            // Set the server table as a value entry
            // For implicit tables, we need to set each key separately
            for (k, v) in server_table.iter() {
                let full_key = format!("mcp_servers.{}.{}", name, k.0.get());
                let keys: Vec<&str> = full_key.split('.').collect();
                let mut current: &mut toml_edit::Table = doc.as_table_mut();
                for &key in &keys[..keys.len() - 1] {
                    current = current
                        .entry(key)
                        .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                        .as_table_mut()
                        .ok_or_else(|| {
                            AdapterError::Other(format!("Failed to create table for {}", key))
                        })?;
                }
                let last_key = keys[keys.len() - 1];
                current[last_key] = toml_edit::item(v.clone());
            }
        }

        Ok(doc.to_string())
    }

    fn target_config_path(&self, home: &Path) -> std::path::PathBuf {
        home.join(".codex").join("config.toml")
    }

    fn normalize_env(&self, env: &HashMap<String, String>) -> HashMap<String, String> {
        // Codex uses ${VAR} syntax (same as canonical)
        env.clone()
    }

    fn extract_unknowns(&self, raw: &JsonValue) -> HashMap<String, JsonValue> {
        // TOML extras are handled differently — during parse we
        // already capture them in the extra fields per server.
        HashMap::new()
    }
}

impl McpFormatStrategy for CodexStrategy {
    fn validate(&self, state: &CanonicalWorkspaceState) -> Result<()> {
        // Codex server IDs must be valid TOML table keys
        for name in state.mcp.keys() {
            if name.is_empty() {
                return Err(AdapterError::Validation(
                    "Server ID cannot be empty".into(),
                ));
            }
            // TOML keys cannot contain brackets or periods for table names
            if name.contains('[') || name.contains(']') || name.contains('.') {
                return Err(AdapterError::Validation(format!(
                    "Server ID '{}' is not a valid TOML table key",
                    name
                )));
            }
        }
        Ok(())
    }

    fn capabilities(&self) -> ClientCapabilities {
        crate::mcp::capabilities::codex_capabilities()
    }
}

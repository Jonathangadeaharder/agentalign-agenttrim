use crate::error::Result;
use crate::models::CanonicalWorkspaceState;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

/// Configuration adapter: translates between canonical and agent-native formats.
pub trait ConfigurationAdapter {
    /// Human-readable target name (e.g., "claude", "cursor", "codex")
    fn target_name(&self) -> &'static str;

    /// Parse agent-native config string into canonical JSON value.
    fn deserialize_to_canonical(&self, raw: &str) -> Result<JsonValue>;

    /// Serialize canonical JSON value into agent-native config string.
    fn serialize_from_canonical(&self, canonical: &JsonValue) -> Result<String>;

    /// Resolve the target config path for this agent.
    fn target_config_path(&self, home: &Path) -> std::path::PathBuf;

    /// Normalize environment variable syntax to target dialect.
    fn normalize_env(&self, env: &HashMap<String, String>) -> HashMap<String, String>;

    /// Extract unknown fields from raw JSON for round-trip preservation.
    fn extract_unknowns(&self, raw: &JsonValue) -> HashMap<String, JsonValue>;
}

/// Strategy for multi-client MCP format translation.
pub trait McpFormatStrategy: ConfigurationAdapter {
    /// Validate that a canonical config is compatible with this agent.
    fn validate(&self, state: &CanonicalWorkspaceState) -> Result<()>;

    /// Get the client's transport capabilities.
    fn capabilities(&self) -> crate::models::ClientCapabilities;

    /// Generate a stdio bridge command if native transport isn't supported.
    fn stdio_bridge_command(&self, _url: &str) -> Option<Vec<String>> {
        None
    }
}

/// Trim analyzer: evaluates resource usage and identifies candidates for pruning.
pub trait TrimAnalyzer {
    /// Analyze usage and return a report of unused/abandoned resources.
    fn analyze_usage(&self, agents_root: &Path) -> Result<Vec<crate::models::UnusedReport>>;

    /// Name of this analyzer (e.g., "skills", "mcp", "processes").
    fn analyzer_name(&self) -> &'static str;
}

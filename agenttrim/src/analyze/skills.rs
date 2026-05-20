use anyhow::Result;
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;
use walkdir::WalkDir;

use crate::analyze::ledger_reader;
use crate::analyze::safety_matrix::SafetyMatrix;
use crate::analyze::static_scanner;
use agentalign_shared::models::{ReportKind, UnusedReport};

/// Analyzes skill usage by cross-referencing installed skills
/// against the usage ledger and safety matrix.
pub struct SkillAnalyzer;

impl SkillAnalyzer {
    /// Scan the agents skills directory and cross-reference with usage ledger.
    ///
    /// `agents_root` — path to `~/.agents/` or equivalent.
    /// `threshold_days` — number of days of inactivity before a skill is
    /// considered unused (default 90).
    /// `projects_root` — optional path for static text reference scanning.
    pub fn analyze(
        agents_root: &Path,
        threshold_days: u64,
        projects_root: Option<&Path>,
    ) -> Result<Vec<UnusedReport>> {
        let skills_dir = agents_root.join("skills");
        if !skills_dir.exists() || !skills_dir.is_dir() {
            return Ok(Vec::new());
        }

        // Load usage data from both SQLite ledger and JSON ledger
        let sqlite_usage = load_sqlite_usage().unwrap_or_default();
        let json_usage = ledger_reader::read_skill_usage().unwrap_or_default();

        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let threshold_secs = (threshold_days as i64) * 86_400;
        let cutoff = now_secs - threshold_secs;

        let mut reports = Vec::new();

        // Iterate over subdirectories in skills dir
        for entry in WalkDir::new(&skills_dir).min_depth(1).max_depth(1) {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().is_dir() && !entry.file_type().is_symlink() {
                continue;
            }

            let skill_name = entry
                .file_name()
                .to_str()
                .unwrap_or("")
                .to_string();

            if skill_name.is_empty() {
                continue;
            }

            let skill_path = entry.path().to_path_buf();

            // Check safety matrix first
            if SafetyMatrix::is_protected(&skill_name) {
                let reason = SafetyMatrix::protection_reason(&skill_name)
                    .unwrap_or("Protected by safety matrix");
                reports.push(UnusedReport {
                    key_identifier: skill_name.clone(),
                    kind: ReportKind::Skill,
                    path_context: Some(skill_path.to_string_lossy().to_string()),
                    last_active_timestamp: None,
                    safe_to_purge: false,
                    reason: format!("Protected: {reason}"),
                });
                continue;
            }

            // Check usage data
            let usage_info = Self::get_usage_info(&skill_name, &sqlite_usage, &json_usage);

            let last_used = usage_info.last_used;
            let _total_calls = usage_info.total_calls;
            let is_unused = last_used.map(|t| t < cutoff).unwrap_or(true);

            // Check file existence
            let file_exists = Self::skill_file_exists(&skill_path);

            // Static text scan in projects (best-effort)
            let project_refs = match projects_root {
                Some(proot) => static_scanner::scan_for_references(
                    &skill_name,
                    &[proot],
                    &["md", "rs", "py", "ts", "js", "json", "toml", "yaml", "yml"],
                )
                .unwrap_or_default(),
                None => Vec::new(),
            };
            let has_project_refs = !project_refs.is_empty();

            // Determine safe_to_purge
            let safe_to_purge = is_unused && !file_exists && !has_project_refs;

            let reason = if !file_exists && is_unused {
                "Skill directory exists but SKILL.md/binary not found; no recent usage"
            } else if is_unused && has_project_refs {
                "Skill appears unused in ledger but has project references"
            } else if is_unused {
                "No usage in telemetry within threshold period"
            } else {
                "Still actively used"
            };

            reports.push(UnusedReport {
                key_identifier: skill_name,
                kind: ReportKind::Skill,
                path_context: Some(skill_path.to_string_lossy().to_string()),
                last_active_timestamp: last_used,
                safe_to_purge,
                reason: reason.to_string(),
            });
        }

        Ok(reports)
    }

    /// Check if a skill is referenced in any project files (static grep).
    #[allow(dead_code)]
    pub fn check_project_references(
        skill_name: &str,
        projects_root: &Path,
    ) -> Result<bool> {
        let results = static_scanner::scan_for_references(
            skill_name,
            &[projects_root],
            &["md", "rs", "py", "ts", "js", "json", "toml", "yaml", "yml"],
        )?;
        Ok(!results.is_empty())
    }

    /// Check if the skill directory contains SKILL.md or binary files.
    fn skill_file_exists(skill_path: &Path) -> bool {
        if !skill_path.exists() {
            return false;
        }
        // Resolve symlinks
        let target = if skill_path.is_symlink() {
            std::fs::read_link(skill_path).unwrap_or_else(|_| skill_path.to_path_buf())
        } else {
            skill_path.to_path_buf()
        };

        if !target.exists() {
            return false; // Broken symlink
        }

        // Check for common skill indicator files
        let indicators = ["SKILL.md", "skill.md", "main.py", "index.js", "index.ts"];
        indicators.iter().any(|name| target.join(name).exists())
    }

    /// Cross-reference a skill name against both SQLite and JSON usage ledgers.
    fn get_usage_info(
        skill_name: &str,
        sqlite_usage: &[agentalign_shared::models::UsageEntry],
        json_usage: &std::collections::HashMap<
            String,
            ledger_reader::SkillUsageEntry,
        >,
    ) -> UsageInfo {
        let mut last_used: Option<i64> = None;
        let mut total_calls: u64 = 0;

        // Check SQLite ledger
        for entry in sqlite_usage {
            if entry.server_id == skill_name {
                if last_used
                    .map(|lu| entry.last_used_timestamp > lu)
                    .unwrap_or(true)
                {
                    last_used = Some(entry.last_used_timestamp);
                }
                total_calls = total_calls.saturating_add(entry.total_call_count);
            }
        }

        // Check JSON ledger (overrides SQLite if newer)
        if let Some(json_entry) = json_usage.get(skill_name) {
            if let Some(ref last_str) = json_entry.last_used {
                // Try to parse the JSON timestamp (could be ISO 8601 or epoch)
                if let Ok(ts) = last_str.parse::<i64>() {
                    if last_used.map(|lu| ts > lu).unwrap_or(true) {
                        last_used = Some(ts);
                    }
                }
            }
            total_calls = total_calls.saturating_add(json_entry.times_used);
        }

        UsageInfo {
            last_used,
            total_calls,
        }
    }
}

/// Internal usage summary for a single skill.
struct UsageInfo {
    last_used: Option<i64>,
    total_calls: u64,
}

/// Load SQLite usage stats, returning empty vec on failure.
fn load_sqlite_usage() -> Result<Vec<agentalign_shared::models::UsageEntry>> {
    ledger_reader::get_usage_stats().map_err(|e| anyhow::anyhow!("Failed to load SQLite usage: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // Helper to create a minimal skill dir.
    fn create_skill_dir(base: &Path, name: &str) -> PathBuf {
        let dir = base.join("skills").join(name);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_analyze_no_skills_dir() {
        let tmp = std::env::temp_dir().join("agenttrim_test_no_skills");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        // No skills/ subdir
        let reports = SkillAnalyzer::analyze(&tmp, 90, None).unwrap();
        assert!(reports.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_skill_file_exists_with_skil_md() {
        let tmp = std::env::temp_dir().join("agenttrim_test_skill_exists");
        let _ = fs::remove_dir_all(&tmp);
        let dir = create_skill_dir(&tmp, "my-skill");
        fs::write(dir.join("SKILL.md"), "# My Skill\n").unwrap();

        assert!(SkillAnalyzer::skill_file_exists(&dir));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_skill_file_exists_empty_dir() {
        let tmp = std::env::temp_dir().join("agenttrim_test_empty_skill");
        let _ = fs::remove_dir_all(&tmp);
        let dir = create_skill_dir(&tmp, "empty-skill");
        // No indicator files
        assert!(!SkillAnalyzer::skill_file_exists(&dir));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_skill_file_exists_broken_symlink() {
        let tmp = std::env::temp_dir().join("agenttrim_test_broken_link");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let skills_dir = tmp.join("skills");
        fs::create_dir_all(&skills_dir).unwrap();
        let link = skills_dir.join("broken-skill");
        // Create a symlink to a non-existent path
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/nonexistent/target", &link).unwrap();
        }
        #[cfg(windows)]
        {
            // Skip symlink test on Windows
            return;
        }

        assert!(!SkillAnalyzer::skill_file_exists(&link));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_check_project_references() {
        let tmp = std::env::temp_dir().join("agenttrim_test_proj_refs");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("config.md"), "Uses agent-browser for testing\n").unwrap();
        fs::write(tmp.join("main.rs"), "use agent_browser;\nfn main() {}\n").unwrap();

        let found = SkillAnalyzer::check_project_references("agent-browser", &tmp).unwrap();
        assert!(found);

        let found = SkillAnalyzer::check_project_references("nonexistent-skill", &tmp).unwrap();
        assert!(!found);

        let _ = fs::remove_dir_all(&tmp);
    }
}

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::analyze::safety_matrix::SafetyMatrix;
use agentalign_shared::models::UnusedReport;

/// Severity level of a validation issue.
#[derive(Debug, Clone, PartialEq)]
pub enum IssueSeverity {
    /// Must be resolved before pruning can proceed.
    Block,
    /// Should be reviewed but does not block pruning.
    Warn,
}

/// A single validation issue found during pre-purge checks.
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    pub item: String,
    pub severity: IssueSeverity,
    pub message: String,
}

/// Pre-purge integrity check engine.
///
/// Runs safety gates and validation checks before allowing
/// any pruning operation to proceed.
pub struct PrePurgeValidation;

impl PrePurgeValidation {
    /// Run all pre-purge checks against a list of prune candidates.
    ///
    /// Returns a list of validation issues. Items with `Block` severity
    /// must be resolved before pruning.
    pub fn validate(reports: &[UnusedReport]) -> Result<Vec<ValidationIssue>> {
        let mut issues = Vec::new();

        for report in reports {
            if !report.safe_to_purge {
                continue; // Skip non-candidates
            }

            // Check 1: Is the item in the safety matrix?
            if SafetyMatrix::is_protected(&report.key_identifier) {
                let reason = SafetyMatrix::protection_reason(&report.key_identifier)
                    .unwrap_or("Protected by safety matrix");
                issues.push(ValidationIssue {
                    item: report.key_identifier.clone(),
                    severity: IssueSeverity::Block,
                    message: format!("Item is protected: {reason}"),
                });
            }

            // Check 2: Is the item referenced in any active config?
            // (This is a soft check — we verify by scanning active config dirs)
            if let Some(ref ctx) = report.path_context {
                let config_path = Path::new(ctx);
                if config_path.exists() {
                    // The path itself exists — this could mean the item is still active
                    issues.push(ValidationIssue {
                        item: report.key_identifier.clone(),
                        severity: IssueSeverity::Warn,
                        message: format!(
                            "Path still exists at {}; verify it's not actively referenced",
                            ctx
                        ),
                    });
                }
            }

            // Check 3: Is backup path writable?
            let backup_dir = Self::backup_dir();
            match Self::check_backup_writable(&backup_dir) {
                Ok(_) => {}
                Err(e) => {
                    issues.push(ValidationIssue {
                        item: report.key_identifier.clone(),
                        severity: IssueSeverity::Warn,
                        message: format!("Backup path may not be writable: {e}"),
                    });
                }
            }

            // Check 4: Does the item have recent file mtime?
            if let Some(ref ctx) = report.path_context {
                let item_path = Path::new(ctx);
                if item_path.exists() {
                    if let Ok(metadata) = item_path.metadata() {
                        if let Ok(modified) = metadata.modified() {
                            if let Ok(elapsed) = modified.elapsed() {
                                if elapsed.as_secs() < 7 * 86_400 {
                                    issues.push(ValidationIssue {
                                        item: report.key_identifier.clone(),
                                        severity: IssueSeverity::Warn,
                                        message: format!(
                                            "File modified less than 7 days ago at {}",
                                            ctx
                                        ),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(issues)
    }

    /// Pre-purge validation: remove protected items and filter out
    /// candidates with active file references.
    ///
    /// Returns only candidates that pass all validation checks.
    /// Protected items are excluded. Items with recently modified
    /// path contexts are excluded. Default inactive threshold: 90 days.
    pub fn validate_candidates(reports: &[UnusedReport]) -> Result<Vec<UnusedReport>> {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let threshold_secs: i64 = 90 * 86_400; // 90 days
        let cutoff = now - threshold_secs;

        let validated: Vec<UnusedReport> = reports
            .iter()
            .filter(|r| {
                // Must be a candidate
                if !r.safe_to_purge {
                    return false;
                }

                // Check 1: Not protected by safety matrix
                if SafetyMatrix::is_protected(&r.key_identifier) {
                    return false;
                }

                // Check 2: No recent file modifications at path_context
                if let Some(ref ctx) = r.path_context {
                    let item_path = Path::new(ctx);
                    if item_path.exists() {
                        if let Ok(metadata) = item_path.metadata() {
                            if let Ok(modified) = metadata.modified() {
                                if let Ok(elapsed) = modified.elapsed() {
                                    if elapsed.as_secs() < 7 * 86_400 {
                                        // Modified within last 7 days — keep
                                        return false;
                                    }
                                }
                            }
                        }
                    }
                }

                // Check 3: Last active timestamp is past threshold
                match r.last_active_timestamp {
                    Some(ts) => ts < cutoff,
                    None => true, // No recorded usage — safe to purge
                }
            })
            .cloned()
            .collect();

        Ok(validated)
    }

    /// Final safety gate before executing a prune on a single item.
    ///
    /// Returns `Ok(())` if the item is safe to prune, or an error
    /// describing why it cannot be pruned.
    #[allow(dead_code)]
    pub fn safety_gate(report: &UnusedReport) -> Result<()> {
        // Block if protected by safety matrix
        if SafetyMatrix::is_protected(&report.key_identifier) {
            anyhow::bail!(
                "Safety gate: {} is protected and cannot be pruned",
                report.key_identifier
            );
        }

        // Block if the report says it's not safe
        if !report.safe_to_purge {
            anyhow::bail!(
                "Safety gate: {} is not marked as safe to purge. Reason: {}",
                report.key_identifier,
                report.reason
            );
        }

        // Block if the path still exists and is very recently modified
        if let Some(ref ctx) = report.path_context {
            let item_path = Path::new(ctx);
            if item_path.exists() {
                if let Ok(metadata) = item_path.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(elapsed) = modified.elapsed() {
                            if elapsed.as_secs() < 7 * 86_400 {
                                anyhow::bail!(
                                    "Safety gate: {} was modified less than 7 days ago at {}",
                                    report.key_identifier,
                                    ctx
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Get the default backup directory path.
    fn backup_dir() -> PathBuf {
        dirs::home_dir()
            .map(|h| h.join(".agents").join("backups"))
            .unwrap_or_else(|| PathBuf::from("/tmp/agenttrim-backups"))
    }

    /// Check that the backup directory exists or can be created.
    fn check_backup_writable(path: &Path) -> Result<()> {
        if path.exists() {
            // Verify it's a directory AND writable
            if !path.is_dir() {
                anyhow::bail!("Backup path exists but is not a directory: {:?}", path);
            }
            // Try creating a test file
            let test_file = path.join(".agenttrim_write_test");
            std::fs::write(&test_file, b"test")?;
            std::fs::remove_file(&test_file)?;
        } else {
            // Try creating the directory
            std::fs::create_dir_all(path)
                .with_context(|| format!("Cannot create backup directory: {:?}", path))?;
            // Clean up if we just created it empty
            let _ = std::fs::remove_dir(path);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentalign_shared::models::ReportKind;
    use std::fs;

    fn make_candidate(id: &str, path: Option<&str>, safe: bool) -> UnusedReport {
        UnusedReport {
            key_identifier: id.to_string(),
            kind: ReportKind::Skill,
            path_context: path.map(|s| s.to_string()),
            last_active_timestamp: Some(100_000), // old timestamp
            safe_to_purge: safe,
            reason: "Test candidate".to_string(),
        }
    }

    #[test]
    fn test_safety_gate_protected() {
        let report = make_candidate("agent-browser", Some("/tmp/test"), true);
        let result = PrePurgeValidation::safety_gate(&report);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("protected"));
    }

    #[test]
    fn test_safety_gate_not_safe() {
        let report = make_candidate("some-skill", Some("/tmp/test"), false);
        let result = PrePurgeValidation::safety_gate(&report);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not marked as safe"));
    }

    #[test]
    fn test_safety_gate_skipped_mtime_without_path_context() {
        let report = make_candidate("clean-skill", None, true);
        let result = PrePurgeValidation::safety_gate(&report);
        assert!(result.is_ok());
    }

    #[test]
    fn test_safety_gate_rejects_recently_modified() {
        let tmp = std::env::temp_dir().join("agenttrim_test_safety_recent");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let fresh_file = tmp.join("fresh_skill.md");
        fs::write(&fresh_file, b"fresh content").unwrap();

        let report = make_candidate("fresh-skill", Some(tmp.to_str().unwrap()), true);
        let result = PrePurgeValidation::safety_gate(&report);
        // Freshly created file is < 7 days old → should be blocked
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("less than 7 days"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_validate_no_candidates() {
        let reports = vec![make_candidate("skill-a", None, false)];
        let issues = PrePurgeValidation::validate(&reports).unwrap();
        // Non-candidates are skipped
        assert!(issues.is_empty());
    }

    #[test]
    fn test_validate_protected_candidate() {
        let report = make_candidate("supabase", None, true);
        let issues = PrePurgeValidation::validate(&[report]).unwrap();
        // Should have at least one Block issue
        assert!(issues.iter().any(|i| i.severity == IssueSeverity::Block));
    }
}

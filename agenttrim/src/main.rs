mod analyze;
mod prune;

use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;

use agentalign_shared::models::McpServerDefinition;

use crate::analyze::validation_hook::{IssueSeverity, PrePurgeValidation};

#[derive(Parser)]
#[command(name = "agenttrim", about = "Telemetry-Driven Pruning & Vacuum Engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan and report unused resources
    Analyze {
        /// Path to ~/.agents/ (default: home dir)
        #[arg(long)]
        agents_root: Option<PathBuf>,

        /// Path to canonical MCP config (default: ~/.agents/mcp_config.json)
        #[arg(long)]
        mcp_config: Option<PathBuf>,

        /// Optional projects root for static reference scanning
        #[arg(long)]
        projects_root: Option<PathBuf>,

        /// Inactivity threshold in days (default: 90)
        #[arg(long, default_value_t = 90)]
        threshold_days: u64,
    },
    /// Remove unused resources (with safety gates)
    Prune {
        /// Path to ~/.agents/ (default: home dir)
        #[arg(long)]
        agents_root: Option<PathBuf>,

        /// Path to canonical MCP config (default: ~/.agents/mcp_config.json)
        #[arg(long)]
        mcp_config: Option<PathBuf>,

        /// Non-interactive mode (still passes safety gates)
        #[arg(long)]
        force: bool,

        /// Show what would be pruned without deleting
        #[arg(long)]
        dry_run: bool,
    },
    /// Deep clean: zombie processes, orphaned caches
    Vacuum {
        /// Show what would be killed without actually killing
        #[arg(long)]
        dry_run: bool,
    },
    /// Configure thresholds and allowlists
    Config,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Analyze {
            agents_root,
            mcp_config,
            projects_root,
            threshold_days,
        } => run_analyze(agents_root, mcp_config, projects_root, threshold_days),
        Commands::Prune {
            agents_root,
            mcp_config,
            force,
            dry_run,
        } => run_prune(agents_root, mcp_config, force, dry_run),
        Commands::Vacuum { dry_run } => {
            run_vacuum(dry_run);
        }
        Commands::Config => {
            println!("⚙️  config — configure thresholds and allowlists");
            // TODO: implement config subcommand
        }
    }
}

fn resolve_agents_root(custom: Option<PathBuf>) -> PathBuf {
    custom.unwrap_or_else(|| {
        dirs::home_dir()
            .expect("Cannot determine home directory")
            .join(".agents")
    })
}

fn resolve_mcp_config(custom: Option<PathBuf>) -> PathBuf {
    custom.unwrap_or_else(|| {
        dirs::home_dir()
            .expect("Cannot determine home directory")
            .join(".agents")
            .join("mcp_config.json")
    })
}

fn run_analyze(
    agents_root_opt: Option<PathBuf>,
    mcp_config_opt: Option<PathBuf>,
    projects_root_opt: Option<PathBuf>,
    threshold_days: u64,
) {
    let agents_root = resolve_agents_root(agents_root_opt);
    let mcp_config = resolve_mcp_config(mcp_config_opt);
    let projects_root = projects_root_opt.as_deref();

    println!("🔍 analyze — scanning for unused skills, MCP servers, and processes...");
    println!("  agents root:  {}", agents_root.display());
    println!("  mcp config:   {}", mcp_config.display());
    if let Some(pr) = projects_root {
        println!("  projects:     {}", pr.display());
    }
    println!("  threshold:    {threshold_days} days");

    match crate::analyze::run_full_analysis(
        &agents_root,
        &mcp_config,
        projects_root,
        threshold_days,
    ) {
        Ok(report) => {
            println!();
            println!("=== Analysis Report ===");
            println!();
            println!("Skills:");
            for skill in &report.skills {
                let status = if skill.safe_to_purge {
                    "CANDIDATE"
                } else {
                    "KEEP"
                };
                println!(
                    "  [{status}] {} — {}",
                    skill.key_identifier, skill.reason
                );
                if let Some(ref path) = skill.path_context {
                    println!("         path: {path}");
                }
            }

            println!();
            println!("MCP Servers:");
            for server in &report.mcp_servers {
                let status = if server.safe_to_purge {
                    "CANDIDATE"
                } else {
                    "KEEP"
                };
                println!(
                    "  [{status}] {} — {}",
                    server.key_identifier, server.reason
                );
            }

            println!();
            println!("Running Processes:");
            for proc in &report.processes {
                let orphan = if proc.is_orphan { " (orphan)" } else { "" };
                println!("  PID {} — {}{orphan}", proc.pid, proc.command);
                if let Some(ref matched) = proc.matched_server {
                    println!("         matched: {matched}");
                }
            }

            println!();
            println!("=== Summary ===");
            println!("  Total candidates:        {}", report.total_candidates);
            println!("  Protected/ignored:       {}", report.protected_ignored);
            println!("  Skills analyzed:         {}", report.skills.len());
            println!("  MCP servers analyzed:   {}", report.mcp_servers.len());
            println!("  Running processes found: {}", report.processes.len());
        }
        Err(e) => {
            eprintln!("  ✗ Analysis failed: {e}");
        }
    }
}

fn run_prune(
    agents_root_opt: Option<PathBuf>,
    mcp_config_opt: Option<PathBuf>,
    force: bool,
    dry_run: bool,
) {
    let agents_root = resolve_agents_root(agents_root_opt);
    let mcp_config = resolve_mcp_config(mcp_config_opt);

    println!("🧹 prune — cleaning unused resources (safety gates enabled)");
    if dry_run {
        println!("  → DRY RUN: no changes will be made");
    }
    if force {
        println!("  → FORCE: non-interactive mode");
    }

    // Run analysis first
    let projects_root = None;
    let threshold_days: u64 = 90;

    let report = match crate::analyze::run_full_analysis(
        &agents_root,
        &mcp_config,
        projects_root,
        threshold_days,
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("  ✗ Analysis failed: {e}");
            return;
        }
    };

    if report.total_candidates == 0 {
        println!("  ✓ No candidates found for pruning.");
        return;
    }

    println!();
    println!("=== Prune Candidates ===");

    // Collect all candidates
    let all_reports: Vec<_> = report
        .skills
        .iter()
        .chain(report.mcp_servers.iter())
        .filter(|r| r.safe_to_purge)
        .collect();

    for r in &all_reports {
        let kind = match r.kind {
            agentalign_shared::models::ReportKind::Skill => "skill",
            agentalign_shared::models::ReportKind::McpServer => "mcp",
            agentalign_shared::models::ReportKind::OrphanedProcess => "process",
            agentalign_shared::models::ReportKind::StaleBackup => "backup",
        };
        println!(
            "  [{kind}] {} — {}",
            r.key_identifier, r.reason
        );
        if let Some(ref path) = r.path_context {
            println!("         path: {path}");
        }
    }

    println!();
    println!("{} item(s) to prune.", all_reports.len());

    // Run validation hooks
    println!();
    println!("=== Validation ===");

    match PrePurgeValidation::validate(
        &all_reports.iter().map(|r| (*r).clone()).collect::<Vec<_>>(),
    ) {
        Ok(issues) => {
            if issues.is_empty() {
                println!("  ✓ No validation issues.");
            } else {
                for issue in &issues {
                    let severity = match issue.severity {
                        IssueSeverity::Block => "BLOCK",
                        IssueSeverity::Warn => "WARN",
                    };
                    println!("  [{severity}] {} — {}", issue.item, issue.message);
                }

                let blocked = issues.iter().any(|i| i.severity == IssueSeverity::Block);
                if blocked {
                    eprintln!("  ✗ Blocking issues found. Resolve before pruning.");
                    return;
                }
            }
        }
        Err(e) => {
            eprintln!("  ✗ Validation error: {e}");
            return;
        }
    }

    // Interactive confirmation (unless force or dry-run)
    if !force && !dry_run {
        println!();
        println!("Proceed with pruning? (yes/no)");
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            eprintln!("  ✗ Failed to read input. Use --force to skip confirmation.");
            return;
        }
        let trimmed = input.trim().to_lowercase();
        if trimmed != "yes" && trimmed != "y" {
            println!("  ✗ Aborted by user.");
            return;
        }
    }

    // Execute pruning
    println!();
    println!("=== Executing Prune ===");

    // Skills pruning
    let skills_reports: Vec<_> = report.skills.iter().filter(|r| r.safe_to_purge).cloned().collect();
    if !skills_reports.is_empty() {
        let skills_root = agents_root.join("skills");
        match crate::prune::prune_skills_unified(&skills_reports, &skills_root, dry_run) {
            Ok(pr) => {
                for removed in &pr.removed {
                    if dry_run {
                        println!("  [DRY_RUN] would remove skill: {removed}");
                    } else {
                        println!("  [REMOVED] skill: {removed}");
                    }
                }
                for skipped in &pr.skipped_protected {
                    println!("  [SKIPPED] protected skill: {skipped}");
                }
                for (id, err) in &pr.skipped_error {
                    eprintln!("  [ERROR] {id}: {err}");
                }
                if let Some(bp) = pr.backup_path {
                    println!("  Backup saved to: {}", bp.display());
                }
            }
            Err(e) => {
                eprintln!("  ✗ Skills pruning failed: {e}");
            }
        }
    }

    // MCP pruning
    let mcp_reports: Vec<_> = report
        .mcp_servers
        .iter()
        .filter(|r| r.safe_to_purge)
        .cloned()
        .collect();
    if !mcp_reports.is_empty() {
        match crate::prune::prune_mcp_unified(&mcp_reports, &mcp_config, dry_run) {
            Ok(pr) => {
                for removed in &pr.removed {
                    if dry_run {
                        println!("  [DRY_RUN] would remove MCP server: {removed}");
                    } else {
                        println!("  [REMOVED] MCP server: {removed}");
                    }
                }
                for skipped in &pr.skipped_protected {
                    println!("  [SKIPPED] protected MCP server: {skipped}");
                }
                for (id, err) in &pr.skipped_error {
                    eprintln!("  [ERROR] {id}: {err}");
                }
                if let Some(bp) = pr.backup_path {
                    println!("  Backup saved to: {}", bp.display());
                }
            }
            Err(e) => {
                eprintln!("  ✗ MCP pruning failed: {e}");
            }
        }
    }

    println!("  ✓ Prune complete.");
}

fn run_vacuum(dry_run: bool) {
    println!("🧹 vacuum — cleaning orphaned MCP subprocesses");

    if dry_run {
        println!("  → DRY RUN: no processes will be killed");
    }

    // In production, this would come from the canonical config.
    // For now, we use an empty server list to find process orphans
    // by known-agent parent detection only.
    let mcp_servers: HashMap<String, McpServerDefinition> = HashMap::new();

    match crate::prune::subprocess::teardown_orphaned_processes(&mcp_servers, dry_run) {
        Ok(killed) => {
            if killed.is_empty() {
                println!("  ✓ No orphaned MCP processes found.");
            } else {
                for proc in &killed {
                    if dry_run {
                        println!("  [DRY_RUN] would kill PID {} — {}", proc.pid, proc.command);
                    } else {
                        println!(
                            "  [{}] PID {} terminated: {}",
                            proc.signal, proc.pid, proc.command
                        );
                    }
                }
                println!("  → {} process(es) handled.", killed.len());
            }
        }
        Err(e) => {
            eprintln!("  ✗ Error scanning processes: {e}");
        }
    }
}

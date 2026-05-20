use clap::{Parser, Subcommand};

mod mcp;
mod sync;

#[derive(Parser)]
#[command(name = "agentalign", about = "Agent Configuration Unification Engine")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan existing agent configs into ~/.agents/
    Migrate,
    /// Push canonical config to all agents
    Sync,
    /// Roll back the last sync transaction
    Restore {
        /// Rollback specific agent (all agents if omitted)
        #[arg(long)]
        agent: Option<String>,

        /// Rollback specific transaction by UUID
        #[arg(long)]
        id: Option<String>,

        /// Show transaction history
        #[arg(long)]
        list: bool,
    },
    /// Show skill usage report
    Status,
    /// Mark a skill as used/unused
    Mark,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Migrate => {
            println!("agentalign — migrate (not yet implemented)");
        }
        Commands::Sync => {
            println!("agentalign — sync (not yet implemented)");
        }
        Commands::Restore { agent, id, list } => {
            if list {
                match sync::transaction::handle_list(agent.as_deref()) {
                    Ok(transactions) => {
                        if transactions.is_empty() {
                            println!("No transactions found.");
                        } else {
                            println!(
                                "{:<38} {:<10} {:<12} {:<50} {:<10}",
                                "ID", "Agent", "Timestamp", "Target Path", "Status"
                            );
                            println!("{}", "-".repeat(130));
                            for tx in &transactions {
                                let status = match tx.status {
                                    agentalign_shared::models::TransactionStatus::Pending => {
                                        "pending"
                                    }
                                    agentalign_shared::models::TransactionStatus::Committed => {
                                        "committed"
                                    }
                                    agentalign_shared::models::TransactionStatus::RolledBack => {
                                        "rolled_back"
                                    }
                                };
                                println!(
                                    "{:<38} {:<10} {:<12} {:<50} {:<10}",
                                    tx.id, tx.agent, tx.timestamp, tx.target_path, status
                                );
                            }
                        }
                    }
                    Err(e) => eprintln!("Error listing transactions: {}", e),
                }
            } else if let Some(tx_id) = id {
                match sync::transaction::handle_rollback_by_id(&tx_id) {
                    Ok(()) => println!("Done."),
                    Err(e) => eprintln!("Error: {}", e),
                }
            } else {
                match sync::transaction::handle_rollback(agent.as_deref()) {
                    Ok(count) => {
                        if count > 0 {
                            println!("Rolled back {} transaction(s).", count);
                        } else {
                            println!("No transactions to roll back.");
                        }
                    }
                    Err(e) => eprintln!("Error: {}", e),
                }
            }
        }
        Commands::Status => {
            println!("agentalign — status (not yet implemented)");
        }
        Commands::Mark => {
            println!("agentalign — mark (not yet implemented)");
        }
    }
}

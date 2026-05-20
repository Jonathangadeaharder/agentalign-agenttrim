fn main() {
    println!("agentalign — Agent Configuration Unification Engine");
    println!("Usage: agentalign <command>");
    println!();
    println!("Commands:");
    println!("  migrate   Scan existing agent configs into ~/.agents/");
    println!("  sync      Push canonical config to all agents");
    println!("  restore   Roll back last sync transaction");
    println!("  status    Show skill usage report");
    println!("  mark      Mark a skill as used/unused");
}

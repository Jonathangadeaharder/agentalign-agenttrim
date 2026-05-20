fn main() {
    println!("agenttrim — Telemetry-Driven Pruning & Vacuum Engine");
    println!("Usage: agenttrim <command>");
    println!();
    println!("Commands:");
    println!("  analyze   Report unused skills, MCP servers, and processes");
    println!("  prune     Remove unused resources (with safety gates)");
    println!("  vacuum    Deep clean: zombie processes, orphaned caches");
    println!("  config    Configure thresholds and allowlists");
}

// src/main.rs  v0.3
//
// Subcommands:
//   todoist-nvim [fetch]       → fetch active tasks → stdout
//   todoist-nvim sync <file>   → sync buffer file → Todoist
//   todoist-nvim completed     → fetch completed tasks → stdout
//   todoist-nvim reopen <id>   → reopen a specific task by ID

mod api;
mod fetch;
mod models;
mod parser;
mod snapshot;
mod sync;

fn main() {
    if let Err(e) = dispatch() {
        eprintln!("{}", e);
        std::process::exit(1);
    }
}

fn dispatch() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("sync") => {
            let path = args.get(2)
                .ok_or("Usage: todoist-nvim sync <buffer-file>")?;
            sync::run(path)
        }
        Some("completed") => fetch::run_completed(),
        Some("reopen") => {
            let id = args.get(2)
                .ok_or("Usage: todoist-nvim reopen <task-id>")?;
            sync::run_reopen(id)
        }
        _ => fetch::run(),
    }
}

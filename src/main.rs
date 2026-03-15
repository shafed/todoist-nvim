// src/main.rs
//
// todoist-nvim v0.2 — Rust binary for the Neovim Todoist plugin.
//
// Subcommands
// ───────────
//   todoist-nvim fetch             → fetch tasks, render to stdout (default)
//   todoist-nvim sync <bufferfile> → sync buffer file to Todoist, summary to stdout
//
// The Lua layer invokes the binary asynchronously via vim.fn.jobstart.
// Errors are written to stderr; the process exits with code 1 so Lua can
// surface them via vim.notify.

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
            let path = args.get(2).ok_or_else(|| {
                "Usage: todoist-nvim sync <buffer-file-path>".to_string()
            })?;
            sync::run(path)
        }
        // "fetch" or no argument → default behaviour
        _ => fetch::run(),
    }
}

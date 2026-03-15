# todoist-nvim

A minimal Neovim plugin that fetches your **Todoist tasks** and renders them as
a read-only Markdown buffer inside Neovim.

```
:TodoistOpen
```

```markdown
# Work

## Backend

- [ ] Fix authentication bug
  - [ ] Write test
  - [ ] Deploy patch

## Infrastructure

- [ ] Upgrade staging server

# Personal

- [ ] Buy groceries
```

> **API version**: This plugin targets the
> [Todoist Unified API v1](https://developer.todoist.com/api/v1/)
> (`api.todoist.com/api/v1`), which replaced the deprecated REST v2 API. The key
> differences are: new base URL, camelCase JSON field names, and cursor-based
> paginated list responses.

---

## Architecture

### Integration strategy — Rust binary + Lua glue

Three realistic options exist for Rust↔Neovim integration:

| Approach                 | Pros                                 | Cons                          |
| ------------------------ | ------------------------------------ | ----------------------------- |
| **Rust binary + Lua** ✅ | Simple, reliable, no RPC boilerplate | Extra build step              |
| nvim-oxi (native plugin) | No Lua required                      | Complex API, fragile ABI      |
| Remote plugin (RPC)      | Full Neovim API access               | Heavy setup, msgpack overhead |

This plugin uses **option 1**: the Rust binary handles all business logic (HTTP,
JSON parsing, hierarchy construction, Markdown rendering) and writes the result
to stdout. A small Lua layer invokes the binary asynchronously with
`vim.fn.jobstart`, captures the output, and populates a scratch buffer.

This keeps the Rust/Lua boundary clean: Rust knows nothing about Neovim; Lua
knows nothing about Todoist.

### Todoist API strategy

Three **Todoist Unified API v1** endpoints are called:

```
GET https://api.todoist.com/api/v1/projects
GET https://api.todoist.com/api/v1/sections
GET https://api.todoist.com/api/v1/tasks
```

API v1 wraps every list response as
`{ "results": [...], "nextCursor": "..." | null }`. The binary follows cursors
until `nextCursor` is `null`, guaranteeing the complete dataset is fetched
regardless of account size. A single `reqwest::blocking::Client` is reused
across all requests (HTTP keep-alive). Blocking I/O is fine here because the
binary runs as a child process — Neovim's `jobstart` keeps the UI thread free.

### Hierarchy construction

```
projects  (sorted by .childOrder)
└── sections  (grouped by projectId, sorted by .sectionOrder)
    └── top-level tasks  (partitioned from subtasks; parentId == null)
        └── subtasks  (HashMap<parentId, Vec<Task>>)
```

Tasks with `section_id == None` or `section_id == ""` are rendered directly
under their project heading. Projects with zero active tasks are omitted.

### Buffer rendering

The Rust binary writes UTF-8 Markdown to stdout. The Lua layer splits the output
on newlines (Neovim's job layer does this automatically), removes the trailing
empty sentinel line, and calls `nvim_buf_set_lines`. The buffer is configured
with:

- `buftype=nofile` — no backing file
- `filetype=markdown` — syntax highlighting
- `modifiable=false` + `readonly=true` — immutable display
- `q` → close, `r` / `<C-r>` → refresh

---

## Requirements

- Neovim 0.8+
- Rust toolchain (`cargo`) — for the one-time build step
- A
  [Todoist API token](https://app.todoist.com/app/settings/integrations/developer)

---

## Installation

### 1 — Set your API token

Add this to your shell profile (`.bashrc`, `.zshrc`, etc.):

```bash
export TODOIST_API_TOKEN="your_token_here"
```

Or set it inside Neovim (e.g. in `init.lua`):

```lua
vim.env.TODOIST_API_TOKEN = "your_token_here"
```

### 2 — Install the plugin

#### lazy.nvim (recommended)

```lua
{
    "your-github-username/todoist-nvim",
    build = "cargo build --release",
    config = function()
        -- setup() is called automatically by plugin/todoist.lua,
        -- but you can call it explicitly here if you prefer:
        -- require("todoist").setup()
    end,
}
```

#### packer.nvim

```lua
use {
    "your-github-username/todoist-nvim",
    run = "cargo build --release",
}
```

#### Manual

```bash
# Clone into your Neovim packages directory
git clone https://github.com/your-github-username/todoist-nvim \
    ~/.local/share/nvim/site/pack/plugins/start/todoist-nvim

# Build the binary
cd ~/.local/share/nvim/site/pack/plugins/start/todoist-nvim
cargo build --release
```

### 3 — Verify

Open Neovim and run:

```vim
:TodoistOpen
```

---

## Usage

| Command        | Effect                                   |
| -------------- | ---------------------------------------- |
| `:TodoistOpen` | Fetch tasks and open the Markdown buffer |
| `r` / `<C-r>`  | Refresh tasks (inside the buffer)        |
| `q`            | Close the buffer                         |

---

## Example buffer output

```markdown
# Work

## Backend

- [ ] Fix authentication bug
  - [ ] Write test
  - [ ] Deploy patch

## Infrastructure

- [ ] Upgrade staging server

# Personal

- [ ] Buy groceries
```

---

## Configuration

`setup()` accepts an options table (all fields are optional and reserved for
future use — the current MVP requires no configuration):

```lua
require("todoist").setup({
    -- future options go here
})
```

---

## Troubleshooting

| Symptom                        | Fix                                                                                   |
| ------------------------------ | ------------------------------------------------------------------------------------- |
| `TODOIST_API_TOKEN is not set` | Export the variable in your shell profile and restart Neovim                          |
| `binary not found`             | Run `cargo build --release` inside the plugin directory                               |
| `401 Unauthorized`             | Your token is wrong or expired — regenerate it on the Todoist developer settings page |
| `Network error`                | Check your internet connection or any firewall rules                                  |
| Buffer shows no tasks          | You have no active tasks in Todoist — congratulations!                                |

---

## Limitations (MVP scope)

- **Read-only**: tasks cannot be edited or completed from Neovim.
- **No real-time sync**: the buffer is a snapshot; use `r` to refresh.
- **Active tasks only**: completed and archived tasks are not shown.
- **Flat subtask rendering**: Todoist supports one level of nesting; deeper
  nesting (if it ever exists) will still render correctly due to the recursive
  renderer, but is untested against the API.

---

## Development

```bash
# Run tests
cargo test

# Build debug binary (faster compile, slower binary)
cargo build

# Build release binary
cargo build --release

# Run the binary directly (requires TODOIST_API_TOKEN to be set)
./target/release/todoist-nvim
```

---

## License

MIT

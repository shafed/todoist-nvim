# todoist-nvim

A minimal Neovim plugin that fetches your **Todoist tasks** and renders them as
an interactive buffer inside Neovim — with checkbox icons, syntax highlighting,
and two-way sync.

```
:TodoistOpen
```

```
- [ ] Fix authentication bug
  - [ ]  Write test
  - [ ]  Deploy patch
-[x] Upgrade staging server
- [ ]  Buy groceries
```

> **API version**: This plugin targets the
> [Todoist Unified API v1](https://developer.todoist.com/api/v1/)
> (`api.todoist.com/api/v1`), which replaced the deprecated REST v2 API.

---

## Features

- ✅ Fetches all active tasks grouped by **project → section → task → subtask**
- ✅ Checkbox icons rendered via extmarks — same icons as
  [render-markdown.nvim](https://github.com/MeanderingProgrammer/render-markdown.nvim)
  (`` / ``)
- ✅ `- ` list marker and `<!--id:...-->` metadata concealed automatically
- ✅ Markdown Tree-sitter highlighting (headings, bold, etc.) injected without
  changing `filetype`
- ✅ Toggle task complete with `x` and sync to Todoist with `<localleader>s`
- ✅ Completed tasks browser (`:TodoistCompleted`) with one-click restore
- ✅ Cursor line shows raw text for easy editing; all other lines are rendered

---

## Architecture

### Integration strategy — Rust binary + Lua glue

| Approach                 | Pros                                 | Cons                          |
| ------------------------ | ------------------------------------ | ----------------------------- |
| **Rust binary + Lua** ✅ | Simple, reliable, no RPC boilerplate | Extra build step              |
| nvim-oxi (native plugin) | No Lua required                      | Complex API, fragile ABI      |
| Remote plugin (RPC)      | Full Neovim API access               | Heavy setup, msgpack overhead |

The Rust binary handles all business logic (HTTP, JSON parsing, hierarchy
construction, Markdown rendering) and writes the result to stdout. A small Lua
layer invokes the binary asynchronously with `vim.fn.jobstart`, captures the
output, and populates a scratch buffer.

### Rendering pipeline

The buffer stays `filetype=todoist` to avoid triggering other Markdown plugins.
Syntax highlighting comes from an injected Tree-sitter `markdown` parser
(`vim.treesitter.start(buf, "markdown")`). Checkbox icons and conceal markers
are applied via `nvim_buf_set_extmark`:

```
Raw line:   - [ ] Fix bug <!--id:abc123-->
Rendered:     Fix bug
```

1. `"- "` list marker → concealed
2. `"[ ]"` / `"[x]"` → overlay icon (`` / ``)
3. `<!--id:...-->` metadata → concealed

The cursor line always shows the raw text so navigation and editing work
correctly.

### Todoist API

Three endpoints are called:

```
GET /api/v1/projects
GET /api/v1/sections
GET /api/v1/tasks
```

All list responses are cursor-paginated (`nextCursor`). The binary follows
cursors until `null`, guaranteeing the complete dataset regardless of account
size.

### Hierarchy

```
projects  (sorted by childOrder)
└── sections  (grouped by projectId, sorted by sectionOrder)
    └── top-level tasks  (parentId == null)
        └── subtasks  (HashMap<parentId, Vec<Task>>)
```

Projects with zero active tasks are omitted.

---

## Requirements

- Neovim 0.9+
- Rust toolchain (`cargo`) — one-time build step
- [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) with the
  `markdown` parser installed
- A
  [Todoist API token](https://app.todoist.com/app/settings/integrations/developer)
- A [Nerd Font](https://www.nerdfonts.com/) in your terminal (for checkbox
  icons)

---

## Installation

### 1 — Set your API token

```bash
export TODOIST_API_TOKEN="your_token_here"
```

Or in `init.lua`:

```lua
vim.env.TODOIST_API_TOKEN = "your_token_here"
```

### 2 — Install the plugin

#### lazy.nvim

```lua
{
    "shafed/todoist-nvim",
    build = "cargo build --release",
    config = function()
        require("todoist").setup()
    end,
}
```

#### Manual

```bash
git clone https://github.com/shafed/todoist-nvim \
    ~/.local/share/nvim/site/pack/plugins/start/todoist-nvim

cd ~/.local/share/nvim/site/pack/plugins/start/todoist-nvim
cargo build --release
```

### 3 — Install Tree-sitter markdown parser

```vim
:TSInstall markdown
```

---

## Usage

### Commands

| Command             | Effect                                 |
| ------------------- | -------------------------------------- |
| `:TodoistOpen`      | Fetch active tasks and open the buffer |
| `:TodoistCompleted` | Open completed tasks (last 30 days)    |
| `:TodoistSync`      | Sync buffer changes → Todoist          |
| `:TodoistRestore`   | Restore completed task under cursor    |

### Keymaps (active buffer)

| Key              | Effect                               |
| ---------------- | ------------------------------------ |
| `x`              | Toggle task complete (`[ ]` ↔ `[x]`) |
| `<localleader>s` | Sync to Todoist                      |
| `<localleader>c` | Open completed tasks buffer          |
| `<CR>`           | Navigate deeper (project/section)    |
| `<BS>`           | Navigate up                          |
| `zf` / `zu`      | Fold / unfold                        |
| `r` / `<C-r>`    | Refresh                              |
| `q`              | Close buffer                         |

### Keymaps (completed buffer)

| Key              | Effect                         |
| ---------------- | ------------------------------ |
| `x`              | Mark / unmark task for restore |
| `<localleader>s` | Sync restores to Todoist       |
| `r` / `<C-r>`    | Refresh                        |
| `q`              | Close buffer                   |

---

## Configuration

Checkbox icons and highlights can be customized to match your setup:

```lua
require("todoist").setup({
    checkbox = {
        unchecked = {
            icon            = " ",           -- default: nf-fa-square_o
            highlight       = "TodoistUnchecked", -- links to Comment by default
            scope_highlight = nil,
        },
        checked = {
            icon            = " ",           -- default: nf-fa-check_square
            highlight       = "TodoistChecked",   -- links to String by default
            scope_highlight = "TodoistCheckedLine",
        },
        custom = {
            { raw = "[-]", icon = "⌛", highlight = "TodoistTodo", scope_highlight = nil },
        },
    },
})
```

The default icons match
[render-markdown.nvim](https://github.com/MeanderingProgrammer/render-markdown.nvim)
defaults and require a Nerd Font.

---

## Troubleshooting

| Symptom                        | Fix                                                                                   |
| ------------------------------ | ------------------------------------------------------------------------------------- |
| `TODOIST_API_TOKEN is not set` | Export the variable in your shell profile and restart Neovim                          |
| `binary not found`             | Run `cargo build --release` inside the plugin directory                               |
| `401 Unauthorized`             | Your token is wrong or expired — regenerate it on the Todoist developer settings page |
| No syntax highlighting         | Run `:TSInstall markdown` and reopen the buffer                                       |
| Icons show as boxes            | Install a [Nerd Font](https://www.nerdfonts.com/) and configure your terminal         |
| Buffer shows no tasks          | You have no active tasks in Todoist — congratulations!                                |

---

## Development

```bash
cargo test
cargo build           # debug
cargo build --release # release
./target/release/todoist-nvim fetch
./target/release/todoist-nvim completed
```

---

## License

MIT

-- lua/todoist.lua
--
-- Lua glue for todoist-nvim.
--
-- Responsibilities:
--   • Locate the compiled Rust binary (release → debug → $PATH).
--   • Run it asynchronously via vim.fn.jobstart so the UI never blocks.
--   • Open / reuse a named scratch buffer and fill it with the Markdown output.
--   • Surface errors from the binary via vim.notify.
--
-- The module exposes two public symbols:
--   M.open()        – called by the :TodoistOpen command
--   M.setup(opts)   – registers the command (call this from your config)

local M = {}

-- ─── Binary discovery ────────────────────────────────────────────────────────

--- Return the absolute path of the todoist-nvim binary, or nil if not found.
local function find_binary()
    -- Resolve the plugin root from *this* file's location.
    -- __FILE__  → .../todoist-nvim/lua/todoist.lua
    -- plugin root → .../todoist-nvim/
    local this_file = debug.getinfo(1, "S").source:sub(2) -- strip leading "@"
    local plugin_root = vim.fn.fnamemodify(this_file, ":h:h")

    local candidates = {
        plugin_root .. "/target/release/todoist-nvim",
        plugin_root .. "/target/debug/todoist-nvim",
        vim.fn.exepath("todoist-nvim"), -- binary installed to $PATH
    }

    for _, path in ipairs(candidates) do
        if path ~= "" and vim.fn.executable(path) == 1 then
            return path
        end
    end

    return nil
end

-- ─── Buffer helpers ──────────────────────────────────────────────────────────

local BUFFER_NAME = "Todoist Tasks"

--- Find an existing todoist buffer by name, or return nil.
local function find_existing_buffer()
    for _, buf in ipairs(vim.api.nvim_list_bufs()) do
        if vim.api.nvim_buf_is_valid(buf) then
            local name = vim.api.nvim_buf_get_name(buf)
            if vim.fn.fnamemodify(name, ":t") == BUFFER_NAME then
                return buf
            end
        end
    end
    return nil
end

--- Write `lines` into `buf`, temporarily lifting the read-only lock.
local function set_buffer_lines(buf, lines)
    vim.api.nvim_buf_set_option(buf, "modifiable", true)
    vim.api.nvim_buf_set_option(buf, "readonly", false)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
    vim.api.nvim_buf_set_option(buf, "modifiable", false)
    vim.api.nvim_buf_set_option(buf, "readonly", true)
end

--- Open `buf` in the current window (or switch to an existing window showing it).
local function focus_buffer(buf)
    local wins = vim.fn.win_findbuf(buf)
    if #wins > 0 then
        vim.api.nvim_set_current_win(wins[1])
    else
        vim.api.nvim_set_current_buf(buf)
    end
end

--- Create a brand-new scratch buffer configured for read-only Markdown display.
local function create_scratch_buffer()
    -- listed = true  → shows in :ls
    -- scratch = true → buftype will be set to nofile below
    local buf = vim.api.nvim_create_buf(true, true)
    vim.api.nvim_buf_set_name(buf, BUFFER_NAME)

    -- Scratch / read-only settings
    vim.api.nvim_buf_set_option(buf, "buftype", "nofile")
    vim.api.nvim_buf_set_option(buf, "bufhidden", "hide")
    vim.api.nvim_buf_set_option(buf, "swapfile", false)
    vim.api.nvim_buf_set_option(buf, "filetype", "markdown")
    vim.api.nvim_buf_set_option(buf, "modifiable", false)
    vim.api.nvim_buf_set_option(buf, "readonly", true)

    -- Quality-of-life keymaps inside the buffer
    local opts = { buffer = buf, noremap = true, silent = true }
    vim.keymap.set("n", "q",      "<cmd>bdelete<cr>", vim.tbl_extend("force", opts, { desc = "Close Todoist buffer" }))
    vim.keymap.set("n", "r",      function() M.open() end, vim.tbl_extend("force", opts, { desc = "Refresh Todoist tasks" }))
    vim.keymap.set("n", "<C-r>",  function() M.open() end, vim.tbl_extend("force", opts, { desc = "Refresh Todoist tasks" }))

    return buf
end

-- ─── Buffer population (called on the main thread via vim.schedule) ──────────

--- @param lines string[]  lines captured from the binary's stdout
function M._open_buffer(lines)
    local buf = find_existing_buffer()

    if buf then
        set_buffer_lines(buf, lines)
        focus_buffer(buf)
    else
        buf = create_scratch_buffer()
        set_buffer_lines(buf, lines)
        focus_buffer(buf)
    end

    -- Place cursor at the top of the buffer.
    vim.api.nvim_win_set_cursor(0, { 1, 0 })
end

-- ─── Main entry point ────────────────────────────────────────────────────────

--- Fetch Todoist tasks and open them in a scratch buffer.
--- The binary is executed asynchronously; the UI remains responsive.
function M.open()
    local binary = find_binary()

    if not binary then
        vim.notify(
            table.concat({
                "todoist-nvim: binary not found.",
                "",
                "Build it with:",
                "  cd <plugin-root>",
                "  cargo build --release",
                "",
                "Or install it to your $PATH:",
                "  cargo install --path <plugin-root>",
            }, "\n"),
            vim.log.levels.ERROR,
            { title = "todoist-nvim" }
        )
        return
    end

    vim.notify("Fetching Todoist tasks…", vim.log.levels.INFO, { title = "todoist-nvim" })

    local stdout_chunks = {}
    local stderr_chunks = {}

    local job_id = vim.fn.jobstart({ binary }, {
        stdout_buffered = true,
        stderr_buffered = true,

        on_stdout = function(_, data, _)
            stdout_chunks = data
        end,

        on_stderr = function(_, data, _)
            stderr_chunks = data
        end,

        on_exit = function(_, code, _)
            if code ~= 0 then
                -- Binary wrote a human-readable error to stderr.
                local msg = table.concat(stderr_chunks, "\n"):gsub("%s+$", "")
                vim.schedule(function()
                    vim.notify(
                        msg ~= "" and msg or ("todoist-nvim exited with code " .. code),
                        vim.log.levels.ERROR,
                        { title = "todoist-nvim" }
                    )
                end)
                return
            end

            -- stdout_chunks is a table of lines; the last element is always ""
            -- (sentinel added by Neovim's job layer).
            if stdout_chunks[#stdout_chunks] == "" then
                table.remove(stdout_chunks)
            end

            vim.schedule(function()
                M._open_buffer(stdout_chunks)
            end)
        end,
    })

    if job_id <= 0 then
        vim.notify(
            "todoist-nvim: failed to start the binary.\nIs it executable?",
            vim.log.levels.ERROR,
            { title = "todoist-nvim" }
        )
    end
end

-- ─── Setup ───────────────────────────────────────────────────────────────────

--- Register the :TodoistOpen command.
--- Call this once from your init.lua / lazy.nvim config block.
---
--- @param opts table|nil  (reserved for future options, currently unused)
function M.setup(opts)
    opts = opts or {}

    vim.api.nvim_create_user_command(
        "TodoistOpen",
        function()
            M.open()
        end,
        {
            desc = "Fetch Todoist tasks and display them in a Markdown scratch buffer",
            nargs = 0,
        }
    )
end

return M

-- lua/todoist.lua  (v0.2 — fetch + sync)
--
-- Public API:
--   M.setup(opts)   → register :TodoistOpen command (call once from config)
--   M.open()        → fetch tasks and open/refresh the scratch buffer
--   M.sync()        → sync the current buffer to Todoist, then re-fetch
--
-- Keymaps inside the Todoist buffer:
--   q          close buffer
--   r / <C-r>  refresh (re-fetch)
--   <localleader>s  sync buffer → Todoist, then re-fetch

local M = {}

-- ─── Binary discovery ────────────────────────────────────────────────────────

local function find_binary()
	local this_file = debug.getinfo(1, "S").source:sub(2)
	local plugin_root = vim.fn.fnamemodify(this_file, ":h:h")

	local candidates = {
		plugin_root .. "/target/release/todoist-nvim",
		plugin_root .. "/target/debug/todoist-nvim",
		vim.fn.exepath("todoist-nvim"),
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

local function find_existing_buffer()
	for _, buf in ipairs(vim.api.nvim_list_bufs()) do
		if vim.api.nvim_buf_is_valid(buf) then
			if vim.fn.fnamemodify(vim.api.nvim_buf_get_name(buf), ":t") == BUFFER_NAME then
				return buf
			end
		end
	end
	return nil
end

local function set_buffer_lines(buf, lines)
	vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
end

local function focus_buffer(buf)
	local wins = vim.fn.win_findbuf(buf)
	if #wins > 0 then
		vim.api.nvim_set_current_win(wins[1])
	else
		vim.api.nvim_set_current_buf(buf)
	end
end

local function create_scratch_buffer()
	local buf = vim.api.nvim_create_buf(true, true)
	vim.api.nvim_buf_set_name(buf, BUFFER_NAME)

	vim.api.nvim_buf_set_option(buf, "buftype", "nofile")
	vim.api.nvim_buf_set_option(buf, "bufhidden", "hide")
	vim.api.nvim_buf_set_option(buf, "swapfile", false)
	vim.api.nvim_buf_set_option(buf, "filetype", "markdown")
	vim.api.nvim_buf_set_option(buf, "modifiable", false)
	vim.api.nvim_buf_set_option(buf, "readonly", true)

	-- Conceal the <!-- id:XXX --> / <!-- project:XXX --> / <!-- section:XXX -->
	-- comments so the buffer looks clean.  The raw text (with IDs) is still
	-- there and will be read by the sync engine.
	vim.api.nvim_buf_set_option(buf, "conceallevel", 2)
	vim.api.nvim_buf_set_option(buf, "concealcursor", "nvic")
	vim.api.nvim_buf_call(buf, function()
		vim.cmd([[syntax match TodoistMeta /\s*<!--\_.\{-}-->/ conceal]])
	end)

	local opts = { buffer = buf, noremap = true, silent = true }

	vim.keymap.set("n", "q", "<cmd>bdelete<cr>", vim.tbl_extend("force", opts, { desc = "Close Todoist buffer" }))

	vim.keymap.set("n", "r", function()
		M.open()
	end, vim.tbl_extend("force", opts, { desc = "Refresh Todoist tasks" }))

	vim.keymap.set("n", "<C-r>", function()
		M.open()
	end, vim.tbl_extend("force", opts, { desc = "Refresh Todoist tasks" }))

	vim.keymap.set("n", "<localleader>s", function()
		M.sync()
	end, vim.tbl_extend("force", opts, { desc = "Sync buffer → Todoist" }))

	return buf
end

-- ─── open() ──────────────────────────────────────────────────────────────────

function M._open_buffer(lines)
	local buf = find_existing_buffer()
	if not buf then
		buf = create_scratch_buffer()
	end
	set_buffer_lines(buf, lines)
	focus_buffer(buf)
	vim.api.nvim_win_set_cursor(0, { 1, 0 })
end

function M.open()
	local binary = find_binary()
	if not binary then
		vim.notify(
			"todoist-nvim: binary not found.\nRun: cargo build --release",
			vim.log.levels.ERROR,
			{ title = "todoist-nvim" }
		)
		return
	end

	vim.notify("Fetching Todoist tasks…", vim.log.levels.INFO, { title = "todoist-nvim" })

	local stdout_chunks = {}
	local stderr_chunks = {}

	vim.fn.jobstart({ binary, "fetch" }, {
		stdout_buffered = true,
		stderr_buffered = true,

		on_stdout = function(_, data)
			stdout_chunks = data
		end,
		on_stderr = function(_, data)
			stderr_chunks = data
		end,

		on_exit = function(_, code)
			if code ~= 0 then
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

			if stdout_chunks[#stdout_chunks] == "" then
				table.remove(stdout_chunks)
			end

			vim.schedule(function()
				M._open_buffer(stdout_chunks)
			end)
		end,
	})
end

-- ─── sync() ──────────────────────────────────────────────────────────────────

function M.sync()
	local binary = find_binary()
	if not binary then
		vim.notify(
			"todoist-nvim: binary not found.\nRun: cargo build --release",
			vim.log.levels.ERROR,
			{ title = "todoist-nvim" }
		)
		return
	end

	-- Find the Todoist buffer.
	local buf = find_existing_buffer()
	if not buf then
		vim.notify("No Todoist buffer found. Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end

	-- Temporarily lift readonly so we can read lines.
	-- (nvim_buf_get_lines doesn't need modifiable, but just to be safe.)
	local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)

	if #lines == 0 then
		vim.notify("Buffer is empty — nothing to sync.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end

	-- Write buffer to a temp file for the binary to read.
	local tmpfile = vim.fn.tempname()
	vim.fn.writefile(lines, tmpfile)

	vim.notify("Syncing to Todoist…", vim.log.levels.INFO, { title = "todoist-nvim" })

	local stdout_chunks = {}
	local stderr_chunks = {}

	vim.fn.jobstart({ binary, "sync", tmpfile }, {
		stdout_buffered = true,
		stderr_buffered = true,

		on_stdout = function(_, data)
			stdout_chunks = data
		end,
		on_stderr = function(_, data)
			stderr_chunks = data
		end,

		on_exit = function(_, code)
			-- Clean up temp file regardless of outcome.
			vim.fn.delete(tmpfile)

			if code ~= 0 then
				local msg = table.concat(stderr_chunks, "\n"):gsub("%s+$", "")
				vim.schedule(function()
					vim.notify(
						msg ~= "" and msg or ("Sync failed (exit " .. code .. ")"),
						vim.log.levels.ERROR,
						{ title = "todoist-nvim" }
					)
				end)
				return
			end

			local summary = table.concat(stdout_chunks, "\n"):gsub("%s+$", "")
			vim.schedule(function()
				-- Show sync summary.
				vim.notify(summary, vim.log.levels.INFO, { title = "todoist-nvim sync" })

				-- Re-fetch to update IDs for newly created tasks and reflect
				-- completed/deleted tasks being gone.
				vim.defer_fn(function()
					M.open()
				end, 500)
			end)
		end,
	})
end

-- ─── setup() ─────────────────────────────────────────────────────────────────

function M.setup(opts)
	opts = opts or {}

	vim.api.nvim_create_user_command("TodoistOpen", function()
		M.open()
	end, { desc = "Open Todoist tasks in a Markdown buffer", nargs = 0 })

	vim.api.nvim_create_user_command("TodoistSync", function()
		M.sync()
	end, { desc = "Sync Todoist buffer changes to Todoist", nargs = 0 })
end

return M

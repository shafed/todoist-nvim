-- lua/todoist.lua  v0.3
--
-- Commands:
--   :TodoistOpen      → active tasks buffer
--   :TodoistCompleted → completed tasks buffer (last 30 days)
--   :TodoistSync      → sync active buffer → Todoist
--
-- Keymaps (inside Todoist buffers):
--   q               close buffer
--   r / <C-r>       refresh
--   <CR>            navigate deeper (project / section / task)
--   <BS>            navigate up
--   zf / zu         fold / unfold
--   x               toggle task complete ([ ] ↔ [x]) and sync
--                   in completed buffer: mark/unmark task for restore
--   <localleader>s  sync (active buffer) / sync restores (completed buffer)
--   <localleader>r  restore task under cursor (completed buffer only)

local nav = require("todoist.nav")

local M = {}

-- ─── Namespace for extmarks (ID concealment) ───────────────────────────────────────────
local NS = vim.api.nvim_create_namespace("todoist_meta")

-- ─── Pending restores state ────────────────────────────────────────────────────────────
-- { [task_id] = true }  — tasks marked for restore, not yet sent to API
local pending_restores = {}

-- ─── Binary discovery ──────────────────────────────────────────────────────────────────

local function find_binary()
	local this_file = debug.getinfo(1, "S").source:sub(2)
	local plugin_root = vim.fn.fnamemodify(this_file, ":h:h:h")
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

-- ─── Buffer registry ─────────────────────────────────────────────────────────────────

local ACTIVE_BUF_NAME = "Todoist Tasks"
local COMPLETED_BUF_NAME = "Todoist Completed"

local function find_buf(name)
	for _, buf in ipairs(vim.api.nvim_list_bufs()) do
		if vim.api.nvim_buf_is_valid(buf) then
			if vim.fn.fnamemodify(vim.api.nvim_buf_get_name(buf), ":t") == name then
				return buf
			end
		end
	end
	return nil
end

-- ─── Extmark-based ID concealment ───────────────────────────────────────────────────

local function apply_extmark_conceal(buf)
	vim.api.nvim_buf_clear_namespace(buf, NS, 0, -1)
	local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
	for lnum, line in ipairs(lines) do
		local s, e = line:find("%s*<!%-%-.*%-%->%s*")
		if s and e then
			vim.api.nvim_buf_set_extmark(buf, NS, lnum - 1, s - 1, {
				end_col = e,
				conceal = "",
			})
		end
	end
end

-- ─── conceallevel on the window ──────────────────────────────────────────────────────

local function set_conceal(buf)
	local function apply(win)
		vim.wo[win].conceallevel = 3
		vim.wo[win].concealcursor = "nvic"
	end
	local win = vim.fn.bufwinid(buf)
	if win ~= -1 then
		apply(win)
	end
	local guard_id = vim.api.nvim_create_autocmd({ "BufEnter", "BufWinEnter", "WinEnter" }, {
		buffer = buf,
		callback = function()
			local w = vim.fn.bufwinid(buf)
			if w ~= -1 then
				apply(w)
			end
		end,
	})
	vim.api.nvim_create_autocmd("BufDelete", {
		buffer = buf,
		once = true,
		callback = function()
			pcall(vim.api.nvim_del_autocmd, guard_id)
		end,
	})
end

-- ─── Buffer creation ────────────────────────────────────────────────────────────────

local function create_buf(name, is_readonly)
	local buf = vim.api.nvim_create_buf(true, true)
	vim.api.nvim_buf_set_name(buf, name)
	vim.bo[buf].buftype = "nofile"
	vim.bo[buf].bufhidden = "hide"
	vim.bo[buf].swapfile = false
	vim.bo[buf].filetype = "markdown"
	if is_readonly then
		vim.bo[buf].modifiable = false
		vim.bo[buf].readonly = true
	end
	return buf
end

local function set_lines(buf, lines)
	local was_modifiable = vim.bo[buf].modifiable
	if not was_modifiable then
		vim.bo[buf].modifiable = true
		vim.bo[buf].readonly = false
	end
	vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
	if not was_modifiable then
		vim.bo[buf].modifiable = false
		vim.bo[buf].readonly = true
	end
end

local function focus_buf(buf)
	local wins = vim.fn.win_findbuf(buf)
	if #wins > 0 then
		vim.api.nvim_set_current_win(wins[1])
	else
		vim.api.nvim_set_current_buf(buf)
	end
end

local function nav_redraw(buf, lines)
	if not lines then
		return
	end
	set_lines(buf, lines)
	apply_extmark_conceal(buf)
	vim.api.nvim_win_set_cursor(0, { 1, 0 })
end

-- ─── Toggle complete under cursor ────────────────────────────────────────────────────

--- Toggle [ ] ↔ [x] on the task line under cursor, then sync + refresh.
local function toggle_complete(buf)
	local row = vim.api.nvim_win_get_cursor(0)[1]
	local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""

	local new_line
	if line:match("%- %[ %]") then
		new_line = line:gsub("%- %[ %]", "- [x]", 1)
	elseif line:match("%- %[x%]") or line:match("%- %[X%]") then
		new_line = line:gsub("%- %[[xX]%]", "- [ ]", 1)
	else
		vim.notify("Cursor is not on a task line.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end

	vim.api.nvim_buf_set_lines(buf, row - 1, row, false, { new_line })
	apply_extmark_conceal(buf)
end

-- ─── Toggle restore mark (completed buffer) ──────────────────────────────────────────

--- Mark/unmark a completed task for batch restore. Does NOT call the API.
local function toggle_restore_mark(buf)
	local cur_win = vim.api.nvim_get_current_win()
	local win
	if vim.api.nvim_win_get_buf(cur_win) == buf then
		win = cur_win
	else
		win = vim.fn.bufwinid(buf)
		if win == -1 then
			vim.notify("Completed buffer is not visible.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
	end
	local row = vim.api.nvim_win_get_cursor(win)[1]
	local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""

	local task_id = line:match("id:(%S+)")
	if not task_id then
		vim.notify("No task ID found on this line.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end

	local new_line
	if pending_restores[task_id] then
		-- снять пометку: вернуть [ ] → [x]
		pending_restores[task_id] = nil
		new_line = line:gsub("%- %[ %]", "- [x]", 1)
		vim.notify("Unmarked: " .. task_id, vim.log.levels.INFO, { title = "todoist-nvim" })
	else
		-- поставить пометку: заменить [x] → [ ]
		pending_restores[task_id] = true
		new_line = line:gsub("%- %[x%]", "- [ ]", 1)
		vim.notify("Marked for restore: " .. task_id, vim.log.levels.INFO, { title = "todoist-nvim" })
	end

	vim.bo[buf].modifiable = true
	vim.bo[buf].readonly = false
	vim.api.nvim_buf_set_lines(buf, row - 1, row, false, { new_line })
	vim.bo[buf].modifiable = false
	vim.bo[buf].readonly = true
	apply_extmark_conceal(buf)
end

-- ─── Sync pending restores (completed buffer \s) ─────────────────────────────────────

local function sync_restores()
	local binary = find_binary()
	if not binary then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local ids = vim.tbl_keys(pending_restores)
	if #ids == 0 then
		vim.notify(
			"No tasks marked for restore. Use x to mark tasks first.",
			vim.log.levels.WARN,
			{ title = "todoist-nvim" }
		)
		return
	end
	vim.notify("Restoring " .. #ids .. " task(s)…", vim.log.levels.INFO, { title = "todoist-nvim" })

	local done = 0
	local failed = 0
	for _, task_id in ipairs(ids) do
		local err = {}
		vim.fn.jobstart({ binary, "reopen", task_id }, {
			stdout_buffered = true,
			stderr_buffered = true,
			on_stderr = function(_, d)
				err = d
			end,
			on_exit = function(_, code)
				done = done + 1
				if code ~= 0 then
					failed = failed + 1
					local msg = table.concat(err, "\n"):gsub("%s+$", "")
					vim.schedule(function()
						vim.notify(
							"Failed to restore " .. task_id .. ": " .. (msg ~= "" and msg or "exit " .. code),
							vim.log.levels.ERROR,
							{ title = "todoist-nvim" }
						)
					end)
				end
				if done == #ids then
					pending_restores = {}
					vim.schedule(function()
						if failed == 0 then
							vim.notify(
								"All " .. #ids .. " task(s) restored!",
								vim.log.levels.INFO,
								{ title = "todoist-nvim" }
							)
						end
						vim.defer_fn(function()
							M.completed()
							vim.defer_fn(function()
								M.open()
							end, 300)
						end, 300)
					end)
				end
			end,
		})
	end
end

-- ─── Active tasks buffer keymaps ──────────────────────────────────────────────────────

local function setup_active_keymaps(buf)
	local o = { buffer = buf, noremap = true, silent = true }

	vim.keymap.set("n", "q", "<cmd>bdelete<cr>", vim.tbl_extend("force", o, { desc = "Close" }))
	vim.keymap.set("n", "<C-r>", function()
		M.open()
	end, vim.tbl_extend("force", o, { desc = "Refresh" }))
	vim.keymap.set("n", "<localleader>s", function()
		M.sync()
	end, vim.tbl_extend("force", o, { desc = "Sync → Todoist" }))
	vim.keymap.set("n", "<localleader>c", function()
		M.completed()
	end, vim.tbl_extend("force", o, { desc = "Open Completed" }))

	-- Navigation
	vim.keymap.set("n", "<CR>", function()
		nav_redraw(buf, nav.enter(buf))
	end, vim.tbl_extend("force", o, { desc = "Navigate deeper" }))
	vim.keymap.set("n", "<BS>", function()
		nav_redraw(buf, nav.back())
	end, vim.tbl_extend("force", o, { desc = "Navigate up" }))

	-- Folding
	vim.keymap.set("n", "zf", function()
		nav_redraw(buf, nav.fold())
	end, vim.tbl_extend("force", o, { desc = "Collapse current view" }))
	vim.keymap.set("n", "zu", function()
		nav_redraw(buf, nav.unfold())
	end, vim.tbl_extend("force", o, { desc = "Expand current view" }))

	-- Complete / reopen task
	vim.keymap.set("n", "x", function()
		toggle_complete(buf)
	end, vim.tbl_extend("force", o, { desc = "Toggle task complete" }))

	-- Re-apply concealment after any text change
	vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI" }, {
		buffer = buf,
		callback = function()
			apply_extmark_conceal(buf)
		end,
	})
end

-- ─── Completed tasks buffer keymaps ───────────────────────────────────────────────────

local function setup_completed_keymaps(buf)
	local o = { buffer = buf, noremap = true, silent = true }
	vim.keymap.set("n", "q", "<cmd>bdelete<cr>", vim.tbl_extend("force", o, { desc = "Close" }))
	vim.keymap.set("n", "r", function()
		M.completed()
	end, vim.tbl_extend("force", o, { desc = "Refresh" }))
	vim.keymap.set("n", "<C-r>", function()
		M.completed()
	end, vim.tbl_extend("force", o, { desc = "Refresh" }))
	-- x marks/unmarks task for restore (no immediate API call)
	vim.keymap.set("n", "x", function()
		toggle_restore_mark(buf)
	end, vim.tbl_extend("force", o, { desc = "Mark/unmark task for restore" }))
	-- \s sends all marked tasks to API at once
	vim.keymap.set("n", "<localleader>s", function()
		sync_restores()
	end, vim.tbl_extend("force", o, { desc = "Sync restores → Todoist" }))
end

-- ─── Treesitter highlighting ────────────────────────────────────────────────────────────

local function start_treesitter(buf)
	local ok, _ = pcall(vim.treesitter.start, buf, "markdown")
	if not ok then
		vim.api.nvim_buf_call(buf, function()
			vim.cmd("setlocal syntax=markdown")
		end)
	end
end

-- ─── open() ──────────────────────────────────────────────────────────────────────

function M._fill_active_buffer(lines)
	local buf = find_buf(ACTIVE_BUF_NAME)
	if not buf then
		buf = create_buf(ACTIVE_BUF_NAME, false)
		setup_active_keymaps(buf)
	end
	nav.load(lines)
	nav.reset()
	set_lines(buf, nav.lines())
	focus_buf(buf)
	start_treesitter(buf)
	apply_extmark_conceal(buf)
	set_conceal(buf)
	vim.api.nvim_win_set_cursor(0, { 1, 0 })
end

function M.open()
	local binary = find_binary()
	if not binary then
		vim.notify(
			"todoist-nvim: binary not found. Run: cargo build --release",
			vim.log.levels.ERROR,
			{ title = "todoist-nvim" }
		)
		return
	end
	vim.notify("Fetching tasks…", vim.log.levels.INFO, { title = "todoist-nvim" })
	local out, err = {}, {}
	vim.fn.jobstart({ binary, "fetch" }, {
		stdout_buffered = true,
		stderr_buffered = true,
		on_stdout = function(_, d)
			out = d
		end,
		on_stderr = function(_, d)
			err = d
		end,
		on_exit = function(_, code)
			if code ~= 0 then
				local msg = table.concat(err, "\n"):gsub("%s+$", "")
				vim.schedule(function()
					vim.notify(msg ~= "" and msg or ("exit " .. code), vim.log.levels.ERROR, { title = "todoist-nvim" })
				end)
				return
			end
			if out[#out] == "" then
				table.remove(out)
			end
			vim.schedule(function()
				M._fill_active_buffer(out)
			end)
		end,
	})
end

-- ─── completed() ─────────────────────────────────────────────────────────────────

function M._fill_completed_buffer(lines)
	local buf = find_buf(COMPLETED_BUF_NAME)
	if not buf then
		buf = create_buf(COMPLETED_BUF_NAME, true)
		setup_completed_keymaps(buf)
	end
	set_lines(buf, lines)
	focus_buf(buf)
	start_treesitter(buf)
	apply_extmark_conceal(buf)
	set_conceal(buf)
	vim.api.nvim_win_set_cursor(0, { 1, 0 })
end

function M.completed()
	local binary = find_binary()
	if not binary then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	vim.notify("Fetching completed tasks…", vim.log.levels.INFO, { title = "todoist-nvim" })
	local out, err = {}, {}
	vim.fn.jobstart({ binary, "completed" }, {
		stdout_buffered = true,
		stderr_buffered = true,
		on_stdout = function(_, d)
			out = d
		end,
		on_stderr = function(_, d)
			err = d
		end,
		on_exit = function(_, code)
			if code ~= 0 then
				local msg = table.concat(err, "\n"):gsub("%s+$", "")
				vim.schedule(function()
					vim.notify(msg ~= "" and msg or ("exit " .. code), vim.log.levels.ERROR, { title = "todoist-nvim" })
				end)
				return
			end
			if out[#out] == "" then
				table.remove(out)
			end
			vim.schedule(function()
				M._fill_completed_buffer(out)
			end)
		end,
	})
end

-- ─── restore_under_cursor() ───────────────────────────────────────────────────────────
-- Kept for :TodoistRestore command (immediate single-task restore)

function M.restore_under_cursor(buf)
	local binary = find_binary()
	if not binary then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local cur_win = vim.api.nvim_get_current_win()
	local win
	if vim.api.nvim_win_get_buf(cur_win) == buf then
		win = cur_win
	else
		win = vim.fn.bufwinid(buf)
		if win == -1 then
			vim.notify("Completed buffer is not visible.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
	end
	local row = vim.api.nvim_win_get_cursor(win)[1]
	local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""
	local task_id = line:match("id:(%S+)")
	if not task_id then
		vim.notify("No task ID found on this line.\nRaw: " .. line, vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	vim.notify("Restoring task " .. task_id .. "…", vim.log.levels.INFO, { title = "todoist-nvim" })
	local out, err = {}, {}
	vim.fn.jobstart({ binary, "reopen", task_id }, {
		stdout_buffered = true,
		stderr_buffered = true,
		on_stdout = function(_, d)
			out = d
		end,
		on_stderr = function(_, d)
			err = d
		end,
		on_exit = function(_, code)
			if code ~= 0 then
				local msg = table.concat(err, "\n"):gsub("%s+$", "")
				vim.schedule(function()
					vim.notify(
						msg ~= "" and msg or ("Restore failed (exit " .. code .. ")."),
						vim.log.levels.ERROR,
						{ title = "todoist-nvim" }
					)
				end)
				return
			end
			vim.schedule(function()
				vim.notify("Task restored!", vim.log.levels.INFO, { title = "todoist-nvim" })
				vim.defer_fn(function()
					M.completed()
					vim.defer_fn(function()
						M.open()
					end, 300)
				end, 300)
			end)
		end,
	})
end

-- ─── sync() ───────────────────────────────────────────────────────────────────────

function M.sync()
	local binary = find_binary()
	if not binary then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local buf = find_buf(ACTIVE_BUF_NAME)
	if not buf then
		vim.notify("No active Todoist buffer. Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
	if #lines == 0 then
		vim.notify("Buffer is empty.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	local tmpfile = vim.fn.tempname()
	vim.fn.writefile(lines, tmpfile)
	vim.notify("Syncing…", vim.log.levels.INFO, { title = "todoist-nvim" })
	local out, err = {}, {}
	vim.fn.jobstart({ binary, "sync", tmpfile }, {
		stdout_buffered = true,
		stderr_buffered = true,
		on_stdout = function(_, d)
			out = d
		end,
		on_stderr = function(_, d)
			err = d
		end,
		on_exit = function(_, code)
			vim.fn.delete(tmpfile)
			if code ~= 0 then
				local msg = table.concat(err, "\n"):gsub("%s+$", "")
				vim.schedule(function()
					vim.notify(msg ~= "" and msg or "Sync failed.", vim.log.levels.ERROR, { title = "todoist-nvim" })
				end)
				return
			end
			local summary = table.concat(out, "\n"):gsub("%s+$", "")
			vim.schedule(function()
				vim.notify(summary, vim.log.levels.INFO, { title = "todoist-nvim sync" })
				vim.defer_fn(function()
					M.open()
				end, 500)
			end)
		end,
	})
end

-- ─── setup() ─────────────────────────────────────────────────────────────────────

function M.setup(opts)
	opts = opts or {}

	vim.api.nvim_create_user_command("TodoistOpen", function()
		M.open()
	end, { desc = "Open active Todoist tasks", nargs = 0 })
	vim.api.nvim_create_user_command("TodoistCompleted", function()
		M.completed()
	end, { desc = "Open completed Todoist tasks (last 30 days)", nargs = 0 })
	vim.api.nvim_create_user_command("TodoistSync", function()
		M.sync()
	end, { desc = "Sync Todoist buffer → Todoist", nargs = 0 })
	vim.api.nvim_create_user_command("TodoistRestore", function()
		local buf = find_buf(COMPLETED_BUF_NAME)
		if not buf then
			vim.notify("Open :TodoistCompleted first.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		M.restore_under_cursor(buf)
	end, { desc = "Restore completed task under cursor", nargs = 0 })
end

return M

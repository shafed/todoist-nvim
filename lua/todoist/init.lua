-- lua/todoist.lua  v0.8
--
-- Commands:
--   :TodoistOpen      → active tasks buffer
--   :TodoistCompleted → completed tasks view (in active buffer, last 30 days)
--   :TodoistSync      → sync active buffer → Todoist
--
-- Keymaps (inside Todoist buffer):
--   q               close buffer
--   r / <C-r>       refresh
--   <CR>            navigate deeper (project / section / task)
--   <BS>            navigate up  (from Completed view: returns to previous view)
--   zf / zu         fold / unfold
--   x               toggle task complete (active view)
--                   mark/unmark task for restore (completed view)
--   <localleader>s  sync (active view) / sync restores (completed view)
--   <localleader>r  restore task under cursor (completed view only)
--   <localleader>c  open completed view

local nav = require("todoist.nav")

local M = {}

-- ─── Namespace ─────────────────────────────────────────────────────────────────────────
local NS = vim.api.nvim_create_namespace("todoist_meta")

-- ─── Default checkbox config ───────────────────────────────────────────────────────────
local CHECKBOX = {
	unchecked = {
		icon = "󰄱  ", -- nf-fa-square_o (same as render-markdown default)
		highlight = "TodoistUnchecked",
		scope_highlight = nil,
	},
	checked = {
		icon = "󰱒  ", -- nf-fa-check_square (same as render-markdown default)
		highlight = "TodoistChecked",
		scope_highlight = nil,
	},
	custom = {},
}

local function char_width(s)
	return vim.fn.strdisplaywidth(s)
end

local function ensure_highlights()
	if vim.fn.hlexists("TodoistUnchecked") == 0 then
		vim.api.nvim_set_hl(0, "TodoistUnchecked", { link = "Comment", default = true })
	end
	if vim.fn.hlexists("TodoistChecked") == 0 then
		vim.api.nvim_set_hl(0, "TodoistChecked", { link = "String", default = true })
	end
end

-- ─── Core checkbox renderer ────────────────────────────────────────────────────────────
local function render_checkbox(buf, lnum0, marker_col, col_s, col_e, cfg, is_cursor)
	if is_cursor then
		return
	end

	vim.api.nvim_buf_set_extmark(buf, NS, lnum0, marker_col, {
		end_col = col_s,
		conceal = "",
	})

	local icon = cfg.icon
	local hl = cfg.highlight
	local icon_w = char_width(icon)
	local raw_w = col_e - col_s

	if icon_w <= raw_w then
		vim.api.nvim_buf_set_extmark(buf, NS, lnum0, col_s, {
			end_col = col_s + icon_w,
			virt_text = { { icon, hl } },
			virt_text_pos = "overlay",
		})
		if icon_w < raw_w then
			vim.api.nvim_buf_set_extmark(buf, NS, lnum0, col_s + icon_w, {
				end_col = col_e,
				conceal = "",
			})
		end
	else
		vim.api.nvim_buf_set_extmark(buf, NS, lnum0, col_s, {
			end_col = col_e,
			virt_text = { { icon, hl } },
			virt_text_pos = "overlay",
		})
		vim.api.nvim_buf_set_extmark(buf, NS, lnum0, col_e, {
			virt_text = { { string.rep(" ", icon_w - raw_w), hl } },
			virt_text_pos = "inline",
		})
	end
end

-- ─── Pending restores state ────────────────────────────────────────────────────────────
local pending_restores = {}

-- ─── Binary discovery ─────────────────────────────────────────────────────────────────
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

-- ─── apply_extmark_conceal ────────────────────────────────────────────────────────────
local function apply_extmark_conceal(buf, cursor_line)
	vim.api.nvim_buf_clear_namespace(buf, NS, 0, -1)
	local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)

	for lnum, line in ipairs(lines) do
		local lnum0 = lnum - 1
		local is_cursor = (cursor_line ~= nil and lnum == cursor_line)

		local ms, me = line:find("%s*<!%-%-.*%-%->%s*")
		if ms and me then
			vim.api.nvim_buf_set_extmark(buf, NS, lnum0, ms - 1, {
				end_col = me,
				conceal = "",
			})
		end

		local matched_custom = false
		for _, custom in ipairs(CHECKBOX.custom) do
			local escaped = vim.pesc(custom.raw)
			local marker_col, cb_start, cb_end = line:match("()%- ()" .. escaped .. "()")
			if marker_col then
				matched_custom = true
				render_checkbox(buf, lnum0, marker_col - 1, cb_start - 1, cb_end - 1, custom, is_cursor)
				if not is_cursor and custom.scope_highlight then
					vim.api.nvim_buf_set_extmark(buf, NS, lnum0, 0, {
						end_col = #line,
						hl_group = custom.scope_highlight,
						priority = 90,
					})
				end
				break
			end
		end

		if not matched_custom then
			local mc, cs, ce = line:match("()%- ()%[ %]()")
			if mc then
				render_checkbox(buf, lnum0, mc - 1, cs - 1, ce - 1, CHECKBOX.unchecked, is_cursor)
				if not is_cursor and CHECKBOX.unchecked.scope_highlight then
					vim.api.nvim_buf_set_extmark(buf, NS, lnum0, 0, {
						end_col = #line,
						hl_group = CHECKBOX.unchecked.scope_highlight,
						priority = 90,
					})
				end
			end

			local mc2, cs2, ce2 = line:match("()%- ()%[[xX]%]()")
			if mc2 then
				render_checkbox(buf, lnum0, mc2 - 1, cs2 - 1, ce2 - 1, CHECKBOX.checked, is_cursor)
				if not is_cursor and CHECKBOX.checked.scope_highlight then
					vim.api.nvim_buf_set_extmark(buf, NS, lnum0, 0, {
						end_col = #line,
						hl_group = CHECKBOX.checked.scope_highlight,
						priority = 90,
					})
				end
			end
		end
	end
end

-- ─── conceallevel ──────────────────────────────────────────────────────────────────────
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

local function setup_cursor_conceal(buf)
	vim.api.nvim_create_autocmd("CursorMoved", {
		buffer = buf,
		callback = function()
			local row = vim.api.nvim_win_get_cursor(0)[1]
			apply_extmark_conceal(buf, row)
		end,
	})
end

-- ─── Buffer creation ────────────────────────────────────────────────────────────────
local function create_buf(name)
	local buf = vim.api.nvim_create_buf(true, true)
	vim.api.nvim_buf_set_name(buf, name)
	vim.bo[buf].buftype = "nofile"
	vim.bo[buf].bufhidden = "hide"
	vim.bo[buf].swapfile = false
	vim.bo[buf].filetype = "todoist"
	pcall(vim.treesitter.start, buf, "markdown")
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

-- ─── Cursor restore after nav_redraw ───────────────────────────────────────────────

local function line_anchor(line)
	return line:match("id:(%S+)") or line:match("project:(%S+)") or line:match("section:(%S+)")
end

local function restore_cursor(buf, anchor, fallback_row)
	if anchor then
		local new_lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
		for i, l in ipairs(new_lines) do
			if line_anchor(l) == anchor then
				vim.api.nvim_win_set_cursor(0, { i, 0 })
				return
			end
		end
	end
	local total = vim.api.nvim_buf_line_count(buf)
	vim.api.nvim_win_set_cursor(0, { math.min(fallback_row, total), 0 })
end

local function nav_redraw(buf, lines)
	if not lines then
		return
	end
	local old_row = vim.api.nvim_win_get_cursor(0)[1]
	local old_line = vim.api.nvim_buf_get_lines(buf, old_row - 1, old_row, false)[1] or ""
	local anchor = line_anchor(old_line)
	set_lines(buf, lines)
	apply_extmark_conceal(buf)
	restore_cursor(buf, anchor, old_row)
end

-- ─── Toggle complete under cursor ────────────────────────────────────────────────────
local function toggle_complete(buf)
	local row = vim.api.nvim_win_get_cursor(0)[1]
	local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""

	if nav.current_view() == nav.VIEW.COMPLETED then
		-- In completed view: x = mark/unmark for restore
		local task_id = line:match("id:(%S+)")
		if not task_id then
			vim.notify("No task ID found on this line.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		local new_line
		if pending_restores[task_id] then
			pending_restores[task_id] = nil
			new_line = line:gsub("%- %[ %]", "- [x]", 1)
			vim.notify("Unmarked: " .. task_id, vim.log.levels.INFO, { title = "todoist-nvim" })
		else
			pending_restores[task_id] = true
			new_line = line:gsub("%- %[x%]", "- [ ]", 1)
			vim.notify("Marked for restore: " .. task_id, vim.log.levels.INFO, { title = "todoist-nvim" })
		end
		local all_lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
		all_lines[row] = new_line
		set_lines(buf, all_lines)
		apply_extmark_conceal(buf, row)
		return
	end

	-- Active view: normal toggle
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
	apply_extmark_conceal(buf, row)
end

-- ─── Sync pending restores ───────────────────────────────────────────────────────────────
local function sync_restores(buf)
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
							M.completed(buf)
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

-- ─── Buffer keymaps ───────────────────────────────────────────────────────────
local function setup_keymaps(buf)
	local o = { buffer = buf, noremap = true, silent = true }
	vim.keymap.set("n", "q", "<cmd>bdelete<cr>", vim.tbl_extend("force", o, { desc = "Close" }))
	vim.keymap.set("n", "r", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			M.completed(buf)
		else
			M.open()
		end
	end, vim.tbl_extend("force", o, { desc = "Refresh" }))
	vim.keymap.set("n", "<C-r>", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			M.completed(buf)
		else
			M.open()
		end
	end, vim.tbl_extend("force", o, { desc = "Refresh" }))
	vim.keymap.set("n", "<localleader>s", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			sync_restores(buf)
		else
			M.sync()
		end
	end, vim.tbl_extend("force", o, { desc = "Sync" }))
	vim.keymap.set("n", "<localleader>c", function()
		M.completed(buf)
	end, vim.tbl_extend("force", o, { desc = "Open Completed view" }))
	vim.keymap.set("n", "<localleader>r", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			M.restore_under_cursor(buf)
		else
			vim.notify("Use :TodoistCompleted first.", vim.log.levels.WARN, { title = "todoist-nvim" })
		end
	end, vim.tbl_extend("force", o, { desc = "Restore task (completed view)" }))
	vim.keymap.set("n", "<CR>", function()
		nav_redraw(buf, nav.enter(buf))
	end, vim.tbl_extend("force", o, { desc = "Navigate deeper" }))
	vim.keymap.set("n", "<BS>", function()
		nav_redraw(buf, nav.back())
	end, vim.tbl_extend("force", o, { desc = "Navigate up" }))
	vim.keymap.set("n", "zf", function()
		nav_redraw(buf, nav.fold())
	end, vim.tbl_extend("force", o, { desc = "Collapse" }))
	vim.keymap.set("n", "zu", function()
		nav_redraw(buf, nav.unfold())
	end, vim.tbl_extend("force", o, { desc = "Expand" }))
	vim.keymap.set("n", "x", function()
		toggle_complete(buf)
	end, vim.tbl_extend("force", o, { desc = "Toggle complete / mark for restore" }))
	vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI" }, {
		buffer = buf,
		callback = function()
			local row = vim.api.nvim_win_get_cursor(0)[1]
			apply_extmark_conceal(buf, row)
		end,
	})
end

-- ─── open() ──────────────────────────────────────────────────────────────────────
function M._fill_active_buffer(lines)
	local buf = find_buf(ACTIVE_BUF_NAME)
	if not buf then
		buf = create_buf(ACTIVE_BUF_NAME)
		setup_keymaps(buf)
		setup_cursor_conceal(buf)
	end
	nav.load(lines)
	nav.reset()
	set_lines(buf, nav.lines())
	focus_buf(buf)
	ensure_highlights()
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

-- ─── completed() — renders into the active buffer as COMPLETED view ──────────
function M.completed(existing_buf)
	local binary = find_binary()
	if not binary then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end

	local buf = existing_buf or find_buf(ACTIVE_BUF_NAME)
	if not buf then
		vim.notify("Open :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
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
				nav.load_completed(out)
				local lines = nav.enter_completed()
				set_lines(buf, lines)
				focus_buf(buf)
				ensure_highlights()
				apply_extmark_conceal(buf)
				set_conceal(buf)
				vim.api.nvim_win_set_cursor(0, { 1, 0 })
			end)
		end,
	})
end

-- ─── restore_under_cursor() ───────────────────────────────────────────────────────────
function M.restore_under_cursor(buf)
	local binary = find_binary()
	if not binary then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local row = vim.api.nvim_win_get_cursor(0)[1]
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
					M.completed(buf)
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
	if nav.current_view() == nav.VIEW.COMPLETED then
		vim.notify("Cannot sync from completed view. Use <BS> to go back.", vim.log.levels.WARN, { title = "todoist-nvim" })
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

-- ─── setup() ─────────────────────────────────────────────────────────────────
--- Config:
---   checkbox = {
---     unchecked = { icon = "󰄱  ", highlight = "TodoistUnchecked", scope_highlight = nil },
---     checked   = { icon = "󰱒  ", highlight = "TodoistChecked",   scope_highlight = "TodoistCheckedLine" },
---     custom    = {
---       { raw = "[-]", icon = "⌛", highlight = "TodoistTodo", scope_highlight = nil },
---     },
---   },
function M.setup(opts)
	opts = opts or {}

	if opts.checkbox then
		local cb = opts.checkbox
		if cb.unchecked then
			CHECKBOX.unchecked = vim.tbl_extend("force", CHECKBOX.unchecked, cb.unchecked)
		end
		if cb.checked then
			CHECKBOX.checked = vim.tbl_extend("force", CHECKBOX.checked, cb.checked)
		end
		if cb.custom then
			CHECKBOX.custom = cb.custom
		end
	end

	vim.api.nvim_create_user_command("TodoistOpen", function()
		M.open()
	end, { desc = "Open active Todoist tasks", nargs = 0 })
	vim.api.nvim_create_user_command("TodoistCompleted", function()
		local buf = find_buf(ACTIVE_BUF_NAME)
		if not buf then
			vim.notify("Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		M.completed(buf)
	end, { desc = "Open completed Todoist tasks view (30 days)", nargs = 0 })
	vim.api.nvim_create_user_command("TodoistSync", function()
		M.sync()
	end, { desc = "Sync Todoist buffer → Todoist", nargs = 0 })
	vim.api.nvim_create_user_command("TodoistRestore", function()
		local buf = find_buf(ACTIVE_BUF_NAME)
		if not buf then
			vim.notify("Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		if nav.current_view() ~= nav.VIEW.COMPLETED then
			vim.notify("Open completed view first (<localleader>c or :TodoistCompleted).", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		M.restore_under_cursor(buf)
	end, { desc = "Restore completed task under cursor", nargs = 0 })
end

return M

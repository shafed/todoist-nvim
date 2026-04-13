local meta = require("todoist.meta")
local nav = require("todoist.nav")
local M = {}
local ui = vim.api.nvim_create_namespace("todoist_ui")
local syncing, creating = false, false
local active_name = "todoist://tasks"
local keymaps = {}
local pending = {}

local function hi()
	if vim.fn.hlexists("TodoistDue") == 0 then
		vim.api.nvim_set_hl(0, "TodoistDue", { link = "Special", default = true })
	end
end

local function bin()
	local f = debug.getinfo(1, "S").source:sub(2)
	local root = vim.fn.fnamemodify(f, ":h:h:h")
	for _, p in ipairs({
		root .. "/target/release/todoist-nvim",
		root .. "/target/debug/todoist-nvim",
		vim.fn.exepath("todoist-nvim"),
	}) do
		if p ~= "" and vim.fn.executable(p) == 1 then
			return p
		end
	end
end

local function buf(name)
	for _, b in ipairs(vim.api.nvim_list_bufs()) do
		if vim.api.nvim_buf_is_valid(b) and vim.api.nvim_buf_get_name(b) == name then
			return b
		end
	end
end

local function put_lines(b, lines)
	local m = vim.bo[b].modifiable
	if not m then
		vim.bo[b].modifiable = true
		vim.bo[b].readonly = false
	end
	vim.api.nvim_buf_set_lines(b, 0, -1, false, lines)
	vim.bo[b].modified = false
	if not m then
		vim.bo[b].modifiable = false
		vim.bo[b].readonly = true
	end
end

local function focus(b)
	local w = vim.fn.win_findbuf(b)
	if #w > 0 then
		vim.api.nvim_set_current_win(w[1])
	else
		vim.api.nvim_set_current_buf(b)
	end
end

local function paint(b)
	vim.api.nvim_buf_clear_namespace(b, ui, 0, -1)
	for i, line in ipairs(vim.api.nvim_buf_get_lines(b, 0, -1, false)) do
		local l = i - 1
		local ds, de = line:find("due:%S.*")
		if ds then
			vim.api.nvim_buf_set_extmark(b, ui, l, ds - 1, {
				end_col = de,
				hl_group = "TodoistDue",
				priority = 110,
			})
		end
	end
end

local function set_conceal(b)
	local function apply(w)
		vim.wo[w].conceallevel = 3
		vim.wo[w].concealcursor = "nvic"
		vim.wo[w].foldmethod = "expr"
		vim.wo[w].foldexpr = "getline(v:lnum)=~'^## '?'>1':getline(v:lnum)=~'^### '?'>2':'='"
		vim.wo[w].foldlevel = 99
	end
	local w = vim.fn.bufwinid(b)
	if w ~= -1 then
		apply(w)
	end
	local id = vim.api.nvim_create_autocmd({ "BufEnter", "BufWinEnter", "WinEnter", "FileType" }, {
		buffer = b,
		callback = function()
			local ww = vim.fn.bufwinid(b)
			if ww ~= -1 then
				apply(ww)
			end
		end,
	})
	vim.api.nvim_create_autocmd("BufDelete", {
		buffer = b,
		once = true,
		callback = function()
			keymaps[b] = nil
			pcall(vim.api.nvim_del_autocmd, id)
		end,
	})
end

local function live_paint(b)
	vim.api.nvim_create_autocmd({ "CursorMoved", "CursorMovedI", "TextChanged", "TextChangedI" }, {
		buffer = b,
		callback = function()
			local w = vim.api.nvim_get_current_win()
			vim.wo[w].conceallevel = 3
			vim.wo[w].concealcursor = "nvic"
			paint(b)
		end,
	})
end

local function mkbuf(name)
	local b = vim.api.nvim_create_buf(false, true)
	vim.api.nvim_buf_set_name(b, name)
	vim.bo[b].buftype = "acwrite"
	vim.bo[b].bufhidden = "wipe"
	vim.bo[b].swapfile = false
	vim.bo[b].filetype = "markdown"
	vim.b[b].todoist = true
	vim.bo[b].expandtab = true
	vim.bo[b].tabstop = 4
	vim.bo[b].shiftwidth = 4
	vim.bo[b].softtabstop = 4
	hi()
	vim.api.nvim_create_autocmd("VimLeavePre", {
		buffer = b,
		callback = function()
			if vim.api.nvim_buf_is_valid(b) then
				pcall(vim.api.nvim_buf_delete, b, { force = true })
			end
		end,
	})
	return b
end

local function restore(b, a, fallback)
	local r = a and meta.find_row_by_anchor(b, a)
	local total = vim.api.nvim_buf_line_count(b)
	vim.api.nvim_win_set_cursor(0, { math.min(r or fallback, total), 0 })
end

local function render(b, r, a, f)
	put_lines(b, r.lines or {})
	meta.attach(b, r.meta)
	paint(b)
	restore(b, a, f)
end

local function redraw(b, r)
	if not r then
		return
	end
	local row = vim.api.nvim_win_get_cursor(0)[1]
	render(b, r, meta.anchor_for_row(b, row), row)
end

local function tid(b)
	local e = meta.get_at_row(b, vim.api.nvim_win_get_cursor(0)[1])
	return e and e.kind == "task" and e.todoist_id or nil
end

local function toggle(b)
	local row = vim.api.nvim_win_get_cursor(0)[1]
	local line = vim.api.nvim_buf_get_lines(b, row - 1, row, false)[1] or ""
	if nav.current_view() == nav.VIEW.COMPLETED then
		local id = tid(b)
		if not id then
			vim.notify("No task ID found on this line.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		if pending[id] then
			pending[id] = nil
			line = line:gsub("%- %[ %]", "- [x]", 1)
		else
			pending[id] = true
			line = line:gsub("%- %[x%]", "- [ ]", 1)
		end
		vim.api.nvim_buf_set_lines(b, row - 1, row, false, { line })
		paint(b)
		return
	end
	if line:match("%- %[ %]") then
		line = line:gsub("%- %[ %]", "- [x]", 1)
	elseif line:match("%- %[x%]") or line:match("%- %[X%]") then
		line = line:gsub("%- %[[xX]%]", "- [ ]", 1)
	else
		vim.notify("Cursor is not on a task line.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	vim.api.nvim_buf_set_lines(b, row - 1, row, false, { line })
	paint(b)
end

local function run_restore(b)
	local exe = bin()
	if not exe then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local ids = vim.tbl_keys(pending)
	if #ids == 0 then
		vim.notify(
			"No tasks marked for restore. Use x to mark tasks first.",
			vim.log.levels.WARN,
			{ title = "todoist-nvim" }
		)
		return
	end
	local done, failed = 0, 0
	for _, id in ipairs(ids) do
		local err = {}
		vim.fn.jobstart({ exe, "reopen", id }, {
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
							"Failed to restore " .. id .. ": " .. (msg ~= "" and msg or "exit " .. code),
							vim.log.levels.ERROR,
							{ title = "todoist-nvim" }
						)
					end)
				end
				if done == #ids then
					pending = {}
					vim.schedule(function()
						if failed == 0 then
							vim.notify(
								"All " .. #ids .. " task(s) restored!",
								vim.log.levels.INFO,
								{ title = "todoist-nvim" }
							)
						end
						vim.defer_fn(function()
							M.completed(b)
							vim.defer_fn(M.open, 300)
						end, 300)
					end)
				end
			end,
		})
	end
end

local function keymap(b)
	local o = { buffer = b, noremap = true, silent = true }
	vim.keymap.set("n", "<Esc>", "<cmd>bdelete<cr>", o)
	vim.keymap.set("n", "r", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			M.completed(b)
		else
			M.open()
		end
	end, o)
	vim.keymap.set("n", "<C-r>", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			M.completed(b)
		else
			M.open()
		end
	end, o)
	vim.keymap.set("n", "<localleader>s", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			run_restore(b)
		else
			M.sync()
		end
	end, o)
	vim.keymap.set("n", "<localleader>c", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			redraw(b, nav.back())
		else
			M.completed(b)
		end
	end, o)
	vim.keymap.set("n", "<localleader>r", function()
		if nav.current_view() == nav.VIEW.COMPLETED then
			M.restore_under_cursor(b)
		else
			vim.notify("Use :TodoistCompleted first.", vim.log.levels.WARN, { title = "todoist-nvim" })
		end
	end, o)
	vim.keymap.set("n", "<Tab>", function()
		redraw(b, nav.enter(b))
	end, o)
	vim.keymap.set("n", "<S-Tab>", function()
		redraw(b, nav.back())
	end, o)
	vim.keymap.set("n", "<M-x>", function()
		toggle(b)
	end, o)
	vim.keymap.set("n", "<CR>", function()
		if vim.fn.foldlevel(vim.fn.line(".")) > 0 then
			vim.cmd("normal! za")
		end
	end, o)
	vim.api.nvim_create_autocmd("BufWriteCmd", {
		buffer = b,
		callback = function()
			vim.bo[b].modified = false
			if nav.current_view() ~= nav.VIEW.COMPLETED then
				M.sync()
			end
		end,
	})
end

function M._fill_active_buffer(lines)
	local b = buf(active_name) or mkbuf(active_name)
	if not keymaps[b] then
		keymap(b)
		live_paint(b)
		keymaps[b] = true
	end
	nav.load(lines)
	nav.reset()
	local r = nav.lines()
	put_lines(b, r.lines)
	meta.attach(b, r.meta)
	focus(b)
	hi()
	paint(b)
	set_conceal(b)
	vim.api.nvim_win_set_cursor(0, { 1, 0 })
end

function M.open()
	local exe = bin()
	if not exe then
		vim.notify(
			"todoist-nvim: binary not found. Run: cargo build --release",
			vim.log.levels.ERROR,
			{ title = "todoist-nvim" }
		)
		return
	end
	local out, err = {}, {}
	vim.notify("Fetching tasks…", vim.log.levels.INFO, { title = "todoist-nvim" })
	vim.fn.jobstart({ exe, "fetch" }, {
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

function M.completed(existing)
	local exe = bin()
	if not exe then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local b = existing or buf(active_name)
	if not b then
		vim.notify("Open :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	local out, err = {}, {}
	vim.notify("Fetching completed tasks…", vim.log.levels.INFO, { title = "todoist-nvim" })
	vim.fn.jobstart({ exe, "completed" }, {
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
				local r = nav.enter_completed()
				put_lines(b, r.lines)
				meta.attach(b, r.meta)
				focus(b)
				hi()
				paint(b)
				set_conceal(b)
				vim.api.nvim_win_set_cursor(0, { 1, 0 })
			end)
		end,
	})
end

function M.restore_under_cursor(b)
	local exe = bin()
	if not exe then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local id = tid(b)
	if not id then
		vim.notify("No task ID found on this line.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	local err = {}
	vim.notify("Restoring task " .. id .. "…", vim.log.levels.INFO, { title = "todoist-nvim" })
	vim.fn.jobstart({ exe, "reopen", id }, {
		stdout_buffered = true,
		stderr_buffered = true,
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
					M.completed(b)
					vim.defer_fn(M.open, 300)
				end, 300)
			end)
		end,
	})
end

function M.sync()
	if syncing then
		vim.notify("Sync already in progress…", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	local exe = bin()
	if not exe then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	local b = buf(active_name)
	if not b then
		vim.notify("No active Todoist buffer. Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	if nav.current_view() == nav.VIEW.COMPLETED then
		vim.notify(
			"Cannot sync from completed view. Use <BS> to go back.",
			vim.log.levels.WARN,
			{ title = "todoist-nvim" }
		)
		return
	end
	local lines = vim.api.nvim_buf_get_lines(b, 0, -1, false)
	if #lines == 0 then
		vim.notify("Buffer is empty.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	local tmp = vim.fn.tempname()
	vim.fn.writefile(meta.serialize_lines(b, lines), tmp)
	syncing = true
	vim.notify("Syncing…", vim.log.levels.INFO, { title = "todoist-nvim" })
	local out, err = {}, {}
	vim.fn.jobstart({ exe, "sync", tmp }, {
		stdout_buffered = true,
		stderr_buffered = true,
		on_stdout = function(_, d)
			out = d
		end,
		on_stderr = function(_, d)
			err = d
		end,
		on_exit = function(_, code)
			syncing = false
			vim.fn.delete(tmp)
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
				vim.defer_fn(M.open, 500)
			end)
		end,
	})
end

function M.create_project()
	if creating then
		vim.notify("Project creation already in progress…", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end
	local exe = bin()
	if not exe then
		vim.notify("todoist-nvim: binary not found.", vim.log.levels.ERROR, { title = "todoist-nvim" })
		return
	end
	vim.ui.input({ prompt = "New project name: " }, function(name)
		if not name or name:match("^%s*$") then
			return
		end
		creating = true
		local err = {}
		vim.notify("Creating project…", vim.log.levels.INFO, { title = "todoist-nvim" })
		vim.fn.jobstart({ exe, "create-project", name }, {
			stdout_buffered = true,
			stderr_buffered = true,
			on_stderr = function(_, d)
				err = d
			end,
			on_exit = function(_, code)
				creating = false
				if code ~= 0 then
					local msg = table.concat(err, "\n"):gsub("%s+$", "")
					vim.schedule(function()
						vim.notify(
							msg ~= "" and msg or "Failed to create project.",
							vim.log.levels.ERROR,
							{ title = "todoist-nvim" }
						)
					end)
					return
				end
				vim.schedule(function()
					vim.notify("Project created!", vim.log.levels.INFO, { title = "todoist-nvim" })
					vim.defer_fn(M.open, 500)
				end)
			end,
		})
	end)
end

function M.setup(_)
	vim.api.nvim_create_user_command("TodoistOpen", M.open, { nargs = 0 })
	vim.api.nvim_create_user_command("TodoistCompleted", function()
		local b = buf(active_name)
		if not b then
			vim.notify("Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		M.completed(b)
	end, { nargs = 0 })
	vim.api.nvim_create_user_command("TodoistSync", M.sync, { nargs = 0 })
	vim.api.nvim_create_user_command("TodoistCreateProject", M.create_project, { nargs = 0 })
	vim.api.nvim_create_user_command("TodoistRestore", function()
		local b = buf(active_name)
		if not b then
			vim.notify("Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
			return
		end
		if nav.current_view() ~= nav.VIEW.COMPLETED then
			vim.notify(
				"Open completed view first (<localleader>c or :TodoistCompleted).",
				vim.log.levels.WARN,
				{ title = "todoist-nvim" }
			)
			return
		end
		M.restore_under_cursor(b)
	end, { nargs = 0 })
end

return M

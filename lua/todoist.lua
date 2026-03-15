-- lua/todoist.lua  (v0.2 — fetch + sync)

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

-- Применяем conceal ПОСЛЕ того как буфер открыт в окне.
-- conceallevel — оконная опция, окно должно существовать.
local function apply_conceal(buf)
	-- Принудительно перебить автодетект filetype
	vim.bo[buf].filetype = "markdown"

	local win = vim.fn.bufwinid(buf)
	if win ~= -1 then
		vim.wo[win].conceallevel = 2
		vim.wo[win].concealcursor = "nvic"
	end

	-- Запускаем Treesitter для подсветки markdown
	local ok, _ = pcall(vim.treesitter.start, buf, "markdown")
	if not ok then
		-- Если Treesitter недоступен — fallback на встроенный syntax
		vim.api.nvim_buf_call(buf, function()
			vim.cmd("setlocal syntax=markdown")
		end)
	end

	-- Скрываем <!-- id:XXX --> / <!-- project:XXX --> / <!-- section:XXX -->
	vim.api.nvim_buf_call(buf, function()
		vim.cmd("syntax match TodoistMeta /\\s*<!--[^-]*-->/ conceal")
	end)
end

local function create_scratch_buffer()
	local buf = vim.api.nvim_create_buf(true, true)
	vim.api.nvim_buf_set_name(buf, BUFFER_NAME)

	vim.bo[buf].buftype = "nofile"
	vim.bo[buf].bufhidden = "hide"
	vim.bo[buf].swapfile = false
	vim.bo[buf].filetype = "markdown"
	-- modifiable=true — пользователь редактирует буфер перед sync

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
	focus_buffer(buf) -- сначала фокус (создаёт окно)
	apply_conceal(buf) -- потом conceal (окно уже есть)
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

	local buf = find_existing_buffer()
	if not buf then
		vim.notify("No Todoist buffer found. Run :TodoistOpen first.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end

	local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)

	if #lines == 0 then
		vim.notify("Buffer is empty — nothing to sync.", vim.log.levels.WARN, { title = "todoist-nvim" })
		return
	end

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
				vim.notify(summary, vim.log.levels.INFO, { title = "todoist-nvim sync" })
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

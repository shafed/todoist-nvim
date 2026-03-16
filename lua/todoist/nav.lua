-- lua/todoist/nav.lua
-- Navigation and folding for todoist-nvim.
-- No Neovim fold objects are used; collapsed state drives re-rendering.

local M = {}

-- ─── View constants ──────────────────────────────────────────────────────────────

M.VIEW = {
	ALL_PROJECTS = "all_projects",
	SINGLE_PROJECT = "single_project",
	SINGLE_SECTION = "single_section",
	SINGLE_TASK = "single_task",
}
local V = M.VIEW

-- ─── State ───────────────────────────────────────────────────────────────────────

local state = {
	view = V.ALL_PROJECTS,
	history = {}, -- stack of { view, ctx }
	ctx = {}, -- { project_id, section_id, task_id }
	collapsed = false,
}

-- ─── Data cache ───────────────────────────────────────────────────────────────

-- projects = [{id, name, sections=[{id,name,tasks=[…]}], tasks=[…]}]
-- Each task = {id, content, checked, indent, subtasks=[…]}
local cache = { projects = {} }

-- ─── Parsing helpers ───────────────────────────────────────────────────────────

local function extract_id(line, key)
	return line:match("<!%-%- " .. key .. ":(%S-) %-%->")
end

local function strip_comment(s)
	return (s:gsub("%s*<!%-%-.-%-%->%s*$", "")):match("^%s*(.-)%s*$")
end

local function leading_spaces(line)
	return #(line:match("^(%s*)"))
end

-- ─── Hierarchy loader ───────────────────────────────────────────────────────────

function M.load(lines)
	local projects = {}
	local cur_proj = nil
	local cur_sec = nil

	for _, line in ipairs(lines) do
		if line:match("^# ") and not line:match("^## ") then
			local pid = extract_id(line, "project")
			if pid then
				local name = strip_comment(line:sub(3))
				cur_proj = { id = pid, name = name, sections = {}, tasks = {} }
				table.insert(projects, cur_proj)
				cur_sec = nil
			end
		elseif line:match("^## ") and not line:match("^### ") then
			local sid = extract_id(line, "section")
			if sid and cur_proj then
				local name = strip_comment(line:sub(4))
				cur_sec = { id = sid, name = name, project_id = cur_proj.id, tasks = {} }
				table.insert(cur_proj.sections, cur_sec)
			end
		elseif line:match("^%s*%- %[.%]%s") then
			local tid = extract_id(line, "id")
			local checked = line:match("%- %[x%]") ~= nil or line:match("%- %[X%]") ~= nil
			local raw = line:match("^%s*%- %[.%]%s(.+)$") or ""
			local content = strip_comment(raw)
			local indent = leading_spaces(line)
			if tid and cur_proj then
				local task = {
					id = tid,
					content = content,
					checked = checked,
					indent = indent,
					subtasks = {},
				}
				if indent == 0 then
					local tbl = cur_sec and cur_sec.tasks or cur_proj.tasks
					table.insert(tbl, task)
				else
					local tbl = cur_sec and cur_sec.tasks or cur_proj.tasks
					local parent = nil
					for j = #tbl, 1, -1 do
						if tbl[j].indent < indent then
							parent = tbl[j]
							break
						end
					end
					if parent then
						table.insert(parent.subtasks, task)
					else
						table.insert(tbl, task)
					end
				end
			end
		end
	end

	cache.projects = projects
end

-- ─── Finders ────────────────────────────────────────────────────────────────────

local function find_project(pid)
	for _, p in ipairs(cache.projects) do
		if p.id == pid then return p end
	end
end

local function find_section(proj, sid)
	for _, s in ipairs(proj.sections) do
		if s.id == sid then return s end
	end
end

local function find_task_anywhere(proj, tid)
	for _, t in ipairs(proj.tasks) do
		if t.id == tid then return t, nil end
		for _, sub in ipairs(t.subtasks) do
			if sub.id == tid then return sub, nil end
		end
	end
	for _, sec in ipairs(proj.sections) do
		for _, t in ipairs(sec.tasks) do
			if t.id == tid then return t, sec end
			for _, sub in ipairs(t.subtasks) do
				if sub.id == tid then return sub, sec end
			end
		end
	end
end

local function find_project_by_section(sid)
	for _, p in ipairs(cache.projects) do
		for _, s in ipairs(p.sections) do
			if s.id == sid then return p, s end
		end
	end
end

local function find_project_by_task(tid)
	for _, p in ipairs(cache.projects) do
		local t, sec = find_task_anywhere(p, tid)
		if t then return p, sec, t end
	end
end

-- ─── Helpers ───────────────────────────────────────────────────────────────────

local function fmt_task(task, extra_indent)
	extra_indent = extra_indent or ""
	local check = task.checked and "x" or " "
	return extra_indent .. "- [" .. check .. "] " .. task.content .. " <!-- id:" .. task.id .. " -->"
end

-- Insert a blank line only when not collapsed
local function blank(out)
	if not state.collapsed then
		table.insert(out, "")
	end
end

-- ─── Renderers ───────────────────────────────────────────────────────────────────

local function render_all_projects()
	local out, collapsed = {}, state.collapsed
	for _, proj in ipairs(cache.projects) do
		table.insert(out, "# " .. proj.name .. " <!-- project:" .. proj.id .. " -->")
		if not collapsed then
			if #proj.tasks > 0 then
				blank(out)
				for _, t in ipairs(proj.tasks) do
					table.insert(out, fmt_task(t))
					for _, sub in ipairs(t.subtasks) do
						table.insert(out, fmt_task(sub, "    "))
					end
				end
			end
			for _, sec in ipairs(proj.sections) do
				blank(out)
				table.insert(out, "## " .. sec.name .. " <!-- section:" .. sec.id .. " -->")
				for _, t in ipairs(sec.tasks) do
					table.insert(out, fmt_task(t))
					for _, sub in ipairs(t.subtasks) do
						table.insert(out, fmt_task(sub, "    "))
					end
				end
			end
		end
		blank(out)
	end
	return out
end

local function render_single_project(proj)
	local out, collapsed = {}, state.collapsed
	table.insert(out, "# " .. proj.name .. " <!-- project:" .. proj.id .. " -->")
	blank(out)
	if not collapsed then
		if #proj.tasks > 0 then
			for _, t in ipairs(proj.tasks) do
				table.insert(out, fmt_task(t))
				for _, sub in ipairs(t.subtasks) do
					table.insert(out, fmt_task(sub, "    "))
				end
			end
			blank(out)
		end
		for _, sec in ipairs(proj.sections) do
			table.insert(out, "## " .. sec.name .. " <!-- section:" .. sec.id .. " -->")
			for _, t in ipairs(sec.tasks) do
				table.insert(out, fmt_task(t))
				for _, sub in ipairs(t.subtasks) do
					table.insert(out, fmt_task(sub, "    "))
				end
			end
			blank(out)
		end
	end
	return out
end

local function render_single_section(sec, proj)
	local out, collapsed = {}, state.collapsed
	table.insert(out, "# " .. proj.name .. " <!-- project:" .. proj.id .. " -->")
	table.insert(out, "## " .. sec.name .. " <!-- section:" .. sec.id .. " -->")
	blank(out)
	if not collapsed then
		for _, t in ipairs(sec.tasks) do
			table.insert(out, fmt_task(t))
			for _, sub in ipairs(t.subtasks) do
				table.insert(out, fmt_task(sub, "    "))
			end
		end
	end
	return out
end

local function render_single_task(task, proj)
	local out, collapsed = {}, state.collapsed
	table.insert(out, "# " .. proj.name .. " <!-- project:" .. proj.id .. " -->")
	blank(out)
	table.insert(out, fmt_task(task))
	if not collapsed and #task.subtasks > 0 then
		blank(out)
		table.insert(out, "### Subtasks")
		blank(out)
		for _, sub in ipairs(task.subtasks) do
			table.insert(out, fmt_task(sub, "    "))
		end
	end
	return out
end

local function render_current()
	local ctx = state.ctx
	if state.view == V.ALL_PROJECTS then
		return render_all_projects()
	elseif state.view == V.SINGLE_PROJECT then
		local proj = find_project(ctx.project_id)
		return proj and render_single_project(proj) or { "(project not found)" }
	elseif state.view == V.SINGLE_SECTION then
		local proj = find_project(ctx.project_id)
		local sec = proj and find_section(proj, ctx.section_id)
		return sec and render_single_section(sec, proj) or { "(section not found)" }
	elseif state.view == V.SINGLE_TASK then
		local proj = find_project(ctx.project_id)
		local task = proj and find_task_anywhere(proj, ctx.task_id)
		return task and render_single_task(task, proj) or { "(task not found)" }
	end
	return {}
end

-- ─── Item under cursor ───────────────────────────────────────────────────────────

local function cursor_item(buf)
	local row = vim.api.nvim_win_get_cursor(0)[1]
	local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""
	return {
		project_id = extract_id(line, "project"),
		section_id = extract_id(line, "section"),
		task_id = extract_id(line, "id"),
	}
end

-- ─── Public navigation API ────────────────────────────────────────────────────────

function M.reset()
	state.view = V.ALL_PROJECTS
	state.history = {}
	state.ctx = {}
	state.collapsed = false
end

function M.lines()
	return render_current()
end

function M.enter(buf)
	local item = cursor_item(buf)
	state.collapsed = false

	local function push(new_view, new_ctx)
		table.insert(state.history, { view = state.view, ctx = vim.deepcopy(state.ctx) })
		state.view = new_view
		state.ctx = new_ctx
		return render_current()
	end

	if item.task_id then
		local project_id = state.ctx.project_id
		if not project_id then
			local proj = find_project_by_task(item.task_id)
			project_id = proj and proj.id
		end
		if project_id then
			if state.view == V.SINGLE_TASK and state.ctx.task_id == item.task_id then
				return nil
			end
			return push(V.SINGLE_TASK, {
				project_id = project_id,
				section_id = state.ctx.section_id,
				task_id = item.task_id,
			})
		end
	end

	if item.section_id then
		local project_id = state.ctx.project_id
		if not project_id then
			local proj = find_project_by_section(item.section_id)
			project_id = proj and proj.id
		end
		if project_id then
			if state.view == V.SINGLE_SECTION and state.ctx.section_id == item.section_id then
				return nil
			end
			return push(V.SINGLE_SECTION, {
				project_id = project_id,
				section_id = item.section_id,
			})
		end
	end

	if item.project_id then
		if state.view == V.SINGLE_PROJECT and state.ctx.project_id == item.project_id then
			return nil
		end
		return push(V.SINGLE_PROJECT, { project_id = item.project_id })
	end

	return nil
end

function M.back()
	if #state.history == 0 then return nil end
	local prev = table.remove(state.history)
	state.view = prev.view
	state.ctx = prev.ctx
	state.collapsed = false
	return render_current()
end

function M.fold()
	state.collapsed = true
	return render_current()
end

function M.unfold()
	state.collapsed = false
	return render_current()
end

function M.label()
	local labels = {
		[V.ALL_PROJECTS] = "All Projects",
		[V.SINGLE_PROJECT] = "Project",
		[V.SINGLE_SECTION] = "Section",
		[V.SINGLE_TASK] = "Task",
	}
	return labels[state.view] or state.view
end

return M

-- lua/todoist/nav.lua
-- Navigation for todoist-nvim.
-- Folding is handled natively by Neovim (foldmethod=expr on the window).

local meta = require("todoist.meta")

local M = {}

M.VIEW = {
    ALL_PROJECTS   = "all_projects",
    SINGLE_PROJECT = "single_project",
    SINGLE_SECTION = "single_section",
    SINGLE_TASK    = "single_task",
    COMPLETED      = "completed",
}
local V = M.VIEW

local state = {
    view = V.ALL_PROJECTS,
    history = {},
    ctx = {},
}

local cache = { projects = {}, completed = {} }

local function extract_id(line, key)
    return line:match("<!%-%- " .. key .. ":(%S+)")
end

local function strip_comment(s)
    return (s:gsub("%s*<!%-%-.-%-%->%s*$", "")):match("^%s*(.-)%s*$")
end

local function leading_spaces(line)
    return #(line:match("^(%s*)"))
end

local function find_project(pid)
    for _, p in ipairs(cache.projects) do
        if p.id == pid then
            return p
        end
    end
end

local function find_section(proj, sid)
    for _, s in ipairs(proj.sections) do
        if s.id == sid then
            return s
        end
    end
end

local function find_task_in_list(tasks, tid)
    for _, task in ipairs(tasks) do
        if task.id == tid then
            return task
        end
        local sub = find_task_in_list(task.subtasks or {}, tid)
        if sub then
            return sub
        end
    end
end

local function find_task_anywhere(proj, tid)
    local task = find_task_in_list(proj.tasks, tid)
    if task then
        return task, nil
    end
    for _, sec in ipairs(proj.sections) do
        local sec_task = find_task_in_list(sec.tasks, tid)
        if sec_task then
            return sec_task, sec
        end
    end
end

local function find_project_by_section(sid)
    for _, proj in ipairs(cache.projects) do
        for _, sec in ipairs(proj.sections) do
            if sec.id == sid then
                return proj, sec
            end
        end
    end
end

local function find_project_by_task(tid)
    for _, proj in ipairs(cache.projects) do
        local task, sec = find_task_anywhere(proj, tid)
        if task then
            return proj, sec, task
        end
    end
end

local function new_render()
    return { lines = {}, meta = {} }
end

local function push_line(render, line, row_meta)
    table.insert(render.lines, line)
    if row_meta then
        render.meta[#render.lines] = row_meta
    end
end

local function fmt_task_line(task, extra_indent)
    extra_indent = extra_indent or ""
    local check = task.checked and "x" or " "
    return extra_indent .. "- [" .. check .. "] " .. task.content
end

local function emit_task(render, task, depth)
    local indent = string.rep("    ", depth or 0)
    push_line(render, fmt_task_line(task, indent), { kind = "task", todoist_id = task.id })
    for _, sub in ipairs(task.subtasks or {}) do
        emit_task(render, sub, (depth or 0) + 1)
    end
end

local function emit_tasks(render, tasks)
    for _, task in ipairs(tasks) do
        emit_task(render, task, 0)
    end
end

function M.load(lines)
    local projects = {}
    local cur_proj = nil
    local cur_sec = nil

    for _, line in ipairs(lines) do
        if line:match("^## ") and not line:match("^### ") then
            local pid = extract_id(line, "project")
            if pid then
                local name = strip_comment(line:sub(4))
                cur_proj = { id = pid, name = name, sections = {}, tasks = {} }
                table.insert(projects, cur_proj)
                cur_sec = nil
            end
        elseif line:match("^### ") and not line:match("^#### ") then
            local sid = extract_id(line, "section")
            if sid and cur_proj then
                local name = strip_comment(line:sub(5))
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
                local task = { id = tid, content = content, checked = checked, indent = indent, subtasks = {} }
                local bucket = cur_sec and cur_sec.tasks or cur_proj.tasks
                if indent == 0 then
                    table.insert(bucket, task)
                else
                    local parent = nil
                    local function find_parent(tasks)
                        for i = #tasks, 1, -1 do
                            local candidate = tasks[i]
                            if candidate.indent < indent then
                                parent = candidate
                                return true
                            end
                            if find_parent(candidate.subtasks or {}) then
                                return true
                            end
                        end
                        return false
                    end
                    find_parent(bucket)
                    if parent then
                        table.insert(parent.subtasks, task)
                    else
                        table.insert(bucket, task)
                    end
                end
            end
        end
    end

    cache.projects = projects
end

function M.load_completed(lines)
    local tasks = {}
    for _, line in ipairs(lines) do
        if line:match("^%s*%- %[[xX]%]") then
            local tid = extract_id(line, "id")
            if tid then
                local raw = line:match("^%s*%- %[[xX]%]%s(.+)$") or ""
                local content = strip_comment(raw)
                table.insert(tasks, { id = tid, content = content, checked = true })
            end
        end
    end
    cache.completed = tasks
end

local function render_all_projects()
    local render = new_render()
    for _, proj in ipairs(cache.projects) do
        push_line(render, "## " .. proj.name, { kind = "project", todoist_id = proj.id })
        if #proj.tasks > 0 then
            push_line(render, "")
            emit_tasks(render, proj.tasks)
        end
        for _, sec in ipairs(proj.sections) do
            push_line(render, "")
            push_line(render, "### " .. sec.name, { kind = "section", todoist_id = sec.id })
            push_line(render, "")
            emit_tasks(render, sec.tasks)
        end
        push_line(render, "")
    end
    return render
end

local function render_single_project(proj)
    local render = new_render()
    push_line(render, "## " .. proj.name, { kind = "project", todoist_id = proj.id })
    if #proj.tasks > 0 then
        push_line(render, "")
        emit_tasks(render, proj.tasks)
    end
    for _, sec in ipairs(proj.sections) do
        push_line(render, "")
        push_line(render, "### " .. sec.name, { kind = "section", todoist_id = sec.id })
        push_line(render, "")
        emit_tasks(render, sec.tasks)
    end
    return render
end

local function render_single_section(sec, proj)
    local render = new_render()
    push_line(render, "## " .. proj.name, { kind = "project", todoist_id = proj.id })
    push_line(render, "### " .. sec.name, { kind = "section", todoist_id = sec.id })
    push_line(render, "")
    emit_tasks(render, sec.tasks)
    return render
end

local function render_single_task(task, proj)
    local render = new_render()
    push_line(render, "## " .. proj.name, { kind = "project", todoist_id = proj.id })
    push_line(render, "")
    emit_task(render, task, 0)
    if #(task.subtasks or {}) > 0 then
        push_line(render, "")
        push_line(render, "#### Subtasks")
        push_line(render, "")
        for _, sub in ipairs(task.subtasks) do
            emit_task(render, sub, 1)
        end
    end
    return render
end

local function render_completed()
    local render = new_render()
    push_line(render, "## Completed Tasks (last 30 days)")
    push_line(render, "")
    for _, task in ipairs(cache.completed) do
        push_line(render, "- [x] " .. task.content, { kind = "task", todoist_id = task.id })
    end
    if #cache.completed == 0 then
        push_line(render, "(no completed tasks found)")
    end
    return render
end

local function not_found(kind)
    return { lines = { "(" .. kind .. " not found)" }, meta = {} }
end

local function render_current()
    local ctx = state.ctx
    if state.view == V.ALL_PROJECTS then
        return render_all_projects()
    elseif state.view == V.SINGLE_PROJECT then
        local proj = find_project(ctx.project_id)
        return proj and render_single_project(proj) or not_found("project")
    elseif state.view == V.SINGLE_SECTION then
        local proj = find_project(ctx.project_id)
        local sec = proj and find_section(proj, ctx.section_id)
        return sec and render_single_section(sec, proj) or not_found("section")
    elseif state.view == V.SINGLE_TASK then
        local proj = find_project(ctx.project_id)
        local task = proj and find_task_anywhere(proj, ctx.task_id)
        return task and render_single_task(task, proj) or not_found("task")
    elseif state.view == V.COMPLETED then
        return render_completed()
    end
    return new_render()
end

local function cursor_item(buf)
    local row = vim.api.nvim_win_get_cursor(0)[1]
    local entry = meta.get_at_row(buf, row)
    if not entry then
        return {}
    end
    if entry.kind == "project" then
        return { project_id = entry.todoist_id }
    elseif entry.kind == "section" then
        return { section_id = entry.todoist_id }
    elseif entry.kind == "task" then
        return { task_id = entry.todoist_id }
    end
    return {}
end

function M.reset()
    state.view = V.ALL_PROJECTS
    state.history = {}
    state.ctx = {}
end

function M.lines()
    return render_current()
end

function M.full_lines()
    return render_all_projects()
end

function M.current_view()
    return state.view
end

function M.enter_completed()
    table.insert(state.history, { view = state.view, ctx = vim.deepcopy(state.ctx) })
    state.view = V.COMPLETED
    state.ctx = {}
    return render_completed()
end

function M.enter(buf)
    local item = cursor_item(buf)
    if state.view == V.COMPLETED then
        return nil
    end

    local function push(new_view, new_ctx)
        table.insert(state.history, { view = state.view, ctx = vim.deepcopy(state.ctx) })
        state.view = new_view
        state.ctx = new_ctx
        return render_current()
    end

    if item.task_id then
        local project_id = state.ctx.project_id
        local section_id = state.ctx.section_id
        if not project_id then
            local proj, sec = find_project_by_task(item.task_id)
            project_id = proj and proj.id
            section_id = sec and sec.id or section_id
        end
        if project_id then
            if state.view == V.SINGLE_TASK and state.ctx.task_id == item.task_id then
                return nil
            end
            return push(V.SINGLE_TASK, {
                project_id = project_id,
                section_id = section_id,
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
    if #state.history == 0 then
        return nil
    end
    local prev = table.remove(state.history)
    state.view = prev.view
    state.ctx = prev.ctx
    return render_current()
end

function M.label()
    local labels = {
        [V.ALL_PROJECTS] = "All Projects",
        [V.SINGLE_PROJECT] = "Project",
        [V.SINGLE_SECTION] = "Section",
        [V.SINGLE_TASK] = "Task",
        [V.COMPLETED] = "Completed",
    }
    return labels[state.view] or state.view
end

return M

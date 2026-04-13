local M = {}

local NS = vim.api.nvim_create_namespace("todoist_line_meta")
local store = {}

local function ensure_store(buf)
	store[buf] = store[buf] or {}
	return store[buf]
end

function M.clear(buf)
	vim.api.nvim_buf_clear_namespace(buf, NS, 0, -1)
	store[buf] = {}
end

function M.attach(buf, rows)
	M.clear(buf)
	if not rows then
		return
	end

	local buf_store = ensure_store(buf)
	for row, entry in pairs(rows) do
		if entry then
			local mark_id = vim.api.nvim_buf_set_extmark(buf, NS, row - 1, 0, {
				right_gravity = false,
				strict = false,
			})
			buf_store[mark_id] = vim.deepcopy(entry)
		end
	end
end

function M.get_at_row(buf, row)
	if row < 1 then
		return nil
	end
	local marks = vim.api.nvim_buf_get_extmarks(buf, NS, { row - 1, 0 }, { row - 1, -1 }, {})
	if #marks == 0 then
		return nil
	end
	local mark_id = marks[1][1]
	local buf_store = ensure_store(buf)
	local entry = buf_store[mark_id]
	return entry and vim.deepcopy(entry) or nil
end

function M.anchor_for_row(buf, row)
	local entry = M.get_at_row(buf, row)
	if not entry or not entry.kind or not entry.todoist_id then
		return nil
	end
	return entry.kind .. ":" .. entry.todoist_id
end

function M.find_row_by_anchor(buf, anchor)
	if not anchor then
		return nil
	end

	local want_kind, want_id = anchor:match("^(.-):(.+)$")
	if not want_kind or not want_id then
		return nil
	end

	local marks = vim.api.nvim_buf_get_extmarks(buf, NS, 0, -1, {})
	local buf_store = ensure_store(buf)

	for _, mark in ipairs(marks) do
		local mark_id, row0 = mark[1], mark[2]
		local entry = buf_store[mark_id]
		if entry and entry.kind == want_kind and entry.todoist_id == want_id then
			return row0 + 1
		end
	end

	return nil
end

local function append_comment(line, entry)
	if not entry or not entry.kind or not entry.todoist_id then
		return line
	end

	local suffix
	if entry.kind == "task" then
		suffix = string.format("<!-- id:%s -->", entry.todoist_id)
	elseif entry.kind == "project" then
		suffix = string.format("<!-- project:%s -->", entry.todoist_id)
	elseif entry.kind == "section" then
		suffix = string.format("<!-- section:%s -->", entry.todoist_id)
	else
		return line
	end

	if line:match("%S") then
		return line:gsub("%s*$", "") .. " " .. suffix
	end
	return line
end

function M.serialize_lines(buf, lines)
	local out = {}
	for row, line in ipairs(lines) do
		out[row] = append_comment(line, M.get_at_row(buf, row))
	end
	return out
end

return M

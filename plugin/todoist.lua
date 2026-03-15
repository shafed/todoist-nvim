-- plugin/todoist.lua
--
-- This file is auto-sourced by Neovim when the plugin directory is on
-- runtimepath. It calls setup() with no arguments so the plugin works
-- out-of-the-box without requiring an explicit setup() call in user config.
--
-- If the user calls require("todoist").setup(opts) themselves (e.g. inside
-- lazy.nvim's `config` function), that's fine — setup() is idempotent.

if vim.g.loaded_todoist_nvim then
	return
end
vim.g.loaded_todoist_nvim = true

require("todoist").setup()

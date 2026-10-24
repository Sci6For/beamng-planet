local M = {}
local http = require("socket.http")

local function set(server_name)
  local _, _, _  = http.request("http://127.0.0.1:3693/rich_presence/"..server_name)
  Steam.setRichPresence("b", "KissMP - "..server_name)
end

local function clear()
  local _, _, _  = http.request("http://127.0.0.1:3693/rich_presence/none")
  Steam.clearRichPresence()
end

M.set = set
M.clear = clear

return M

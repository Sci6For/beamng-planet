local M = {}

local http = require("socket.http")
http.TIMEOUT = 0.05

local first_update = false
local planetManaged = false

M.bridge_connected = false
M.incorrect_install = true
M.install_path = ""

local extension_load_list = {
  -- network first
  "kissmp_network",

  -- other important client stuff
  "kissmp_mods",
  "kissmp_config",

  -- ui goes last
  "kissmp_ui",
}

local extensions_connected_list = {
  "kissmp_transform",
  "kissmp_levelmanager",
  "kissmp_players",
  "kissmp_vehiclemanager",
  "kissmp_voicechat",
}

local function check_bridge_connect()
  local b, _, _  = http.request("http://127.0.0.1:3693/check")
  if b and b == "ok" then
    M.bridge_launched = true
  else
    M.bridge_launched = false
  end

  return M.bridge_launched
end

local function onUpdate()
  if planetManaged then return end
  if first_update then return end
  first_update = true

  -- verify mod install
  local filepath = FS:findOverrides("/lua/ge/extensions/kissmp/main.lua")
  if filepath and filepath[1] then
    filepath = filepath[1]:gsub("\\", "/"):match("/mods.+")
    M.install_path = filepath

    local modname = string.lower(filepath)
    modname = modname:gsub('dir:/', '')
    modname = modname:gsub('/mods/', '')
    modname = modname:gsub('repo/', '')
    modname = modname:gsub('unpacked/', '')
    modname = modname:gsub('/', '')
    modname = modname:gsub('.zip$', '')
    M.install_name = modname

    M.incorrect_install = false
  else
    M.incorrect_install = true
  end

  -- load extensions
  loadJsonMaterialsFile("/art/kissmp/playermodels/main.materials.json")
  for i=1, #extension_load_list do
    extensions.load(extension_load_list[i])
  end

  -- check if bridge works
  check_bridge_connect()

  -- run global init hook
  extensions.hook("onKissMPLoaded")
end

local function load_connected_extensions()
  for i=1, #extensions_connected_list do
    extensions.load(extensions_connected_list[i])
  end
end

local function unload_connected_extensions()
  for i=1, #extensions_connected_list do
    extensions.unload(extensions_connected_list[i])
  end
end

local function onExtensionUnloaded()
  for i=1, #extensions_connected_list do
    extensions.unload(extensions_connected_list[i])
  end
  for i=1, #extension_load_list do
    extensions.unload(extension_load_list[i])
  end
end

M.check_bridge_connect = check_bridge_connect
M.load_connected_extensions = load_connected_extensions
M.unload_connected_extensions = unload_connected_extensions

M.onUpdate = onUpdate
M.onExtensionUnloaded = onExtensionUnloaded
M.onExtensionLoaded = function()
  setExtensionUnloadMode(M, "manual")
end

function M.planetConnect(config)
  planetManaged=true
  if not kissmp_network then extensions.load('kissmp_network') end
  if not kissmp_network then return nil,'networkUnavailable' end
  return kissmp_network.planetConnect(config)
end
function M.planetDisconnect(reason)
  if kissmp_network then return kissmp_network.planetDisconnect(reason) end
end
return M

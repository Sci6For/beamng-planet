local M = {}
local function planetManaged() return kissmp_network and kissmp_network.isPlanetManaged and kissmp_network.isPlanetManaged() end

local current_level = ""
local level_connected = false

M.is_loading = false
local function load_level(filename)
  if planetManaged() then return false,'managedLevelOwnedByPlanet' end
  if FS:fileExists(filename) then
    M.is_loading = true
    freeroam_freeroam.startFreeroam(filename)
  else
    log("E", "kissmp_levelmanager.load_level", "Level does not exist!")
    kissmp_network.disconnect()
  end
end

local function join_current_level()
  load_level(current_level)
end

local function onKissMPConnected(delay_level_load, server_info)
  if planetManaged() then return end
  current_level = server_info.map
  if not delay_level_load then
    join_current_level()
  end
end

local function onKissMPDisconnected()
  if planetManaged() then return end
  if level_connected or M.is_loading then
    M.is_loading = false
    level_connected = false
    returnToMainMenu()
  end
end

local function onClientEndMission()
  if planetManaged() then return end
  if level_connected then
    level_connected = false
    kissmp_network.disconnect()
  end
end

local function onClientPostStartMission()
  M.is_loading = false
  level_connected = true
end

local function onExtensionLoaded()
  setExtensionUnloadMode(M, "manual")
end

M.load_level = load_level

M.onKissMPFinishedDownloads = join_current_level
M.onKissMPConnected = onKissMPConnected
M.onKissMPDisconnected = onKissMPDisconnected

M.onClientEndMission = onClientEndMission
M.onClientPostStartMission = onClientPostStartMission
M.onExtensionLoaded = onExtensionLoaded

return M

local M = {}
local planetActivation = nil
local planetApplying = false
local function planetManaged() return kissmp_network and kissmp_network.isPlanetManaged and kissmp_network.isPlanetManaged() end
function M.planetSetActivationCallback(callback) planetActivation=callback end

local string_buffer = require("string.buffer")

local view_distance = nil

M.raw_transforms = {}
M.received_transforms = {}
M.local_transforms = {}
M.raw_positions = {}
M.inactive = {}

M.threshold = 3
M.rot_threshold = 2.5
M.velocity_error_limit = 10

M.hidden = {}

local transform_pos = vec3()
local camera_pos = vec3()
local function update(dt)
  if planetManaged() then return end
  -- Get rotation/angular velocity from vehicle lua
  for vid, v in vehiclesIterator() do
    if not M.inactive[vid] then
      v:queueLuaCommand("kissmp_vehicle.update_transform_info()")
    end
  end

  -- Don't apply velocity while paused. If we do, velocity gets stored up and released when the game resumes.
  local apply_velocity = not bullettime.getPause()
  camera_pos:set(core_camera.getPositionXYZ())
  for id, transform in pairs(M.received_transforms) do
    --apply_transform(dt, id, transform, apply_velocity)
    local vehicle = getObjectByID(id)
    transform_pos:set(transform.position[1], transform.position[2], transform.position[3])
    if vehicle and apply_velocity and (not kissmp_vehiclemanager.ownership[id]) then
      if view_distance and (transform_pos:squaredDistance(camera_pos) > view_distance) then
        if not M.inactive[id] then
          vehicle:setActive(0)
          M.inactive[id] = true
        end
      else
        if M.inactive[id] then
          vehicle:setActive(1)
          M.inactive[id] = false
        end
        vehicle:queueLuaCommand(string.format(
          "kissmp_transforms.update(%f)",
          dt))
      end
    end
  end
end

local function update_vehicle_transform(data)
  if planetManaged() and not planetApplying then return false,"activationRequired" end
  local transform = data.transform
  transform.owner = data.vehicle_id
  transform.sent_at = data.sent_at

  local id = kissmp_vehiclemanager.id_map[transform.owner or -1] or -1
  if kissmp_vehiclemanager.ownership[id] then return end
  M.raw_positions[transform.owner or -1] = transform.position
  M.received_transforms[id] = transform

  local vehicle = getObjectByID(id)
  transform.time_past = clamp(kissmp_vehiclemanager.get_current_time() - transform.sent_at, 0, 0.1) * 0.9 + 0.001
  if vehicle and (not M.inactive[id]) then
    vehicle:queueLuaCommand(string.format(
      "kissmp_transforms.set_target_transform(%q)",
      string_buffer.encode(transform)))
  end
end

local function push_transform(id, t)
  M.local_transforms[id] = string_buffer.decode(t)
end

local function onKissMPSettingsChanged(config)
  view_distance = config["perf.enable_view_distance"] and config["perf.view_distance"] * config["perf.view_distance"] or nil
end

M.update_vehicle_transform = update_vehicle_transform
M.push_transform = push_transform

M.onUpdate = update
M.onKissMPSettingsChanged = onKissMPSettingsChanged
M.onExtensionLoaded = function()
  setExtensionUnloadMode(M, "manual")
end

function M.planetApplyTransform(data,identity)
  if type(planetActivation)~='function' or not planetActivation(identity) then return false,'activationRequired' end
  planetApplying=true
  local ok,result,err=pcall(update_vehicle_transform,data)
  planetApplying=false
  if not ok then return false,result end
  if result==false then return false,err end
  return true
end
function M.planetResetPredictors(id)
  if not id then M.received_transforms={};M.raw_transforms={};M.local_transforms={};M.raw_positions={} end
end
return M

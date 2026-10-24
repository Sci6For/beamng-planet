local SyncController = {}
SyncController.__index = SyncController

function SyncController.new()
  local self = setmetatable({}, SyncController)
  self.contr = nil

  self.electrics_arm_name = "nitrousOxideArm"
  self.override_electrics = "nitrousOxideOverride"
  return self
end

function SyncController:set_controller(value)
  self.contr = value

  local jbeam_controller_data = v.data.controller[self.contr.name]
  if jbeam_controller_data then
    self.electrics_arm_name = jbeam_controller_data.electricsArmName or self.electrics_arm_name
    self.override_electrics = jbeam_controller_data.electricsOverrideName or self.override_electrics
  end

  return true
end

function SyncController:get()
  return {
    electrics.values[self.electrics_arm_name],
    electrics.values[self.override_electrics]
  }
end

function SyncController:set(data)
  electrics.values[self.electrics_arm_name] = 1 - data[1]
  self.contr.toggleActive()
  if data[2] then
    self.contr.setOverride(data[2] == 1)
  end
end

return SyncController
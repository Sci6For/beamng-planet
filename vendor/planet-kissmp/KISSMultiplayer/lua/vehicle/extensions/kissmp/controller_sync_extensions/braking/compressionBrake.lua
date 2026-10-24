local SyncController = {}
SyncController.__index = SyncController

function SyncController.new()
  local self = setmetatable({}, SyncController)
  self.contr = nil

  self.electrics_name_setting = "mainEngine_compressionBrake_setting"
  return self
end

function SyncController:set_controller(value)
  self.contr = value

  local jbeam_controller_data = v.data.controller[self.contr.name]
  if jbeam_controller_data then
    local engine_name = jbeam_controller_data.controlledEngine or "mainEngine"

    self.electrics_name_setting = jbeam_controller_data.electricsNameSetting or (engine_name .. "_compressionBrake_setting")
  end

  return true
end

function SyncController:get()
  return {
    electrics.values[self.electrics_name_setting] or 0
  }
end

function SyncController:set(data)
  self.contr.setCompressionBrakeCoef(data[1])
end

return SyncController

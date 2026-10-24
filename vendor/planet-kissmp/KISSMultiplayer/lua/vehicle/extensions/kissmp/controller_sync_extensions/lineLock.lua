local SyncController = {}
SyncController.__index = SyncController

function SyncController.new()
  local self = setmetatable({}, SyncController)
  self.contr = nil

  self.electrics_name = "linelock"
  return self
end

function SyncController:set_controller(value)
  self.contr = value

  local jbeam_controller_data = v.data.controller[self.contr.name]
  if jbeam_controller_data then
    self.electrics_name = jbeam_controller_data.electricsName or "linelock"
  end

  return true
end

function SyncController:get()
  return {
    electrics.values[self.electrics_name] or 0
  }
end

function SyncController:set(data)
  self.contr.setLineLock(data[1])
end

return SyncController
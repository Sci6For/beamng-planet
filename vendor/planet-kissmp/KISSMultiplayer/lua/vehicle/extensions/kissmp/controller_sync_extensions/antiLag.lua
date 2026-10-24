local SyncController = {}
SyncController.__index = SyncController

function SyncController.new()
  local self = setmetatable({}, SyncController)
  self.contr = nil
  return self
end

function SyncController:set_controller(value)
  self.contr = value

  return true
end

function SyncController:get()
  return {
    electrics.values.alsState or "idle"
  }
end

function SyncController:set(data)
  self.contr.setAntilagState(data[1])
end

return SyncController
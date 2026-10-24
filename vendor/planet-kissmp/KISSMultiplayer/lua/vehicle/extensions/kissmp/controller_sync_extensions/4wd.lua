local SyncController = {}
SyncController.__index = SyncController

function SyncController.new()
  local self = setmetatable({}, SyncController)
  self.contr = nil
  self.unmapped_data = {}
  return self
end

function SyncController:set_controller(value)
  self.contr = value
  return self.contr.serialize and self.contr.serialize() ~= nil
end

function SyncController:get()
  local data = self.contr and self.contr.serialize()
  return {
    data.mode4WD or 0,
    data.modeRange or 0
  }
end

function SyncController:set(data)
  self.unmapped_data.mode4WD = data[1]
  self.unmapped_data.modeRange = data[2]
  if self.contr then
    self.contr.deserialize(self.unmapped_data)
  end
end

return SyncController
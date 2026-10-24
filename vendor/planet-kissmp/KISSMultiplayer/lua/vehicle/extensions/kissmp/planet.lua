-- Trusted vehicle-side Planet pose adapter. Right-handed body xyz = pitch, roll, yaw.
local M={}
function M.sample(operation,generation,frameEpoch,frameId)
 local e=electrics and electrics.values or {}
 local q={obj:getRotation()}
 local main=controller and controller.mainController;local state=main and main.getState and main.getState() or {}
 local gearbox=powertrain and powertrain.getDevice and powertrain.getDevice('gearbox')
 local p={position={obj:getPositionXYZ()},orientation=q,linear_velocity={obj:getVelocityXYZ()},
  angular_velocity={obj:getPitchAngularVelocity(),obj:getRollAngularVelocity(),obj:getYawAngularVelocity()},
  inputs={e.throttle_input or 0,e.brake_input or 0,e.steering_input or 0,e.parkingbrake_input or 0},
  electrics={throttle_input=e.throttle_input or 0,brake_input=e.brake_input or 0,steering_input=e.steering_input or 0,parkingbrake=e.parkingbrake_input or 0,clutch=e.clutch_input or 0},
  gearbox={lock_coef=gearbox and gearbox.lockCoef or 1,grb_idx=state.grb_idx or 0,grb_mde=state.grb_mde or '',grb_bhv=state.grb_bhv or 'arcade',frzn=state.frzn or false}}
 obj:queueGameEngineLua(string.format('if extensions.beamngEarth_planetMultiplayer then extensions.beamngEarth_planetMultiplayer.onVehiclePose(%d,%q,%q,%q,%q,%q) end',objectId,jsonEncode(p),operation,generation,frameEpoch,frameId))
end
return M

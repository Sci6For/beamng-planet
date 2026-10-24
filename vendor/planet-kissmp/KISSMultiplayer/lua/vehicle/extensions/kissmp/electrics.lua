local M = {}

local string_buffer = require("string.buffer")

local prev_electrics = {}

local ignored_keys = {
  throttle = true,
  throttle_input = true,
  brake = true,
  brake_input = true,
  clutch = true,
  clutch_input = true,
  clutchRatio = true,
  parkingbrake = true,
  parkingbrake_input = true,
  steering = true,
  steering_input = true,
  regenThrottle = true,
  reverse = true,
  parking = true,
  lights = true,
  turnsignal = true,
  hazard = true,
  hazard_enabled = true,
  signal_R = true,
  signal_L = true,
  gear = true,
  gear_M = true,
  gear_A = true,
  gearIndex = true,
  exhaustFlow = true,
  engineLoad = true,
  airspeed = true,
  axle_FL = true,
  airflowspeed = true,
  watertemp = true,
  driveshaft_F = true,
  rpmspin = true,
  wheelspeed = true,
  oil = true,
  rpm = true,
  altitude = true,
  avgWheelAV = true,
  lowpressure = true,
  lowhighbeam = true,
  lowbeam = true,
  highbeam = true,
  oiltemp = true,
  rpmTacho = true,
  axle_FR = true,
  fuel_volume = true,
  driveshaft = true,
  fuel = true,
  engineThrottle = true,
  fuelCapacity = true,
  fuelVolume = true,
  turboSpin = true,
  turboRPM = true,
  turboBoost = true,
  virtualAirspeed = true,
  turboRpmRatio = true,
  lockupClutchRatio = true,
  abs = true,
  absActive = true,
  tcs = true,
  tcsActive = true,
  esc = true,
  escActive = true,
  brakelights = true,
  radiatorFanSpin = true,
  smoothShiftLogicAV = true,
  accXSmooth = true,
  accYSmooth = true,
  accZSmooth = true,
  trip = true,
  odometer = true,
  steeringUnassisted = true,
  boost = true,
  superchargerBoost = true,
  gearModeIndex = true,
  hPatternAxisX = true,
  hPatternAxisY = true,
  tirePressureControl_activeGroupPressure = true,
  reverse_wigwag_L = true,
  reverse_wigwag_R = true,
  highbeam_wigwag_L = true,
  highbeam_wigwag_R = true,
  lowhighbeam_signal_L = true,
  lowhighbeam_signal_R = true,
  brakelight_signal_L = true,
  brakelight_signal_R = true,
  isYCBrakeActive = true,
  isTCBrakeActive = true,
  isABSBrakeActive = true,
  dseWarningPulse = true,
  dseRollingOver = true,
  dseRollOverStopped = true,
  dseCrashStopped = true,
  modeRangeBox = true,
  mode4WD = true,
  hasESC = true,
  hasTCS = true,
  alsActive = true,
  alsState = true,
  airIntake = true,
  ignition = true,
  running = true,
  electricalLoadCoef = true,
  nop = true,
  boostMax = true,
  turboBoostMax = true,
  parkingbrakelight = true,
  brakeGlow_FR = true,
  brakeGlow_FL = true,
  brakeGlow_RR = true,
  brakeGlow_RL = true,
  minGearIndex = true,
  maxGearIndex = true,
  checkengine = true,
}

local electrics_handlers = {
  lights_state = function(v)
    electrics.setLightsState(v)
  end,
  fog = function(v)
    electrics.set_fog_lights(v)
  end,
  lightbar = function(v)
    electrics.set_lightbar_signal(v)
  end,
  horn = function(v)
    electrics.horn(v > 0.5)
  end,
  ignitionLevel = function(v)
    if v == 2 and electrics.values.ignitionLevel == 3 then
      -- this means we're currently starting the engine and it's supposed to run
      return
    end
    electrics.setIgnitionLevel(v)
  end,
  engineRunning = function(v)
    if v == 1 then
      electrics.setIgnitionLevel(3)
      controller.mainController.setEngineIgnition(true)
      controller.mainController.setStarter(true)
    end
  end,
  hasABS = function(v)
    if v > 0.5 then
      wheels.setABSBehavior("realistic")
    else
      wheels.setABSBehavior("off")
    end
  end
}

local function ignore_key(key)
  ignored_keys[key] = true
end

local function send()
  local diff_count = 0
  local data = {
    diff = {}
  }
  for key, value in pairs(electrics.values) do
    if not ignored_keys[key] and type(value) == 'number' then
      if prev_electrics[key] ~= value then
        data.diff[key] = value
        diff_count = diff_count + 1
      end
      prev_electrics[key] = value
    end
  end
  local data = {
    ElectricsUndefinedUpdate = {objectId, data}
  }
  if diff_count > 0 then
    obj:queueGameEngineLua(string.format(
      "kissmp_network.send_data(%q, true)",
      jsonEncode(data)))
  end
end

local function apply_diff_signals(diff)
  local signal_left_input = diff.signal_left_input or electrics.values.signal_left_input or 0
  local signal_right_input = diff.signal_right_input or electrics.values.signal_right_input or 0
  local hazard_enabled = (signal_left_input == 1 and signal_right_input == 1)

  if hazard_enabled then
    electrics.set_warn_signal(true)
  else
    electrics.set_warn_signal(false)
    electrics.set_left_signal(signal_left_input == 1)
    electrics.set_right_signal(signal_right_input == 1)
  end

  diff.signal_left_input = nil
  diff.signal_right_input = nil
end

local function update_advanced_coupler_state(coupler_control_controller, value)
  -- the value indicates "notattached"
  local is_open = value > 0.5
  if not is_open then
    coupler_control_controller.tryAttachGroupImpulse()
  else
    coupler_control_controller.detachGroup()
  end
end

local function apply_diff(buffer_data)
  local diff = string_buffer.decode(buffer_data)
  if diff.signal_left_input or diff.signal_right_input then
    apply_diff_signals(diff)
  end

  for k, v in pairs(diff) do
    local handler = electrics_handlers[k]
    if handler then
      handler(v)
    else
      electrics.values[k] = v
    end
  end
end

local function onKissMPVehLoaded()
  -- Ignore powertrain electrics
  local devices = powertrain.getDevices()
  for _, device in pairs(devices) do
    if device.electricsName and device.visualShaftAngle then
      ignore_key(device.electricsName)
    end
    if device.electricsThrottleName then 
      ignore_key(device.electricsThrottleName)
    end
    if device.electricsThrottleFactorName then
      ignore_key(device.electricsThrottleFactorName)
    end
    if device.electricsClutchRatio1Name then
      ignore_key(device.electricsClutchRatio1Name)
    end
    if device.electricsClutchRatio2Name then
      ignore_key(device.electricsClutchRatio2Name)
    end
  end

  -- Ignore common led electrics
  for i = 0, 10 do
    ignore_key("led"..tostring(i))
  end

  -- Ignore controller electrics
  if v.data.controller and type(v.data.controller) == 'table' then 
    for _, controller_data in pairs(v.data.controller) do
      if controller_data.fileName == "lightbar" and controller_data.modes then
        -- ignore lightbar electrics
        local modes = tableFromHeaderTable(controller_data.modes)
        for _, vm in pairs(modes) do
          local configEntries = tableFromHeaderTable(deepcopy(vm.config))
          for _, j in pairs(configEntries) do
            ignore_key(j.electric)
          end 
        end
      elseif controller_data.fileName == "jato" then
        -- ignore jato fuel
        ignore_key("jatofuel")
      elseif controller_data.fileName == "beaconSpin" then
        -- ignore beacon spin
        ignore_key(controller_data.electricsName or "beaconSpin")
      elseif controller_data.fileName == "twoStepLaunch" then
        -- ignore two step state
        ignore_key(controller_data.electricsName or "twoStep")
      elseif controller_data.fileName == "nitrousOxideInjection" then
        -- ignore nitrous
        ignore_key(controller_data.electricsArmName or "nitrousOxideArm")
        ignore_key(controller_data.electricsOverrideName or "nitrousOxideOverride")
      elseif controller_data.fileName == "lineLock" then
        -- ignore line lock state
        ignore_key(controller_data.electricsName or "linelock")
      elseif controller_data.fileName == "braking/compressionBrake" then
        -- ignore compression brake state
        local engine_name = controller_data.controlledEngine or "mainEngine"

        ignore_key(controller_data.electricsNameActual or (engine_name .. "_compressionBrake_actual"))
        ignore_key(controller_data.electricsNameSetting or (engine_name .. "_compressionBrake_setting"))
        ignore_key(controller_data.electricsNameIsEnabled or (engine_name .. "_compressionBrake_isEnabled"))
        ignore_key(controller_data.electricsNameLevelIndex or (engine_name .. "_compressionBrake_levelIndex"))
      elseif controller_data.fileName == "advancedCouplerControl" then
        -- register handler for syncing advanced couplers
        local electric = controller_data.name .. "_notAttached"
        local coupler_control_controller = controller.getController(controller_data.name)
        electrics_handlers[electric] = function(v) update_advanced_coupler_state(coupler_control_controller, v) end

        -- ignore the related couplers, we'll manage them now
        for _, vn in pairs(tableFromHeaderTable(controller_data.couplerNodes)) do
          local cid1 = beamstate.nodeNameMap[vn.cid1]
          local cid2 = beamstate.nodeNameMap[vn.cid2]

          if cid1 then
            kissmp_couplers.ignore_coupler_node(cid1)
          end

          if cid2 then
            kissmp_couplers.ignore_coupler_node(cid2)
          end
        end
      end
    end
  end

  -- Ignore commonly used disp_* electrics used on vehicles with gear displays
  for k,v in pairs(electrics.values) do
    if type(k) == 'string' and k:startswith("disp_") or k:startswith("auto_") then
      ignored_keys[k] = true
    end
  end

  -- Ignore common extension/controller electrics
  if _G["4ws"] and type(_G["4ws"]) == 'table' then
    ignored_keys["4ws"] = true
  end

  -- Ignore custom JBeam electrics
  local electrics_jbeam = v.data.electrics or v.data.components.electrics
  if electrics_jbeam then
    local jbeamCustomValues = electrics_jbeam.customValues or {}
    for _, value in ipairs(tableFromHeaderTable(jbeamCustomValues)) do
      ignored_keys[value.electricsName] = true
    end
  end

  -- Ignore pneumatic states
  for k,v in ipairs(controller.getControllersByType("pneumatics/airbrakes")) do
    ignored_keys[v.name.."_pressure_service"] = true
    ignored_keys[v.name.."_pressure_parking"] = true
  end

  for k,v in pairs(energyStorage.getStorages()) do
    if v.type == "pressureTank" then
      ignored_keys[v.pressureElectricName] = true
      ignored_keys[v.pressureConsumerElectricName] = true
      ignored_keys[v.pressureConsumerCoefElectricName] = true
      ignored_keys[v.pneumaticPTOConsumerPressureElectricsName] = true
      ignored_keys[v.pneumaticPTOConsumerFlowElectricsName] = true
    end
  end
end

M.send = send
M.apply_diff = apply_diff
M.ignore_key = ignore_key

M.onKissMPVehLoaded = onKissMPVehLoaded

return M

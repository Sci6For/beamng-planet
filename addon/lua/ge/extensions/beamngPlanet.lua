-- Planet chooser, staged drive setup and owned covered launch boundary.
local M = {}
-- Small source pins are checked once by the UI before any route mutation.
-- Do not hash the executable on the UI thread; packaging/install pins it.
function M.checkCompatibility()
  local failure={ok=false,errorCode='unsupportedGameSource',path='compatibility API/pins unavailable'}
  if type(jsonReadFile)~='function' or type(readFile)~='function' or type(hashStringSHA256)~='function' then return failure end
  local ok,pins=pcall(jsonReadFile,'/lua/ge/extensions/beamngPlanet.compatibility.json')
  if not ok or type(pins)~='table' or type(pins.gameFiles)~='table' or not next(pins.gameFiles) then return failure end
  for path,expected in pairs(pins.gameFiles) do
    local read,bytes=pcall(readFile,'/'..path)
    local hashed,actual=false,nil
    if read and type(bytes)=='string' then hashed,actual=pcall(hashStringSHA256,bytes) end
    if not hashed or type(actual)~='string' or actual:lower()~=expected then
      failure.path=path
      return failure
    end
  end
  return {ok=true}
end
-- Hot reload retires the previous transport owner before publishing this one.
local previous = rawget(_G, 'beamngPlanetMapOwner')
if previous and previous.closeChooser then previous.closeChooser() end
rawset(_G, 'beamngPlanetMapOwner', M)
local function ownsMap() return rawget(_G, 'beamngPlanetMapOwner')==M end
local chooser, search = nil, nil
local selectedPoint, vehicle, setup = nil, nil, {}
local coordinator, closing, realClock = nil, false, 0
local retireReady
local sessionState={mode='solo',phase='off'}
local sessionIntent
local selectorSerial = 0
local selectorSelection
local counter = rawget(_G, 'beamngPlanetChooserCounter') or 0
local function ext(name) return extensions and extensions[name] end
local function reply(id, err, ready, results, pending)
  return {requestId=id, readyKeys=ready or {}, results=results or {}, errorCode=err, pending=pending}
end
local function requestId(id) return type(id)=='string' and #id>0 and #id<=128 end
local function point(lat, lon)
  return type(lat)=='number' and type(lon)=='number' and lat==lat and lon==lon and math.abs(lat)<=90 and math.abs(lon)<=180
end
local function tile(key)
  if type(key)~='string' then return nil end
  local layer,zs,xs,ys=key:match('^(%a+)/(%d+)/(%d+)/(%d+)$')
  if layer~='map' and layer~='sat' and layer~='labels' then return nil end
  local z,x,y=tonumber(zs),tonumber(xs),tonumber(ys)
  if not z or z>18 or x>=2^z or y>=2^z then return nil end
  if key~=string.format('%s/%d/%d/%d',layer,z,x,y) then return nil end
  return {z,x,y,layer}
end
local function copy(value)
  if type(value)~='table' then return value end
  local out={};for k,v in pairs(value) do out[k]=copy(v) end;return out
end
function M.getState()
  local session=M.getSessionState()
  return {chooserOpen=chooser~=nil,chooserToken=chooser,selectedPoint=copy(selectedPoint),vehicle=copy(vehicle),setup=copy(setup),session=session,launch=coordinator and coordinator.status() or nil}
end
local function result(ok,code) return {ok=ok,errorCode=code,state=M.getState()} end
local function active()
  local launch=coordinator and coordinator.status()
  return ownsMap() and chooser~=nil and not closing and not (launch and (launch.covered or launch.phase=='ready'))
end
-- Reuse the launch contract on an isolated pure model, never the live core.
local function validSelection(v,s)
  local model=require('beamngEarth/planetLaunch')
  return model.stage(model.new(),{version=1,id='validation',lat=0,lon=0,vehicle=v,setup=s}).ok
end
local function available(v)
  if type(v)~='table' or type(v.model)~='string' or type(v.config)~='string' then return false end
  local mp=ext('beamngEarth_planetMultiplayer');if mp and mp.isVehicleAllowed and not mp.isVehicleAllowed(v.model,v.config) then return false end
  local source=core_vehicles and core_vehicles.getModel(v.model)
  -- Do not use the configurator's silent fallback to another config.
  return source and source.configs and source.configs[v.config]~=nil and true or false
end

local function hex(v,n) return type(v)=='string' and #v==n and not v:find('[^0-9a-f]') end
local function closed(v,keys)
  if type(v)~='table' or getmetatable(v) then return false end
  local count=0;for k in pairs(v) do if not keys[k] then return false end;count=count+1 end
  local expected=0;for _ in pairs(keys) do expected=expected+1 end
  return count==expected
end
local function endpoint(v,auto)
  if not closed(v,{address=true,port=true}) or type(v.address)~='string' or type(v.port)~='number' or v.port%1~=0 or v.port<1 or v.port>65535 then return false end
  local a=v.address
  if auto and (a=='0.0.0.0' or a=='::') then return true end
  local parts={};for n in a:gmatch('[^.]+') do parts[#parts+1]=n end
  if a:match('^%d+%.%d+%.%d+%.%d+$') and #parts==4 then
    for _,n in ipairs(parts) do if not n:match('^%d+$') or tostring(tonumber(n))~=n or tonumber(n)>255 then return false end end
    local first=tonumber(parts[1]);return first>0 and first~=127 and first<224
  end
  if a=='::' or a=='::1' or a:lower():sub(1,2)=='ff' or a:find('[^0-9a-fA-F:]') then return false end
  if a:find(':::',1,true) or (a:sub(1,1)==':' and a:sub(1,2)~='::') or (a:sub(-1)==':' and a:sub(-2)~='::') then return false end
  local compressed=a:find('::',1,true)
  if compressed and a:find('::',compressed+2,true) then return false end
  local count=0;for chunk in a:gmatch('[^:]+') do if #chunk>4 then return false end;count=count+1 end
  return (compressed and count<8) or (not compressed and count==8)
end
local function nameValid(v)
  if type(v)~='string' or v:match('^%s*$') or v:find('[%z\1-\31\127]') then return false end
  local _,n=v:gsub('[^\128-\191]','');return n<=64
end
local function adapter() return ext('beamngEarth_planetMultiplayer') end
function M.getSessionState()
  local a=adapter()
  if sessionIntent and a and a.getSessionStatus then
    local s=a.getSessionStatus(sessionIntent.operationId,sessionIntent.generation)
    if type(s)=='table' and s.operationId==sessionIntent.operationId and s.generation==sessionIntent.generation then
      local phases={off=true,starting=true,connecting=true,lobby=true,preparing=true,active=true,leaving=true,error=true}
      if phases[s.phase] then sessionState.phase=s.phase end
      sessionState.peerStatus=type(s.peerStatus)=='string' and s.peerStatus:sub(1,128) or nil
      sessionState.errorCode=s.errorCode
      sessionState.hostReady=s.hostReady==true and type(s.hostLocation)=='table' and point(s.hostLocation.lat,s.hostLocation.lon)
      sessionState.hostLocation=sessionState.hostReady and copy(s.hostLocation) or nil
      sessionState.modSync=copy(s.modSync)
      -- Destination comes only through the descriptor-validating local adapter.
      local d=s.destination
      if not sessionState.destinationAccepted and type(d)=='table' and point(d.lat,d.lon) and type(d.name)=='string' and #d.name<=512 and validSelection({model='validation',config='validation'},d.setup or {}) then
        if sessionState.mode~='join' then selectedPoint={lat=d.lat,lon=d.lon,name=d.name} end
        setup=copy(d.setup or {});sessionState.destinationAccepted=true
      end
    end
    local lease=ext('beamngEarthAutostart')
    local helper=lease and lease.getPlanetHelperStatus and lease.getPlanetHelperStatus()
    local lan=helper and helper.lan
    if lan and lan.operationId==sessionIntent.operationId and lan.generation==sessionIntent.generation then
      if lan.state=='error' then sessionState.phase='error';if sessionState.errorCode~='vehicleSyncRestartRequired' then sessionState.errorCode=lan.errorCode end end
      if sessionState.mode=='host' and lan.ticket and endpoint(lan.ticket.endpoint,false) and hex(lan.ticket.roomId,32) and hex(lan.ticket.certificateSha256,64) and hex(lan.ticket.joinSecret,32) then sessionState.ticket=copy(lan.ticket) end
    end
  end
  return copy(sessionState)
end
local function retireSession()
  local old=sessionIntent;local mode=sessionState.mode;local restoreError
  sessionIntent=nil;sessionState={mode='solo',phase='off'} -- invalidate before callbacks
  if old then
    local a=adapter();if a and a.leaveSession then local r=a.leaveSession(old.operationId,old.generation);if r and not r.ok then restoreError=r.errorCode end end
    local lease=ext('beamngEarthAutostart')
    if lease and lease.setPlanetLanIntent then old.state='leave';lease.setPlanetLanIntent(mode,old,chooser);lease.setPlanetLanIntent('solo',nil,chooser) end
  end
  return old~=nil,restoreError
end
function M.leaveSession()
  if not ownsMap() or not chooser then return result(false,'chooserClosed') end
  local _,restoreError=retireSession()
  local launch=coordinator and coordinator.status()
  if launch and (launch.covered or launch.phase=='ready') then coordinator.cancel() end
  return result(not restoreError,restoreError)
end
function M.setSessionMode(mode)
  if not active() then return result(false,'chooserClosed') end
  if mode~='solo' and mode~='host' and mode~='join' then return result(false,'invalidMode') end
  if mode=='solo' then return M.leaveSession() end
  -- Host/Join buttons select UI intent; only validated request changes session.
  return result(true)
end
local function requestSession(mode,ep,name,ticket)
  if not active() then return result(false,'chooserClosed') end
  if sessionIntent then return result(false,'sessionInProgress') end
  if extensions and extensions.load then extensions.load('beamngEarth_planetMultiplayer') end
  local a,lease=adapter(),ext('beamngEarthAutostart')
  if not a or not a.requestSession or not a.bindLaunch or not a.getLaunchBarrier or not lease or not lease.setPlanetLanIntent then return result(false,'lanUnavailable') end
  if type(hashStringSHA256)~='function' then return result(false,'lanUnavailable') end
  local serial=(rawget(_G,'beamngPlanetSessionCounter') or 0)+1
  local op=hashStringSHA256(chooser..':'..tostring(serial)..':'..tostring(realClock)):sub(1,32):lower()
  if not hex(op,32) then return result(false,'lanUnavailable') end
  local intent={operationId=op,generation=tostring(serial),state='start',endpoint=copy(ep),playerName=name,ticket=copy(ticket)}
  local request=copy(intent);request.state=nil;request.mode=mode;request.chooserToken=chooser
  if mode=='host' then request.destination={lat=selectedPoint.lat,lon=selectedPoint.lon,name=selectedPoint.name,setup=copy(setup)};request.destination.setup.trafficMode='disabled' end
  local r=a.requestSession(request)
  if not r or not r.ok then return result(false,r and r.errorCode or 'lanUnavailable') end
  if not lease.setPlanetLanIntent(mode,intent,chooser) then if a.leaveSession then a.leaveSession(op,intent.generation) end;return result(false,'leaseUnavailable') end
  rawset(_G,'beamngPlanetSessionCounter',serial)
  sessionIntent=intent;sessionState={mode=mode,phase=mode=='host' and 'starting' or 'connecting',operationId=op,generation=intent.generation}
  if mode=='join' then selectedPoint=nil;setup={} end
  return result(true)
end
function M.hostSession(name,override)
  if not selectedPoint then return result(false,'pointRequired') end
  name=name or 'Player';local ep=override or {address='0.0.0.0',port=3698}
  if not nameValid(name) or not endpoint(ep,true) then return result(false,'invalidEndpoint') end
  return requestSession('host',ep,name,nil)
end
function M.joinSession(text,name,override)
  if type(text)~='string' or #text>1024 or not nameValid(name) or type(jsonDecode)~='function' then return result(false,'invalidTicket') end
  local ok,t=pcall(jsonDecode,text)
  if not ok or not closed(t,{endpoint=true,roomId=true,certificateSha256=true,joinSecret=true}) or not endpoint(t.endpoint,false) or not hex(t.roomId,32) or not hex(t.certificateSha256,64) or not hex(t.joinSecret,32) or (override~=nil and not endpoint(override,false)) then return result(false,'invalidTicket') end
  return requestSession('join',override or t.endpoint,name,{roomId=t.roomId,certificateSha256=t.certificateSha256,joinSecret=t.joinSecret})
end

function M.selectPoint(lat,lon,name)
  if not active() then return result(false,'chooserClosed') end
  if sessionIntent then
    local s=M.getSessionState()
    if s.phase=='error' or s.errorCode then return result(false,s.errorCode or 'sessionRejected') end
    if s.mode~='join' then return result(false,'sessionDestinationLocked') end
    if not s.hostReady or not s.destinationAccepted then return result(false,'hostNotReady') end
  end
  if not point(lat,lon) or (name~=nil and (type(name)~='string' or #name>512)) then return result(false,'invalidPoint') end
  selectedPoint={lat=lat,lon=lon,name=name or ''};return result(true)
end
function M.setVehicle(selection)
  if not active() then return result(false,'chooserClosed') end
  if not validSelection(selection,setup) then return result(false,'invalidVehicle') end
  if not available(selection) then return result(false,'vehicleUnavailable') end
  vehicle=copy(selection);return result(true)
end
function M.getSetupOptions()
  local groups={}
  if extensions and extensions.load then extensions.load('freeroam_configuratorOptions_freeroamOptions') end
  local level=core_levels and core_levels.getLevelByName('beamng_earth')
  if level and extensions and extensions.hook then extensions.hook('onFreeroamConfiguratorGetOptions',level,groups) end
  table.sort(groups,function(a,b) return (a.order or math.huge)<(b.order or math.huge) end)
  return groups
end
function M.getLaunchSelection()
  if not active() then return result(false,'chooserClosed') end
  if sessionIntent then
    local s=M.getSessionState()
    if s.phase=='error' or s.errorCode then return result(false,s.errorCode or 'sessionRejected') end
    if s.mode=='join' and (not s.hostReady or not s.destinationAccepted) then return result(false,'hostNotReady') end
  end
  if not selectedPoint then return result(false,'pointRequired') end
  if not vehicle then return result(false,'vehicleRequired') end
  if not available(vehicle) then return result(false,'vehicleUnavailable') end
  if not validSelection(vehicle,setup) then return result(false,'invalidSelection') end
  return {ok=true,selection={lat=selectedPoint.lat,lon=selectedPoint.lon,name=selectedPoint.name,vehicle=copy(vehicle),setup=copy(setup)}}
end
local setupKeys={traffic_trafficMode='trafficMode',environment_timeOfDay='timeOfDay',environment_date='date',environment_timePlay='timePlay'}
function M.setSetup(values)
  if sessionIntent then return result(false,'sessionDestinationLocked') end
  if not active() then return result(false,'chooserClosed') end
  if type(values)~='table' or getmetatable(values)~=nil then return result(false,'invalidSetup') end
  local nextSetup=copy(setup)
  for k,v in pairs(values) do
    if not setupKeys[k] then return result(false,'invalidSetup') end
    nextSetup[setupKeys[k]]=v
  end
  if not validSelection({model='validation',config='validation'},nextSetup) then return result(false,'invalidSetup') end
  setup=nextSetup;return result(true)
end
function M.getVehicleSelectorToken()
  if not active() or not selectorSelection then return nil end
  return chooser..':'..tostring(selectorSerial)
end
function M.confirmVehicleSelection(token,model,config,additional)
  if not token or token~=M.getVehicleSelectorToken() then return result(false,'staleSelector') end
  if type(model)~='string' or type(config)~='string' then return result(false,'invalidVehicle') end
  return selectorSelection(model,config,additional)
end
function M.openVehicleSelector()
  if not active() then return result(false,'chooserClosed') end
  if extensions and extensions.load then extensions.load('ui_vehicleSelector_general') end
  local selector=ext('ui_vehicleSelector_general') or rawget(_G,'ui_vehicleSelector_general')
  if not selector or not selector.openVehicleSelectorForChallenge then return result(false,'selectorUnavailable') end
  selectorSerial=selectorSerial+1
  local owner,serial=chooser,selectorSerial
  local function selected(model,config,additional)
    if not active() or chooser~=owner or serial~=selectorSerial then return end
    if model then
      local source=core_vehicles.getModel(model)
      local paints={}
      for _,key in ipairs({'paint','paint2','paint3'}) do
        local name=type(additional)=='table' and additional[key]
        if name then
          local paint=source and source.model and source.model.paints and source.model.paints[name]
          if not paint then return result(false,'paintUnavailable') end
          -- Stock paints also carry catalogue metadata; launch stores only
          -- the physical paint parameters accepted by vehicle spawning.
          local physical={}
          for _,field in ipairs({'baseColor','metallic','roughness','clearcoat','clearcoatRoughness'}) do physical[field]=copy(paint[field]) end
          paints[#paints+1]=physical
        end
      end
      local accepted=M.setVehicle({model=model,config=config,paints=#paints>0 and paints or nil})
      if not accepted.ok then return accepted end
      if selector.trackRecentVehicle then selector.trackRecentVehicle(model,config) end
    end
    selectorSerial=selectorSerial+1
    extensions.ui_router.navigate('planet.setup')
    return result(true)
  end
  selectorSelection=selected
  -- Generic stock selection API accepts our route and owns both select/cancel.
  -- Unlike its configurator wrapper it introduces no Freeroam breadcrumb target.
  selector.openVehicleSelectorForChallenge(selected,'planet.vehicle',{unrestricted=true})
  local details=ext('ui_vehicleSelector_detailsInteraction') or rawget(_G,'ui_vehicleSelector_detailsInteraction')
  if details and details.setCustomDetailsButtons then
    details.setCustomDetailsButtons({{callback=selected,meta={label='Выбрать автомобиль',icon='car',primary=true,isDoubleClickAction=true}}})
  end
  return result(true)
end
-- Task 6 calls this after snapshot validation, BEFORE mission startup. The hook
-- writes only the existing per-start spawningOptionsHelper. Never save settings.
function M.applySetup(id,values)
  if not ownsMap() then return result(false,'staleChooser') end
  if type(id)~='string' or #id==0 or #id>128 then return result(false,'invalidLaunchId') end
  local s=values or setup
  if not validSelection({model='validation',config='validation'},s) then return result(false,'invalidSetup') end
  local manual=s.timeOfDay~=nil or s.date~=nil or s.timePlay~=nil
  if manual and extensions and extensions.load then extensions.load('beamngEarth_sky') end
  local sky=ext('beamngEarth_sky') or rawget(_G,'beamngEarth_sky')
  if manual and (not sky or not sky.setPlanetOverride or not sky.setPlanetOverride(id)) then return result(false,'skyUnavailable') end
  local options={}
  for key,field in pairs(setupKeys) do options[key]=s[field] end
  if extensions and extensions.load then
    extensions.load('freeroam_freeroam');extensions.load('freeroam_configuratorOptions_freeroamOptions')
  end
  extensions.hook('onFreeroamConfiguratorApplyOptions',options)
  return result(true)
end
function M.clearSetupOverride(id)
  local sky=ext('beamngEarth_sky') or rawget(_G,'beamngEarth_sky')
  if sky and sky.clearPlanetOverride then return sky.clearPlanetOverride(id) end
  return false
end
function M.openChooser()
  if not ownsMap() then return {ok=false,errorCode='staleChooser'} end
  if rawget(_G,'beamngPlanetCleanup') then return result(false,'cleanupInProgress') end
  if coordinator and coordinator.status().phase=='ready' then retireReady() end
  if chooser then return {ok=true,state=M.getState()} end
  if extensions and extensions.load then
    extensions.load('beamngEarth_core'); extensions.load('beamngEarth_settings'); extensions.load('beamngEarthAutostart')
  end
  local lease,core=ext('beamngEarthAutostart'),ext('beamngEarth_core')
  counter=counter+1;rawset(_G,'beamngPlanetChooserCounter',counter)
  local token='planet-chooser-'..tostring(counter)
  if not lease or not lease.setPlanetLease('preview',token,'') then return {ok=false,errorCode='previewUnavailable',state=M.getState()} end
  local result=core and core.setPlanetPreview and core.setPlanetPreview(token)
  if not result or not result.ok then lease.clearPlanetLease(token);return {ok=false,errorCode='previewUnavailable',state=M.getState()} end
  chooser=token
  selectedPoint,vehicle,setup=nil,nil,{}
  sessionState,sessionIntent={mode='solo',phase='off'},nil
  return {ok=true,state=M.getState()}
end
local function releaseChooser(owner)
  M.pauseMap()
  local core,lease=ext('beamngEarth_core'),ext('beamngEarthAutostart')
  if chooser==owner then
    retireSession()
    if ownsMap() and core and core.setPlanetPreview then core.setPlanetPreview(nil) end
    if lease and lease.clearPlanetLease then lease.clearPlanetLease(owner) end
    chooser=nil;selectorSerial=selectorSerial+1
  end
end
retireReady=function()
  if coordinator and coordinator.retireReady().ok then releaseChooser(chooser);coordinator=nil end
end
local function makeCoordinator()
  local owner=chooser
  local core,lease=ext('beamngEarth_core'),ext('beamngEarthAutostart')
  if not core or not core.stageLaunch or not lease then return nil end
  local preparationSerial=0
  local helperAt,helper=0,nil
  local engine={available=available,applySetup=M.applySetup,clearSetup=M.clearSetupOverride}
  engine.lanIdentity=function() return sessionIntent and {operationId=sessionIntent.operationId,generation=sessionIntent.generation} end
  engine.onLocalReadiness=function(id,status) local a=adapter();if a and a.onLocalReadiness then a.onLocalReadiness(id,status) end end
  engine.lanBarrier=function(id,identity)
    local a=adapter()
    if not sessionIntent or identity.operationId~=sessionIntent.operationId or identity.generation~=sessionIntent.generation then return {errorCode='staleSession'} end
    return a and a.getLaunchBarrier and a.getLaunchBarrier(id,identity.operationId,identity.generation) or {errorCode='lanUnavailable'}
  end
  engine.cover=function(status)
    if status.phase=='cancelled' and not status.covered and not status.externalEnd then
      status.returnRoute=closing and 'menu' or 'planet.select'
    end
    if guihooks then guihooks.trigger('BeamNGPlanetLaunch',status) end
    if status.phase=='cancelled' and not status.covered and not status.externalEnd then
      if closing then releaseChooser(owner);closing=false
      elseif ownsMap() and chooser==owner then extensions.ui_router.navigate('planet.select') end
    elseif status.phase=='ready' and not status.covered and ownsMap() then
      extensions.ui_router.navigate('play')
    end
  end
  engine.enter=function(tag,callback) core_gamestate.requestEnterLoadingScreen(tag,callback) end
  engine.exit=function(tag) core_gamestate.requestExitLoadingScreen(tag) end
  engine.input=function(id,on)
    local filter=extensions.core_input_actionFilter
    local inputOwner='beamngPlanetLaunch:'..id
    if on then
      filter.setActionMapPlayerMask(inputOwner,'VehicleCommonActionMap',0)
      filter.setActionMapPlayerMask(inputOwner,'VehicleSpecificActionMap',0)
    else filter.clearActionMapPlayerMasks(inputOwner) end
  end
  -- Complete the prior world's ordinary teardown before invoking freeroam, so
  -- stock cannot queue an unowned startFreeroam callback that outlives Cancel.
  engine.prepare=function(callback)
    preparationSerial=preparationSerial+1
    local serial=preparationSerial
    local function finished() if serial==preparationSerial then callback() end end
    if scenetree.MissionGroup or scenetree.MissionCleanup then
      endActiveGameMode(triggerDelayedStartGenerator('beamngPlanetLaunch','freeroam',finished,true))
    else finished() end
  end
  engine.teardown=function(callback)
    preparationSerial=preparationSerial+1
    local cleanup={owner=M}
    rawset(_G,'beamngPlanetCleanup',cleanup)
    endActiveGameMode(triggerDelayedStartGenerator('beamngPlanetLaunch','cancel',function()
      if rawget(_G,'beamngPlanetCleanup')==cleanup then rawset(_G,'beamngPlanetCleanup',nil) end
      callback()
    end,true))
  end
  engine.start=function(snapshot)
    -- Normal stock vehicle-spawn table; no customLoadingFunction delaying the
    -- Earth onClientStartMission hook, and no configurator fallback/default.
    local v=snapshot.vehicle
    local a=adapter();local config=sessionIntent and a and a.resolveVehicleConfig and a.resolveVehicleConfig(v.model,v.config) or v.config
    local options={config=config,paint=v.paints and v.paints[1],paint2=v.paints and v.paints[2],paint3=v.paints and v.paints[3]}
    return freeroam_freeroam.startFreeroamByName('beamng_earth',nil,nil,{v.model,options})
  end
  engine.helperStatus=function()
    if realClock>=helperAt then
      helperAt=realClock+2
      helper=lease.getPlanetHelperStatus and lease.getPlanetHelperStatus() or nil
    end
    return helper
  end
  local leases={
    launching=function(id) helperAt=0;return chooser==owner and lease.setPlanetLease('launching',owner,id) end,
    active=function(id) return chooser==owner and lease.setPlanetLease('active',owner,id) end,
    preview=function(id)
      if chooser~=owner then return false end
      local ok=lease.returnPlanetPreview(owner,id)
      if ok then core.setPlanetPreview(owner) end
      return ok
    end,
  }
  return require('beamngEarth/planetCoordinator').new(engine,core,leases,function() return realClock end)
end
local function snapshot()
  local selected=M.getLaunchSelection()
  if not selected.ok then return nil,selected.errorCode end
  local serial=(rawget(_G,'beamngPlanetLaunchCounter') or 0)+1
  rawset(_G,'beamngPlanetLaunchCounter',serial)
  local spec=selected.selection;spec.version=1;spec.id='planet-launch-'..tostring(serial)
  return spec
end
function M.start()
  local spec,err=snapshot()
  if not spec then return result(false,err) end
  coordinator=coordinator or makeCoordinator()
  if not coordinator then return result(false,'coreUnavailable') end
  if sessionIntent then
    local a=adapter();local r=a and a.bindLaunch and a.bindLaunch(spec.id,sessionIntent.operationId,sessionIntent.generation)
    if not r or not r.ok then return result(false,r and r.errorCode or 'lanUnavailable') end
  end
  M.pauseMap()
  local r=coordinator.start(spec);return result(r.ok,r.errorCode)
end
local function matching(id)
  local s=coordinator and coordinator.status()
  return ownsMap() and s and (id==nil or id==s.id)
end
function M.acknowledgeCover(id)
  if not matching(id) then return result(false,'staleLaunch') end
  local r=coordinator.acknowledgeCover(id);return result(r.ok,r.errorCode)
end
function M.cancel(id)
  if not matching(id) then return result(false,'staleLaunch') end
  if sessionIntent then return M.leaveSession() end
  local r=coordinator.cancel();return result(r.ok,r.errorCode)
end
function M.continueWaiting(id)
  if not matching(id) then return result(false,'staleLaunch') end
  local r=coordinator.continueWaiting();return result(r.ok,r.errorCode)
end
function M.retry(id)
  if sessionIntent then return result(false,'lanRetryRequiresLeave') end
  if not matching(id) then return result(false,'staleLaunch') end
  -- The immutable accepted selection remains authoritative during recovery.
  local spec={version=1,id='',lat=selectedPoint.lat,lon=selectedPoint.lon,name=selectedPoint.name,vehicle=copy(vehicle),setup=copy(setup)}
  local serial=(rawget(_G,'beamngPlanetLaunchCounter') or 0)+1
  rawset(_G,'beamngPlanetLaunchCounter',serial);spec.id='planet-launch-'..tostring(serial)
  local r=coordinator.retry(spec);return result(r.ok,r.errorCode)
end
function M.onUpdate(dtReal)
  realClock=realClock+math.max(tonumber(dtReal) or 0,0)
  if ownsMap() and coordinator then coordinator.update(dtReal) end
end
function M.onClientEndMission()
  if ownsMap() and coordinator and coordinator.status().phase=='ready' then retireReady() end
end
-- Called by the runtime guard. Stock route changes during loading/play must
-- never release the accepted launch lease, popup or input ownership.
function M.leaveChooser()
  local s=coordinator and coordinator.status()
  if s and (s.covered or s.phase=='ready') then return {ok=true,preserveSelection=true,state=M.getState()} end
  return M.closeChooser()
end
function M.mapTiles(request)
  local id=type(request)=='table' and request.requestId or nil
  if not ownsMap() or not chooser then return reply(id,'chooserClosed') end
  if not requestId(id) or type(request.keys)~='table' or #request.keys>128 then return reply(id,'invalidMapRequest') end
  local list,allowed={},{}
  for k,key in pairs(request.keys) do
    if type(k)~='number' or k%1~=0 or k<1 or k>#request.keys then return reply(id,'invalidMapRequest') end
    local parsed=tile(key); if not parsed then return reply(id,'invalidMapRequest') end
    allowed[key]=true;list[k]=parsed
  end
  local settings=ext('beamngEarth_settings')
  if not settings or not settings.mapTiles then return reply(id,'mapUnavailable') end
  local found=settings.mapTiles(list) or {}; local ready,seen={},{}
  for _,key in ipairs(found) do if allowed[key] and not seen[key] then ready[#ready+1]=key;seen[key]=true end end
  return reply(id,nil,ready)
end
function M.searchMap(request)
  local id=type(request)=='table' and request.requestId or nil
  if not ownsMap() or not chooser then return reply(id,'chooserClosed') end
  if not requestId(id) or type(request.query)~='string' or #request.query<3 or #request.query>1024 or not point(request.lat,request.lon) then return reply(id,'invalidSearch') end
  local core=ext('beamngEarth_core')
  if core and core.cancelPlanetGeocode then core.cancelPlanetGeocode(chooser) end
  search={id=id,chooser=chooser}
  if not core or not core.requestPlanetGeocode or not core.requestPlanetGeocode(chooser,id,request.query,request.lat,request.lon) then search=nil;return reply(id,'searchUnavailable') end
  return reply(id,nil,nil,nil,true)
end
function M.pollMapSearch(id)
  if not ownsMap() or not chooser then return reply(id,'chooserClosed') end
  if not search or search.id~=id or search.chooser~=chooser then return reply(id,'staleRequest') end
  local core=ext('beamngEarth_core');local result=core and core.takePlanetGeocode and core.takePlanetGeocode(chooser,id)
  if not result then return reply(id,nil,nil,nil,true) end
  search=nil;local results={}
  for _,p in ipairs(result.results or {}) do
    if #results>=7 then break end
    if type(p)=='table' and point(p.lat,p.lon) and type(p.name)=='string' then
      results[#results+1]={lat=p.lat,lon=p.lon,name=p.name:sub(1,1024)}
    end
  end
  return reply(id,result.error and 'searchUnavailable' or nil,nil,results)
end
function M.pauseMap()
  if not ownsMap() then return {ok=false,errorCode='staleChooser'} end
  local settings,core=ext('beamngEarth_settings'),ext('beamngEarth_core')
  if settings and settings.mapTiles then settings.mapTiles({}) end
  if chooser and core and core.cancelPlanetGeocode then core.cancelPlanetGeocode(chooser) end
  search=nil
  return {ok=true}
end
function M.closeChooser()
  if not ownsMap() then return {ok=false,errorCode='staleChooser'} end
  local s=coordinator and coordinator.status()
  if s and (s.covered or s.phase=='ready') then
    closing=true
    if not s.cleaning then if sessionIntent then M.leaveSession() else coordinator.cancel() end end
    return {ok=true,state=M.getState()}
  end
  if sessionIntent then M.leaveSession() end
  releaseChooser(chooser);coordinator=nil
  return {ok=true,state=M.getState()}
end
M.onExtensionUnloaded=M.closeChooser
return M

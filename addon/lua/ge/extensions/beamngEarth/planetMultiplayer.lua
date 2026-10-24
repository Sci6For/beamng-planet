-- Planet LAN operation owner. Remote messages select typed state transitions only.
local M={}
local frames=require('beamngEarth/planetFrames')
local codec=require('beamngEarth/planetNetCodec')
local function copy(t) if type(t)~='table' then return t end;local r={};for k,v in pairs(t) do r[k]=copy(v) end;return r end
local function id(s) return type(s)=='string' and #s==32 and s:match('^[0-9a-f]+$')~=nil end
local function increment(s) local a={s:byte(1,#s)};local carry=1;for i=#a,1,-1 do local v=a[i]-48+carry;a[i]=48+v%10;carry=math.floor(v/10) end;local out=string.char(unpack(a));return carry==1 and '1'..out or out end
local function ext(name) return (extensions and extensions[name]) or rawget(_G,name) end
function M.new(adapter)
 local self,s={},nil
 local function send(command)
  if not s or not s.room then return false end;s.outSeq=increment(s.outSeq)
  return adapter.send({room=s.room,world_epoch=s.world,sequence=s.outSeq,command=command})
 end
 local function hold(on,reason) if adapter.hold(on,reason,s and s.generation)==false then return false end;if s then s.held=on end;return true end
 local function fail(reason) if s then s.phase='error';s.errorCode=reason;s.resumed=false;adapter.resetPredictors();hold(true,reason) end end
 function self.failSession(reason) fail(reason) end
 function self.sendCommand(c) return send(c) end
 function self.requestSession(v)
  if s or type(v)~='table' or (v.mode~='host' and v.mode~='join') or not id(v.operationId) or not codec.validDecimal(v.generation) or type(v.endpoint)~='table' or type(v.playerName)~='string' or #v.playerName==0 then return {ok=false,errorCode='invalidSession'} end
  v=copy(v)
  if v.mode=='host' and v.destination and adapter.resolveSetup then v.destination.setup=adapter.resolveSetup(v.destination.setup) end
  s={operationId=v.operationId,generation=v.generation,mode=v.mode,intent=copy(v),phase='starting',outSeq='0',controlSeq='0',clock=0,vehicles={},sources={},destination=nil}
  if adapter.start then adapter.start(copy(v)) end;return {ok=true}
 end
 function self.bindLaunch(launch,op,generation) if not s or s.operationId~=op or s.generation~=generation or s.launch then return {ok=false,errorCode='staleSession'} end;s.launch=launch;return {ok=true} end
 function self.acceptIdentity(room,world,owner)
  if not s or not id(room) or not codec.validDecimal(world) or not id(owner) then return false end;s.room,s.world,s.owner=room,world,owner;return true
 end
 function self.getSessionStatus(op,generation)
  if not s or op~=s.operationId or generation~=s.generation then return {operationId=op,generation=generation,phase='off'} end
  return {operationId=op,generation=generation,phase=s.phase,peerStatus=s.peerStatus,errorCode=s.errorCode,destination=copy(s.destination)}
 end
 function self.getLaunchBarrier(launch,op,generation)
  local r={launchId=launch,operationId=op,generation=generation,resumed=false}
  if not s or s.launch~=launch or s.operationId~=op or s.generation~=generation then return r end
  r.errorCode=s.errorCode;local t=s.txn;local b=adapter.binding()
  if t and t.applied and t.binding and b and b.serverSession==t.binding.serverSession and b.anchorId==t.binding.anchorId and b.commitSerial==t.binding.commitSerial then
   r.resumed=s.resumed==true;r.worldEpoch=s.world;r.frameId=t.frame.frame_id;r.frameEpoch=t.frame.frame_epoch;r.frameOperation=t.operation;r.serverSession=b.serverSession;r.anchorId=b.anchorId;r.commitSerial=b.commitSerial
  end;return r
 end
 local function localReady(status)
  local t=s and s.txn;if not t or t.readySent then return false end
  if not status or status.phase~='ready' or status.serverSession==nil or status.anchorId==nil or status.commitSerial==nil then return false end
  t.localBinding={serverSession=status.serverSession,anchorId=status.anchorId,commitSerial=status.commitSerial};if not hold(true,'Preparing shared world') then fail('holdUnavailable');return false end;t.readySent=true;s.phase='preparing'
  return send({type='LocalReady',operation=t.operation})
 end
 function self.onLocalReadiness(launch,status) if not s or s.launch~=launch then return false end;return localReady(status) end
 function self.onControl(message,generation)
  if not s or (generation and generation~=s.generation) or type(message)~='table' or type(message.command)~='table' or not codec.validDecimal(message.sequence) then return false end
  local c=message.command
  if c.type=='Welcome' then if s.owner or not id(c.connection_id) then return false end;s.owner=c.connection_id;s.room=message.room;s.world=message.world_epoch;s.controlSeq=message.sequence;if s.mode=='host' and s.intent.destination then local d=s.intent.destination;send({type='HostWorld',location={d.lat,d.lon,0},name=d.name or '',setup=d.setup}) end;return true end
  if message.room~=s.room or message.world_epoch~=s.world or codec.compare(message.sequence,s.controlSeq)<=0 then return false end
  s.controlSeq=message.sequence
  if c.type=='Descriptor' then
   if not adapter.descriptor then return false end;local d,digest=adapter.descriptor(c.canonical_json,s.intent);if not d then fail('descriptorMismatch');return false end
   s.destination=copy(d.environment and d.environment.destination);s.phase='lobby';send({type='DescriptorAgreement',world_digest=digest});return true
  elseif c.type=='PrepareFrame' then
   if not id(c.operation) or not frames.validate(c.frame) or (s.frame and codec.compare(c.frame.frame_epoch,s.frame.frame_epoch)<=0) then return false end
   local t=frames.prepare(c);if not t then return false end
   if s.txn and not s.resumed and (s.txn.committed or codec.compare(c.frame.frame_epoch,s.txn.frame.frame_epoch)<=0) then return false end
   s.txn=t;t.started=s.clock;s.resumed=false;s.phase='preparing';adapter.resetPredictors();local dynamic=s.frame~=nil;t.dynamic=dynamic
   if dynamic then hold(true,'Preparing shared world') end
   if adapter.prepare(t,dynamic)==false then fail('framePrepareFailed');return false end;return true
  elseif c.type=='CommitFrame' then
   local t=s.txn;if not t or t.operation~=c.operation or not t.readySent or t.committed then return false end;t.committed=true
   if adapter.commit(t,t.dynamic)==false then fail('frameApplyFailed');return false end;return true
  elseif c.type=='Resume' then
   local t=s.txn;if not t or t.operation~=c.operation or not t.committed or not t.applied then return false end
   local b=adapter.binding();if not b or b.serverSession~=t.binding.serverSession or b.anchorId~=t.binding.anchorId or b.commitSerial~=t.binding.commitSerial then return false end
   s.frame=t.frame;s.resumed=true;s.phase='active';hold(false);return true
  elseif c.type=='Abort' then if not s.txn or c.operation~=s.txn.operation then return false end;fail('frameAborted');return true
  elseif c.type=='Error' then fail('sessionRejected');return true
  elseif c.type=='Status' then s.peerStatus=c.status;return true
  elseif c.type=='Spawn' then
   local v=c.vehicle;if not v or not id(v.vehicle_id) or not id(v.owner) or not codec.validDecimal(v.vehicle_epoch) then return false end
   if s.vehicles[v.vehicle_id] then return false end
   if adapter.vehicleAvailable and not adapter.vehicleAvailable(v) then fail('vehicleUnavailable');return false end
   s.vehicles[v.vehicle_id]=copy(v);if adapter.spawn then adapter.spawn(s.vehicles[v.vehicle_id],v.owner==s.owner) end;return true
  elseif c.type=='Reset' or c.type=='Remove' then
   local v=s.vehicles[c.vehicle_id];if not v or not codec.validDecimal(c.vehicle_epoch) or codec.compare(c.vehicle_epoch,v.vehicle_epoch)<0 then return false end
   v.vehicle_epoch=c.vehicle_epoch;s.sources[c.vehicle_id]=nil;adapter.resetPredictors(c.vehicle_id)
   if c.type=='Remove' then if adapter.remove then adapter.remove(v) end;s.vehicles[c.vehicle_id]=nil elseif adapter.reset then adapter.reset(v) end;return true
  end;return false
 end
 function self.onState(state,generation)
  if not s or not s.resumed or (generation and generation~=s.generation) or type(state)~='table' or state.room~=s.room or state.world_epoch~=s.world or state.frame_id~=s.frame.frame_id or state.frame_epoch~=s.frame.frame_epoch then return false end
  local v=s.vehicles[state.vehicle_id];if not v or state.owner~=v.owner or state.vehicle_epoch~=v.vehicle_epoch or state.owner==s.owner or not codec.validDecimal(state.source_seq) then return false end
  local old=s.sources[state.vehicle_id];if old and codec.compare(state.source_seq,old)<=0 then return false end
  local pose=frames.fromNetworkPose(state,s.frame);if not pose then return false end;s.sources[state.vehicle_id]=state.source_seq
  if adapter.remote then local ok,reason=adapter.remote(v,pose,state,s.frame);if ok==false then fail(reason or 'vehicleUnavailable');return false end end;return true
 end
 function self.publishPose(pose,vehicle)
  if not s or not s.resumed or not s.frame or not vehicle then return false end
  local p=frames.toNetworkPose(pose,s.frame);if not p then return false end
  s.sourceSeq=increment(s.sourceSeq or '0');p.room=s.room;p.world_epoch=s.world;p.frame_id=s.frame.frame_id;p.frame_epoch=s.frame.frame_epoch;p.owner=s.owner;p.vehicle_id=vehicle.vehicle_id;p.vehicle_epoch=vehicle.vehicle_epoch;p.source_seq=s.sourceSeq;p.source_monotonic_time=s.clock;p.inputs=pose.inputs or {0,0,0,0};p.electrics=pose.electrics or {throttle_input=0,brake_input=0,clutch=0,parkingbrake=0,steering_input=0};p.gearbox=pose.gearbox or {lock_coef=0,grb_idx=0,frzn=false};return adapter.publish and adapter.publish(p)
 end
 function self.update(dt)
  if not s then return end;s.clock=s.clock+math.max(0,tonumber(dt)or 0)
  if adapter.poll then adapter.poll(s) end
  local t=s.txn;if t and not s.resumed and s.phase~='error' then
   if s.clock-t.started>=120 then send({type='Abort',operation=t.operation,reason='Timeout'});fail('readyTimeout');return end
   local b=adapter.binding()
   if b and b.errorCode then fail(b.errorCode);return end
   if t.dynamic and not t.readySent and b and b.prepared and b.operation==t.operation then localReady({phase='ready',serverSession=b.serverSession,anchorId=b.anchorId,commitSerial=b.commitSerial}) end
   if t.committed and not t.applied and b and b.applied and (not t.dynamic or b.operation==t.operation) and b.serverSession~=nil and b.anchorId~=nil and b.commitSerial~=nil then
    t.binding=copy(b);adapter.resetPredictors();frames.applied(t,t.operation);send({type='FrameApplied',operation=t.operation})
   end
  end
  if s.resumed and adapter.tick then adapter.tick(dt,s,self) end
 end
 function self.leaveSession(op,generation)
  if not s or (op and(op~=s.operationId or generation~=s.generation)) then return {ok=false,errorCode='staleSession'} end
  send({type='Leave'});hold(false);local old=s;s=nil;adapter.resetPredictors();if adapter.clear then adapter.clear(old) end;adapter.disconnect();return {ok=true}
 end
 self.leave=self.leaveSession
 function self.isManaged() return s~=nil end
 function self.identity() return s and {operationId=s.operationId,generation=s.generation} end
 function self.raw() return s end
 return self
end
-- Engine bindings are local trusted code; no incoming message becomes a Lua command.
local held=nil
local function core() return ext('beamngEarth_core') end
local function network() return ext('kissmp_network') end
local adapter={}
function adapter.send(m) local n=network();return n and n.planetSendControl(m) end
function adapter.publish(v) local n=network();return n and n.planetPublishState(v) end
function adapter.hold(on,reason,generation)
 if on and not held then local v=getPlayerVehicle and getPlayerVehicle(0);if not v or not v.getActive then return false end;held={object=v,id=v:getID(),active=v:getActive(),generation=generation};if held.active then v:setActive(0) end
 elseif not on and held then local h=held;held=nil;local v=getObjectByID and getObjectByID(h.id);if v==h.object and v.getActive and not v:getActive() and h.active then v:setActive(1) end end
 if guihooks then guihooks.trigger('BeamNGPlanetEncounter',{covered=on==true,reason=reason}) end;return true
end
function adapter.prepare(t,dynamic) local c=core();return c and c.planetPrepareFrame and c.planetPrepareFrame(t.operation,t.frame,dynamic) end
function adapter.commit(t,dynamic) local c=core();return c and c.planetCommitFrame and c.planetCommitFrame(t.operation,dynamic) end
function adapter.binding() local c=core();return c and c.planetFrameStatus and c.planetFrameStatus() end
function adapter.resetPredictors(id) local t=ext('kissmp_transform');if t and t.planetResetPredictors then t.planetResetPredictors(id) end end
function adapter.disconnect() local n=network();if n and n.planetDisconnect then n.planetDisconnect('left') end;local c=core();if c and c.planetEndSession then c.planetEndSession() end end
function adapter.descriptor(bytes,intent)
 if type(bytes)~='string' or #bytes>1024*1024 or not jsonDecode or not hashStringSHA256 then return nil end
 local ok,d=pcall(jsonDecode,bytes);if not ok or type(d)~='table' or not d.environment or not d.environment.destination then return nil end
 local dst=d.environment.destination;local setup=dst.setup;if type(dst.lat)~='number' or type(dst.lon)~='number' or not setup or setup.trafficMode~='disabled' or type(setup.timeOfDay)~='number' or type(setup.date)~='number' then return nil end
 -- The bridge already verifies exact local game/client/catalogue fingerprints before forwarding.
 return d,hashStringSHA256(bytes):lower()
end

local remoteObjects={}
local function footprint(obj)
 local box=obj and obj.getSpawnWorldOOBB and obj:getSpawnWorldOOBB();local h=box and box:getHalfExtents()
 if not h or type(h.x)~='number' or type(h.y)~='number' or type(h.z)~='number' or h.x<=0 or h.y<=0 or h.z<=0 then return nil end
 local r=math.sqrt(h.x*h.x+h.y*h.y+h.z*h.z)+2
 if r~=r or r>200 then return nil end;return r
end
local conversion=false
local conversionGrace=0
function adapter.resolveSetup(setup)
 setup=copy(setup or {});setup.trafficMode='disabled'
 -- Audited 0.40 core_environment convention: noon=0, midnight=.5; date is epoch SECONDS.
 local named={sunrise=.775,morning=.825,earlyNoon=.9,noon=0,lateNoon=.1,afternoon=.175,evening=.23,sunset=.245,night=.5}
 local env=ext('core_environment');local current=env and env.getTimeOfDay and env.getTimeOfDay()
 if setup.timeOfDay=='current' or setup.timeOfDay=='currentUtc' then
  local clock=os.date(setup.timeOfDay=='currentUtc' and '!*t' or '*t');setup.timeOfDay=((clock.hour*3600+clock.min*60+clock.sec)/86400-.5)%1
 elseif type(setup.timeOfDay)~='number' then setup.timeOfDay=(env and env.timeOfDayKeyToTime and env.timeOfDayKeyToTime[setup.timeOfDay]) or named[setup.timeOfDay] or (current and current.time) or 0 end
 if type(setup.date)~='number' then
  setup.date=current and current.year and current.month and current.day and os.time({year=current.year,month=current.month,day=current.day,hour=12}) or os.time()
 end
 if setup.timePlay==nil or setup.timePlay=='level' then setup.timePlay=current and current.play and 'normal' or 'disabled' end
 return setup
end
function adapter.spawn(v,owned) if owned then local car=getPlayerVehicle(0);if car then v.localId=car:getID();car:queueLuaCommand("extensions.load('kissmp_planet')") end end end
function adapter.vehicleAvailable(v)
 if type(v.model)~='string' or type(v.config)~='string' or not core_vehicles or not core_vehicles.getModel then return false end
 local config=v.config:gsub('^/','');local model,key=config:match('^vehicles/([^/]+)/([^/]+)%.pc$')
 if model then if model~=v.model then return false end else key=config:gsub('%.pc$','') end
 local data=core_vehicles.getModel(v.model)
 return data and data.configs and data.configs[key]~=nil or false
end
function adapter.remove(v)
 local obj=remoteObjects[v.vehicle_id];remoteObjects[v.vehicle_id]=nil;if obj and getObjectByID(obj:getID())==obj then obj:delete() end
end
function adapter.reset(v) local obj=remoteObjects[v.vehicle_id];if obj then obj:reset() end end
function adapter.clear() for id,obj in pairs(remoteObjects) do if getObjectByID(obj:getID())==obj then obj:delete() end end;remoteObjects={} end
function adapter.remote(v,pose,state)
 local c=core();local owner=getPlayerVehicle(0);if not owner or not c then return end
 local p=owner:getPosition();local d=math.sqrt((pose.position[1]-p.x)^2+(pose.position[2]-p.y)^2+(pose.position[3]-p.z)^2)
 local allowed=not conversion and d<=250 and c.planetCollisionCoverage and c.planetCollisionCoverage(pose.position,pose.linear_velocity,10)
 local obj=remoteObjects[v.vehicle_id]
 if not allowed then if obj and obj:getActive() then obj:setActive(0) end;return end
 if not obj then
  if not adapter.vehicleAvailable(v) or not core_vehicles.spawnNewVehicle then return false,'vehicleUnavailable' end
  obj=core_vehicles.spawnNewVehicle(v.model,{config=v.config,autoEnterVehicle=false,pos=vec3(unpack(pose.position)),rot=quat(unpack(pose.orientation))})
  if not obj then return false,'vehicleUnavailable' end;obj:setActive(0);remoteObjects[v.vehicle_id]=obj
  obj:queueLuaCommand("extensions.load('kissmp_vehicle');extensions.load('kissmp_transforms');extensions.load('kissmp_input');extensions.load('kissmp_gearbox');extensions.hook('onKissMPVehLoaded');extensions.hook('kissUpdateOwnership',false)")
 end
 local radius=footprint(obj);if not radius or not c.planetCollisionCoverage(pose.position,pose.linear_velocity,radius) then if obj:getActive() then obj:setActive(0) end;return end
 if not obj:getActive() then obj:setActive(1);obj:queueLuaCommand("if kissmp_transforms and kissmp_transforms.planetReset then kissmp_transforms.planetReset() end") end
 local transform={position=pose.position,rotation=pose.orientation,velocity=pose.linear_velocity,
   angular_velocity={pose.angular_velocity[3],pose.angular_velocity[1],pose.angular_velocity[2]},time_past=.001}
 local buffer=require('string.buffer');obj:queueLuaCommand(string.format("kissmp_transforms.set_target_transform(%q)",buffer.encode(transform)))
 local e=state.electrics or {throttle_input=state.inputs[1],brake_input=state.inputs[2],steering_input=state.inputs[3],parkingbrake=state.inputs[4],clutch=0}
 obj:queueLuaCommand(string.format("kissmp_input.apply(%q)",buffer.encode(e)))
 if state.gearbox then obj:queueLuaCommand(string.format("kissmp_gearbox.apply(%q)",buffer.encode(state.gearbox))) end
end
local previousReset=adapter.resetPredictors
function adapter.resetPredictors(id)
 previousReset(id)
 for vehicle,obj in pairs(remoteObjects) do if not id or vehicle==id then if obj:getActive() then obj:setActive(0) end;obj:queueLuaCommand("if kissmp_transforms and kissmp_transforms.planetReset then kissmp_transforms.planetReset() end") end end
end
function adapter.tick(dt,s,self)
 conversionGrace=math.max(0,conversionGrace-1)
 local v=getPlayerVehicle(0);if not v then return end
 if not s.spawnRequested then
  local model=v:getJBeamFilename();local config=v.partConfig
  if type(config)=='string' then config=config:gsub('^/','') end
  if not adapter.vehicleAvailable({model=model,config=config}) then self.failSession('vehicleUnavailable');return end
  if type(config)=='string' and model and hashStringSHA256 then
   s.spawnRequested=hashStringSHA256(s.owner..s.operationId..'vehicle'..tostring(s.vehicleSerial or 0)):sub(1,32):lower()
   self.sendCommand({type='Spawn',vehicle={vehicle_id=s.spawnRequested,vehicle_epoch='1',owner=s.owner,model=model,config=config,colors={{1,1,1,1,0,0,0,0},{1,1,1,1,0,0,0,0},{1,1,1,1,0,0,0,0}}}})
  end
 end
 s.poseTimer=(s.poseTimer or 0)+dt;s.interestTimer=(s.interestTimer or 0)+dt
 if s.poseTimer>=1/30 then s.poseTimer=0;v:queueLuaCommand(string.format("extensions.load('kissmp_planet');kissmp_planet.sample(%q,%q,%q,%q)",s.operationId,s.generation,s.frame.frame_epoch,s.frame.frame_id)) end
 if s.interestTimer>=.25 then
  s.interestTimer=0;local p=v:getPosition();local n=frames.toNetworkPose({position={p.x,p.y,p.z},orientation={0,0,0,1},linear_velocity={0,0,0},angular_velocity={0,0,0}},s.frame)
  if n then
   local x,y,z=unpack(n.position);local a=6378137;local e2=6.6943799901413165e-3;local r=math.sqrt(x*x+y*y);local lat=math.atan2(z,r*(1-e2))
   for i=1,6 do local sn=math.sin(lat);local N=a/math.sqrt(1-e2*sn*sn);lat=math.atan2(z+e2*N*sn,r) end
   local h=r*math.cos(lat)+z*math.sin(lat)-a*math.sqrt(1-e2*math.sin(lat)^2)
   self.sendCommand({type='Interest',centre_wgs84={math.deg(lat),math.deg(math.atan2(y,x)),h},radius=2000})
  end
 end
 -- Re-check every engine frame, including when no fresh remote datagram arrives.
 for id,obj in pairs(remoteObjects) do local position=obj:getPosition();local velocity=obj:getVelocity();local p=v:getPosition();local d=(position-p):length()
  local radius=footprint(obj);if d>250 or not radius or not core().planetCollisionCoverage({position.x,position.y,position.z},{velocity.x,velocity.y,velocity.z},radius) then if obj:getActive() then obj:setActive(0) end
  elseif obj:getActive() and not conversion then obj:queueLuaCommand(string.format('kissmp_transforms.update(%.9g)',math.min(dt,.05))) end
 end
end

local instance=M.new(adapter)
function adapter.poll(s)
 local lease=ext('beamngEarthAutostart');local status=lease and lease.getPlanetHelperStatus and lease.getPlanetHelperStatus();local lan=status and status.lan
 if lan and lan.operationId==s.operationId and lan.generation==s.generation and lan.state=='error' then
  if s.errorCode~=lan.errorCode then instance.failSession(lan.errorCode or 'sessionRejected') end;return
 end
 if network() and s.connected then return end
 if s.phase=='error' then return end
 if not lan or lan.operationId~=s.operationId or lan.generation~=s.generation or (lan.state~='lobby' and lan.state~='preparing' and lan.state~='active') then return end
 local path='settings/beamngEarthAutostart/lan/'..s.operationId..'-'..s.generation..'/launch.json';local cfg=jsonReadFile and jsonReadFile(path)
 if not cfg or not cfg.intent or cfg.intent.operationId~=s.operationId or cfg.intent.generation~=s.generation or cfg.bridgeAddress~='127.0.0.1:7894' or cfg.earthProxyAddress~='127.0.0.1:47615' or not id(cfg.capability) then return end
 if extensions and extensions.load then extensions.load('kissmp_network');extensions.load('kissmp_transform') end
 local n=network();if not n then return end
 local c=core();if c and c.planetBeginSession then c.planetBeginSession(s.operationId,s.generation) end
 local ok=n.planetConnect({bridgeAddress=cfg.bridgeAddress,capability=cfg.capability,generation=s.generation,onControl=instance.onControl,onState=instance.onState,onDisconnect=function() local active=instance.raw();if active==s then instance.failSession('disconnected') end end})
 s.connected=ok==true
end
for _,name in ipairs({'requestSession','bindLaunch','acceptIdentity','getSessionStatus','getLaunchBarrier','onLocalReadiness','onControl','onState','publishPose','update','leaveSession','leave','isManaged','identity'}) do M[name]=instance[name] end
function M.onVehiclePose(localId,encoded,operation,generation,frameEpoch,frameId)
 local s=instance.raw();if not s or not s.resumed or conversion or conversionGrace>0 then return false end
 if operation~=s.operationId or generation~=s.generation or frameEpoch~=s.frame.frame_epoch or frameId~=s.frame.frame_id then return false end
 local v=s.vehicles[s.spawnRequested];local car=getPlayerVehicle(0);if not v or not car or car:getID()~=localId then return false end
 local ok,pose=pcall(jsonDecode,encoded);if not ok then return false end;return instance.publishPose(pose,v)
end
function M.onFrameConversion(active) conversion=active==true;conversionGrace=3;adapter.resetPredictors() end
function M.onVehicleResetted(localId)
 local s=instance.raw();if not s or not s.resumed or conversion or conversionGrace>0 then return end
 local v=s.vehicles[s.spawnRequested];local car=getPlayerVehicle(0);if v and car and car:getID()==localId then instance.sendCommand({type='Reset',vehicle_id=v.vehicle_id,vehicle_epoch=v.vehicle_epoch}) end
end
function M.onVehicleDestroyed(localId)
 local s=instance.raw();if not s or conversion or conversionGrace>0 then return end
 local v=s.vehicles[s.spawnRequested];if v and v.localId==localId then instance.sendCommand({type='Remove',vehicle_id=v.vehicle_id,vehicle_epoch=v.vehicle_epoch});s.spawnRequested=nil;s.vehicleSerial=(s.vehicleSerial or 0)+1 end
end
M.onUpdate=M.update
-- This owner is created in the menu, before stock level startup unloads auto
-- extensions. Keep its transaction and transport callbacks until explicit leave.
M.onExtensionLoaded=function() setExtensionUnloadMode(M,'manual') end
M.onExtensionUnloaded=function() instance.leaveSession() end
return M

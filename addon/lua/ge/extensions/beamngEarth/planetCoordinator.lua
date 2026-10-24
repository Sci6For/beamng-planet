-- Owns cover, input and launch ordering. Physics pause belongs exclusively to core.
local M={}
local function copy(v)
  if type(v)~='table' then return v end
  local out={};for k,x in pairs(v) do out[k]=copy(x) end;return out
end
local function decimal(v) return type(v)=='string' and v:match('^%d+$') and (v=='0' or v:sub(1,1)~='0') and (#v<20 or (#v==20 and v<='18446744073709551615')) end
local ranks={selected=1,loadingLevel=2,terrain=3,nearCells=4,vehicle=5,settling=6,ready=7}
local facts={'importsComplete','terrainReady','supportReady','vehicleReady','materialsReady','placed','cameraReady'}
function M.new(engine,earth,leases,clock)
  local self,current={},nil
  local value={phase='selected',covered=false,stalled=false}
  local function publish() engine.cover(copy(value)) end
  local function exitTag(a)
    if a.tag then a.tag=false;engine.exit('beamngPlanetLaunch') end
  end
  local function fail(a,code)
    if current~=a then return end
    value.phase,value.errorCode,value.stalled='error',code or 'launchFailed',false
    exitTag(a);publish()
  end
  local function begin(a)
    if current~=a or a.preparing or not a.stockAck or not a.coverAck then return end
    a.preparing=true
    engine.prepare(function()
      if current~=a then return end
      if not engine.available(a.spec.vehicle) then fail(a,'vehicleUnavailable');return end
      local result=engine.applySetup(a.spec.id,a.spec.setup)
      if not result or not result.ok then fail(a,result and result.errorCode or 'setupFailed');return end
      a.started=true
      if not engine.start(copy(a.spec)) then fail(a,'levelUnavailable');return end
      -- Stock loading is free to finish/resume naturally. Our opaque cover stays.
      exitTag(a)
    end)
  end
  function self.start(snapshot)
    if current or value.cleaning then return {ok=false,errorCode='launchInProgress'} end
    local validation=require('beamngEarth/planetLaunch')
    local checked=validation.stage(validation.new(),snapshot)
    if not checked.ok then return checked end
    if not engine.available(snapshot.vehicle) then return {ok=false,errorCode='vehicleUnavailable'} end
    local result=earth.stageLaunch(snapshot)
    if not result or not result.ok then return result or {ok=false,errorCode='coreUnavailable'} end
    local a={spec=copy(snapshot),last=clock(),maxima={phase=2},serial=-1}
    a.lan=engine.lanIdentity and copy(engine.lanIdentity()) or nil
    current=a
    value={id=a.spec.id,phase='loadingLevel',covered=true,stalled=false,lan=a.lan~=nil}
    if not leases.launching(a.spec.id) then fail(a,'leaseUnavailable');return {ok=false,errorCode='leaseUnavailable'} end
    engine.input(a.spec.id,true);a.input=true
    publish()
    a.tag=true
    engine.enter('beamngPlanetLaunch',function()
      if current~=a then return end
      a.stockAck=true;begin(a)
    end)
    return {ok=true}
  end
  function self.acknowledgeCover(id)
    local a=current
    if not a or id~=a.spec.id or value.phase=='error' then return {ok=false,errorCode='staleLaunch'} end
    a.coverAck=true;begin(a);return {ok=true}
  end
  local function finishCancel(a,after)
    exitTag(a);engine.clearSetup(a.spec.id)
    if a.input then engine.input(a.spec.id,false) end
    value.cleaning=false
    if after then after() else
      leases.preview(a.spec.id);value.covered=false;publish()
    end
  end
  local function cancel(after)
    local a=current
    if not a then return {ok=false,errorCode='noLaunch'} end
    current=nil -- invalidate EVERY callback before any teardown or external call
    value={id=a.spec.id,phase='cancelled',covered=true,stalled=false,cleaning=true}
    earth.cancelLaunch(a.spec.id);leases.preview(a.spec.id);publish()
    engine.teardown(function() finishCancel(a,after) end)
    return {ok=true}
  end
  function self.cancel() return cancel() end
  function self.retireReady()
    local a=current
    if not a or value.phase~='ready' then return {ok=false,errorCode='notReady'} end
    current=nil
    earth.cancelLaunch(a.spec.id);engine.clearSetup(a.spec.id)
    value={id=a.spec.id,phase='cancelled',covered=false,stalled=false,externalEnd=true}
    publish();return {ok=true}
  end
  function self.retry(snapshot)
    if not current or (not value.stalled and value.phase~='error') then return {ok=false,errorCode='notRecoverable'} end
    local validate=require('beamngEarth/planetLaunch')
    if not validate.stage(validate.new(),snapshot).ok or snapshot.id==current.spec.id then return {ok=false,errorCode='invalidLaunchSpec'} end
    local retained=copy(snapshot)
    return cancel(function()
      local result=self.start(retained)
      if not result.ok then
        -- Even a pre-stage retry failure has a Back action under the retained
        -- cover. This recovery context owns no pause, loading tag or input.
        current=current or {spec=retained,last=clock(),maxima={},serial=-1}
        value={id=retained.id,phase='error',covered=true,stalled=false,errorCode=result.errorCode};publish()
      end
    end)
  end
  function self.continueWaiting()
    if not current or value.phase=='error' or not value.stalled then return {ok=false,errorCode='notStalled'} end
    current.last=clock();value.stalled=false;publish();return {ok=true}
  end
  function self.update(dtReal)
    local a=current
    if not a or value.phase=='error' or value.phase=='ready' then return end
    local helper=engine.helperStatus and engine.helperStatus()
    if helper and helper.state=='error' then fail(a,helper.errorCode);return end
    if a.lan then
      local lan=helper and helper.lan
      if lan and lan.operationId==a.lan.operationId and lan.generation==a.lan.generation and lan.state=='error' then
        fail(a,lan.errorCode or 'sessionRejected');return
      end
      local barrier=engine.lanBarrier and engine.lanBarrier(a.spec.id,copy(a.lan))
      if barrier and barrier.errorCode then fail(a,barrier.errorCode);return end
    end
    local s=earth.getLaunchStatus(a.spec.id)
    if s and s.id==a.spec.id then
      -- Core commit serial is monotonic across sessions. Never reaccept an old
      -- committed server generation after observing a newer one.
      local serial=tonumber(s.commitSerial)
      if not serial or serial>=a.serial then
        local progress=false
        if serial and serial>a.serial then a.serial=serial;a.maxima={};progress=true end
        local function achieved(key,n)
          if n>(a.maxima[key] or 0) then a.maxima[key]=n;progress=true end
        end
        achieved('phase',ranks[s.phase] or 0)
        for _,key in ipairs(facts) do if s[key]==true then achieved(key,1) end end
        for _,pair in ipairs({{'readyCells','requiredCells'},{'nearSurfaceReady','nearSurfaceRequired'}}) do
          local n,limit=tonumber(s[pair[1]]),tonumber(s[pair[2]])
          if n and limit and n>=0 and limit>0 then achieved(pair[1],math.min(n,limit)) end
        end
        local stable=tonumber(s.stableSeconds)
        if stable and stable==stable and stable>0 then achieved('stableSeconds',math.min(.75,stable)) end
        if progress then a.last=clock();value.stalled=false end
        local covered,stalled=value.covered,value.stalled
        value=copy(s);value.lan=a.lan~=nil;value.covered=covered;value.stalled=stalled
        -- The staged pure model says selected until the mission consumes it.
        if value.phase=='selected' then value.phase='loadingLevel' end
        if s.phase=='error' then value.stalled=false;exitTag(a)
        elseif s.phase=='ready' then
          local resumed=true
          if a.lan then
            if engine.onLocalReadiness then engine.onLocalReadiness(a.spec.id,copy(s)) end
            local barrier=engine.lanBarrier and engine.lanBarrier(a.spec.id,copy(a.lan))
            resumed=barrier and barrier.launchId==a.spec.id and barrier.operationId==a.lan.operationId and barrier.generation==a.lan.generation and barrier.resumed==true
              and decimal(barrier.worldEpoch)
              and decimal(barrier.frameEpoch)
              and type(barrier.frameId)=='string' and #barrier.frameId==32 and not barrier.frameId:find('[^0-9a-f]')
              and type(barrier.frameOperation)=='string' and #barrier.frameOperation==32 and not barrier.frameOperation:find('[^0-9a-f]')
              and barrier.serverSession==s.serverSession and barrier.anchorId==s.anchorId and barrier.commitSerial==s.commitSerial
            value.localReady=true;value.lanReady=resumed==true
            if barrier and barrier.errorCode then fail(a,barrier.errorCode);return end
          end
          if not resumed then
            value.phase='network'
            a.networkWaitStarted=a.networkWaitStarted or clock()
            if clock()-a.networkWaitStarted>=120 then fail(a,'readyTimeout');return end
          elseif a.started and a.coverAck and s.serverSession~=nil and s.anchorId~=nil and serial~=nil then
            if leases.active(a.spec.id) then
              value.covered=false;exitTag(a);engine.input(a.spec.id,false);a.input=false
            else fail(a,'leaseUnavailable');return end
          else value.phase='loadingLevel' end
        end
      end
    end
    if value.phase~='ready' and value.phase~='error' and clock()-a.last>=120 then value.stalled=true end
    publish()
  end
  function self.status() return copy(value) end
  return self
end
return M

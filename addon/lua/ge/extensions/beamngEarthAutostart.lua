local M = {}
local directory = 'settings/beamngEarthAutostart/'
local session, sequence, elapsed, active = nil, 0, 0, false
local lease, loaded = nil, false
local lanMode, lanIntent, lanSequence = 'solo', nil, 0
-- Retired chooser identities survive hot reload in this engine VM. Callers
-- must create a fresh opaque chooser token for each new chooser ownership.
local registry = rawget(_G, 'beamngEarthAutostartLeaseRegistry')
if not registry then
  registry = {retired = {}}
  rawset(_G, 'beamngEarthAutostartLeaseRegistry', registry)
end
local function current() return loaded and registry.owner == M end
local audioSequence,audioElapsed,audioQueue,audioPending,audioStatus=0,0,{},nil,nil
local audioCommands={ping=true,speak=true,cancelSpeech=true,chooseFolder=true,play=true,pause=true,next=true,previous=true,setVolume=true,setShuffle=true,setVoice=true}
function M.planetAudioCommand(value)
  if not current() or type(value)~='table' or not audioCommands[value.command] then return false end
  local command={command=value.command}
  if value.command=='speak' then
    if type(value.text)~='string' or #value.text>1024 or type(value.eventId)~='string' then return false end
    command.text,command.eventId,command.routeVersion=value.text,value.eventId,value.routeVersion
    command.createdAt=os.time()
    if guihooks then guihooks.trigger('BeamNGPlanetNavigation', {text=value.text}) end
    if ui_message then pcall(ui_message,value.text,5,'beamngPlanetNavigation') end
  elseif value.command=='setVolume' then
    if (value.channel~='music' and value.channel~='voice') or type(value.value)~='number' or value.value~=value.value or value.value<0 or value.value>1 then return false end
    command.channel,command.value=value.channel,value.value
  elseif value.command=='setShuffle' or value.command=='setVoice' then
    if type(value.value)~='boolean' then return false end
    command.value=value.value
  elseif value.command=='play' then
    if value.trackId~=nil and (type(value.trackId)~='string' or not value.trackId:match('^%d+$')) then return false end
    command.trackId=value.trackId
  end
  if value.command=='speak' or value.command=='cancelSpeech' then
    for i=#audioQueue,1,-1 do if audioQueue[i].command=='speak' then table.remove(audioQueue,i) end end
  end
  if #audioQueue>=16 then return false end
  if value.command=='cancelSpeech' then table.insert(audioQueue,1,command) else audioQueue[#audioQueue+1]=command end
  return true
end
function M.getPlanetAudioStatus()
  local ok,status=pcall(jsonReadFile,directory..'audio-status.json')
  if ok and type(status)=='table' and status.session==session and status.version==1 then audioStatus=status end
  return audioStatus or {ready=false,error='Подключение звука…',tracks={}}
end
local function audioPoll()
  if not audioPending and #audioQueue==0 then return end
  local ok,status=pcall(jsonReadFile,directory..'audio-status.json')
  if ok and type(status)=='table' and status.session==session and status.version==1 then
    audioStatus=status
    if audioPending and type(status.sequence)=='number' and status.sequence>=audioPending.sequence then audioPending=nil end
  end
  if not audioPending and #audioQueue>0 then
    audioPending=table.remove(audioQueue,1);audioSequence=audioSequence+1
    audioPending.session,audioPending.sequence,audioPending.version=session,audioSequence,1
  end
  if audioPending then pcall(jsonWriteFile,directory..'audio-request.json',audioPending,false) end
end
local function retireLease()
  if registry.token then registry.retired[registry.token] = true end
  registry.token, lease = nil, nil
  lanMode, lanIntent = 'solo', nil
end
local function validToken(token)
  -- Lua strings are bytes; count UTF-8 characters to match Rust's char limit.
  if type(token) ~= 'string' or token:match('^%s*$') then return false end
  local _, count = token:gsub('[^\128-\191]', '')
  return count <= 128
end
local lastNotice, writeFailed = nil, false
local messages = {
  waiting = 'BeamNG Earth: помощник ожидает карту Earth.',
  starting = 'BeamNG Earth: сервер запускается.',
  ready = 'BeamNG Earth: сервер готов.',
  stopping = 'BeamNG Earth: сервер останавливается.'
}
local errors = {
  invalidConfig = 'Проверьте настройки помощника.',
  profileUnavailable = 'Профиль игры недоступен для записи.',
  gameConflict = 'Обнаружена другая сессия игры или помощника.',
  portBusy = 'Порт сервера занят другим процессом.',
  backendHashMismatch = 'Контрольная сумма сервера не совпадает.',
  backendStartFailed = 'Не удалось запустить сервер.',
  backendReadyTimeout = 'Сервер не успел подготовиться.',
  backendFailed = 'Сервер завершился с ошибкой.',
  retryLimit = 'Повторы запуска сервера исчерпаны. Проверьте журнал помощника.'
}
local function notice(message)
  if ui_message then pcall(ui_message, message, 6, 'beamngEarthAutostart') end
end
local lanStates={off=true,starting=true,connecting=true,lobby=true,preparing=true,active=true,leaving=true,error=true}
local lanErrors={invalidConfig=true,portBusy=true,hashMismatch=true,startFailed=true,processFailed=true,readyTimeout=true,invalidStatus=true}
local function validDecimal(v)
  return type(v)=='string' and v:match('^%d+$') and (v=='0' or v:sub(1,1)~='0') and (#v<20 or (#v==20 and v<='18446744073709551615'))
end
local function writeRequest()
  if not current() then return end
  local state, levelId, chooserToken, launchId = 'idle', '', '', ''
  if lease and lease.kind == 'launching' then
    state, levelId = 'launching', 'beamng_earth'
    chooserToken, launchId = lease.chooserToken, lease.launchId
  elseif lease and lease.kind == 'preview' then
    state, chooserToken = 'preview', lease.chooserToken
  elseif active then
    state, levelId = 'active', 'beamng_earth'
    if lease and lease.kind == 'active' then
      chooserToken, launchId = lease.chooserToken, lease.launchId
    end
  end
  sequence = sequence + 1
  local ok, result = pcall(jsonWriteFile, directory .. 'request.json', {
    formatVersion = 3, session = session, sequence = sequence,
    state = state, levelId = levelId, chooserToken = chooserToken, launchId = launchId,
    mode = lanMode, lan = lanIntent
  }, true)
  local failed = not ok or result == false
  if failed and not writeFailed then notice('BeamNG Earth: не удалось записать запрос в профиль игры.') end
  writeFailed = failed
end
function M.getPlanetHelperStatus()
  if not current() then return nil end
  local ok, status = pcall(jsonReadFile, directory .. 'status.json')
  if not ok or type(status) ~= 'table' or status.formatVersion ~= 3 or status.session ~= session then return nil end
  if status.state=='error' then
    if type(status.errorCode)~='string' or not errors[status.errorCode] then return nil end
  elseif not messages[status.state] then return nil end
  local lan = status.lan
  if type(lan)~='table' or not lanIntent or lan.operationId~=lanIntent.operationId or lan.generation~=lanIntent.generation or type(lan.acknowledgedSequence)~='number' or lan.acknowledgedSequence>sequence or lan.acknowledgedSequence<lanSequence then lan=nil end
  if lan and (not lanStates[lan.state] or not validDecimal(lan.statusSequence) or (lan.state=='error' and not lanErrors[lan.errorCode]) or (lan.state~='error' and lan.errorCode~=nil)) then lan=nil end
  if lan then
    local value={operationId=lan.operationId,generation=lan.generation,acknowledgedSequence=lan.acknowledgedSequence,statusSequence=lan.statusSequence,state=lan.state,errorCode=lan.errorCode}
    if type(lan.ticket)=='table' and type(lan.ticket.endpoint)=='table' then
      local t=lan.ticket
      value.ticket={endpoint={address=t.endpoint.address,port=t.endpoint.port},roomId=t.roomId,certificateSha256=t.certificateSha256,joinSecret=t.joinSecret}
    end
    lan=value
  end
  return {formatVersion=3,session=session,state=status.state,errorCode=status.state=='error' and status.errorCode or nil,lan=lan}
end
function M.setPlanetLanIntent(mode, intent, chooserToken)
  if not current() or not lease or lease.chooserToken~=chooserToken or registry.retired[chooserToken] then return false end
  if mode=='solo' then lanMode,lanIntent='solo',nil;writeRequest();return true end
  if mode~='host' and mode~='join' then return false end
  if type(intent)~='table' or type(intent.operationId)~='string' or #intent.operationId~=32 or intent.operationId:find('[^0-9a-f]') or not validDecimal(intent.generation) then return false end
  if intent.state~='start' and intent.state~='leave' then return false end
  if intent.state=='leave' and (not lanIntent or lanMode~=mode or lanIntent.operationId~=intent.operationId or lanIntent.generation~=intent.generation) then return false end
  if type(intent.endpoint)~='table' or type(intent.endpoint.address)~='string' or type(intent.endpoint.port)~='number' or intent.endpoint.port<1 or intent.endpoint.port>65535 or intent.endpoint.port%1~=0 then return false end
  if type(intent.playerName)~='string' or #intent.playerName>256 then return false end
  -- Copy allowlisted fields; caller tables cannot mutate a published lease.
  lanMode=mode
  lanIntent={operationId=intent.operationId,generation=intent.generation,state=intent.state,endpoint={address=intent.endpoint.address,port=intent.endpoint.port},playerName=intent.playerName}
  if type(intent.ticket)=='table' then lanIntent.ticket={roomId=intent.ticket.roomId,certificateSha256=intent.ticket.certificateSha256,joinSecret=intent.ticket.joinSecret} end
  lanSequence=sequence+1
  writeRequest();return true
end
local function readStatus()
  local status=M.getPlanetHelperStatus()
  if not status then return end
  local message = messages[status.state]
  local key = status.state
  if status.state == 'error' then
    if type(status.errorCode) ~= 'string' or not errors[status.errorCode] then return end
    message = 'BeamNG Earth: ' .. errors[status.errorCode]
    key = key .. ':' .. status.errorCode
  end
  if not message then return end
  if key ~= lastNotice then notice(message); lastNotice = key end
end
local function heartbeat()
  writeRequest()
  readStatus()
end
function M.setPlanetLease(kind, chooserToken, launchId)
  if not current() or not validToken(chooserToken) or type(launchId) ~= 'string'
      or registry.retired[chooserToken] then return false end
  if kind == 'preview' then
    if launchId ~= '' then return false end
    if lease and lease.chooserToken == chooserToken then
      return lease.kind == 'preview'
    end
    retireLease()
    lease = {kind = kind, chooserToken = chooserToken, launchId = '', retiredLaunches = {}}
  elseif kind == 'launching' or kind == 'active' then
    if not validToken(launchId) or not lease or lease.chooserToken ~= chooserToken then return false end
    if lease.retiredLaunches[launchId] then return false end
    if kind == 'active' then
      if not active or lease.kind == 'preview' or lease.launchId ~= launchId then return false end
    elseif lease.launchId == launchId and lease.kind == 'active' then return false end
    -- Explicit retry by the current chooser can replace a launch. Late hooks
    -- carrying the previous id cannot reacquire it, even during mission-end.
    if lease.launchId ~= '' and lease.launchId ~= launchId then
      lease.retiredLaunches[lease.launchId] = true
    end
    lease.kind, lease.launchId = kind, launchId
  else return false end
  registry.token, elapsed = chooserToken, 0
  heartbeat()
  return true
end
function M.returnPlanetPreview(chooserToken,launchId)
  if not current() or not lease or lease.chooserToken~=chooserToken or not validToken(launchId) then return false end
  if lease.kind=='preview' then return lease.returnedLaunchId==launchId end
  if lease.launchId~=launchId then return false end
  lease.retiredLaunches[launchId]=true
  lease.kind,lease.launchId,lease.returnedLaunchId='preview','',launchId
  elapsed=0;heartbeat();return true
end
function M.clearPlanetLease(chooserToken)
  if not current() or not lease or lease.chooserToken ~= chooserToken then return false end
  retireLease()
  elapsed = 0
  heartbeat()
  return true
end
function M.onExtensionLoaded()
  -- Invalidate the previous module even if its unload hook was skipped.
  if registry.token then registry.retired[registry.token] = true end
  registry.owner, registry.token = M, nil
  loaded, lease = true, nil
  -- This counter survives extension reloads in the same engine VM, including
  -- reloads in one wall-clock second. A new VM also gets a different time/id.
  local counter = (rawget(_G, 'beamngEarthAutostartSessionCounter') or 0) + 1
  rawset(_G, 'beamngEarthAutostartSessionCounter', counter)
  session = tostring(os.time()) .. ':' .. tostring(counter) .. ':' .. tostring({})
  sequence, elapsed, lastNotice, writeFailed = 0, 0, nil, false
  audioSequence,audioElapsed,audioQueue,audioPending,audioStatus=0,0,{},nil,nil
  local ok = pcall(function() FS:directoryCreate(directory, true) end)
  if not ok then notice('BeamNG Earth: не удалось создать каталог настроек в профиле игры.') end
  active = scenetree.MissionGroup ~= nil and getCurrentLevelIdentifier() == 'beamng_earth'
  heartbeat()
end
function M.onUpdate(dtReal)
  if not current() then return end
  -- Simulation time can stop in pause; real frame time still drives the IPC.
  elapsed = elapsed + math.max(tonumber(dtReal) or 0, 0)
  audioElapsed=audioElapsed+math.max(tonumber(dtReal) or 0,0)
  if audioElapsed>=0.5 then audioElapsed=0;audioPoll() end
  if elapsed >= 2 then elapsed = elapsed % 2; heartbeat() end
end
function M.onClientStartMission()
  if not current() then return end
  active = getCurrentLevelIdentifier() == 'beamng_earth'
  -- Planet stays launching until its coordinator proves world readiness.
  if lease and lease.kind == 'active' and not active then retireLease() end
  elapsed = 0
  heartbeat()
end
function M.onClientEndMission()
  if not current() then return end
  M.planetAudioCommand({command='cancelSpeech'})
  if lease and lease.kind == 'active' then retireLease() end
  -- End hook runs before the old level's objects/identifier are cleared.
  active, elapsed = false, 0
  heartbeat()
end
function M.onExtensionUnloaded()
  if not current() then return end
  retireLease()
  active = false
  writeRequest()
  loaded = false
end
return M

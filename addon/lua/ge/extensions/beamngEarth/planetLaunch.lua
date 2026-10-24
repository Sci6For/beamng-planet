-- Pure launch intent and identity model. It outlives core's per-level reset.
local M = {}

local function finite(n) return type(n) == 'number' and n == n and math.abs(n) < math.huge end
local function text(s, limit) return type(s) == 'string' and #s > 0 and #s <= limit end
local function fields(t, allowed)
  if type(t) ~= 'table' or getmetatable(t) ~= nil then return false end
  for k in pairs(t) do if not allowed[k] then return false end end
  return true
end
local function copy(t)
  if type(t) ~= 'table' then return t end
  local out = {}
  for k, v in pairs(t) do out[k] = copy(v) end
  return out
end
local function unit(n) return finite(n) and n >= 0 and n <= 1 end
local function paintsValid(paints)
  if paints == nil then return true end
  if type(paints) ~= 'table' or getmetatable(paints) ~= nil then return false end
  local count = 0
  for k, paint in pairs(paints) do
    if type(k) ~= 'number' or k % 1 ~= 0 or k < 1 or k > 3 then return false end
    count = count + 1
    if not fields(paint, {baseColor=true, metallic=true, roughness=true, clearcoat=true, clearcoatRoughness=true}) then return false end
    if type(paint.baseColor) ~= 'table' or getmetatable(paint.baseColor) ~= nil then return false end
    local components = 0
    for i, n in pairs(paint.baseColor) do
      if type(i) ~= 'number' or i % 1 ~= 0 or i < 1 or i > 4 or not unit(n) then return false end
      components = components + 1
    end
    if components ~= 4 then return false end
    for _, key in ipairs({'metallic', 'roughness', 'clearcoat', 'clearcoatRoughness'}) do
      if paint[key] ~= nil and not unit(paint[key]) then return false end
    end
  end
  return count == #paints
end
-- Configurator choices are retained; Task 5 translates parkedOnly/police into
-- the stock helper's flags. fromSetting is the helper's existing default.
local traffic = {fromSetting=true, disabled=true, parkedOnly=true, enabled=true, police=true}
local timePlay = {disabled=true, level=true, fast=true, normal=true, slow=true, realtime=true}
local namedTime = {default=true, current=true, currentUtc=true, sunrise=true, morning=true,
  earlyNoon=true, noon=true, lateNoon=true, afternoon=true, evening=true, sunset=true, night=true}
local function valid(spec)
  if not fields(spec, {version=true,id=true,lat=true,lon=true,name=true,vehicle=true,setup=true})
      or spec.version ~= 1 or not text(spec.id, 128)
      or not finite(spec.lat) or spec.lat < -90 or spec.lat > 90
      or not finite(spec.lon) or spec.lon < -180 or spec.lon > 180
      or (spec.name ~= nil and (type(spec.name) ~= 'string' or #spec.name > 512)) then return false end
  local v = spec.vehicle
  if not fields(v, {model=true,config=true,paints=true}) or not text(v.model, 128)
      or not v.model:match('^[%w_-]+$') or not text(v.config, 512) then return false end
  -- Installed mods use decimal config names (for example Serenity's 4.0ex).
  -- Keep these as data while rejecting path traversal and executable strings.
  if v.config:find('..',1,true) or (not v.config:match('^[%w_%.%- ]+$') and not v.config:match('^vehicles/[%w_-]+/[%w_%.%- ]+%.pc$')) then return false end
  if not paintsValid(v.paints) then return false end
  local s = spec.setup
  if not fields(s, {trafficMode=true,timeOfDay=true,date=true,timePlay=true}) then return false end
  if s.trafficMode ~= nil and not traffic[s.trafficMode] then return false end
  if s.timePlay ~= nil and not timePlay[s.timePlay] then return false end
  if s.timeOfDay ~= nil and not namedTime[s.timeOfDay] and not unit(s.timeOfDay) then return false end
  if s.date ~= nil and (not finite(s.date) or s.date % 1 ~= 0 or math.abs(s.date) > 8640000000000) then return false end
  return true
end

function M.new() return {usedIds={}, managed=false, serial=0} end
function M.current(state)
  if not state.intent or state.value.phase == 'cancelled' or state.value.phase == 'error' then return nil end
  return copy(state.intent)
end
-- A failed covered attempt still owns startup until explicitly cancelled.
function M.ordinaryAllowed(state)
  return not state.preview and (not state.intent or state.value.phase == 'cancelled' or state.value.phase == 'ready')
end
function M.stage(state, spec)
  if not valid(spec) then return {ok=false,errorCode='invalidLaunchSpec'} end
  if M.current(state) then return {ok=false,errorCode='launchInProgress'} end
  if state.usedIds[spec.id] then return {ok=false,errorCode='launchIdReused'} end
  state.intent, state.consumed, state.preview, state.managed = copy(spec), false, nil, true
  state.usedIds[spec.id] = true
  state.requestId = nil
  state.value = {id=spec.id,phase='selected',stalled=false}
  return {ok=true}
end
function M.consume(state, levelId)
  if levelId ~= 'beamng_earth' or state.consumed or not M.current(state) then return nil end
  state.consumed, state.value.phase = true, 'loadingLevel'
  return copy(state.intent)
end
function M.cancel(state, id)
  if not state.value or id ~= state.value.id then return {ok=false} end
  state.value.phase, state.value.serverSession, state.value.anchorId = 'cancelled', nil, nil
  state.value.commitSerial, state.requestId = nil, nil
  return {ok=true}
end
function M.request(state, id, session, requestId)
  if not M.current(state) or not state.consumed or id ~= state.intent.id
      or session == nil or requestId == nil then return false end
  state.requestId = requestId
  state.value.serverSession, state.value.phase = session, 'terrain'
  state.value.anchorId, state.value.commitSerial, state.value.errorCode = nil, nil, nil
  return true
end
function M.accept(state, id, session, requestId)
  return M.current(state) ~= nil and state.consumed == true and id ~= nil and session ~= nil
    and requestId ~= nil and id == state.intent.id and session == state.value.serverSession
    and requestId == state.requestId
end
function M.committed(state, id, session, requestId, anchorId)
  if anchorId == nil or not M.accept(state,id,session,requestId) then return false end
  state.serial = state.serial + 1
  state.value.anchorId, state.value.commitSerial = anchorId, state.serial
  state.requestId = nil
  -- Import is an identity fact, not proof of a playable world (Task 2).
  return true
end
function M.fail(state, id, code)
  if not state.value or state.value.id ~= id then return false end
  state.value.phase, state.value.errorCode, state.requestId = 'error', code, nil
  return true
end
-- The snapshot/consumed identity belongs to the confirmed attempt. Requests
-- and imported anchors belong to the world that resetState is about to drop.
function M.resetWorld(state)
  state.requestId = nil
  if state.value then
    state.value.serverSession, state.value.anchorId, state.value.commitSerial = nil, nil, nil
    if state.consumed and M.current(state) then state.value.phase = 'loadingLevel' end
  end
end
function M.disconnect(state) M.resetWorld(state) end
function M.preview(state, token)
  if token ~= nil and not text(token,128) then return {ok=false,errorCode='invalidPreviewToken'} end
  state.managed, state.preview = true, token
  if token ~= nil then
    state.intent, state.value, state.requestId, state.consumed = nil, nil, nil, false
  end
  return {ok=true}
end
function M.status(state, id)
  if not state.value or state.value.id ~= id then return nil end
  return copy(state.value)
end
return M

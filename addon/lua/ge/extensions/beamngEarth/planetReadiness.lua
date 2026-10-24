-- Bounded, engine-independent predicates. Missing facts never establish readiness.
local M = {}
local function finite(n) return type(n) == 'number' and n == n and math.abs(n) < math.huge end
local function vector(v) return v and finite(v.x) and finite(v.y) and finite(v.z) end
local function copy(t)
  if type(t) ~= 'table' then return t end
  local out = {}; for k,v in pairs(t) do out[k] = copy(v) end; return out
end
function M.new(id, radius)
  radius = radius or 128
  assert(radius == 128, 'Planet initial radius is fixed at 128 m')
  return {id=id,radius=radius,box={-radius,-radius,radius,radius},stable=0,
    value={id=id,phase='terrain',requiredCells=4,readyCells=0,stalled=false}}
end
function M.update(s, f, dtReal)
  local v = s.value
  if v.phase == 'cancelled' or v.phase == 'error' then return copy(v) end
  if f.cancelled then v.phase='cancelled'; return copy(v) end
  -- Task 6 owns the attempt-wide 120 s no-progress watchdog. These are facts,
  -- never elapsed-time guesses; collisionSerial is diagnostic, not progress.
  local r=f.roads or {}
  local surface=f.nearSurface or {required=1,ready=f.rendered and 1 or 0}
  v.importsComplete=f.importsComplete == true
  v.terrainReady=f.terrain == true and f.covers == true
  v.nearMode=f.nearMode
  v.nearSurfaceRequired,v.nearSurfaceReady=surface.required,surface.ready
  v.supportReady,v.vehicleReady=f.support == true,f.vehicle == true
  v.materialsReady,v.placed,v.cameraReady=f.materials == true,f.placed == true,f.camera == true
  v.collisionSerial=r.collisionSerial
  v.stableSeconds=s.stable
  v.blockedReason=nil
  local function wait(phase, reason)
    v.phase=phase; s.stable=0; s.origin=nil; s.moving=0
    v.blockedReason=reason or phase
    v.stableSeconds=0
    return copy(v)
  end
  local function fail(code) v.phase='error'; v.errorCode=code; return copy(v) end
  if f.errorCode then return fail(f.errorCode) end
  if f.id ~= s.id or f.serverSession == nil or f.anchorId == nil or f.commitSerial == nil
      or f.currentSession ~= f.serverSession or f.currentAnchor ~= f.anchorId
      or f.currentCommit ~= f.commitSerial then return wait('terrain') end
  v.serverSession,v.anchorId,v.commitSerial=f.serverSession,f.anchorId,f.commitSerial
  if s.commit and s.commit ~= f.commitSerial then return wait('terrain') end
  s.commit=f.commitSerial
  if not f.terrain or not f.importsComplete or not f.rendered or not f.covers
      or not finite(surface.required) or surface.required<=0 or surface.ready~=surface.required then return wait('terrain') end
  v.readyCells=r.ready or 0
  if r.errors and #r.errors > 0 then return fail('nearCellFailed') end
  if r.required ~= v.requiredCells or r.ready ~= v.requiredCells then return wait('nearCells','roadCells') end
  if not r.settled or r.collisionSerial == nil then return wait('nearCells','collisionRebuild') end
  if not f.support then return wait('nearCells','groundSupport') end
  if not f.vehicle or not f.materials or not f.placed or not f.camera then return wait('vehicle') end
  if not vector(f.position) or not vector(f.velocity) or not finite(f.upZ) then return fail('unstableVehicle') end
  local p,w=f.position,f.velocity
  local speed=math.sqrt(w.x*w.x+w.y*w.y+w.z*w.z)
  if f.upZ < 0.5 then return fail('unstableVehicle') end
  if not s.origin then s.origin=copy(p); s.stable=0 end
  local o=s.origin
  if (p.x-o.x)^2+(p.y-o.y)^2+(p.z-o.z)^2 > 1 then return fail('unstableVehicle') end
  v.phase='settling'
  -- safeTeleport places the wheels on the ground, then suspension settles in
  -- real physics. A brief vertical velocity is not a failed placement. Keep
  -- the cover and restart the complete stable window, without moving origin
  -- or relaxing the 1 m displacement/finite/upright/support gates. Persistent
  -- movement still fails after three simulated seconds, never reveals early.
  if speed > 1 or w.z < -0.1 then
    s.stable=0;v.stableSeconds=0
    if finite(f.physicsDt) and f.physicsDt>0 then
      s.moving=(s.moving or 0)+math.min(f.physicsDt,dtReal or 0)
    end
    if (s.moving or 0)>=3 then return fail('unstableVehicle') end
    return copy(v)
  end
  s.moving=0
  -- Actual simulation time is required. A paused frame cannot buy stability.
  if finite(f.physicsDt) and f.physicsDt > 0 then s.stable=s.stable+math.min(f.physicsDt,dtReal or 0) end
  v.stableSeconds=math.min(s.stable,0.75)
  if s.stable >= 0.75 then v.phase='ready' end
  return copy(v)
end
return M

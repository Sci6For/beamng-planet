-- A clipped through road is not a real dead end. Keep newly recycled traffic
-- facing its available continuation, and recycle cars approaching that cut.
-- Stock AI, natural dead ends and one-way directions retain their behavior.
local M = {}
local nodes, cutEnds
local target, original, wrapper

local function roadDirection(a, b)
  if not nodes or not map or map.getMap().nodes ~= nodes or not nodes[a] or not nodes[b] then return nil end
  local edge = nodes[a].links[b] or nodes[b].links[a]
  if not edge or edge.oneWay or not cutEnds or (not not cutEnds[a]) == (not not cutEnds[b]) then return nil end
  return cutEnds[a] and 1 or -1
end

local function uninstall()
  if target and target.finalizeSpawnPoint == wrapper then target.finalizeSpawnPoint = original end
  target, original, wrapper = nil, nil, nil
end

function M.refresh()
  local utils = gameplay_traffic_trafficUtils
  if not nodes or not utils or not utils.finalizeSpawnPoint or target == utils then return end
  uninstall()
  target, original = utils, utils.finalizeSpawnPoint
  local stock = original
  wrapper = function(pos, dir, a, b, options)
    -- Explicit off-road/lateral placement is not ordinary legal traffic.
    if (not dir or (options and options.legalDirection)) and not (options and options.roadLateralXnorm) then
      local desired = a and b and roadDirection(a, b)
      if desired then
        local adjusted = {}
        for k, v in pairs(options or {}) do adjusted[k] = v end
        -- Stock forces roadDir=1 when its width estimate is one lane, even
        -- on a two-way road. Order the endpoints inward so that this forced
        -- direction and the normal multi-lane choice agree.
        if desired < 0 then a, b = b, a end
        adjusted.roadDir = 1
        -- Let stock placement select the matching lane and construct direction
        -- together; rotating a placed car alone would leave it facing oncoming traffic.
        return stock(pos, dir, a, b, adjusted)
      end
    end
    return stock(pos, dir, a, b, options)
  end
  utils.finalizeSpawnPoint = wrapper
end

function M.bind(currentNodes, ends)
  nodes, cutEnds = currentNodes, ends
  M.refresh()
end

function M.headingOut(pos, dir)
  if not nodes or not map or map.getMap().nodes ~= nodes or not map.findClosestRoad then return false end
  local a, b = map.findClosestRoad(pos)
  local inward = a and b and roadDirection(a, b)
  if not inward then return false end
  local delta = nodes[b].pos - nodes[a].pos
  return dir:dot(delta) * inward < 0
end

function M.reset()
  uninstall()
  nodes, cutEnds = nil, nil
end

return M

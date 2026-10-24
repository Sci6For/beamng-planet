-- Pure route guidance. Progress uses GPS's existing projected path, never the rebased world origin.
local M = {}
local version, steps, last, outside = nil, {}, nil, false
local function clamp(v,a,b) return math.max(a,math.min(b,v)) end
local function lanePhrase(lanes)
  if type(lanes)~='table' or #lanes<2 or #lanes>12 then return nil end
  local good={}
  for i,l in ipairs(lanes) do if l.valid==true then good[#good+1]=i end end
  if #good==0 or #good==#lanes then return nil end
  if #good==1 then
    if good[1]==1 then return 'Двигайтесь с левой полосы' end
    if good[1]==#lanes then return 'Двигайтесь с правой полосы' end
    return 'Займите полосу '..tostring(good[1])..' слева'
  end
  if good[#good]-good[1]+1~=#good then return nil end
  if good[1]==1 then return 'Держитесь левее' end
  if good[#good]==#lanes then return 'Держитесь правее' end
  return 'Держитесь средних полос'
end
local function phrase(m)
  if m.type=='arrive' or m.type=='depart' or m.type=='exit roundabout' or m.type=='exit rotary' then return nil end
  if m.type=='roundabout' or m.type=='rotary' then
    local names={'первый','второй','третий','четвёртый','пятый','шестой','седьмой','восьмой'}
    return m.exit and ('На круговом движении выберите '..(names[m.exit] or tostring(m.exit))..' съезд') or 'Въезжайте на круговое движение'
  end
  local modifier=m.modifier or ''
  if modifier=='uturn' then return 'Развернитесь' end
  if m.type=='new name' or m.type=='continue' or m.type=='notification' then return nil end
  local left=modifier:find('left',1,true)
  local right=modifier:find('right',1,true)
  if m.type=='fork' then
    return left and 'Держитесь левее' or right and 'Держитесь правее' or 'Продолжайте движение прямо'
  end
  if m.type=='off ramp' or m.type=='on ramp' then
    return left and 'Съезжайте налево' or right and 'Съезжайте направо' or 'Следуйте на съезд'
  end
  if m.type=='merge' then
    return left and 'Вливайтесь в поток слева' or right and 'Вливайтесь в поток справа' or 'Вливайтесь в поток'
  end
  if modifier:find('left',1,true) then return 'Поверните налево' end
  if modifier:find('right',1,true) then return 'Поверните направо' end
  if m.type=='fork' or m.type=='end of road' then return 'Продолжайте движение прямо' end
end
function M.reset() version,steps,last,outside=nil,{},nil,false end
function M.setRoute(v, points, maneuvers)
  M.reset(); version=v
  if not points or #points<2 then return end
  local scale=111320*math.cos(math.rad(points[1][1]))
  local x,y,cum={},{},{0}
  for i,p in ipairs(points) do
    x[i],y[i]=(p[2]-points[1][2])*scale,(p[1]-points[1][1])*111320
    if i>1 then cum[i]=cum[i-1]+math.sqrt((x[i]-x[i-1])^2+(y[i]-y[i-1])^2) end
  end
  local cursor=1
  for _,m in ipairs(maneuvers or {}) do
    local text=phrase(m)
    local lane=lanePhrase(m.lanes)
    if lane and m.type~='arrive' and m.type~='depart' then text=text and (lane..'. '..text) or lane end
    if text and type(m.lat)=='number' and type(m.lon)=='number' then
      local px,py=(m.lon-points[1][2])*scale,(m.lat-points[1][1])*111320
      local best,score,at,segment=math.huge,math.huge,nil,cursor
      local expected=tonumber(m.distanceFromStart)
      -- A loop can visit the same position more than once. Use OSRM's along-route
      -- distance to distinguish those visits instead of announcing the later turn now.
      for i=cursor,#points-1 do
        local dx,dy=x[i+1]-x[i],y[i+1]-y[i]
        local length2=dx*dx+dy*dy
        local t=length2>0 and clamp(((px-x[i])*dx+(py-y[i])*dy)/length2,0,1) or 0
        local d=(px-x[i]-t*dx)^2+(py-y[i]-t*dy)^2
        local arc=cum[i]+t*math.sqrt(length2)
        local cost=d+(expected and (arc-expected)^2*0.04 or 0)
        if cost<score then best,score,at,segment=d,cost,arc,i end
        if not expected and d<16 then break end
      end
      cursor=segment
      if best<6400 then steps[#steps+1]={s=at,text=text,id=m.index or #steps+1} end
    end
  end
end
function M.update(v, progress, off, speed, dt, validatedUntil)
  if v~=version then return nil end
  -- Track movement while local roads are unavailable, without consuming any
  -- maneuver or mistaking normal travel during that pause for a later teleport.
  if validatedUntil and validatedUntil<progress then last=progress;return nil end
  if off>80 then
    last=nil
    if not outside then outside=true;return {command='cancelSpeech'} end
    return nil
  end
  outside=false
  speed=math.max(0,speed or 0)
  local jumped=last and math.abs(progress-last)>math.max(150,speed*math.max(1,dt or 1)*3)
  local backwards=last and progress<last-10
  last=progress
  if jumped or backwards then
    for _,m in ipairs(steps) do if m.s-progress<clamp(speed*12,80,400) then m.done=true end end
    return {command='cancelSpeech'}
  end
  for _,m in ipairs(steps) do
    local distance=m.s-progress
    if distance < -10 then m.done=true end
    if not m.done and distance>=-10 then
      if validatedUntil and m.s>validatedUntil then return nil end
      local final=distance<=clamp(speed*3,15,60)
      if final or (not m.warned and distance<=clamp(speed*12,80,400)) then
        if final then m.done=true else m.warned=true end
        local text=m.text
        if not final then text='Через '..tostring(math.max(10,math.floor(distance/10+0.5)*10))..' метров. '..text end
        return {routeVersion=v,eventId=tostring(v)..':'..m.id..(final and ':now' or ':ahead'),text=text}
      end
      return nil
    end
  end
end
return M

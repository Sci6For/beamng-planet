-- Conservative local display of a server route. Every displayed segment belongs
-- to one directed, connected edge of the loaded car graph; unsupported gaps end
-- the display rather than inventing a shortcut. No engine or planner calls.
local M = {}
local sqrt, min, max = math.sqrt, math.min, math.max
local function clamp(x,a,b) return max(a,min(b,x)) end
local function project(a,b,x,y)
  local dx,dy=b.x-a.x,b.y-a.y
  local l2=dx*dx+dy*dy
  local t=l2>0 and clamp(((x-a.x)*dx+(y-a.y)*dy)/l2,0,1) or 0
  local p={x=a.x+t*dx,y=a.y+t*dy,z=(a.z or 0)+t*((b.z or 0)-(a.z or 0))}
  return t,p,sqrt((x-p.x)^2+(y-p.y)^2)
end

function M.advance(e,x,y,previous,moved)
  local lo=max(0,(previous or 0)-10)
  local hi=min(e.cum[#e.cum],(previous or 0)+max(30,(moved or 0)*2+10))
  local best={i=1,t=0,d=math.huge,s=previous or 0}
  for i=1,#e.x-1 do
    if e.cum[i]<=hi and e.cum[i+1]>=lo then
      local span=e.cum[i+1]-e.cum[i]
      local a,b={x=e.x[i],y=e.y[i]},{x=e.x[i+1],y=e.y[i+1]}
      local t=project(a,b,x,y)
      t=span>0 and clamp(t,max(0,(lo-e.cum[i])/span),min(1,(hi-e.cum[i])/span)) or 0
      local d=sqrt((x-a.x-t*(b.x-a.x))^2+(y-a.y-t*(b.y-a.y))^2)
      if d<best.d then best={i=i,t=t,d=d,s=e.cum[i]+t*span} end
    end
  end
  return best.i,best.t,best.d,best.s
end

function M.graph(nodes)
  local g={edges={},out={}}
  local seen={}
  local function add(a,b,data)
    if data.oneWay and data.inNode~=a then return end
    if data.lanes and not data.lanes:find(data.inNode==a and '+' or '-',1,true) then return end
    local pa,pb=nodes[a].pos,nodes[b].pos
    local len=sqrt((pb.x-pa.x)^2+(pb.y-pa.y)^2)
    if len<0.1 then return end
    local r=min(data.inRadius or nodes[a].radius or 3.2,data.outRadius or nodes[b].radius or 3.2)
    local edge={a=a,b=b,p=pa,q=pb,len=len,snap=min(8,max(3,r+1))}
    g.edges[#g.edges+1]=edge;g.out[a]=g.out[a] or {};g.out[a][#g.out[a]+1]=edge
  end
  for a,n in pairs(nodes or {}) do for b,data in pairs(n.links or {}) do
    local key=tostring(a)<tostring(b) and tostring(a)..'|'..tostring(b) or tostring(b)..'|'..tostring(a)
    if nodes[b] and not seen[key] and type(data)=='table' and (data.drivability or 1)>0 then
      seen[key]=true;add(a,b,data);add(b,a,data)
    end
  end end
  return g
end

local function aligned(edge,dx,dy)
  local len=sqrt(dx*dx+dy*dy)
  return len>0.01 and ((edge.q.x-edge.p.x)*dx+(edge.q.y-edge.p.y)*dy)/(edge.len*len)>0.25
end

function M.build(g,e,i,t,player,heading,lookahead)
  if not player or type(player.z)~='number' or not heading then return nil end
  local dx,dy=e.x[i+1]-e.x[i],e.y[i+1]-e.y[i]
  local current,position,lastT,best=nil,nil,nil,math.huge
  for _,edge in ipairs(g.edges) do
    if aligned(edge,dx,dy) and aligned(edge,heading.x,heading.y) then
      local f,p,d=project(edge.p,edge.q,player.x,player.y)
      local dz=math.abs(player.z-p.z)
      if d<=edge.snap and dz<=4 and d*d+dz*dz<best then current,position,lastT,best=edge,p,f,d*d+dz*dz end
    end
  end
  if not current then return nil end
  local path,s0={{pos=position,wp=current.a}},e.cum[i]+t*(e.cum[i+1]-e.cum[i])
  local s,k,lastS=s0,i,s0
  local function append(p,wp)
    local last=path[#path]
    if (last.pos.x-p.x)^2+(last.pos.y-p.y)^2+(last.pos.z-p.z)^2<0.001 then
      if wp then last.wp=wp end
    else path[#path+1]={pos={x=p.x,y=p.y,z=p.z},wp=wp} end
  end
  while s<min(e.cum[#e.cum],s0+(lookahead or 1500))-0.01 do
    if k>=#e.x then break end
    local nextS=min(s+5,e.cum[k+1],s0+(lookahead or 1500))
    if nextS<=s+0.001 then k=k+1 else
      local span=e.cum[k+1]-e.cum[k]
      local f=span>0 and (nextS-e.cum[k])/span or 0
      local x,y=e.x[k]+f*(e.x[k+1]-e.x[k]),e.y[k]+f*(e.y[k+1]-e.y[k])
      local chosen,point,fraction,score=nil,nil,nil,math.huge
      local function consider(edge)
        if not aligned(edge,e.x[k+1]-e.x[k],e.y[k+1]-e.y[k]) then return end
        local u,p,d=project(edge.p,edge.q,x,y)
        if d>edge.snap then return end
        local distance
        if edge==current then
          if u<lastT-0.001 then return end
          distance=(u-lastT)*edge.len
        else
          if edge.b==current.a then return end
          distance=(1-lastT)*current.len+u*edge.len
        end
        if distance>(nextS-lastS)*2+6 then return end
        if d<score then chosen,point,fraction,score=edge,p,u,d end
      end
      consider(current)
      for _,edge in ipairs(g.out[current.b] or {}) do consider(edge) end
      if not chosen then break end
      if chosen~=current then append(current.q,current.b) end
      current,lastT,position=chosen,fraction,point
      append(point,fraction>=0.999 and current.b or nil)
      s,lastS=nextS,nextS
    end
  end
  if #path<2 then return nil end
  -- The last partial segment is still this legal edge for the existing AI path.
  path[#path].wp=current.b
  return {path=path,s=s}
end

-- A bounded projection for trimming already validated arrows. It never asks
-- the stock tracker to re-plan from an off-road position or jump to a later loop.
function M.trim(path,pos)
  local bestI,bestT,bestD=nil,nil,math.huge
  local walked=0
  for i=1,#path-1 do
    local a,b=path[i].pos,path[i+1].pos
    local t,p,d=project(a,b,pos.x,pos.y)
    if math.abs(pos.z-p.z)<=4 and d<bestD then bestI,bestT,bestD=i,t,d end
    walked=walked+sqrt((b.x-a.x)^2+(b.y-a.y)^2)
    if walked>=40 then break end
  end
  if bestD<=12 then return bestI,bestT,bestD end
end

return M

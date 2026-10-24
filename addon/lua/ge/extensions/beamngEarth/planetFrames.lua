-- Planet poses are ECEF doubles; quaternions xyzw rotate body axes into ECEF.
-- Angular velocity is ALWAYS right-handed body xyz radians/sec, never ENU.
local M={}
local function finite(n) return type(n)=='number' and n==n and math.abs(n)<math.huge end
local function vector(v,n) if type(v)~='table' then return false end;for i=1,n do if not finite(v[i]) then return false end end;return true end
local function dot(a,b) return a[1]*b[1]+a[2]*b[2]+a[3]*b[3] end
local function unit(q) if not vector(q,4) then return nil end;local n=math.sqrt(q[1]^2+q[2]^2+q[3]^2+q[4]^2);if math.abs(n-1)>1e-4 then return nil end;return {q[1]/n,q[2]/n,q[3]/n,q[4]/n} end
local function mul(a,b) return {a[4]*b[1]+a[1]*b[4]+a[2]*b[3]-a[3]*b[2],a[4]*b[2]-a[1]*b[3]+a[2]*b[4]+a[3]*b[1],a[4]*b[3]+a[1]*b[2]-a[2]*b[1]+a[3]*b[4],a[4]*b[4]-a[1]*b[1]-a[2]*b[2]-a[3]*b[3]} end
local function matrixQuat(m)
 local t=m[1][1]+m[2][2]+m[3][3];local q={0,0,0,0}
 if t>0 then local s=math.sqrt(t+1)*2;q={ (m[3][2]-m[2][3])/s,(m[1][3]-m[3][1])/s,(m[2][1]-m[1][2])/s,s/4 }
 else local i=1;if m[2][2]>m[i][i] then i=2 end;if m[3][3]>m[i][i] then i=3 end;local j=i%3+1;local k=j%3+1;local s=math.sqrt(1+m[i][i]-m[j][j]-m[k][k])*2;q[i]=s/4;q[j]=(m[i][j]+m[j][i])/s;q[k]=(m[i][k]+m[k][i])/s;q[4]=(m[k][j]-m[j][k])/s end
 return unit(q)
end
function M.definition(lat,lon,h,id,epoch,quantum)
 local p,l=math.rad(lat),math.rad(lon);local s,c=math.sin(p),math.cos(p);local so,co=math.sin(l),math.cos(l);local n=6378137/math.sqrt(1-6.6943799901413165e-3*s*s)
 return {frame_id=id,frame_epoch=epoch,origin_wgs84={lat,lon,h},origin_ecef={(n+h)*c*co,(n+h)*c*so,(n*(1-6.6943799901413165e-3)+h)*s},enu_basis={{-so,co,0},{-s*co,-s*so,c},{c*co,c*so,s}},height_quantum=quantum}
end
function M.validate(f)
 if type(f)~='table' or not vector(f.origin_ecef,3) or type(f.enu_basis)~='table' or not finite(f.height_quantum) or f.height_quantum<=0 then return false end
 local b=f.enu_basis;for i=1,3 do if not vector(b[i],3) then return false end;for j=1,i do if math.abs(dot(b[i],b[j])-(i==j and 1 or 0))>1e-8 then return false end end end
 local cross={b[1][2]*b[2][3]-b[1][3]*b[2][2],b[1][3]*b[2][1]-b[1][1]*b[2][3],b[1][1]*b[2][2]-b[1][2]*b[2][1]};return dot(cross,b[3])>1-1e-8
end
local function convert(p,f,inverse)
 if not M.validate(f) or type(p)~='table' or not vector(p.position,3) or not vector(p.linear_velocity,3) or not vector(p.angular_velocity,3) then return nil,'invalidPose' end
 local q=unit(p.orientation);if not q then return nil,'invalidQuaternion' end
 local b,o=f.enu_basis,f.origin_ecef;local rot={{b[1][1],b[2][1],b[3][1]},{b[1][2],b[2][2],b[3][2]},{b[1][3],b[2][3],b[3][3]}}
 if inverse then rot=b end;local d={p.position[1],p.position[2],p.position[3]};if inverse then for i=1,3 do d[i]=d[i]-o[i] end end
 local pos,vel={},{};for i=1,3 do pos[i]=dot(rot[i],d)+(inverse and 0 or o[i]);vel[i]=dot(rot[i],p.linear_velocity) end
 return {position=pos,orientation=unit(mul(matrixQuat(rot),q)),linear_velocity=vel,angular_velocity={p.angular_velocity[1],p.angular_velocity[2],p.angular_velocity[3]}}
end
function M.toNetworkPose(p,f) return convert(p,f,false) end
function M.fromNetworkPose(p,f) return convert(p,f,true) end
-- Transaction object is owned by its accepted operation; application never implies Resume.
function M.prepare(t) if not t or not M.validate(t.frame) or not t.operation then return nil,'invalidFrame' end;return {operation=t.operation,frame=t.frame,applied=false} end
function M.applied(t,operation) if not t or t.operation~=operation or t.applied then return false end;t.applied=true;return true end
return M

-- Own temporary, verified LAN vehicle mounts without editing personal mods.
local M={}
local paths=require('beamngEarth/planetLaunch')
local function safe(path)
 if type(path)~='string' or path:find('//',1,true) then return false end
 for part in path:gmatch('[^/]+') do if not paths.safePathComponent(part) then return false end end
 return true
end
local function canonical(model,config)
 if type(model)~='string' or type(config)~='string' then return nil end
 local path=config:gsub('^/','')
 if not path:find('/',1,true) then path='vehicles/'..model..'/'..path:gsub('%.pc$','')..'.pc' end
 return path
end
function M.new(fs,refresh)
 local self,mounted,removed,overrides,configs,hash={},{},{},{},{},nil
 local function invoke(method,path)
  if not fs or type(fs[method])~='function' then return false end
  local ok,result=pcall(fs[method],fs,path);return ok and result==true
 end
 function self.restore()
  local ok=true;local changed=#mounted>0 or #removed>0 or hash~=nil
  for i=#mounted,1,-1 do if invoke('unmount',mounted[i]) then table.remove(mounted,i) else ok=false end end
  for i=#removed,1,-1 do if invoke('mount',removed[i]) then table.remove(removed,i) else ok=false end end
  overrides={};configs={};hash=nil
  if changed and refresh then local done=pcall(refresh);ok=done and ok end
  return ok
 end
 function self.apply(plan)
  if type(plan)~='table' or plan.restartRequired or type(plan.catalogueHash)~='string' or #plan.catalogueHash~=64 or plan.catalogueHash:find('[^0-9a-f]') then return false end
  if hash then return hash==plan.catalogueHash end
  if #mounted>0 or #removed>0 or type(plan.mounts)~='table' or type(plan.unmounts)~='table' or type(plan.configOverrides)~='table' or type(plan.configFiles)~='table' then return false end
  local root='/temp/beamngEarth/planet-mods/'..plan.catalogueHash
  local seen={}
  for _,item in ipairs(plan.mounts) do
   local path=type(item)=='table' and item.path
   if not safe(path) or item.kind~='zip' or path:sub(1,#root+8)~=root..'/mounts/' or path:sub(-4)~='.zip' or seen[path] or not invoke('fileExists',path) then return false end
   seen[path]=true
  end
  for _,path in ipairs(plan.unmounts) do
   if not safe(path) or (not path:match('^/mods/.+%.zip$') and not path:match('^/mods/unpacked/[^/]+/$')) or seen[path] then return false end
   seen[path]=true
  end
  for original,path in pairs(plan.configOverrides) do
   if not paths.validConfigPath(original) or path~=root..'/profile/'..original or not invoke('fileExists',path) then return false end
  end
  for _,path in ipairs(plan.configFiles) do if not paths.validConfigPath(path) then return false end end
  -- Record each completed operation before the next so failure can restore it.
  for _,path in ipairs(plan.unmounts) do
   if not invoke('unmount',path) then self.restore();return false end
   removed[#removed+1]=path
  end
  for _,item in ipairs(plan.mounts) do
   if not invoke('mount',item.path) then self.restore();return false end
   mounted[#mounted+1]=item.path
  end
  hash=plan.catalogueHash
  for _,path in ipairs(plan.configFiles) do configs[path]=true end
  for original,path in pairs(plan.configOverrides) do overrides[original]=path end
  if refresh then local ok=pcall(refresh);if not ok then self.restore();return false end end
  return true
 end
 function self.resolve(model,config) return overrides[canonical(model,config)] or config end
 function self.allowed(model,config) return not hash or configs[canonical(model,config)]==true end
 function self.canonical(model,config)
  for original,path in pairs(overrides) do if path==config or path:sub(2)==config then return original end end
  return canonical(model,config)
 end
 return self
end
return M

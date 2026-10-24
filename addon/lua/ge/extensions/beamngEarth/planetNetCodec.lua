-- Planet control framing. Epochs remain decimal strings through the Lua ABI.
local M = {}
local MAX = 1024 * 1024
local BUDGET = 256 * 1024
function M.validDecimal(s)
  return type(s)=='string' and (s=='0' or s:match('^[1-9][0-9]*$') ~= nil)
    and (#s<20 or (#s==20 and s<='18446744073709551615'))
end
function M.compare(a,b)
  if not M.validDecimal(a) or not M.validDecimal(b) then return nil,'invalidEpoch' end
  if #a ~= #b then return #a<#b and -1 or 1 end
  return a==b and 0 or (a<b and -1 or 1)
end
function M.new() return {input='',output={},queuedBytes=0,offset=1,generation=nil} end
function M.frame(body)
  if type(body)~='string' or #body==0 or #body>MAX then return nil,'invalidLength' end
  local n=#body
  return string.char(math.floor(n/16777216)%256,math.floor(n/65536)%256,math.floor(n/256)%256,n%256)..body
end
function M.feed(p,bytes)
  if #p.input+#bytes>MAX+4+BUDGET then return nil,'inputOverflow' end
  p.input=p.input..bytes
  if #p.input<4 then return nil end
  local a,b,c,d=p.input:byte(1,4)
  local n=((a*256+b)*256+c)*256+d
  if n==0 or n>MAX then p.input='';return nil,'invalidLength' end
  if #p.input<n+4 then return nil end
  local body=p.input:sub(5,n+4);p.input=p.input:sub(n+5)
  return body
end
function M.enqueue(p,body)
  local wire,err=M.frame(body);if not wire then return nil,err end
  if #p.output>=32 or p.queuedBytes+#wire>8*1024*1024 then return nil,'queueOverflow' end
  p.output[#p.output+1]=wire;p.queuedBytes=p.queuedBytes+#wire;return true
end
function M.flush(p,s,budget)
  local remaining=math.min(tonumber(budget) or BUDGET,BUDGET)
  while remaining>0 and #p.output>0 do
    local wire=p.output[1];local part=wire:sub(p.offset,p.offset+remaining-1)
    local sent,err,partial=s:send(part)
    local n=sent or partial or 0
    if n<0 or n>#part then return nil,'invalidWrite' end
    p.offset=p.offset+n;remaining=remaining-n
    if p.offset>#wire then p.queuedBytes=p.queuedBytes-#wire;table.remove(p.output,1);p.offset=1 end
    if err and err~='timeout' then return nil,err end
    if n==0 then break end
  end
  return true
end
function M.poll(p,s,callback,budget)
  local remaining=math.min(tonumber(budget) or BUDGET,BUDGET)
  while remaining>0 do
    local body,err=M.feed(p,'')
    if err then return nil,err end
    if body then callback(body)
    else
      local bytes,why,partial=s:receive(math.min(remaining,16384))
      local chunk=bytes or partial or ''
      if #chunk>0 then
        remaining=remaining-#chunk
        local value,e=M.feed(p,chunk);if e then return nil,e end
        if value then callback(value) end
      end
      if why and why~='timeout' then return nil,why end
      if #chunk==0 then break end
    end
  end
  return true
end
return M

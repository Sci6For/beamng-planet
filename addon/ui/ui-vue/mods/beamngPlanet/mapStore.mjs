import { OVERVIEW, normalizeViewport, validPoint, requestedTiles, visibleTiles, labelTiles, fallbackTiles, tileUrl } from './mapModel.mjs'
import { isGlobeZoom } from './globeModel.mjs'
import places from './assets/places.mjs'

const MAX_BYTES=64*1024*1024, TILE_BYTES=256*256*4
let identity=0
// One module-owned store survives chooser component mounts and vehicle/setup navigation.
let persistent
export function getPlanetMapStore(api) {
  // The stock API method uses this.apiCallbacks; never detach its receiver.
  return persistent ||= createMapStore(createLuaMapAdapter((script,callback)=>api.engineLua(script,callback)))
}
export function preloadOverview() {
  if(typeof Image!=='undefined') {const image=new Image(); image.src=OVERVIEW; return image}
}
export function createMapStore(adapter) {
  const prefix=`planet-map-${++identity}-`, listeners=new Set(), ready=new Map(),bad=new Set()
  let viewport={lat:0,lon:0,z:Math.log2(500/256),width:900,height:500},selectedPoint=null,opened=false,disposed=false
  let serial=0,epoch=0,searchEpoch=0,mapTimer,searchTimer,pollTimer,results=[],errorCode=null,query='',clock=0,hasViewport=false
  const next=()=>prefix+(++serial)
  const emit=()=>{for(const fn of listeners) fn(snapshot())}
  const clear=()=>{clearTimeout(mapTimer);clearTimeout(searchTimer);clearTimeout(pollTimer)}
  function evict() {
    const wanted=new Set(requestedTiles(viewport).map(t=>t.key))
    const visible=visibleTiles(viewport), displayed=fallbackTiles(visible,new Set(ready.keys()))
    const keep=new Set([...visible.map(t=>t.key),...displayed.map(t=>t.key),...labelTiles(viewport).map(t=>t.key)])
    const rank=k=>keep.has(k)?2:wanted.has(k)?1:0
    const ordered=[...ready].sort((a,b)=>rank(a[0])-rank(b[0]) || a[1].used-b[1].used)
    let bytes=[...ready.values()].reduce((sum,t)=>sum+t.bytes,0)
    for(const [key,t] of ordered) {
      if(ready.size<=512 && bytes<=MAX_BYTES) break
      ready.delete(key); bytes-=t.bytes
    }
  }
  function snapshot() {
    const keys=new Set(ready.keys()), tiles=isGlobeZoom(viewport.z)?[]:visibleTiles(viewport)
    const rendered=isGlobeZoom(viewport.z)?[]:[...fallbackTiles(tiles,keys),...tiles.filter(t=>keys.has(t.key)),...labelTiles(viewport).filter(t=>keys.has(t.key))]
    for(const t of rendered) {const entry=ready.get(t.key); if(entry) entry.used=++clock}
    return {overview:OVERVIEW,viewport:{...viewport},hasViewport,selectedPoint:selectedPoint&&{...selectedPoint},opened,
      readyKeys:[...keys],tiles:rendered.map(t=>({...t,url:tileUrl(t.key)})),
      imageBytes:[...ready.values()].reduce((s,t)=>s+t.bytes,0),results:results.map(p=>({...p})),query,errorCode}
  }
  function schedule() {
    clearTimeout(mapTimer);clearTimeout(pollTimer)
    if(opened&&!disposed&&!isGlobeZoom(viewport.z)) {const version=epoch; mapTimer=setTimeout(()=>requestMap(version),150)}
  }
  async function requestMap(version) {
    if(!opened||disposed||version!==epoch) return
    const requestId=next(),keys=requestedTiles(viewport).map(t=>t.key)
    try {
      const reply=await adapter.map({requestId,keys})
      if(!opened||disposed||version!==epoch||reply?.requestId!==requestId) return
      const nextError=keys.some(key=>bad.has(key))?'imageDecodeFailed':reply.errorCode||null
      let changed=errorCode!==nextError
      errorCode=nextError
      for(const key of Array.isArray(reply.readyKeys)?reply.readyKeys:[]) if(keys.includes(key)&&!bad.has(key)) {
        if(!ready.has(key)) {ready.set(key,{bytes:TILE_BYTES,used:++clock});changed=true}
      }
      if(changed) {evict();emit()}
    } catch {if(opened&&version===epoch){errorCode='mapUnavailable';emit()}}
    if(opened&&!disposed&&version===epoch&&keys.some(key=>!ready.has(key)&&!bad.has(key)))
      pollTimer=setTimeout(()=>requestMap(version),500)
  }
  function search(value) {
    query=String(value||'').trim().slice(0,256); const version=++searchEpoch
    clearTimeout(searchTimer);errorCode=null
    const q=query.toLocaleLowerCase()
    results=q?places.filter(p=>(p.name+' '+p.english).toLocaleLowerCase().includes(q)).slice(0,8).map(p=>({...p,source:'local'})):[]
    emit()
    if(!opened||disposed||query.length<3||!adapter.geocode) return
    const requestId=next(),request={requestId,query,lat:viewport.lat,lon:viewport.lon}
    searchTimer=setTimeout(async()=>{
      try {
        const reply=await adapter.geocode(request)
        if(!opened||disposed||version!==searchEpoch||reply?.requestId!==requestId) return
        errorCode=reply.errorCode||null
        const online=(Array.isArray(reply.results)?reply.results:[]).filter(validPoint).map(p=>({lat:p.lat,lon:p.lon,name:String(p.name||'').slice(0,512),source:'online'}))
        results=[...results,...online].slice(0,15);emit()
      } catch {if(opened&&version===searchEpoch){errorCode='searchUnavailable';emit()}}
    },300)
  }
  return {
    open(v) {if(disposed) return; if(v) {viewport=normalizeViewport(v);hasViewport=true}opened=true;epoch++;bad.clear();schedule();emit()},
    setViewport(v) {const normalized=normalizeViewport(v);if(hasViewport&&Object.keys(normalized).every(k=>normalized[k]===viewport[k]))return;viewport=normalized;hasViewport=true;epoch++;bad.clear();schedule();emit()},
    select(p) {if(!validPoint(p)) throw new Error('invalidPoint'); selectedPoint={lat:p.lat,lon:p.lon,name:String(p.name||'').slice(0,512)};emit()},
    clearSelection() {selectedPoint=null;emit()},
    snapshot,search,
    imageError(key) {ready.delete(key);bad.add(key);if(bad.size>512) bad.delete(bad.values().next().value);errorCode='imageDecodeFailed';emit()},
    imageLoaded(key,width,height) {const e=ready.get(key);if(!e)return; if(!Number.isFinite(width*height)||width<=0||height<=0){this.imageError(key);return}const bytes=width*height*4;if(e.bytes===bytes)return;e.bytes=bytes;evict();emit()},
    subscribe(fn) {listeners.add(fn);return ()=>listeners.delete(fn)},
    close() {if(!opened)return;opened=false;epoch++;searchEpoch++;clear();adapter.close?.();emit()},
    dispose() {this.close();disposed=true;listeners.clear();ready.clear()}
  }
}

// Callbacks are bounded, cancellable and only carry JSON values into fixed Lua APIs.
export function createLuaMapAdapter(engineLua) {
  const pending=new Map();let generation=0
  function call(method,value) {
    const g=generation
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{pending.delete(timer);reject(new Error('bridgeTimeout'))},5000)
      pending.set(timer,reject)
      const literal=JSON.stringify(JSON.stringify(value))
      try {engineLua(`extensions.beamngPlanet.${method}(jsonDecode(${literal}))`,reply=>{
        clearTimeout(timer);pending.delete(timer)
        if(g!==generation) return reject(new Error('closed'))
        resolve(reply)
      })} catch(error){clearTimeout(timer);pending.delete(timer);reject(error)}
    })
  }
  return {map:r=>call('mapTiles',r),async geocode(r) {
    const g=generation;let reply=await call('searchMap',r)
    for(let i=0;reply?.pending&&i<40&&g===generation;i++) {
      await new Promise(resolve=>{const timer=setTimeout(()=>{pending.delete(timer);resolve()},250);pending.set(timer,()=>resolve())})
      if(g!==generation) throw new Error('closed')
      reply=await call('pollMapSearch',r.requestId)
    }
    return reply?.pending?{requestId:r.requestId,errorCode:'searchTimeout'}:reply
  },close() {
    generation++;for(const [timer,reject] of pending){clearTimeout(timer);reject(new Error('closed'))}pending.clear()
    engineLua('extensions.beamngPlanet.pauseMap()')
  }}
}

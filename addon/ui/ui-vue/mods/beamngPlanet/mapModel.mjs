// Mercator/projection and ancestor layout adapted from BeamNG.Earth mapmath.js,
// pinned 011d6b54e42407b2f3a6deaddc47305cf2325a47 (MIT, see assets/NOTICE.txt).
export const MAX_LAT = 85.05112878
export const OVERVIEW = '/ui/ui-vue/mods/beamngPlanet/assets/world-overview.svg'
export const clampLat = lat => Math.max(-MAX_LAT, Math.min(MAX_LAT, lat))
export const wrapLon = lon => ((lon + 180) % 360 + 360) % 360 - 180
export function validPoint(p) {
  return p && Number.isFinite(p.lat) && Number.isFinite(p.lon) && Math.abs(p.lat)<=90 && Math.abs(p.lon)<=180
}
export function project(lat,lon,z) {
  const s=256*2**z, phi=clampLat(lat)*Math.PI/180
  return {x:(lon+180)/360*s,y:(1-Math.log(Math.tan(phi)+1/Math.cos(phi))/Math.PI)/2*s}
}
export function unproject(x,y,z) {
  const s=256*2**z
  return {lat:180/Math.PI*Math.atan(Math.sinh(Math.PI-2*Math.PI*y/s)),lon:wrapLon(x/s*360-180)}
}
export function normalizeViewport(v) {
  if(!validPoint(v) || ![v.z,v.width,v.height].every(Number.isFinite)) throw new Error('invalidViewport')
  return {lat:clampLat(v.lat),lon:v.lon,z:Math.max(0,Math.min(18,v.z)),width:Math.max(1,Math.min(8192,v.width)),height:Math.max(1,Math.min(8192,v.height))}
}
export function parseTileKey(key) {
  const m=typeof key==='string' && /^(sat|map|labels)\/(0|[1-9]\d*)\/(0|[1-9]\d*)\/(0|[1-9]\d*)$/.exec(key)
  if(!m) throw new Error('invalidTileKey')
  const [,layer,zs,xs,ys]=m, z=Number(zs),x=Number(xs),y=Number(ys)
  if(z>18 || x>=2**z || y>=2**z) throw new Error('invalidTileKey')
  return {layer,z,x,y,key}
}
export function tileUrl(key) {
  const {layer,z,x,y}=parseTileKey(key)
  return `/temp/beamngEarth/maptiles/${layer==='sat'?'':layer+'/'}${z}/${x}/${y}.${layer==='labels'?'png':'jpg'}`
}
export function visibleTiles(v,border=0,layer='sat',maxZoom=18) {
  const z=Math.min(Math.floor(v.z),maxZoom),scale=2**(v.z-z),c=project(v.lat,v.lon,z),size=256*scale,n=2**z
  const left=c.x-v.width/2/scale,top=c.y-v.height/2/scale,out=[]
  for(let y=Math.max(0,Math.floor(top/256)-border);y<=Math.min(n-1,Math.floor((top+v.height/scale)/256)+border);y++)
    for(let ux=Math.floor(left/256)-border;ux<=Math.floor((left+v.width/scale)/256)+border;ux++) {
      const x=((ux%n)+n)%n
      out.push({z,x,y,ux,key:`${layer}/${z}/${x}/${y}`,left:(ux*256-left)*scale,top:(y*256-top)*scale,size})
    }
  return out.sort((a,b)=>(a.left+a.size/2-v.width/2)**2+(a.top+a.size/2-v.height/2)**2-((b.left+b.size/2-v.width/2)**2+(b.top+b.size/2-v.height/2)**2))
}
export function labelTiles(view,border=0) {
  // EOX's transparent label overlay ends at native zoom 14. Reserve at most
  // half the bridge budget for it, reducing resolution only on huge displays.
  let z=Math.min(14,Math.floor(view.z))
  while(z>0 && new Set(visibleTiles(view,1,'labels',z).map(t=>t.key)).size>64) z--
  return visibleTiles(view,border,'labels',z)
}
export function requestedTiles(view) {
  const tiles=visibleTiles(view,1),coarse=new Map(),fine=new Map()
  for(const t of tiles) {
    for(const d of [Math.min(t.z,4),Math.min(t.z,1)]) if(d>0) {
      const z=t.z-d,x=Math.floor(t.x/2**d),y=Math.floor(t.y/2**d),key=`sat/${z}/${x}/${y}`
      coarse.set(key,{z,x,y,key})
    }
    fine.set(t.key,t)
  }
  const labelRequests=new Map()
  for(const t of labelTiles(view,1)) if(!labelRequests.has(t.key)) labelRequests.set(t.key,t)
  const labels=[...labelRequests.values()]
  return [...coarse.values()].sort((a,b)=>a.z-b.z).concat([...fine.values()]).slice(0,128-labels.length).concat(labels)
}
export function fallbackTiles(tiles,ready) {
  const out=new Map()
  for(const t of tiles) {
    if(ready.has(t.key)) continue
    for(let d=1;d<=t.z;d++) {
      const f=2**d,z=t.z-d,ux=Math.floor(t.ux/f),x=((ux%2**z)+2**z)%2**z,y=Math.floor(t.y/f),key=`${t.key.split('/')[0]}/${z}/${x}/${y}`
      if(!ready.has(key)) continue
      out.set(`${key}/${ux}`,{z,x,y,key,renderKey:`${key}/${ux}`,left:t.left-(t.ux-ux*f)*t.size,top:t.top-(t.y-y*f)*t.size,size:t.size*f}); break
    }
  }
  return [...out.values()].sort((a,b)=>a.z-b.z)
}
export function placeLabels(places,v,selectedPoint=null) {
  const c=project(v.lat,v.lon,v.z),size=256*2**v.z,out=[]
  const candidates=selectedPoint?.name && validPoint(selectedPoint)?[selectedPoint,...places]:places
  for(const p of candidates) {
    const m=project(p.lat,p.lon,v.z);let dx=m.x-c.x;dx-=Math.round(dx/size)*size
    const x=v.width/2+dx,y=v.height/2+m.y-c.y,width=p.name.length*6+8
    if(x<width/2||x>v.width-width/2||y<15||y>v.height-35)continue
    if(out.some(t=>Math.abs(t.x-x)<(t.width+width)/2&&Math.abs(t.y-y)<17))continue
    out.push({name:p.name,x,y,width});if(out.length>=40)break
  }
  return out
}

<template>
  <section class="planet-map" aria-label="Выбор места на планете">
    <div class="map-canvas" ref="canvas" tabindex="0" role="application" aria-label="Карта мира: стрелки — перемещение, плюс и минус — масштаб"
      @pointerdown="down" @pointermove="move" @pointerup="up" @pointercancel="cancelDrag" @wheel.prevent="wheel" @keydown="key">
      <div v-if="globeMode" class="globe-stage" :style="{width:globeDiameter+'px',height:globeDiameter+'px'}">
        <canvas ref="globeCanvas" class="globe-image" :style="{width:globeDiameter+'px',height:globeDiameter+'px'}" aria-hidden="true" />
        <span v-for="label in globePlaces" :key="label.name" class="place-label" :style="{left:globeDiameter/2+label.x+'px',top:globeDiameter/2+label.y+'px'}">{{ label.name }}</span>
        <span v-if="globeMarker" class="point-marker" :style="{left:globeDiameter/2+globeMarker.x+'px',top:globeDiameter/2+globeMarker.y+'px'}" aria-label="Выбранное место">●</span>
      </div>
      <div v-else class="map-layer" :style="{transform:`translate3d(${dragOffset.x}px,${dragOffset.y}px,0)`}">
      <img v-for="copy in overviewCopies" :key="copy.left" :src="state.overview" class="map-image overview" alt="" draggable="false" :style="rect(copy)" />
      <img v-for="tile in state.tiles" :key="tile.renderKey || tile.key+'/'+tile.ux" :src="tile.url" class="map-image" alt="" draggable="false" :style="rect(tile)"
        @load="store.imageLoaded(tile.key,$event.target.naturalWidth,$event.target.naturalHeight)" @error="store.imageError(tile.key)" />
      <span v-if="marker" class="point-marker" :style="{left:marker.x+'px',top:marker.y+'px'}" aria-label="Выбранное место">●</span>
      </div>
      <div class="map-tools" @pointerdown.stop @pointerup.stop>
        <BngButton @click="zoom(1)" aria-label="Приблизить">+</BngButton>
        <BngButton @click="zoom(-1)" aria-label="Отдалить">−</BngButton>
        <BngButton @click="world">Мир</BngButton>
      </div>
      <details class="credits" @pointerdown.stop @pointerup.stop @wheel.stop @keydown.stop>
        <summary>Источники: Natural Earth · EOxCloudless / Copernicus · USGS / USDA NAIP · © OpenStreetMap contributors</summary>
        <p>Обзор: Natural Earth (public domain). Подписи: EOX IT Services GmbH (maps.eox.at), © OpenStreetMap contributors и другие. Поиск: Photon / OpenStreetMap.</p>
        <p>EOxCloudless <a href="https://cloudless.eox.at" target="_blank" rel="noopener noreferrer">https://cloudless.eox.at</a> by EOX IT Services GmbH (Contains modified Copernicus Sentinel data 2023), <a href="https://creativecommons.org/licenses/by-nc-sa/4.0/" target="_blank" rel="noopener noreferrer">CC BY-NC-SA 4.0</a>.</p>
        <p>EOxCloudless <a href="https://cloudless.eox.at" target="_blank" rel="noopener noreferrer">https://cloudless.eox.at</a> by EOX IT Services GmbH (Contains modified Copernicus Sentinel data 2016 &amp; 2017), <a href="https://creativecommons.org/licenses/by/4.0/" target="_blank" rel="noopener noreferrer">CC BY 4.0</a>.</p>
        <p>USGS The National Map (public domain); USDA NAIP (public domain); USDA NAIP via USGS The National Map (public domain).</p>
        <p>Региональная аэрофотосъёмка: {{ regionalImageryCredits }}</p>
      </details>
    </div>
    <aside class="map-controls">
      <h2>Место старта</h2>
      <label>Найти страну, город или адрес<BngInput v-model="query" @update:modelValue="store.search($event)" placeholder="Название места" /></label>
      <p class="hint">Страны доступны без сети. Для подробной карты и адресов может потребоваться загрузка.</p>
      <ul class="search-results"><li v-for="(point,index) in state.results" :key="index"><BngButton @click="choose(point,true)">{{ point.name }}</BngButton></li></ul>
      <label>Широта<BngInput v-model="latitude" type="number" placeholder="−90 … 90" /></label>
      <label>Долгота<BngInput v-model="longitude" type="number" placeholder="−180 … 180" /></label>
      <BngButton @click="coordinates">Выбрать координаты</BngButton>
      <p v-if="inputError" role="alert">Введите широту от −90 до 90 и долготу от −180 до 180.</p>
      <p v-if="state.selectedPoint" class="selection">{{ state.selectedPoint.name || 'Точка на карте' }}<br />{{ state.selectedPoint.lat.toFixed(5) }}, {{ state.selectedPoint.lon.toFixed(5) }}</p>
      <p v-else class="selection">Место ещё не выбрано. Нажмите на карту или введите координаты.</p>
      <p v-if="state.errorCode" role="status" class="hint">{{ errorText }}</p>
      <slot :selectedPoint="state.selectedPoint" />
      <PlanetAudio />
    </aside>
  </section>
</template>

<script setup>
import PlanetAudio from './PlanetAudio.vue'
import { ref, shallowRef, computed, onMounted, onUnmounted } from 'vue'
import { BngButton, BngInput } from '@/common/components/base'
import { getPlanetMapStore } from './mapStore.mjs'
import { project, unproject, clampLat, validPoint } from './mapModel.mjs'
import { forwardGlobe, inverseGlobe, rotateGlobe, globeLabels, isGlobeZoom, drawGlobe, disposeGlobe } from './globeModel.mjs'
import places from './assets/places.mjs'
// Default imagery-chain credits from the pinned Earth 1.5.3 client.
const regionalImageryCredits="State of Maryland, MD iMAP / DoIT six-inch imagery; CT ECO 2023 orthoimagery (CC0); MassGIS 2025 aerial imagery; DC OCTO 2025 orthophoto (CC BY 4.0); Indiana IGIO orthoimagery (CC0); OGRIP OSIP orthoimagery (public domain); NC OneMap orthoimagery (NC CGIA, NC 911 Board); PEMA orthoimagery via PASDA; VGIN VBMP imagery; NYS ITS GIS Program Office orthoimagery; NJ OIT Office of GIS 2020 orthoimagery; Delaware FirstMap 2024 imagery; VCGI orthoimagery; (c) swisstopo SWISSIMAGE; IGN BD ORTHO (Licence Ouverte 2.0); PNOA (c) IGN Espana (CC BY 4.0); Beeldmateriaal.nl / PDOK (CC BY 4.0); geoportail.lu orthophoto (CC0); Datenquelle: basemap.at (CC BY 4.0); Autonome Provinz Bozen - Suedtirol orthophoto 2024; Geoportale Nazionale (MASE) orthophoto 2012; (c) CUZK ortofoto (CC BY 4.0); Ortofoto: Maa-amet; Digitaal Vlaanderen orthophoto; Bayerische Vermessungsverwaltung DOP20 (CC BY 4.0); Geobasis NRW DOP (dl-de/zero-2-0); LGL Baden-Wuerttemberg DOP20 (dl-de/by-2-0); LGLN Niedersachsen DOP20 (dl-de/by-2-0); Geoportal Berlin TrueDOP 2024 (dl-de/zero-2-0); HVBG Hessen DOP20; GSI Japan (Geospatial Information Authority of Japan) seamless photo; (c) Spatial Services NSW (CC BY 4.0); (c) State of Tasmania theLIST; Ontario GEO imagery (Open Government Licence - Ontario)"
const props=defineProps({store:Object})
const emit=defineEmits(['select'])
const store=props.store || getPlanetMapStore(window.bngApi)
const state=shallowRef(store.snapshot()),canvas=ref(null),query=ref(state.value.query),latitude=ref(''),longitude=ref(''),inputError=ref(false)
const dragOffset=shallowRef({x:0,y:0})
const globeCanvas=ref(null)
let drag=null,observer,unsubscribe,dragFrame=null,globeFrame=null,globeImage=null,renderedGlobeCanvas=null
const rect=t=>({left:t.left+'px',top:t.top+'px',width:t.size+'px',height:t.size+'px'})
const globeMode=computed(()=>isGlobeZoom(state.value.viewport.z))
const globeDiameter=computed(()=>Math.max(120,Math.min(560,Math.min(state.value.viewport.width-32,state.value.viewport.height-48)*.88)))
const globeRadius=computed(()=>globeDiameter.value/2)
const globeCenter=computed(()=>rotateGlobe(state.value.viewport,globeMode.value?dragOffset.value.x:0,globeMode.value?dragOffset.value.y:0,globeRadius.value))
const globePlaces=computed(()=>globeLabels(places,globeCenter.value,globeRadius.value))
const globeMarker=computed(()=>{
  const p=state.value.selectedPoint;if(!p)return null
  const position=forwardGlobe(p.lat,p.lon,globeCenter.value)
  return position.visible?{x:position.x*globeRadius.value,y:position.y*globeRadius.value}:null
})
function scheduleGlobe() {
  if(!globeMode.value||globeFrame!==null)return
  globeFrame=requestAnimationFrame(()=>{globeFrame=null;renderGlobe()})
}
function renderGlobe() {
  if(globeMode.value&&globeCanvas.value) {
    renderedGlobeCanvas=globeCanvas.value
    drawGlobe(renderedGlobeCanvas,globeImage,globeCenter.value,Math.min(drag?240:480,globeDiameter.value))
  }
}
const overviewCopies=computed(()=>{
  const v=state.value.viewport,c=project(v.lat,v.lon,v.z),size=256*2**v.z,out=[]
  // Detailed views use cached raster ancestors. Do not rasterize the entire
  // vector world at street zoom (z18 would make a 67-million-pixel image).
  if(size>8192)return out
  const first=Math.floor((c.x-v.width/2)/size),last=Math.floor((c.x+v.width/2)/size)
  for(let copy=first;copy<=last;copy++) out.push({left:v.width/2-c.x+copy*size,top:v.height/2-c.y,size})
  return out
})
const marker=computed(()=>{
  const p=state.value.selectedPoint;if(!p)return null
  const v=state.value.viewport,c=project(v.lat,v.lon,v.z),m=project(p.lat,p.lon,v.z),s=256*2**v.z
  let dx=m.x-c.x;dx-=Math.round(dx/s)*s
  return {x:v.width/2+dx,y:v.height/2+m.y-c.y}
})
const errorText=computed(()=>state.value.errorCode==='imageDecodeFailed'?'Часть карты недоступна. Показан сохранённый обзор.':'Подробная карта или адресный поиск пока недоступны. Обзор и выбор координат работают без сети.')
function choose(point,center=false) {
  store.select(point);emit('select',store.snapshot().selectedPoint)
  if(center)store.setViewport({...state.value.viewport,lat:point.lat,lon:point.lon,z:Math.max(5,state.value.viewport.z)})
}
function coordinates() {
  const p={lat:Number(latitude.value),lon:Number(longitude.value)}
  inputError.value=latitude.value===''||longitude.value===''||!validPoint(p)
  if(!inputError.value)choose(p,true)
}
function world() {cancelDrag();const v=state.value.viewport;store.setViewport({...v,lat:0,lon:0,z:Math.max(0,Math.log2(Math.min(v.width,v.height)/256))})}
function zoom(delta) {cancelDrag();store.setViewport({...state.value.viewport,z:state.value.viewport.z+delta})}
function down(event) {
  if(event.button!==0||drag)return
  canvas.value.setPointerCapture(event.pointerId)
  drag={id:event.pointerId,x:event.clientX,y:event.clientY,view:{...state.value.viewport},dx:0,dy:0}
}
function move(event) {
  if(!drag||event.pointerId!==drag.id)return
  drag.dx=event.clientX-drag.x;drag.dy=event.clientY-drag.y
  if(dragFrame===null)dragFrame=requestAnimationFrame(()=>{
    dragFrame=null
    if(drag) {dragOffset.value={x:drag.dx,y:drag.dy};renderGlobe()}
  })
}
function cancelDrag() {
  if(dragFrame!==null)cancelAnimationFrame(dragFrame)
  if(drag&&canvas.value?.hasPointerCapture(drag.id))canvas.value.releasePointerCapture(drag.id)
  dragFrame=null;drag=null;dragOffset.value={x:0,y:0}
  scheduleGlobe()
}
function up(event) {
  if(!drag||event.pointerId!==drag.id)return
  if(Object.keys(drag.view).some(k=>drag.view[k]!==state.value.viewport[k])){cancelDrag();return}
  const bounds=canvas.value.getBoundingClientRect(),x=event.clientX-bounds.left,y=event.clientY-bounds.top,v=drag.view,c=project(v.lat,v.lon,v.z)
  const dx=event.clientX-drag.x,dy=event.clientY-drag.y
  if(canvas.value.hasPointerCapture(event.pointerId))canvas.value.releasePointerCapture(event.pointerId)
  cancelDrag()
  if(isGlobeZoom(v.z)) {
    if(Math.hypot(dx,dy)<5) {
      const radius=globeRadius.value,p=inverseGlobe((x-v.width/2)/radius,(y-v.height/2)/radius,v)
      if(p)choose(p)
    } else store.setViewport({...v,...rotateGlobe(v,dx,dy,globeRadius.value)})
    scheduleGlobe();return
  }
  if(Math.hypot(dx,dy)<5)choose(unproject(c.x+x-v.width/2,c.y+y-v.height/2,v.z))
  else {const p=unproject(c.x-dx,c.y-dy,v.z);store.setViewport({...v,lat:clampLat(p.lat),lon:p.lon})}
}
function wheel(event) {if(!drag)zoom(event.deltaY<0?.5:-.5)}
function key(event) {
  if(event.key==='+'||event.key==='='){event.preventDefault();zoom(1);return}
  if(event.key==='-'){event.preventDefault();zoom(-1);return}
  const move={ArrowLeft:[-100,0],ArrowRight:[100,0],ArrowUp:[0,-100],ArrowDown:[0,100]}[event.key]
  if(!move)return;event.preventDefault();cancelDrag()
  if(globeMode.value) {store.setViewport({...state.value.viewport,...rotateGlobe(state.value.viewport,-move[0],-move[1],globeRadius.value)});return}
  const v=state.value.viewport,c=project(v.lat,v.lon,v.z),p=unproject(c.x+move[0],c.y+move[1],v.z)
  store.setViewport({...v,lat:clampLat(p.lat),lon:p.lon})
}
onMounted(()=>{
  globeImage=new Image();globeImage.onload=scheduleGlobe;globeImage.src=state.value.overview
  unsubscribe=store.subscribe(value=>{
    if(drag&&Object.keys(drag.view).some(k=>drag.view[k]!==value.viewport[k]))cancelDrag()
    if(renderedGlobeCanvas&&!isGlobeZoom(value.viewport.z)){disposeGlobe(renderedGlobeCanvas);renderedGlobeCanvas=null}
    state.value=value
    scheduleGlobe()
  })
  if(!state.value.hasViewport) {
    const {width,height}=canvas.value.getBoundingClientRect()
    store.open({...state.value.viewport,width,height,z:Math.max(0,Math.log2(Math.min(width,height)/256))})
  } else store.open()
  observer=new ResizeObserver(entries=>{const {width,height}=entries[0].contentRect;store.setViewport({...store.snapshot().viewport,width,height})})
  observer.observe(canvas.value)
})
onUnmounted(()=>{cancelDrag();if(globeFrame!==null)cancelAnimationFrame(globeFrame);disposeGlobe(renderedGlobeCanvas);renderedGlobeCanvas=null;globeImage.onload=null;observer?.disconnect();unsubscribe?.();store.close()})
</script>

<style scoped>
.planet-map{display:grid;grid-template-columns:minmax(0,1fr) minmax(19rem,22rem);min-height:0;flex:1;overflow:hidden;color:var(--bng-ter-10,#fff);font-family:var(--fnt-defs,Arial,sans-serif)}
.map-canvas{position:relative;overflow:hidden;min-height:0;background:radial-gradient(circle at 50% 45%,#294a5a,#101b25 72%);touch-action:none;cursor:crosshair}
.globe-stage{position:absolute;left:50%;top:50%;transform:translate(-50%,-50%);pointer-events:none;filter:drop-shadow(0 18px 24px #0009)}.globe-image{display:block}
.map-layer{position:absolute;inset:0;pointer-events:none}
.map-image{position:absolute;pointer-events:none;user-select:none}.map-tools{position:absolute;left:1rem;top:1rem;display:flex;gap:.35rem;padding:.25rem;background:var(--bng-black-o8,#171717dd);border:1px solid #ffffff2b;border-radius:var(--bng-corners-2,.3rem)}.credits{position:absolute;bottom:0;left:0;right:0;padding:.45rem .7rem;background:#111d;font-size:.7rem;line-height:1.25;max-height:45%;overflow:auto;pointer-events:auto}.credits summary{cursor:pointer}.credits p{margin:.6rem 0}.credits a{color:#bde2ff}
.place-label{position:absolute;transform:translate(-50%,-50%);font-size:11px;color:#f4f4e5;text-shadow:0 1px 2px #162d33;pointer-events:none;white-space:nowrap}
.map-controls{background:var(--bng-black-o8,#242424);border-left:1px solid #ffffff2b;padding:1.2rem;overflow:auto}.map-controls h2{margin:0 0 1rem;font-size:1.2rem}.map-controls label{display:grid;gap:.4rem;margin-top:.8rem}.hint{font-size:.85rem;color:var(--bng-off-white,#ddd);opacity:.8;line-height:1.4}.selection{border-left:.25rem solid var(--bng-orange,#ff6600);padding:.7rem;background:#fff1;line-height:1.5}.point-marker{position:absolute;transform:translate(-50%,-50%);color:var(--bng-orange,#ff6600);font-size:2rem;text-shadow:0 0 3px #000;pointer-events:none}.search-results{padding:0;list-style:none;max-height:12rem;overflow:auto}.search-results li{margin:.2rem 0}.search-results button{width:100%;text-align:left}.map-canvas:focus-visible{outline:2px solid var(--bng-orange,#ff6600);outline-offset:-3px}
@media(max-width:850px){.planet-map{grid-template-columns:minmax(0,1fr) minmax(15rem,19rem)}.map-controls{padding:.8rem}}
@media(max-width:650px){.planet-map{display:flex;flex-direction:column}.map-canvas{min-height:45%;flex:1}.map-controls{border-left:0;border-top:1px solid #ffffff2b;max-height:48%;padding:.8rem}}
</style>

<template>
  <div class="planet-flow" v-bng-scoped-nav="{scopeId:'root',activateOnMount:true,preferAutoFocus:true}" v-bng-on-ui-nav:back="back">
    <header><h1>Планета</h1><span>{{ selecting ? '1 · Место' : '2 · Автомобиль и поездка' }}</span></header>
    <PlanetSession :session="state.session" :selected-point="state.selectedPoint" @state="updateState" @intent="sessionIntent=$event" />
    <p v-if="sessionIntent!=='solo'" class="hint lan-traffic">Движение отключено в локальной сети.</p>
    <div v-if="selecting && state.session?.mode==='join' && pointEnabled" class="hint guest-destination"><span>Выберите место появления на карте. Машина появится на подходящей дороге рядом с выбранной точкой.</span><BngButton accent="secondary" @click="centerHost">Показать место ведущего</BngButton></div>
    <PlanetMap v-if="selecting && state.chooserOpen && pointEnabled" :store="store" @select="selectPoint" />
    <main v-else-if="!selecting" class="drive-setup">
      <section class="destination"><h2>{{ state.selectedPoint?.name || 'Выбранная точка' }}</h2>
        <p v-if="state.selectedPoint">{{ state.selectedPoint.lat.toFixed(5) }}, {{ state.selectedPoint.lon.toFixed(5) }}</p>
        <h2>Автомобиль</h2><p>{{ state.vehicle ? `${state.vehicle.model} · ${state.vehicle.config}` : 'Выберите автомобиль и комплектацию' }}</p>
        <BngButton accent="secondary" sound-class="bng_main_secondary" @click="chooseVehicle">Выбрать автомобиль</BngButton>
      </section>
      <section v-if="!state.session?.operationId" class="options"><h2>Настройки поездки</h2>
        <p class="hint">{{ hasTimeOverride ? 'Параметры времени действуют только в этой поездке.' : 'Пока вы не измените параметры времени, действуют настройки Земли.' }}</p>
        <template v-for="option in visibleOptions" :key="option.key">
          <label class="option"><span>{{ labels[option.key] }}</span>
            <DayCycleLengthSelect v-if="option.key === 'environment_timePlay'" :model-value="option.value" variant="named" :level-seconds="option.levelSeconds" show-disabled :threshold="80" @update:modelValue="value => changeOption(option, value)" />
            <BngDatePicker v-else-if="option.type === 'date'" :model-value="option.value" :reset-value="option.resetValue" reset-label="Сбросить" today-label="Сегодня" @change="value => changeOption(option, value)" />
            <BngSmartSelect v-else :model-value="option.value" :items="option.options || []" :threshold="80" @update:modelValue="value => changeOption(option, value)" />
          </label>
        </template>
      </section>
    </main>
    <p v-if="error" role="alert" class="error">{{ error }}</p>
    <footer>
      <BngButton accent="secondary" sound-class="bng_main_secondary" @click="back">Назад</BngButton>
      <BngButton v-if="selecting" class="primary-action" accent="main" sound-class="bng_main_primary" :disabled="!pointEnabled || !state.selectedPoint || busy" @click="next">Далее</BngButton>
      <BngButton v-else class="primary-action" accent="main" sound-class="bng_main_primary" :disabled="!pointEnabled || !state.selectedPoint || !state.vehicle || busy || (sessionIntent!=='solo' && !state.session?.operationId)" @click="start">Начать поездку</BngButton>
    </footer>
  </div>
</template>
<script setup>
import {ref,computed,onMounted,onBeforeUnmount} from 'vue'
import {useRoute} from 'vue-router'
import {lua} from '@/bridge'
import {run} from '@/bridge/libs/Lua.js'
import {BngButton,BngSmartSelect,BngDatePicker} from '@/common/components/base'
import DayCycleLengthSelect from '@/modules/environmentControls/components/DayCycleLengthSelect.vue'
import {vBngOnUiNav,vBngScopedNav} from '@/common/directives'
import PlanetMap from './PlanetMap.vue'
import PlanetSession from './PlanetSession.vue'
import {getPlanetMapStore} from './mapStore.mjs'
import {canChoosePoint,sessionMessage} from './sessionModel.mjs'
const route=useRoute(),selecting=computed(()=>route.name==='planet.select')
const store=getPlanetMapStore(window.bngApi)
const state=ref({chooserOpen:false,selectedPoint:null,vehicle:null,setup:{}}),options=ref([]),busy=ref(false),error=ref('')
const sessionIntent=ref('solo')
const pointEnabled=computed(()=>canChoosePoint(state.value.session))
function centerHost(){const p=state.value.session?.hostLocation;if(p)store.setViewport({...store.snapshot().viewport,lat:p.lat,lon:p.lon,z:15})}
function updateState(value){
 const previous=state.value.session,left=!!previous?.operationId && !value.session?.operationId
 const joined=value.session?.mode==='join' && previous?.operationId!==value.session.operationId
 const becameReady=value.session?.mode==='join' && value.session.hostReady && (!previous?.hostReady || joined)
 state.value=value
 if(value.session?.operationId || left)sessionIntent.value=value.session?.mode||'solo'
 if(joined)store.clearSelection()
 if(becameReady)centerHost()
}
const visibleOptions=computed(()=>options.value.filter(option=>sessionIntent.value==='solo' || option.key!=='traffic_trafficMode'))
const hasTimeOverride=computed(()=>['timeOfDay','date','timePlay'].some(key=>state.value.setup?.[key]!==undefined))
const labels={traffic_trafficMode:'Движение',environment_timeOfDay:'Время суток',environment_date:'Дата',environment_timePlay:'Смена дня и ночи'}
const fields={traffic_trafficMode:'trafficMode',environment_timeOfDay:'timeOfDay',environment_date:'date',environment_timePlay:'timePlay'}
const callPlanet=(method,...args)=>run('extensions.beamngPlanet.'+method,args,{})
let alive=true,selectionSerial=0
function accept(reply) {
  if (!alive) return false
  if (reply?.state) updateState(reply.state)
  if (!reply?.ok) {error.value=reply?.errorCode ? sessionMessage({errorCode:reply.errorCode}) : 'Не удалось сохранить выбор. Попробуйте ещё раз.';return false}
  error.value='';return true
}
async function navigate(name) {await lua.extensions.ui_router.navigate(name)}
onMounted(async()=>{
  try {
    const current=await callPlanet('getState')
    if (!alive) return
    if (!current?.chooserOpen) {
      const opened=await callPlanet('openChooser')
      if (!accept(opened)) return
      store.clearSelection()
    } else updateState(current)
    const groups=await callPlanet('getSetupOptions')
    if (!alive) return
    options.value=(Array.isArray(groups)?groups:[]).flatMap(group=>Array.isArray(group.options)?group.options:[]).filter(option=>fields[option.key])
    for (const option of options.value) if (state.value.setup?.[fields[option.key]]!==undefined) option.value=state.value.setup[fields[option.key]]
  } catch {if(alive) error.value='Не удалось открыть выбор поездки.'}
})
onBeforeUnmount(()=>{alive=false;selectionSerial++})
async function selectPoint(point) {
  const serial=++selectionSerial;busy.value=true
  try {const reply=await callPlanet('selectPoint',point.lat,point.lon,point.name || '');if(serial===selectionSerial) accept(reply)}
  catch {if(alive) error.value='Не удалось выбрать точку.'}
  finally {if(serial===selectionSerial) busy.value=false}
}
async function next() {if(pointEnabled.value && state.value.selectedPoint && !busy.value) await navigate('planet.setup')}
async function back() {
  if(busy.value) return
  if(selecting.value) {await callPlanet('closeChooser');await navigate('menu')}
  else await navigate('planet.select')
}
async function chooseVehicle() {
  try {accept(await callPlanet('openVehicleSelector'))} catch {error.value='Не удалось открыть список автомобилей.'}
}
async function changeOption(option,value) {
  if(busy.value) return
  busy.value=true
  try {if(accept(await callPlanet('setSetup',{[option.key]:value}))) option.value=value}
  catch {error.value='Не удалось сохранить настройки.'}
  finally {busy.value=false}
}
async function start() {
  if(busy.value || !pointEnabled.value || !state.value.selectedPoint || !state.value.vehicle || (sessionIntent.value!=='solo' && !state.value.session?.operationId)) return
  busy.value=true
  try {
    // Task 6 supplies the real coordinator. Only an accepted start opens loading.
    if(accept(await callPlanet('start'))) await navigate('planet.loading')
  } catch {error.value='Не удалось начать поездку. Вернитесь к выбору и попробуйте ещё раз.'}
  finally {busy.value=false}
}
</script>
<style scoped>
.planet-flow{box-sizing:border-box;width:100%;height:100%;min-height:0;padding-bottom:4.5em;overflow:hidden;display:flex;flex-direction:column;background:var(--bng-black-o8,#171717);color:var(--bng-off-white,#f5f5f5);font-family:var(--fnt-defs,Arial,sans-serif)}
header{flex-shrink:0;display:flex;align-items:center;justify-content:space-between;padding:.6rem 1.4rem;gap:1rem;border-bottom:1px solid #ffffff25;background:var(--bng-black-o4,#252525)}h1{font-size:1.8rem;margin:.3rem 0}h2{font-size:1.2rem}header span,.hint{opacity:.75}
.guest-destination{padding:.5rem 1.4rem;display:flex;gap:1rem;align-items:center;flex-shrink:0}.drive-setup{display:grid;grid-template-columns:minmax(15rem,1fr) minmax(22rem,1.4fr);gap:2rem;padding:1.5rem;overflow:auto;flex:1;min-height:0}.destination,.options{padding:1.2rem;background:var(--bng-black-o4,#252525);border-radius:var(--bng-corners-2,.3rem)}
.option{display:grid;grid-template-columns:minmax(8rem,1fr) minmax(12rem,1.5fr);align-items:center;gap:1rem;margin:1.2rem 0}.hint{font-size:.9rem}
footer{flex-shrink:0;display:flex;align-items:center;justify-content:space-between;gap:1rem;padding:.75rem 1.25rem;border-top:1px solid #ffffff25;background:var(--bng-black-o4,#252525)}.primary-action{margin-left:auto}.error{color:#ffad79;padding:0 1.4rem;margin:.4rem 0}@media(max-width:850px){.drive-setup{grid-template-columns:1fr}}
</style>

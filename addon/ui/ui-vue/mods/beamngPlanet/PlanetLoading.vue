<template>
  <div ref="cover" class="planet-loading" v-bng-scoped-nav="{scopeId:'planet-loading',activateOnMount:true,preferAutoFocus:true}">
    <header><h1>Планета</h1></header>
    <main role="status" aria-live="polite"><h2>{{ loadingText(state) }}</h2>
      <p v-if="state.lan && (state.stalled || state.errorCode)">Покиньте сессию и создайте её заново или подключитесь повторно.</p>
      <p v-else-if="state.stalled">Давно нет новых результатов. Можно продолжить ожидание, повторить загрузку или вернуться к выбору.</p>
      <p v-if="actionError" role="alert">{{ actionError }}</p>
    </main>
    <footer>
      <BngButton accent="secondary" :disabled="busy || state.cleaning" @click="act('cancel')">{{ state.lan ? 'Покинуть сессию' : 'Назад' }}</BngButton>
      <BngButton v-if="!state.lan && state.stalled && !state.errorCode" accent="secondary" :disabled="busy" @click="act('continueWaiting')">Продолжить ожидание</BngButton>
      <BngButton v-if="!state.lan && (state.stalled || state.errorCode) && state.errorCode!=='roadsDisabled'" class="primary-action" accent="main" :disabled="busy || state.cleaning" @click="act('retry')">Повторить</BngButton>
    </footer>
  </div>
</template>
<script>
export default {position:'fullscreen',animated: false,wrapper:{fade:false,blur:false,style:'transparent'}}
</script>
<script setup>
import {ref,watch,nextTick,onBeforeUnmount} from 'vue'
import {run} from '@/bridge/libs/Lua.js'
import {BngButton} from '@/common/components/base'
import {vBngScopedNav} from '@/common/directives'
import {loadingText} from './loading.mjs'
const props=defineProps({state:{type:Object,default:()=>({})},popupActive:Boolean})
const cover=ref(),busy=ref(false),actionError=ref('')
let serial=0,frame
// A stock loading callback can be forced/synchronous. Acknowledge only this
// actual current full-viewport element after a rendered frame, never a timer.
// The host replaces status fields every frame. Compare these two values,
// otherwise an array-returning getter restarts the two-frame paint proof on
// every delete/reassign of the unchanged id and can starve acknowledgement.
watch([()=>props.state.id,()=>props.popupActive],async()=>{
  const ticket=++serial,id=props.state.id
  if(!id || !props.popupActive) return
  await nextTick()
  frame=requestAnimationFrame(()=>{frame=requestAnimationFrame(async()=>{
    if(ticket!==serial || !props.popupActive || !cover.value?.isConnected) return
    const box=cover.value.getBoundingClientRect()
    const style=getComputedStyle(cover.value)
    if(box.left>0 || box.top>0 || box.right<innerWidth || box.bottom<innerHeight || style.opacity!=='1' || style.visibility==='hidden') return
    try {await run('extensions.beamngPlanet.acknowledgeCover',[id],{})} catch {actionError.value='Не удалось подтвердить экран загрузки. Вернитесь к выбору.'}
  })})
},{immediate:true,flush:'post'})
onBeforeUnmount(()=>{serial++;cancelAnimationFrame(frame)})
async function act(method) {
  if(busy.value || props.state.cleaning) return
  busy.value=true;actionError.value=''
  try {const r=await run('extensions.beamngPlanet.'+method,[props.state.id],{});if(!r?.ok) actionError.value='Действие недоступно. Попробуйте ещё раз.'}
  catch {actionError.value='Не удалось выполнить действие.'}
  finally {busy.value=false}
}
</script>
<style scoped>
.planet-loading{position:fixed;inset:0;width:100vw!important;height:100vh!important;max-height:none!important;max-width:none!important;opacity:1!important;background:radial-gradient(circle at 50% 38%,#303a40,#171717 65%);color:var(--bng-off-white,#f5f5f5);display:flex;flex-direction:column;font-family:var(--fnt-defs,Arial,sans-serif)}
header{padding:1rem 2rem;border-bottom:1px solid #ffffff25;background:var(--bng-black-o4,#252525)}main{flex:1;display:flex;flex-direction:column;justify-content:center;align-items:center;text-align:center;padding:2rem}main h2{font-size:1.45rem;max-width:42rem}main p{max-width:42rem;line-height:1.45}h1{margin:0}footer{display:flex;gap:1rem;padding:1rem 2rem;border-top:1px solid #ffffff25;background:var(--bng-black-o4,#252525)}.primary-action{margin-left:auto}
</style>

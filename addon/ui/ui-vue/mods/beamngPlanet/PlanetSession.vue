<template>
  <section class="planet-session" aria-label="Локальная сеть">
    <div class="modes" role="group" aria-label="Режим поездки">
      <BngButton accent="secondary" :aria-pressed="choice==='solo'" :disabled="busy" @click="choose('solo')">Одиночная поездка</BngButton>
      <BngButton accent="secondary" :aria-pressed="choice==='host'" :disabled="busy || connected" @click="choose('host')">Создать сессию</BngButton>
      <BngButton accent="secondary" :aria-pressed="choice==='join'" :disabled="busy || connected" @click="choose('join')">Подключиться</BngButton>
    </div>
    <div v-if="choice!=='solo'" class="lobby">
      <span class="network-label">Локальная сеть · до двух игроков</span>
      <template v-if="!connected">
        <label>Ваше имя <input v-bng-text-input v-model="name" maxlength="64" autocomplete="off" /></label>
        <label v-if="choice==='join'" class="join-code">Код подключения <textarea v-bng-text-input v-model="code" maxlength="1024" rows="2" spellcheck="false" /></label>
        <details><summary>Другой сетевой адрес</summary><div class="endpoint"><label>Адрес <input v-bng-text-input v-model="address" placeholder="Например, 192.168.1.5" /></label><label>Порт <input v-bng-text-input v-model="port" type="number" min="1" max="65535" /></label></div></details>
        <p v-if="choice==='host' && !selectedPoint">Выберите любое место на карте, затем создайте сессию.</p>
        <BngButton accent="main" :disabled="busy || (choice==='host' && !selectedPoint) || (choice==='join' && !validCode)" @click="connect">{{ choice==='host' ? 'Создать сессию' : 'Подключиться' }}</BngButton>
      </template>
      <p role="status" aria-live="polite">{{ message }}</p>
      <template v-if="session.ticket">
        <p>Адрес для второго игрока: {{ session.ticket.endpoint.address }}:{{ session.ticket.endpoint.port }}</p>
        <label class="join-code">Код подключения <textarea v-bng-text-input :value="JSON.stringify(session.ticket)" readonly rows="2" spellcheck="false" @focus="$event.target.select()" /></label>
        <BngButton accent="secondary" @click="copyCode">Копировать код подключения</BngButton>
      </template>
      <p v-if="session.mode==='join' && connected">Место и условия поездки задаёт ведущий. Выберите свой автомобиль.</p>
      <p v-if="session.phase==='error'">Проверьте соединение локальной сети и разрешения сетевого доступа на обоих компьютерах.</p>
      <BngButton v-if="connected && session.phase==='error'" accent="secondary" :disabled="busy" @click="retry">Повторить</BngButton>
      <BngButton v-if="connected" accent="secondary" :disabled="busy" @click="leave">Покинуть сессию</BngButton>
    </div>
    <p v-if="error" role="alert" class="session-error">{{ error }}</p>
  </section>
</template>
<script setup>
import {ref,computed,onMounted,onBeforeUnmount,watch} from 'vue'
import {BngButton} from '@/common/components/base'
import {vBngTextInput} from '@/common/directives'
import {run} from '@/bridge/libs/Lua.js'
import {parseTicket,validEndpoint,sessionMessage} from './sessionModel.mjs'
const props=defineProps({session:{type:Object,default:()=>({mode:'solo',phase:'off'})},selectedPoint:{type:Object,default:null}})
const emit=defineEmits(['state','intent'])
const choice=ref(props.session.mode||'solo'),name=ref('Player'),code=ref(''),address=ref(''),port=ref(3698),busy=ref(false),error=ref('')
const connected=computed(()=>!!props.session.operationId),validCode=computed(()=>!!parseTicket(code.value)),message=computed(()=>sessionMessage(props.session))
watch(()=>props.session.mode,mode=>{choice.value=mode||'solo'})
let alive=true,timer,polling=false,revision=0
const call=(method,...args)=>run('extensions.beamngPlanet.'+method,args,{})
function accept(reply){if(!alive)return;if(reply?.state)emit('state',reply.state);error.value=reply?.ok?'':sessionMessage({errorCode:reply?.errorCode||'invalidStatus'})}
async function choose(mode){if(busy.value)return;if(mode==='solo' && connected.value) await leave();choice.value=mode;emit('intent',mode)}
async function connect(){
 if(busy.value)return
 const override=address.value.trim()?{address:address.value.trim(),port:Number(port.value)}:undefined
 if(override && !validEndpoint(override,choice.value==='host')){error.value='Введите сетевой IP-адрес и порт от 1 до 65535.';return}
 if(!name.value.trim() || [...name.value].length>64 || /[\x00-\x1f\x7f]/.test(name.value)){error.value='Введите ваше имя.';return}
 if(choice.value==='join' && !validCode.value){error.value=sessionMessage({errorCode:'invalidTicket'});return}
 revision++;busy.value=true
 try {accept(await call(choice.value==='host'?'hostSession':'joinSession',...(choice.value==='host'?[name.value,override]:[code.value,name.value,override])))}
 catch{error.value='Не удалось открыть сетевой сеанс.'}finally{busy.value=false}
}
async function leave(){revision++;busy.value=true;try{accept(await call('leaveSession'))}catch{error.value='Не удалось покинуть сеанс.'}finally{busy.value=false}}
async function retry(){const mode=choice.value;await leave();if(!error.value){choice.value=mode;emit('intent',mode);await connect()}}
async function copyCode(){try{await navigator.clipboard.writeText(JSON.stringify(props.session.ticket));error.value=''}catch{error.value='Выделите код подключения и скопируйте его вручную.'}}
async function poll(){
 if(!alive || polling || busy.value || !connected.value)return
 const requested=revision,operationId=props.session.operationId,generation=props.session.generation
 polling=true
 try{const state=await call('getState');if(alive && requested===revision && !busy.value && props.session.operationId===operationId && props.session.generation===generation && state?.session?.operationId===operationId && state.session.generation===generation)emit('state',state)}catch{}finally{polling=false}
}
onMounted(()=>{timer=setInterval(poll,1500)})
onBeforeUnmount(()=>{alive=false;revision++;clearInterval(timer)})
</script>
<style scoped>
.planet-session{flex-shrink:0;padding:.7rem 1.4rem;max-height:42%;overflow:auto;border-bottom:1px solid #ffffff25;background:var(--bng-black-o4,#252525)}.modes{display:flex;gap:.5rem;flex-wrap:wrap}.lobby{display:flex;gap:.65rem 1rem;align-items:center;flex-wrap:wrap;padding:.7rem 0 .2rem}.lobby p{margin:.25rem 0}.lobby label{display:flex;gap:.4rem;align-items:center}.join-code{flex:1 1 22rem}.join-code textarea{width:100%;min-width:12rem}.endpoint{display:flex;gap:.5rem;flex-wrap:wrap}.network-label{opacity:.8}input,textarea{box-sizing:border-box;font:inherit;color:inherit;background:var(--bng-black-o8,#171717);border:1px solid #ffffff66;padding:.5rem;border-radius:var(--bng-corners-2,.3rem);max-width:100%}input:focus-visible,textarea:focus-visible,summary:focus-visible{outline:2px solid var(--bng-orange,#ff6600);outline-offset:2px}details{width:100%}summary{cursor:pointer;width:max-content;max-width:100%}.session-error{color:#ffad79;margin:.3rem 0}
</style>


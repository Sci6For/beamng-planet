<template>
  <details class="planet-audio" @toggle="toggle" @mousedown.stop @pointerdown.stop @wheel.stop @dblclick.stop @keydown.stop>
    <summary>Музыка и навигатор</summary>
    <div class="audio-body">
      <p v-if="!state.ready" role="status">{{ state.error || 'Подключение звука…' }}</p>
      <BngButton @click="send('chooseFolder')">Выбрать папку с музыкой</BngButton>
      <label>Песня
        <select :value="state.currentId" @change="send('play',{trackId:$event.target.value})">
          <option value="" disabled>MP3 / WAV</option>
          <option v-for="track in state.tracks" :key="track.id" :value="track.id">{{ track.name }}</option>
        </select>
      </label>
      <div class="audio-buttons">
        <BngButton @click="send('previous')" aria-label="Предыдущая песня">◀</BngButton>
        <BngButton @click="send(state.playing?'pause':'play')">{{ state.playing?'Пауза':'Играть' }}</BngButton>
        <BngButton @click="send('next')" aria-label="Следующая песня">▶</BngButton>
      </div>
      <label><input type="checkbox" :checked="state.shuffle" @change="send('setShuffle',{value:$event.target.checked})" />Перемешивать</label>
      <label>Громкость музыки <input aria-label="Громкость музыки" type="range" min="0" max="1" step="0.05" :value="state.musicVolume" @change="volume('music',$event)" /></label>
      <label><input type="checkbox" :checked="state.voiceEnabled" @change="send('setVoice',{value:$event.target.checked})" />Голосовые подсказки</label>
      <label>Громкость голоса <input aria-label="Громкость голоса" type="range" min="0" max="1" step="0.05" :value="state.voiceVolume" @change="volume('voice',$event)" /></label>
      <p v-if="state.voiceError" role="status">{{ state.voiceError }}</p>
      <p v-if="state.ready && state.error" role="status">{{ state.error }}</p>
      <p class="audio-hint">Выберите цель на карте и постройте маршрут. Музыка приглушается во время подсказок.</p>
    </div>
  </details>
</template>
<script setup>
import {ref,onUnmounted} from 'vue'
import {BngButton} from '@/common/components/base'
import {run} from '@/bridge/libs/Lua.js'
import {audioState} from './audioModel.mjs'
const state=ref(audioState(null))
let timer,alive=true,busy=false
async function refresh(){
  if(busy || !alive)return
  busy=true
  try {const value=await run('extensions.beamngEarthAutostart.getPlanetAudioStatus',[],{});if(alive)state.value=audioState(value)}
  catch {if(alive)state.value=audioState({error:'Помощник звука недоступен. Запустите игру через «Планету».'})}
  finally {busy=false}
}
async function send(command,fields={}){
  try {
    const ok=await run('extensions.beamngEarthAutostart.planetAudioCommand',[{command,...fields}],{})
    if(!ok)state.value.error='Команда не принята. Повторите через секунду.'
  }catch {state.value.error='Помощник звука недоступен.'}
}
function volume(channel,event){send('setVolume',{channel,value:Number(event.target.value)})}
function toggle(event){
  clearInterval(timer)
  if(event.target.open){send('ping');refresh();timer=setInterval(refresh,1000)}
}
onUnmounted(()=>{alive=false;clearInterval(timer)})
</script>
<style scoped>
.planet-audio{color:#fff;background:#25272bef;border-left:3px solid #ff6600;border-radius:4px;font:inherit;max-width:24rem;pointer-events:auto}
summary{padding:.65rem 1rem;cursor:pointer;font-weight:600}
.audio-body{padding:0 .8rem .8rem;display:flex;flex-direction:column;gap:.4rem;max-height:65vh;overflow:auto}
label{display:flex;gap:.5rem;align-items:center;flex-wrap:wrap}select{width:100%;background:#383b40;color:white;padding:.4rem;border:1px solid #75787e;border-radius:3px}
input[type=range]{flex:1;accent-color:#ff6600;min-width:7rem}input[type=checkbox]{accent-color:#ff6600}.audio-buttons{display:flex;gap:.4rem}p{font-size:.9em;margin:.3rem 0}.audio-hint{color:#bdc0c5}
</style>

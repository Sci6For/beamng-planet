export function audioState(value) {
  const v=value && typeof value==='object'?value:{}
  return {ready:v.ready===true,playing:v.playing===true,voiceAvailable:v.voiceAvailable===true,
    voiceEnabled:v.voiceEnabled!==false,shuffle:v.shuffle===true,
    musicVolume:Number.isFinite(v.musicVolume)?v.musicVolume:0.65,
    voiceVolume:Number.isFinite(v.voiceVolume)?v.voiceVolume:0.9,
    voiceError:typeof v.voiceError==='string'?v.voiceError:'',error:typeof v.error==='string'?v.error:'',
    tracks:Array.isArray(v.tracks)?v.tracks.filter(t=>t && typeof t.id==='string' && typeof t.name==='string'):[],
    currentId:typeof v.currentId==='string'?v.currentId:''}
}

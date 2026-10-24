export const initialSession=()=>({mode:'solo',phase:'off',peerStatus:'',ticket:null})
export const canChoosePoint=session=>session?.phase!=='error' && !session?.errorCode && (session?.mode!=='join' || session.hostReady===true)
const hex=(v,n)=>typeof v==='string' && new RegExp(`^[0-9a-f]{${n}}$`).test(v)
const closed=(v,keys)=>v && typeof v==='object' && !Array.isArray(v) && Object.keys(v).length===keys.length && keys.every(k=>Object.hasOwn(v,k))
export function validEndpoint(v,auto=false) {
 if(!closed(v,['address','port']) || !Number.isInteger(v.port) || v.port<1 || v.port>65535 || typeof v.address!=='string') return false
 const a=v.address
 if(auto && (a==='0.0.0.0' || a==='::')) return true
 if(/^\d+\.\d+\.\d+\.\d+$/.test(a)) {const ns=a.split('.');return ns.every(n=>String(Number(n))===n && Number(n)<=255) && +ns[0]>0 && +ns[0]!==127 && +ns[0]<224 && a!=='255.255.255.255'}
 if(!/^[0-9a-fA-F:]+$/.test(a) || !a.includes(':') || a==='::' || a==='::1' || /^ff/i.test(a)) return false
 try {return new URL(`http://[${a}]/`).hostname.length>2} catch{return false}
}
export function parseTicket(text) {
 if(typeof text!=='string' || new TextEncoder().encode(text).length>1024) return null
 try {const v=JSON.parse(text);return closed(v,['endpoint','roomId','certificateSha256','joinSecret']) && validEndpoint(v.endpoint) && hex(v.roomId,32) && hex(v.certificateSha256,64) && hex(v.joinSecret,32) ? v:null} catch{return null}
}
const errors={vehicleModsMismatch:'Автомобильные моды или сохранённые комплектации различаются. Установите и включите одинаковые версии модов и комплектации в профилях «Планеты» на обоих ПК, затем создайте сеанс заново.',vehicleUnavailable:'Автомобиль или его комплектация недоступны. Проверьте, что нужный мод включён в профиле «Планеты» на обоих ПК.',lanUnavailable:'Компонент сетевой игры недоступен.',invalidTicket:'Неверный билет подключения.',portBusy:'Сетевой порт занят.',readyTimeout:'Подготовка заняла слишком много времени.',processFailed:'Сетевой процесс завершился.',startFailed:'Не удалось запустить сетевую игру.',invalidStatus:'Не удалось подтвердить состояние сеанса.'}
const phases={off:'Выберите режим поездки.',starting:'Запуск сетевой игры…',connecting:'Подключение к ведущему…',lobby:'Ожидание игроков.',preparing:'Ожидание общей подготовки мира…',active:'Сетевая игра готова.',leaving:'Выход из сеанса…'}
export function sessionMessage(s) {
 const extraErrors={hostNotReady:'Дождитесь, пока ведущий загрузит мир и войдёт в игру.',vehicleSyncRestartRequired:'Не удалось подключить моды во время игры. Вернитесь в главное меню. Для модов со скриптами установите одинаковые версии на обоих ПК и перезапустите игру.',disconnected:'Соединение с ведущим потеряно. Подключитесь повторно.',sessionRejected:'Ведущий отклонил подключение. Проверьте совместимость и подключитесь повторно.',descriptorMismatch:'Версии мира или сетевой игры различаются. Обновите «Планету» на обоих ПК.'}
 if(s?.errorCode && extraErrors[s.errorCode]) return extraErrors[s.errorCode]
 if(s?.errorCode || s?.phase==='error') return errors[s.errorCode] || 'Не удалось подготовить сетевую игру. Попробуйте ещё раз.'
 if(s?.modSync?.state==='downloading') {
  const total=Number(s.modSync.bytesTotal),done=Number(s.modSync.bytesDone),percent=Number.isFinite(total) && total>0 && Number.isFinite(done) ? ` ${Math.min(100,Math.max(0,Math.floor(done/total*100)))}%` : ''
  return `Загрузка автомобильных модов ведущего…${percent}`
 }
 if(s?.modSync?.state==='mountRequired') return 'Подключение автомобильных модов ведущего…'
 if(s?.mode==='join' && s.phase==='lobby') return s.hostReady ? 'Ведущий в игре. Выберите место появления и свой автомобиль.' : 'Ведущий загружает мир. Выбор места появится, когда он войдёт в игру.'
 return phases[s?.phase] || phases.off
}

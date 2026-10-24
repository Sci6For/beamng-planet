// The host is installed by the mod entry, independently of the current route.
export function createLoadingHost({open,update}) {
  let popup,state
  const retired=new Set()
  return {
    receive(next) {
      if(!next?.id || retired.has(next.id)) return
      if(next.covered) {
        if(!popup) {state={...next};popup=open(state)}
        else {if(state.id!==next.id) retired.add(state.id);update(state,next)}
      } else if(popup && next.id===state.id) {
        retired.add(next.id);popup.close(true);popup=state=undefined
      }
    },
    owns(id) {return !!popup && state.id===id},
    dispose() {popup?.close(true);popup=state=undefined},
  }
}
const stages={selected:'Подготовка поездки',loadingLevel:'Загрузка уровня',terrain:'Подготовка рельефа',nearCells:'Загрузка ближайших дорог',vehicle:'Подготовка автомобиля',settling:'Проверка устойчивости автомобиля',network:'Синхронизация сетевой сессии',ready:'Поездка готова',cancelled:'Завершение загрузки'}
const helperErrors={invalidConfig:'Проверьте настройки помощника.',profileUnavailable:'Профиль игры недоступен для записи.',gameConflict:'Обнаружена другая сессия игры или помощника.',portBusy:'Порт сервера занят другим процессом.',backendHashMismatch:'Контрольная сумма сервера не совпадает.',backendStartFailed:'Не удалось запустить сервер.',backendReadyTimeout:'Сервер не успел подготовиться.',backendFailed:'Сервер завершился с ошибкой.',retryLimit:'Повторы запуска сервера исчерпаны. Проверьте журнал помощника.'}
export function loadingText(s={}) {
  if(s.errorCode==='processFailed') return 'Сетевой процесс завершился. Покиньте сессию и создайте её заново.'
  if(s.errorCode==='readyTimeout') return 'Сетевая синхронизация не завершилась. Проверьте подключение второго ПК и создайте сессию заново.'
  if(['disconnected','lanUnavailable','staleSession'].includes(s.errorCode)) return 'Соединение с сетевой сессией потеряно. Покиньте сессию и подключитесь заново.'
  if(helperErrors[s.errorCode]) return helperErrors[s.errorCode]+' Вернитесь к выбору или повторите после устранения причины.'
  if(s.errorCode==='roadsDisabled') return 'Для этого старта нужно включить дороги в настройках Земли. Вернитесь к выбору.'
  if(['pauseOwnershipUnavailable','pauseOwnershipChanged'].includes(s.errorCode)) return 'Физика на паузе. Снимите паузу обычной игровой клавишей и повторите поездку либо вернитесь к выбору.'
  if(s.errorCode==='vehicleUnavailable') return 'Выбранный автомобиль или комплектация недоступны. Вернитесь к выбору.'
  if(s.errorCode==='vehicleModsMismatch') return 'Автомобильные моды или комплектации различаются. Установите и включите одинаковые версии в профилях «Планеты» на обоих ПК и создайте сеанс заново.'
  if(s.errorCode) return 'Не удалось подготовить поездку. Повторите загрузку или вернитесь к выбору.'
  const blocked={groundSupport:'Проверка опорной поверхности под автомобилем',collisionRebuild:'Подготовка столкновений ближайших дорог'}
  let text=blocked[s.blockedReason] || stages[s.phase] || 'Подготовка поездки'
  if(Number.isFinite(s.requiredCells) && s.requiredCells>0 && Number.isFinite(s.readyCells)) text+=` · участки дорог ${s.readyCells} из ${s.requiredCells}`
  if(Number.isFinite(s.nearSurfaceRequired) && s.nearSurfaceRequired>0 && Number.isFinite(s.nearSurfaceReady)) text+=` · поверхность ${s.nearSurfaceReady} из ${s.nearSurfaceRequired}`
  return text
}

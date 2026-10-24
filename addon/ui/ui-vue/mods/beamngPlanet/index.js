import router from '@/router'
import {lua,useBridge} from '@/bridge'
import {reactive} from 'vue'
import {addPopup,PopupTypes} from '@/services/popup'
import {run} from '@/bridge/libs/Lua.js'
import {registerPlanet} from './menuRoutes.mjs'
import {getPlanetMapStore,preloadOverview} from './mapStore.mjs'
import {createLoadingHost} from './loading.mjs'

let registration,removeGuard,loadingHost,events,launchListener,finishUnloading
async function closeOwnedChooser() {
  let done,endingId
  const completed=new Set()
  const stopped=new Promise(resolve=>{done=resolve})
  finishUnloading=status=>{if(!status.covered){completed.add(status.id);if(status.id===endingId) done()}}
  const result=await run('extensions.beamngPlanet.closeChooser',[],{})
  endingId=result?.state?.launch?.id
  if(!result?.state?.launch?.covered || completed.has(endingId)) done()
  await stopped
}
export async function onLoad() {
  if(registration) return
  await lua.extensions.load('beamngPlanet')
  const compatibility=await run('extensions.beamngPlanet.checkCompatibility',[],{})
  if(compatibility?.ok!==true) throw Error('Обычное меню сохранено: версия игры не поддерживается для Планеты. '+(compatibility?.path||'Проверка совместимости недоступна.'))
  const [grid,setup,vehicle,loading]=await Promise.all([
    import('./PlanetMenuGrid.vue'),import('./PlanetSetup.vue'),
    import('./PlanetVehicle.vue'),
    import('./PlanetLoading.vue'),
  ])
  loadingHost=createLoadingHost({
    open:state=>addPopup(loading.default,{state:reactive(state)},PopupTypes.priority).promise,
    update:(state,next)=>{const target=reactive(state);for(const key of Object.keys(target)) delete target[key];Object.assign(target,next)},
  })
  events=useBridge().events
  let launchEvents=0
  launchListener=async status=>{
    const revision=++launchEvents
    if(status.returnRoute && !status.covered && loadingHost.owns(status.id)) {
      // Cancellation has already torn down the engine world. Preserve cover
      // while the chooser/menu mounts, so play cannot flash between screens.
      try {await router.replace({name:status.returnRoute})} catch {return}
      if(revision!==launchEvents) return
    }
    loadingHost.receive(status)
    finishUnloading?.(status)
  }
  events.on('BeamNGPlanetLaunch',launchListener)
  try {
    // Register the host before querying, so an already accepted launch recovers
    // its cover after a UI remount and never depends on planet.loading's route.
    const before=launchEvents,current=await run('extensions.beamngPlanet.getState',[],{})
    if(launchEvents===before && current?.launch) launchListener(current.launch)
    registration=await registerPlanet({router,planetGrid:grid.default,
    views:{select:setup.default,setup:setup.default,loading:loading.default,vehicle:vehicle.default},
    register:(...args)=>lua.extensions.ui_router_routeManager.registerModRoutes(...args),
    unregister:(...args)=>lua.extensions.ui_router_routeManager.unregisterModRoutes(...args)})
  } catch(error) {
    await closeOwnedChooser()
    events.off('BeamNGPlanetLaunch',launchListener);loadingHost.dispose();loadingHost=undefined;finishUnloading=undefined
    throw error
  }
  removeGuard=router.afterEach(async(to,from)=>{
    if(from.name?.startsWith('planet.') && !to.name?.startsWith('planet.')) {
      const result=await run('extensions.beamngPlanet.leaveChooser',[],{})
      if(!result?.preserveSelection) getPlanetMapStore(window.bngApi).clearSelection()
    }
  })
  preloadOverview()
}
export async function onUnload() {
  removeGuard?.();removeGuard=undefined
  const previous=registration;registration=undefined
  try {
    // Keep the persistent host/listener until ordinary teardown finishes.
    await closeOwnedChooser()
    await previous?.restore()
  } finally {
    events?.off('BeamNGPlanetLaunch',launchListener)
    loadingHost?.dispose();loadingHost=undefined;finishUnloading=undefined
  }
}

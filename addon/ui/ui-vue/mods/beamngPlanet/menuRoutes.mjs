// BeamNG 0.40 view-only adapter. Never re-register the named menu children:
// addRoute(name) would detach them from their unnamed MainMenu parent.
export const SOURCE = 'beamngPlanet.routes'
const MENUS = ['menu', 'menu.discover', 'menu.others']
const installed = new WeakMap()
async function refresh(router) {
  const current = router.currentRoute.value
  if (current?.matched?.some(record => MENUS.includes(record.name))) {
    await router.replace({path:current.path,query:current.query,hash:current.hash,force:true})
  }
}
export async function install(router, originalRoutes, planetGrid) {
  if (installed.has(router)) return installed.get(router)
  const saved = []
  let restored = false
  const adapter = {async restore() {
    if (restored) return
    restored = true
    for (const {record,components} of saved) record.components = components
    installed.delete(router)
    await refresh(router)
  }}
  try {
    // Validate the complete pinned shape before mutating even the first view.
    const records = MENUS.map(name => {
      const record = originalRoutes.find(record => record.name === name)
      const matched = router.resolve({name}).matched
      if (!record?.components?.default || matched.length < 2 || matched.at(-1) !== record) throw Error('Unsupported main menu route: '+name)
      return record
    })
    for (const record of records) {
      saved.push({record,components:record.components})
      record.components = {...record.components,default:planetGrid}
    }
    installed.set(router,adapter)
    await refresh(router)
    return adapter
  } catch (error) {await adapter.restore();throw error}
}
export function planetRoutes(views) {
  const menuMeta = {infoBar:{visible:true},topBar:{visible:false},uiApps:{shown:false}}
  const route = (name,path,component,title,backTarget,extra={}) => ({name,path,component,meta:{...menuMeta,...extra,luaRoute:{title,backTarget,scopeTree:{root:{}},...extra.luaRoute}}})
  return [
    route('planet.select','/planet/select',views.select,'Планета','menu'),
    route('planet.setup','/planet/setup',views.setup,'Автомобиль и поездка','planet.select'),
    route('planet.loading','/planet/loading',views.loading,'Загрузка планеты','planet.setup'),
    route('planet.vehicle','/planet/vehicle',views.vehicle,'Выбор автомобиля','planet.setup',{
      gridSelector:{backendName:'vehicleSelector',routePath:'/planet/vehicle',defaultPath:{keys:['allModels']}},
      luaRoute:{onEnter:'ui_gridSelectorRouteLifecycleCallbacks.onRouteEnter',onMount:'ui_gridSelectorRouteLifecycleCallbacks.onRouteMount',onLeave:'ui_gridSelectorRouteLifecycleCallbacks.onRouteLeave',scopeTree:{root:{'grid-selector-grid':{},'grid-selector-details':{backTarget:'grid-selector-grid',backTargetType:'scope'}}}}
    }),
  ]
}
export async function registerPlanet({router,planetGrid,views,register,unregister}) {
  let adapter,restored=false
  const removers=[]
  async function restore() {
    if (restored) return
    restored=true
    // Restore the active menu before removing standalone routes.
    await adapter?.restore()
    if (router.currentRoute.value.name?.startsWith('planet.')) await router.replace({name:'menu'})
    for (const remove of removers.reverse()) remove()
    await unregister(SOURCE,{fallbackRoute:'menu'})
  }
  try {
    adapter=await install(router,router.getRoutes(),planetGrid)
    const routes=planetRoutes(views)
    for (const record of routes) {
      if (router.hasRoute(record.name)) throw Error('Planet route already owned: '+record.name)
      removers.push(router.addRoute(record))
    }
    const result=await register(SOURCE,routes.map(({name,path,meta})=>({name,path,meta})))
    if (!result?.success) throw Error('Planet route registration failed')
    return {restore}
  } catch (error) {await restore();throw error}
}

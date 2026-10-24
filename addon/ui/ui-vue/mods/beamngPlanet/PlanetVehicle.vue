<template>
  <LayoutMenu
    :nav-scope="'root'"
    :nav-active="false"
    :breadcrumbs="breadcrumbItems"
    :hideBreadcrumbLastItem="false"
    @breadcrumb-click="onBreadcrumbClick"
    @breadcrumb-back="onBreadBack"
  >
    <p v-if="selectionError" class="planet-vehicle-error" role="alert">{{ selectionError }}</p>
    <GridSelector
      ref="gridSelectorRef"
      class="grid-selector-screen-content"
      v-bind="{ ...attrs, ...gridSelectorProps }"
      :loading="isBootingSelector"
      v-on="gridSelectorListeners"
    >
      <template #item-details="{ activeItem, activeItemDetails, executeButton, toggleFavourite, exploreFolder, goToMod }">
        <VehicleDetails
          :activeItem="activeItem"
          :activeItemDetails="activeItemDetails"
          :toggleFavourite="toggleFavourite"
          :exploreFolder="exploreFolder"
          :goToMod="goToMod"
          :showHeaderTitle="true"
          :buttonOverride="{label:'Выбрать автомобиль',icon:'car'}"
          @execute-button="({ buttonId, additionalData }) => executeButton(buttonId, additionalData)"
          @override-click="({ additionalData }) => confirmVehicle(activeItemDetails, additionalData)"
        />
      </template>
      <template #management-details="{ managementDetails, executeButton }">
        <ManagementDetails
          :managementDetails="managementDetails"
          :executeButton="executeButton"
        />
      </template>
    </GridSelector>
  </LayoutMenu>
</template>

<script setup>
import { computed, onMounted, onBeforeUnmount, ref, useAttrs, watch } from "vue"
import { useRoute } from "vue-router"
import { LayoutMenu } from "@/common/layouts"
import GridSelector from "@/common/modules/gridSelector/GridSelector.vue"
import useGridSelectorController from "@/common/modules/gridSelector/composables/useGridSelectorController"
import { lua } from "@/bridge"
import { run } from "@/bridge/libs/Lua.js"
import { useRouteDataStore } from "@/services/routeData"
import VehicleDetails from "@/modules/vehicleselect/components/VehicleDetails.vue"
import ManagementDetails from "@/modules/vehicleselect/components/ManagementDetails.vue"

defineOptions({
  inheritAttrs: false,
})

// The stock details cache outlives numeric button callbacks after hovering a
// different config. Confirm the exact displayed model/config through our owner.
const selectorToken=run('extensions.beamngPlanet.getVehicleSelectorToken',[],{})
const selectionError=ref('')
let confirming=false,disposed=false
onBeforeUnmount(()=>{disposed=true})
async function confirmVehicle(details,additionalData) {
  if (disposed || confirming || !details?.modelKey || !details?.configKey) return
  confirming=true;selectionError.value=''
  try {
    const token=await selectorToken
    if (disposed) return
    const reply=await run('extensions.beamngPlanet.confirmVehicleSelection',[
      token,details.modelKey,details.configKey,additionalData || {},
    ],{})
    if (!reply?.ok) selectionError.value='Не удалось выбрать автомобиль. Выберите комплектацию ещё раз. ('+(reply?.errorCode || 'unknown')+')'
  } catch {selectionError.value='Не удалось передать выбор в игру. Вернитесь назад и откройте автомобили ещё раз.'}
  finally {confirming=false}
}
const attrs = useAttrs()
const route = useRoute()
const routeDataStore = useRouteDataStore()
const gridSelectorRef = ref(null)
const defaultPath = Object.freeze({ keys: ["allModels"] })
const gridScrollableContentElement = computed(() => gridSelectorRef.value?.$el?.querySelector(".grid-content") ?? null)
const defaultPathSegments = Object.freeze([...(Array.isArray(defaultPath.keys) ? defaultPath.keys : [])])
const openedFromGarage = ref(false)

function resolveRoutePathSegments() {
  if (routeDataStore.routeName !== route.name) {
    return [...defaultPathSegments]
  }

  const routePathSegments = routeDataStore.route?.params?.path?.keys
  return Array.isArray(routePathSegments) ? [...routePathSegments] : [...defaultPathSegments]
}

function isModelRootPath(segments) {
  if (!Array.isArray(segments) || segments.length === 0) return true
  return segments[0] === defaultPathSegments[0]
}

function onBackFromGrid(backEvent = {}) {
  if (!isModelRootPath(backEvent?.currentPathSegments)) {
    if (typeof backEvent.preventDefault === "function") {
      backEvent.preventDefault()
    }
    const routeName = typeof route.name === "string" ? route.name : ""
    if (!routeName) return
    void lua.extensions.ui_router.navigate(routeName, {
      path: { keys: [...defaultPathSegments] },
    }, null)
    return
  }

  if (!openedFromGarage.value) return
  if (typeof backEvent.preventDefault === "function") {
    backEvent.preventDefault()
  }
  lua.extensions.ui_router.navigate("garage", null, null)
}

const {
  gridSelectorProps,
  gridSelectorListeners,
  onBreadBack,
  isBootingSelector,
} = useGridSelectorController({
  backendName: "vehicleSelector",
  enableDefaultDoubleClick: false,
  requestItemDoubleClick: async event => {
    event.preventDefault()
    const details=await lua.ui_gridSelector.getDetails('vehicleSelector',event.details)
    await confirmVehicle(details)
  },
  defaultPath,
  defaultDetailsMode: "advanced",
  gridScrollableContentElement,
  requestNavigation: onGridNavigateRequest,
  requestBackFromGrid: onBackFromGrid,
  getCurrentPathSegments: resolveRoutePathSegments,
})

const breadcrumbItems = computed(() => Array.isArray(routeDataStore.breadcrumbs) ? routeDataStore.breadcrumbs : [])
const onBreadcrumbClick = async item => {
  if (!item?.routeName || item.decorator || item.abstract) return
  await lua.extensions.ui_router.navigate(item.routeName, item.params)
}

function onGridNavigateRequest(navigationRequest) {
  const gotoPath = Array.isArray(navigationRequest?.gotoPath)
    ? navigationRequest.gotoPath
    : (Array.isArray(navigationRequest?.item?.gotoPath) ? navigationRequest.item.gotoPath : null)
  const routeName = typeof route.name === "string" ? route.name : ""
  if (!Array.isArray(gotoPath) || gotoPath.length === 0 || !routeName) return

  navigationRequest?.preventDefault?.()

  const itemParams = navigationRequest?.item?.params
  const targetParams = {
    ...(itemParams && typeof itemParams === "object" ? itemParams : {}),
    path: { keys: [...gotoPath] },
  }
  void lua.extensions.ui_router.navigate(routeName, targetParams, null)
}

const syncGarageContext = async () => {
  openedFromGarage.value = !!(await lua.ui_vehicleSelector_general.isOpenedFromGarage())
}

onMounted(syncGarageContext)
watch(() => route.name, syncGarageContext)
</script>

<style lang="scss">
.planet-vehicle-error {
  position:absolute;z-index:20;top:4em;left:20%;right:20%;padding:0.75em;background:#802b16;color:white;
}
.grid-selector-screen-content {
  height: 100%;
}
</style>

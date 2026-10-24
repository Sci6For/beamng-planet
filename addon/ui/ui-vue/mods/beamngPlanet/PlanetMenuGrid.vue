<!-- BeamNG 0.40 MainGridView adaptation. Original view SHA-256 986208472a60e5dfa9812faeddb86fb75898a2e2b49cf9d294c32e92455446a9; bCDDL 1.1 (http://beamng.com/bCDDL-1.1.txt); only the Quick Start tile and its action change. -->
<template>
  <div class="center-wrap">
    <div
      class="stage"
      :class="{
        'is-simple': isSimpleMenu,
        'is-open': !!openView,
        'is-more': activeView === 'more',
        'is-experiences': activeView === 'experiences',
        'is-wiping': wiping,
      }"
      v-bng-on-ui-nav:back,menu="openView === 'more' ? closeMoreRoute : undefined"
    >
      <div ref="mainRef" class="primary">
        <div class="row row-big">
          <Huge2
            ref="experiencesTileRef"
            class="tile tile-experiences"
            icon-id="globe"
            :bg-img-abs="PLANET_CARD"
            sound-class="bng_main_primary"
            :bng-scoped-nav-autofocus="firstTime ? 'force' : undefined"
            @click="openPlanet"
          >Планета</Huge2>
          <Big2
            class="tile tile-freeroam"
            icon-id="road"
            :bg-img="IMG_PATH + 'freeroam.jpg'"
            sound-class="bng_main_primary"
            v-bng-route-target="defaultWizardRoute"
          >{{ $tt("ui.playmodes.freeroam") }}</Big2>
          <Big2
            class="tile tile-splitscreen"
            icon-id="splitScreen"
            :bg-img="IMG_PATH + 'splitscreen.jpg'"
            :tag="$tt('ui.playmodes.new')"
            sound-class="bng_main_primary"
            v-bng-route-target="'menu.discover.multiseat'"
          >{{ $tt("ui.experiences.splitscreen.discover.page.title") }}</Big2>
        </div>

        <div class="row row-square">
          <Big2Square
            class="tile tile-career"
            icon-id="house"
            :bg-img="IMG_PATH + 'career.jpg'"
            sound-class="bng_main_primary"
            v-bng-route-target="'career.profiles'"
          >{{ $tt("ui.playmodes.career") }}</Big2Square>
          <Big2Square
            class="tile"
            icon-id="drift01"
            :bg-img="IMG_PATH + 'others.jpg'"
            v-bng-route-target="'menu.gameplay'"
          >{{ $tt("ui.menu.gameplaySelector.title") }}</Big2Square>
          <Big2Square
            class="tile"
            icon-id="rallyHelmet"
            :bg-img="IMG_PATH + 'rally.jpg'"
            @click="rallyDisclaimer()"
          >{{ $tt("ui.playmodes.rally") }}</Big2Square>
          <Big2Square
            v-if="!isSimpleMenu"
            ref="moreTileRef"
            class="tile"
            icon-id="BNGFolder"
            sound-class="bng_main_primary"
            @click="navigate('menu.others')"
          >{{ $tt("ui.mainmenu.more") }}</Big2Square>

          <div class="simple-column tile">
            <MenuButtonSimple
              v-if="isSimpleMenu"
              sound-class="bng_main_secondary"
              v-bng-route-target="'menu.replay'"
            >
              {{ $tt("ui.dashboard.replay") }}
            </MenuButtonSimple>
            <MenuButtonSimple
              sound-class="bng_main_secondary"
              v-bng-route-target="'options'"
            >
              {{ $tt("ui.mainmenu.options") }}
            </MenuButtonSimple>
            <MenuButtonSimple
              v-if="repoEnabled && !isSimpleMenu"
              sound-class="bng_main_secondary"
              v-bng-route-target="'menu.mods.repository'"
            >
              {{ $tt("ui.mainmenu.repo") }}
              <template #subtext v-if="modCounts.total > 0">&nbsp;({{ modCounts.active }} / {{ modCounts.total }})</template>
            </MenuButtonSimple>
            <MenuButtonSimple
              v-else-if="!isSimpleMenu"
              :accent="modsAfterUpdate ? 'danger' : undefined"
              :icon="modsAfterUpdate ? icons.danger : undefined"
              icon-color="#ff2d00"
              sound-class="bng_main_secondary"
              v-bng-route-target="'menu.mods.local'"
            >
              {{ $tt("ui.mainmenu.mods") }}
              <template #subtext v-if="modCounts.total > 0">&nbsp;({{ modCounts.active }} / {{ modCounts.total }})</template>
            </MenuButtonSimple>
            <MenuButtonSimple
              sound-class="bng_main_secondary"
              v-bng-route-target="'menu.credits'"
            >
              {{ $tt("ui.mainmenu.credits") }}
            </MenuButtonSimple>
            <MenuButtonSimple
              v-if="!isSimpleMenu"
              accent="red"
              sound-class="bng_main_secondary"
              @click="quitGame()"
            >
              {{ $tt("ui.inputActions.general.quit.title") }}
            </MenuButtonSimple>
          </div>
        </div>
      </div>

      <template v-if="!isSimpleMenu">
        <MoreView
          ref="moreRef"
          class="overlay-view more"
          :visible="activeView === 'more'"
          :no-blur="moreBlurOff"
          :addons="addons"
          @back="closeMoreRoute"
        />
      </template>
      <ExperiencesView
        v-if="activeView === 'experiences'"
        ref="experiencesRef"
        class="overlay-view experiences"
      />
      <LineWipe ref="wipeRef" />
    </div>
  </div>
</template>

<script setup>
import { ref, unref, computed, inject, nextTick, watch } from "vue"
import { useRoute } from "vue-router"
import { ACCENTS, icons } from "@/common/components/base"
import { vBngOnUiNav, vBngRouteTarget } from "@/common/directives"
import { $translate, SysInfo } from "@/services"
import { openConfirmation } from "@/services/popup"
import { useSettings } from "@/services/settings"
import { useBridge } from "@/bridge"
import { run, runRaw } from "@/bridge/libs/Lua.js"
import Huge2 from "@/modules/mainmenu/components/buttons/Huge2.vue"
import Big2 from "@/modules/mainmenu/components/buttons/Big2.vue"
import Big2Square from "@/modules/mainmenu/components/buttons/Big2Square.vue"
import MenuButtonSimple from "@/modules/mainmenu/components/buttons/Simple.vue"
import LineWipe from "@/modules/mainmenu/components/LineWipe.vue"
import RallyDisclaimerContent from "@/modules/mainmenu/components/RallyDisclaimerContent.vue"
import MoreView from "@/modules/mainmenu/views/MoreView.vue"
import ExperiencesView from "@/modules/mainmenu/views/ExperiencesView.vue"
import { setFocus } from "@/services/uiNavFocus"

import { getPlanetMapStore, preloadOverview } from './mapStore.mjs'

const PLANET_CARD = '/ui/ui-vue/mods/beamngPlanet/assets/planet-card.jpg'
const { lua } = useBridge()
const route = useRoute()

const $simplemenu = inject("$simplemenu", ref(false))
const isSimpleMenu = computed(() => unref($simplemenu))

const IMG_PATH = "images/mainmenu/"

defineProps({
  firstTime: Boolean,
  addons: Object, // object with named buttons
})

const settings = useSettings()
const repoEnabled = computed(() => settings.values.onlineFeatures === "enable" && !settings.values.disableModsAfterUpdate)
const modsAfterUpdate = computed(() => settings.values.disableModsAfterUpdate)
const modCounts = SysInfo.modCounts

const WIZARD_ROUTE_BY_STEP = Object.freeze({
  level: "menu.freeroamLevels",
  vehicle: "menu.freeroamLevels.vehicles",
  options: "menu.freeroamLevels.vehicles.options",
  multiplayer: "menu.freeroamLevels.vehicles.options.multiplayer",
})
const defaultWizardRoute = computed(() => {
  const configuredStep = settings.getValue("freeroamSetupDefaultStep") || "level"

  if (isSimpleMenu.value && configuredStep === "multiplayer") {
    return WIZARD_ROUTE_BY_STEP.level
  }

  return WIZARD_ROUTE_BY_STEP[configuredStep] || WIZARD_ROUTE_BY_STEP.level
})

const mainBackground = inject("mainBackground", null)

const EXPERIENCES_ROUTE = "menu.discover"
const MORE_ROUTE = "menu.others"

function overlayForRoute(name) {
  if (name === EXPERIENCES_ROUTE) return "experiences"
  if (name === MORE_ROUTE) return "more"
  return null
}

// openView is the requested overlay, activeView stays set until its closing wipe ends
const openView = ref(overlayForRoute(route.name))
const activeView = ref(openView.value)
const wiping = ref(false)
const blurOff = ref(false)

const moreBlurOff = computed(() => blurOff.value || activeView.value !== "more")

const overlayRefs = {
  more: { tile: () => moreTileRef.value, view: () => moreRef.value },
  experiences: { tile: () => experiencesTileRef.value, view: () => experiencesRef.value },
}

let moreBusy = false
async function toggleView(name, open) {
  if (moreBusy || (openView.value === name) === open) return
  moreBusy = true
  const slideshow = unref(mainBackground)
  slideshow?.pause?.() // avoid slideshow crossfade for better performance
  openView.value = open ? name : null
  if (open) activeView.value = name
  const tileEl = overlayRefs[name].tile()?.getElement?.()
  if (tileEl) {
    wiping.value = true
    await nextTick()
    const view = overlayRefs[name].view()
    await wipeRef.value.play({
      source: tileEl,
      cover: mainRef.value,
      reveal: view.$el,
      bounds: [mainRef.value.getBoundingClientRect(), ...view.getContentRects()],
      reverse: !open,
    })
    wiping.value = false
  }
  if (open) {
    await nextTick()
    overlayRefs[name].view()?.focusDefault?.()
  }
  if (!open) {
    activeView.value = null
    tileEl && setFocus(tileEl)
  }
  blurOff.value = false
  slideshow?.play?.()
  moreBusy = false
  syncMenuOverlays()
}

function syncMenuOverlays() {
  const target = overlayForRoute(route.name)
  if (target === "experiences") toggleView("experiences", true)
  else if (target === "more") toggleView("more", true)
  else {
    if (openView.value === "experiences") toggleView("experiences", false)
    else if (openView.value === "more") toggleView("more", false)
  }
}
watch(() => route.name, syncMenuOverlays, { immediate: true })

// MainMenu remains the original parent and retains useRouteReady signaling.
let openingPlanet = false
async function openPlanet() {
  if (openingPlanet) return
  openingPlanet = true
  try {
    const reply = await run('extensions.beamngPlanet.openChooser', [], {})
    if (!reply?.ok) return
    if (!reply.state?.selectedPoint) getPlanetMapStore(window.bngApi).clearSelection()
    navigate('planet.select')
  } finally { openingPlanet = false }
}
preloadOverview()

const closeMoreRoute = () => navigate("menu")

// wipes the whole grid away from the clicked button, then leaves for another screen
async function wipeTo(route, event) {
  if (moreBusy) return
  moreBusy = true
  const slideshow = unref(mainBackground)
  slideshow?.pause?.()
  await wipeRef.value.play({ source: event.currentTarget, cover: mainRef.value, keep: true })
  slideshow?.play?.()
  navigate(route)
  moreBusy = false
}

const mainRef = ref(null)
const moreRef = ref(null)
const experiencesRef = ref(null)
const experiencesTileRef = ref(null)
const moreTileRef = ref(null)
const wipeRef = ref(null)

const navigate = (state, params = undefined) => nextTick(() => lua.extensions.ui_router.navigate(state, params || null, null))
function quitGame() {
  lua.quit()
  runRaw("TorqueScript.eval('quit();')", false)
}

async function rallyDisclaimer() {
  const accepted = await openConfirmation(
    $translate.instant("ui.rally.experimentalTitle"),
    { component: RallyDisclaimerContent },
    [
      { label: $translate.instant("ui.common.cancel"), value: false, extras: { cancel: true, accent: ACCENTS.secondary } },
      { label: $translate.instant("ui.common.okay"), value: true, extras: { confirm: true } },
    ],
    "rally",
    false,
  )
  if (accepted) {
    lua.ui_gameplaySelector_general.openRallySelector()
  }
}
</script>

<style lang="scss" scoped>
@use "@/styles/modules/mixins" as *;

$rem: calc-ui-rem();

// keep in sync with the height in buttons/Big2.vue
$row-big: 18em;

.center-wrap {
  align-self: center;
  display: flex;
  flex-direction: column;
  flex-wrap: nowrap;
  align-items: stretch;
  margin: 0 calc-ui-rem(15);
}

.stage {
  --gap: 0.5em;
  --primary-width: 80em;

  position: relative;
  font-size: $rem;
  margin-bottom: calc-ui-rem(2);

  &.is-simple {
    --primary-width: 64em;
  }
}

.primary {
  display: flex;
  flex-direction: column;
  gap: var(--gap);
  width: var(--primary-width);
  max-width: 100%;

  .is-open:not(.is-wiping) > & {
    visibility: hidden;
  }

  .tile {
    margin: 0;
  }
}

.row {
  display: flex;
  gap: var(--gap);

  > .tile,
  > .simple-column {
    flex: 1 1 0;
    min-width: 0;
  }
  > .tile {
    width: auto;
  }
}

.row-big > .tile {
  --button-height: #{$row-big};
}
.row > .tile-experiences {
  flex-grow: 4;
}
.row > .tile-freeroam,
.row > .tile-splitscreen {
  flex-grow: 3;
}
.is-simple .row {
  > .tile-experiences {
    flex-grow: 3;
  }
  > .tile-freeroam,
  > .tile-splitscreen {
    flex-grow: 2;
  }
}

.row-square {
  align-items: flex-start;
}
.simple-column {
  display: flex;
  flex-direction: column;
  gap: var(--gap);
  align-self: stretch;

  > * {
    flex: 1 1 0;
    width: 100%;
    min-height: 0;
  }
}

$skew-angle: 5deg;
.primary.skew .tile {
  transform: skewX(-$skew-angle);

  :deep(.text),
  :deep(.btn-content) {
    transform: skewX($skew-angle);
  }
}

.overlay-view {
  position: absolute;
  inset: 0;
  z-index: 1;
  visibility: hidden;
  pointer-events: none;
}
.is-more > .more,
.is-experiences > .experiences {
  visibility: visible;
}
.is-open.is-more:not(.is-wiping) > .more,
.is-open.is-experiences:not(.is-wiping) > .experiences {
  pointer-events: auto;
}
</style>

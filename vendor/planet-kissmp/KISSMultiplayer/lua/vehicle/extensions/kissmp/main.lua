local M = {}

local extension_load_list = {
  "kissmp_vehicle",
  "kissmp_transforms",
  "kissmp_input",
  "kissmp_gearbox",
  "kissmp_electrics",
  "kissmp_controllers",
  "kissmp_couplers",
}

local function onExtensionLoaded()
  for i=1, #extension_load_list do
    extensions.load(extension_load_list[i])
  end

  extensions.hook("onKissMPVehLoaded")
end

M.onExtensionLoaded = onExtensionLoaded

return M

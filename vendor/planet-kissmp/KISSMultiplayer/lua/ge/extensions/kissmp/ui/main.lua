local M = {}
local imgui = ui_imgui

local function draw(dt)
  kissmp_ui.tabs.favorites.draw_add_favorite_window(gui)
  if kissmp_ui.show_download then return end

  if not kissmp_ui.gui.isWindowVisible("KissMP") then return end
  imgui.SetNextWindowBgAlpha(kissmp_ui.window_opacity)
  imgui.PushStyleVar2(imgui.StyleVar_WindowMinSize, imgui.ImVec2(300, 300))
  imgui.SetNextWindowViewport(imgui.GetMainViewport().ID)
  if imgui.Begin("KissMP "..kissmp_network.VERSION_STR) then
    imgui.Text("Player name:")
    imgui.InputText("##name", kissmp_ui.player_name)
    if kissmp_network.connection.connected then
      if imgui.Button("Disconnect") then
        kissmp_network.disconnect()
      end
    end

    imgui.Dummy(imgui.ImVec2(0, 5))

    if imgui.BeginTabBar("server_tabs##") then
      if imgui.BeginTabItem("Server List") then
        kissmp_ui.tabs.server_list.draw(dt)
        imgui.EndTabItem()
      end
      if imgui.BeginTabItem("Direct Connect") then
        kissmp_ui.tabs.direct_connect.draw()
        imgui.EndTabItem()
      end
      if imgui.BeginTabItem("Create Server") then
        kissmp_ui.tabs.create_server.draw()
        imgui.EndTabItem()
      end
      if imgui.BeginTabItem("Favourites") then
        kissmp_ui.tabs.favorites.draw()
        imgui.EndTabItem()
      end
      if imgui.BeginTabItem("Settings") then
        kissmp_ui.tabs.settings.draw(dt)
        imgui.EndTabItem()
      end
      imgui.EndTabBar()
    end
  end
  imgui.End()
  imgui.PopStyleVar()
end

local function init(m)
  m.tabs.server_list.refresh(m)
  m.tabs.favorites.load(m)
  m.tabs.favorites.update(m)
  m.tabs.server_list.update_filtered(m)
end

M.draw = draw
M.init = init

return M

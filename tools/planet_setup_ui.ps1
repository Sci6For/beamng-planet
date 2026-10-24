param([Parameter(Mandatory=$true)][string]$Worker)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()
$form = New-Object System.Windows.Forms.Form
$form.Text = 'Планета — установка для BeamNG.drive'
$form.Size = New-Object System.Drawing.Size(720,540)
$form.MinimumSize = $form.Size
$form.StartPosition = 'CenterScreen'
$form.Font = New-Object System.Drawing.Font('Segoe UI',10)
function LabelAt($text,$x,$y,$w,$h) {
    $label = New-Object System.Windows.Forms.Label
    $label.Text=$text; $label.SetBounds($x,$y,$w,$h); $form.Controls.Add($label)
}
LabelAt 'Планета + мультиплеер' 24 18 650 32
LabelAt 'Все компоненты включены. Нужна совместимая BeamNG.drive 0.40; установщик проверит файлы игры. Закройте игру перед установкой.' 24 56 650 54
LabelAt 'Папка игры (в ней находится BeamNG.drive.exe)' 24 116 650 24
$game = New-Object System.Windows.Forms.TextBox
$game.SetBounds(24,145,550,28); $form.Controls.Add($game)
try {
    $steam = (Get-ItemProperty 'HKCU:\Software\Valve\Steam').SteamPath
    $candidate = Join-Path $steam 'steamapps\common\BeamNG.drive'
    if (Test-Path -LiteralPath (Join-Path $candidate 'BeamNG.drive.exe')) { $game.Text=$candidate }
} catch {}
$browse = New-Object System.Windows.Forms.Button
$browse.Text='Обзор…'; $browse.SetBounds(584,143,95,31); $form.Controls.Add($browse)
$browse.Add_Click({
    $dialog=New-Object System.Windows.Forms.FolderBrowserDialog
    $dialog.Description='Выберите папку BeamNG.drive'
    if($dialog.ShowDialog() -eq 'OK') { $game.Text=$dialog.SelectedPath }
    $dialog.Dispose()
})
LabelAt 'Отдельная папка «Планеты» (мод, профиль и кэш городов)' 24 186 650 24
$destination=New-Object System.Windows.Forms.TextBox
$destination.Text=Join-Path $env:LOCALAPPDATA 'BeamNGPlanet'
$destination.SetBounds(24,214,655,28); $form.Controls.Add($destination)
$status=New-Object System.Windows.Forms.Label
$importCars=New-Object System.Windows.Forms.CheckBox
$importCars.Text='Скопировать автомобильные моды из основного профиля (включая отключённые)'
$importCars.SetBounds(24,250,655,28); $form.Controls.Add($importCars)
$status.SetBounds(24,282,655,42); $status.Text='После установки скопируйте строку ниже в параметры запуска BeamNG в Steam.'
$form.Controls.Add($status)
$output=New-Object System.Windows.Forms.TextBox
$output.Multiline=$true; $output.ReadOnly=$true; $output.SetBounds(24,325,655,66)
$form.Controls.Add($output)
$copy=New-Object System.Windows.Forms.Button
$copy.Text='Скопировать для Steam'; $copy.SetBounds(24,405,220,38); $copy.Enabled=$false
$form.Controls.Add($copy)
$copy.Add_Click({ [System.Windows.Forms.Clipboard]::SetText($output.Text) })
$install=New-Object System.Windows.Forms.Button
$install.Text='Установить'; $install.SetBounds(500,405,179,38); $form.Controls.Add($install)
$script:job=$null
$timer=New-Object System.Windows.Forms.Timer
$timer.Interval=250
$timer.Add_Tick({
    if ($script:job -and $script:job.HasExited) {
        $timer.Stop()
        try {
            $result=Get-Content -LiteralPath $script:report -Raw -Encoding UTF8 | ConvertFrom-Json
            if(-not $result.ok) { throw $result.error }
            $output.Text=$result.launchOptions; $copy.Enabled=$true
            $status.Text='Готово. Steam → BeamNG.drive → Свойства → Общие → Параметры запуска. Вставьте строку ниже и запускайте игру через Steam.'
            if($result.vehicleImportError) {
                [System.Windows.Forms.MessageBox]::Show(('Мод установлен, но машины не скопированы: '+$result.vehicleImportError),'Планета') | Out-Null
            }
            $install.Text='Установлено'
        } catch {
            $status.Text='Установка не завершена. См. сообщение об ошибке.'
            [System.Windows.Forms.MessageBox]::Show($_.Exception.Message,'Планета — ошибка') | Out-Null
            $install.Enabled=$true
        } finally {
            $game.Enabled=$true; $destination.Enabled=$true; $browse.Enabled=$true
            $script:job.Dispose(); $script:job=$null
            if(Test-Path -LiteralPath $script:report) { Remove-Item -LiteralPath $script:report }
        }
    }
})
$install.Add_Click({
    if(-not [IO.Path]::IsPathRooted($game.Text) -or -not [IO.Path]::IsPathRooted($destination.Text)) {
        [System.Windows.Forms.MessageBox]::Show('Выберите полные пути к папкам.','Планета') | Out-Null; return
    }
    if($game.Text.Contains('"') -or $destination.Text.Contains('"')) { return }
    $script:report=Join-Path ([IO.Path]::GetTempPath()) ('planet-setup-'+[guid]::NewGuid().ToString('N')+'.json')
    $info=New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName=$Worker; $info.UseShellExecute=$false; $info.CreateNoWindow=$true
    $gameArg=$game.Text.TrimEnd('\'); $destArg=$destination.Text.TrimEnd('\')
    $info.Arguments='--install --game-root "'+$gameArg+'" --destination "'+$destArg+'" --report "'+$script:report+'"'
    if($importCars.Checked) { $info.Arguments+=' --import-vehicles' }
    try {
        $script:job=[System.Diagnostics.Process]::Start($info)
        $install.Enabled=$false; $game.Enabled=$false; $destination.Enabled=$false; $browse.Enabled=$false
        $status.Text='Проверка версии игры, распаковка и установка…'; $timer.Start()
    } catch { [System.Windows.Forms.MessageBox]::Show($_.Exception.Message,'Планета') | Out-Null }
})
$form.Add_FormClosing({ if($script:job -and -not $script:job.HasExited) { $_.Cancel=$true } })
[void]$form.ShowDialog()
$timer.Dispose(); $form.Dispose()

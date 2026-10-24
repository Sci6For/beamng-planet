param([string]$Directory, [string]$Session, [int]$OwnerPid, [string]$SelfTest, [string]$TestMp3)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName PresentationCore,WindowsBase,System.Windows.Forms
$script:seq=0
$script:tracks=@()
$script:current=-1
$script:playing=$false
$script:errorText=''
$script:musicVolume=0.65
$script:voiceVolume=0.9
$script:voiceEnabled=$true
$script:shuffle=$false
$script:folder=''
$script:failedTracks=0
$script:player=New-Object System.Windows.Media.MediaPlayer
$script:synth=$null
$script:voiceAvailable=$false
$script:voiceError='Русский голос Windows недоступен. Текстовые подсказки остаются включены.'
try {
  $script:synth=New-Object -ComObject SAPI.SpVoice
  $ru=@($script:synth.GetVoices() | Where-Object {$_.GetAttribute('Language') -match '(^|;)419(;|$)'})
  if($ru.Count -gt 0) {
    $script:synth.Voice=$ru[0]
    $script:voiceAvailable=$true
    $script:voiceError=''
  }
} catch { $script:voiceError='Не удалось включить русский голос Windows: '+$_.Exception.Message }

function Test-Request($r,[int]$Size) {
  if($Size -gt 16384 -or $null -eq $r -or $r.version -ne 1 -or $r.session -cne $Session) {return $false}
  if($r.sequence -isnot [long] -and $r.sequence -isnot [int]) {return $false}
  if($r.sequence -le $script:seq -or $r.sequence -gt 9007199254740991) {return $false}
  if($r.command -notin @('ping','speak','cancelSpeech','chooseFolder','play','pause','next','previous','setVolume','setShuffle','setVoice')) {return $false}
  if($r.command -eq 'speak') {
    if($r.text -isnot [string] -or $r.text.Length -gt 512 -or !$r.text.Trim() -or $r.eventId -isnot [string] -or $r.eventId.Length -gt 128) {return $false}
  }
  if($r.command -eq 'setVolume') {
    if($r.channel -notin @('music','voice') -or $r.value -isnot [ValueType] -or [double]::IsNaN([double]$r.value) -or $r.value -lt 0 -or $r.value -gt 1) {return $false}
  }
  if($r.command -in @('setShuffle','setVoice') -and $r.value -isnot [bool]) {return $false}
  return $true
}
function Update-Volume {
  $speaking=$script:voiceAvailable -and $script:synth.Status.RunningState -eq 2
  $script:player.Volume=$script:musicVolume * $(if($speaking){0.25}else{1})
  if($script:synth) {$script:synth.Volume=[int]($script:voiceVolume*100)}
}
function Cancel-Speech {
  if($script:synth) {try {[void]$script:synth.Speak('',3)} catch {$script:voiceAvailable=$false;$script:voiceError='Речевое устройство Windows недоступно.'}}
  $script:player.Volume=$script:musicVolume
}
function Save-Settings {
  if(!$Directory) {return}
  @{folder=$script:folder;musicVolume=$script:musicVolume;voiceVolume=$script:voiceVolume;voiceEnabled=$script:voiceEnabled;shuffle=$script:shuffle} |
    ConvertTo-Json | Set-Content -LiteralPath (Join-Path $Directory 'audio-settings.json') -Encoding UTF8
}
function Read-Tracks([string]$Path) {
  $script:player.Stop(); $script:playing=$false; $script:current=-1; $script:tracks=@();$script:failedTracks=0
  if(!(Test-Path -LiteralPath $Path -PathType Container)) {$script:errorText='Папка с музыкой недоступна.';return}
  $script:folder=$Path
  # Top-level folder only: bounded scan, no recursion or copying into the map cache.
  $files=@(Get-ChildItem -LiteralPath $Path -File | Where-Object {$_.Extension.ToLowerInvariant() -in @('.mp3','.wav')} | Select-Object -First 5001)
  $script:errorText=if($files.Count -gt 5000){'Показаны первые 5000 песен. Выберите папку поменьше.'}else{''}
  $i=0
  $script:tracks=@($files | Select-Object -First 5000 | ForEach-Object {
    $i++; @{id=[string]$i;name=$_.Name;path=$_.FullName}
  })
}
function Play-Track([int]$Index) {
  if(!$script:tracks.Count) {$script:errorText='Выберите папку с MP3 или WAV.';return}
  $script:current=($Index+$script:tracks.Count)%$script:tracks.Count
  $checked=0
  while($checked -lt $script:tracks.Count) {
    $path=$script:tracks[$script:current].path
    if(Test-Path -LiteralPath $path -PathType Leaf) {break}
    $script:errorText='Песня недоступна: '+$script:tracks[$script:current].name
    $checked++;$script:current=($script:current+1)%$script:tracks.Count
  }
  if($checked -eq $script:tracks.Count) {$script:player.Stop();$script:playing=$false;return}
  $script:player.Open([Uri]::new($path)); Update-Volume
  $script:player.Play();$script:playing=$true
}
function Next-Track([int]$Direction=1) {
  if(!$script:tracks.Count) {return}
  $idx=$script:current+$Direction
  if($script:shuffle -and $script:tracks.Count -gt 1) {
    $idx=($script:current+(Get-Random -Minimum 1 -Maximum $script:tracks.Count))%$script:tracks.Count
  }
  Play-Track $idx
}
$script:player.add_MediaEnded({$script:failedTracks=0;Next-Track})

$script:player.add_MediaFailed({
  $script:errorText='Не удалось воспроизвести песню; файл пропущен.'
  $script:failedTracks++
  if($script:failedTracks -lt $script:tracks.Count) {Next-Track} else {$script:playing=$false;$script:player.Stop()}
})
function Apply-Command($r) {
  switch($r.command) {
    'speak' {
      if($script:voiceEnabled -and $script:voiceAvailable -and $r.createdAt -and ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds()-[long]$r.createdAt) -le 8) {
        Cancel-Speech
        try {[void]$script:synth.Speak([string]$r.text,3)} catch {$script:voiceAvailable=$false;$script:voiceError='Речевое устройство Windows недоступно.'}
      }
    }
    'cancelSpeech' {Cancel-Speech}
    'chooseFolder' {
      $dialog=New-Object System.Windows.Forms.FolderBrowserDialog
      $dialog.Description='Выберите папку с музыкой (MP3, WAV)'
      $dialog.SelectedPath=$script:folder
      try {if($dialog.ShowDialog() -eq 'OK') {Read-Tracks $dialog.SelectedPath;Save-Settings}} finally {$dialog.Dispose()}
    }
    'play' {
      $script:failedTracks=0
      if($r.trackId) {
        $idx=-1
        for($i=0;$i -lt $script:tracks.Count;$i++){if($script:tracks[$i].id -ceq [string]$r.trackId){$idx=$i;break}}
        if($idx -ge 0){Play-Track $idx}else{$script:errorText='Песня отсутствует в выбранной папке.'}
      } elseif($script:current -ge 0) {$script:player.Play();$script:playing=$true} else {Play-Track 0}
    }
    'pause' {$script:player.Pause();$script:playing=$false}
    'next' {$script:failedTracks=0;Next-Track}
    'previous' {$script:failedTracks=0;Next-Track -1}
    'setVolume' {if($r.channel -eq 'voice'){$script:voiceVolume=[double]$r.value}else{$script:musicVolume=[double]$r.value};Save-Settings}
    'setShuffle' {$script:shuffle=[bool]$r.value;Save-Settings}
    'setVoice' {$script:voiceEnabled=[bool]$r.value;if(!$script:voiceEnabled){Cancel-Speech};Save-Settings}
  }
  Update-Volume
}
function Pump {
  $frame=New-Object System.Windows.Threading.DispatcherFrame
  $timer=New-Object System.Windows.Threading.DispatcherTimer
  $timer.Interval=[TimeSpan]::FromMilliseconds(100)
  $timer.add_Tick({$frame.Continue=$false})
  $timer.Start()
  try {[System.Windows.Threading.Dispatcher]::PushFrame($frame)}finally{$timer.Stop()}
}
function Publish {
  $state=@{version=1;session=$Session;sequence=$script:seq;ready=$true;voiceAvailable=$script:voiceAvailable;voiceError=$script:voiceError;
    playing=$script:playing;voiceEnabled=$script:voiceEnabled;musicVolume=$script:musicVolume;voiceVolume=$script:voiceVolume;shuffle=$script:shuffle;
    currentId=$(if($script:current -ge 0){$script:tracks[$script:current].id}else{''});error=$script:errorText;
    tracks=@($script:tracks | ForEach-Object {@{id=$_.id;name=$_.name}})}
  $tmp=Join-Path $Directory 'audio-status.tmp'
  $target=Join-Path $Directory 'audio-status.json'
  [IO.File]::WriteAllText($tmp,($state | ConvertTo-Json -Depth 5 -Compress),[Text.UTF8Encoding]::new($false))
  if([IO.File]::Exists($target)){[IO.File]::Replace($tmp,$target,[NullString]::Value)}else{[IO.File]::Move($tmp,$target)}
}

if($SelfTest) {
  $Session='test';$script:seq=2
  $good=[pscustomobject]@{version=1;session='test';sequence=3;command='ping'}
  $r=@{oversizeRejected=!(Test-Request $good 16385)}
  $good.session='old';$r.oldSessionRejected=!(Test-Request $good 50);$good.session='test'
  $good.sequence=2;$r.duplicateRejected=!(Test-Request $good 50)
  $r.invalidVolumeRejected=!(Test-Request ([pscustomobject]@{version=1;session='test';sequence=3;command='setVolume';channel='music';value=2}) 50)
  $r.longSpeechRejected=!(Test-Request ([pscustomobject]@{version=1;session='test';sequence=3;command='speak';eventId='x';text=('a'*513)}) 900)
  $r.voiceAvailable=$script:voiceAvailable;$r.voiceError=$script:voiceError
  $testDir=Split-Path -Parent $SelfTest
  $wave=Join-Path $testDir 'Проверка.wav'
  $w=[IO.BinaryWriter]::new([IO.File]::Create($wave))
  try {
    $w.Write([Text.Encoding]::ASCII.GetBytes('RIFF'));$w.Write([int]1636);$w.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '));$w.Write([int]16)
    $w.Write([int16]1);$w.Write([int16]1);$w.Write([int]8000);$w.Write([int]16000);$w.Write([int16]2);$w.Write([int16]16)
    $w.Write([Text.Encoding]::ASCII.GetBytes('data'));$w.Write([int]1600);$w.Write([byte[]]::new(1600))
  }finally{$w.Dispose()}
  $script:tracks=@(@{id='1';name='Проверка.wav';path=$wave});$script:musicVolume=0
  Play-Track 0
  for($i=0;$i -lt 10;$i++){Pump}
  $r.unicodeWaveOpened=$script:player.NaturalDuration.HasTimeSpan
  $script:player.Stop();$script:tracks=@(@{id='1';name='missing';path=(Join-Path $testDir 'missing.wav')})
  Play-Track 0;$r.missingTrackError=($script:errorText -like '*недоступна*' -and !$script:playing)
  $script:tracks=@(1..700 | ForEach-Object {@{id=[string]$_;name='missing';path=(Join-Path $testDir ('missing'+$_+'.wav'))}})
  Play-Track 0;$r.missingBatchSkipped=(!$script:playing)
  $script:player.Open([Uri]::new($TestMp3));$script:player.Volume=0;$script:player.Play()
  for($i=0;$i -lt 10;$i++){Pump}
  $r.mp3Opened=$script:player.NaturalDuration.HasTimeSpan
  $script:player.Stop()
  $r.voiceGenerated=$false
  if($script:voiceAvailable) {
    $speechPath=Join-Path $testDir 'Голос.wav'
    $stream=New-Object -ComObject SAPI.SpFileStream
    try {
      $stream.Open($speechPath,3,$false)
      $script:synth.AudioOutputStream=$stream
      [void]$script:synth.Speak('Через двести метров поверните направо',0)
      $stream.Close()
      $r.voiceGenerated=((Get-Item -LiteralPath $speechPath).Length -gt 44)
    }finally{$script:synth.AudioOutputStream=$null;[void][Runtime.InteropServices.Marshal]::ReleaseComObject($stream)}
  }
  $script:musicVolume=0.6;Cancel-Speech;$r.duckRestored=($script:player.Volume -eq 0.6)
  $r | ConvertTo-Json | Set-Content -LiteralPath $SelfTest -Encoding UTF8
  $script:player.Close();if($script:synth){[void][Runtime.InteropServices.Marshal]::ReleaseComObject($script:synth)};exit 0
}

try {
  $settingsPath=Join-Path $Directory 'audio-settings.json'
  if(Test-Path -LiteralPath $settingsPath) {
    $s=Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    foreach($key in @('musicVolume','voiceVolume')) {if($s.$key -ge 0 -and $s.$key -le 1){Set-Variable -Scope Script -Name $key -Value ([double]$s.$key)}}
    if($s.voiceEnabled -is [bool]){$script:voiceEnabled=$s.voiceEnabled}
    if($s.shuffle -is [bool]){$script:shuffle=$s.shuffle}
    if($s.folder){Read-Tracks $s.folder}
  }
}catch{$script:errorText='Настройки музыки сброшены.'}
$tick=0
try {
  while(Get-Process -Id $OwnerPid -ErrorAction SilentlyContinue) {
    try {
      $changed=$false
      $path=Join-Path $Directory 'audio-worker-request.json'
      if(Test-Path -LiteralPath $path) {
        $f=Get-Item -LiteralPath $path
        if($f.Length -le 16384) {
          $r=Get-Content -LiteralPath $path -Raw -Encoding UTF8 | ConvertFrom-Json
          if(Test-Request $r $f.Length) {$script:seq=$r.sequence;Apply-Command $r;$changed=$true}
        }
      }
      Update-Volume
      if($changed -or $tick%10 -eq 0){Publish}
      $tick++
    } catch {$script:errorText='Ошибка звука: '+$_.Exception.Message;[IO.File]::WriteAllText((Join-Path $Directory 'audio-error.log'),($_ | Out-String),[Text.UTF8Encoding]::new($false))}
    Pump
  }
} finally {Cancel-Speech;$script:player.Close();if($script:synth){[void][Runtime.InteropServices.Marshal]::ReleaseComObject($script:synth)}}

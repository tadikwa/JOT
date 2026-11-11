param(
    [Parameter(Mandatory)][string]$App,      # folder holding jot.exe + jot-test-profile marker
    [Parameter(Mandatory)][string]$Audio,    # folder of synthetic .wav
    [ValidateSet('off', 'fast', 'quality')][string]$Mode = 'fast',
    [string]$Only = '',
    [string]$Tag = '',
    [int]$TimeoutSec = 150
)
# Real binary, isolated profile: plants each WAV as an interrupted dictation, lets the
# launch-time recovery run it through LocalTranscriptionService, then stops that PID only.
$ErrorActionPreference = 'Stop'
$exe = Join-Path $App 'jot.exe'
$profileDir = Join-Path $App 'profile'
New-Item -ItemType Directory -Force $profileDir | Out-Null
if (-not (Test-Path (Join-Path $App 'jot-test-profile'))) { throw 'no jot-test-profile marker: refusing to run' }
$settings = Join-Path $profileDir 'settings.json'
@{
    uiLanguage = 'french'; hasCompletedOnboarding = $true; hotkeyKey = 'rightWin'
    dictationActivationMode = 'hold'; transcriptionEngine = 'local'; localReformulation = $Mode
    audioRetentionDays = 7; soundsEnabled = $false
} | ConvertTo-Json | ForEach-Object { [IO.File]::WriteAllText($settings, $_) }

function JotProcs { Get-CimInstance Win32_Process -Filter "Name='jot.exe' OR Name='llama-server.exe'" | Select-Object ProcessId, Name, ExecutablePath }
$before = JotProcs
"PRE processes: " + (($before | ForEach-Object { "$($_.Name)#$($_.ProcessId) $($_.ExecutablePath)" }) -join ' ; ')

$wavs = Get-ChildItem $Audio -Filter *.wav | Where-Object { -not $Only -or $_.BaseName -match $Only } | Sort-Object Name
foreach ($wav in $wavs) {
    $id = [guid]::NewGuid().ToString()
    $folder = Join-Path $profileDir "recordings\e2e-$Tag$($wav.BaseName)-$($id.Substring(0,8))"
    New-Item -ItemType Directory -Force $folder | Out-Null
    Copy-Item $wav.FullName (Join-Path $folder 'audio.wav')
    $seconds = [math]::Round(($wav.Length - 44) / 32000.0, 2)
    @{ id = $id; startedAt = (Get-Date).ToUniversalTime().ToString("yyyy-MM-dd'T'HH:mm:ss'Z'"); status = 'recorded'
       audioDurationSeconds = $seconds; gapMarkers = @() } | ConvertTo-Json | ForEach-Object { [IO.File]::WriteAllText((Join-Path $folder "meta.json"), $_) }

    $watch = [Diagnostics.Stopwatch]::StartNew()
    $p = Start-Process $exe -ArgumentList '--background' -WorkingDirectory $App -PassThru
    "LAUNCH pid=$($p.Id) path=$exe case=$($wav.BaseName) mode=$Mode audio=${seconds}s"
    $meta = $null; $remote = @{}
    while ($watch.Elapsed.TotalSeconds -lt $TimeoutSec) {
        Start-Sleep -Milliseconds 250
        if ($p.HasExited) { "EXITED early code=$($p.ExitCode)"; break }
        $owners = @($p.Id) + @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)" | ForEach-Object ProcessId)
        Get-NetTCPConnection -OwningProcess $owners -ErrorAction SilentlyContinue | Where-Object { $_.RemoteAddress -notin @('127.0.0.1', '::1', '0.0.0.0', '::') } | ForEach-Object { $remote["$($_.RemoteAddress):$($_.RemotePort)"] = 1 }
        try { $meta = Get-Content -Raw -Encoding UTF8 (Join-Path $folder 'meta.json') | ConvertFrom-Json } catch { continue }
        if ($meta.status -notin @('recorded', 'transcribing', 'recording')) { break }
    }
    $elapsed = [math]::Round($watch.Elapsed.TotalSeconds, 1)
    $children = Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" | Where-Object { $_.ExecutablePath -like "$profileDir*" }
    if (-not $p.HasExited) { Stop-Process -Id $p.Id; $p.WaitForExit(10000) | Out-Null }
    Start-Sleep -Milliseconds 800
    $orphans = Get-CimInstance Win32_Process -Filter "Name='llama-server.exe'" | Where-Object { $_.ExecutablePath -like "$profileDir*" }
    [pscustomobject]@{
        case = $wav.BaseName; mode = $Mode; status = $meta.status; model_id = $meta.modelId; error = $meta.errorCode
        launch_to_result_s = $elapsed; llama_children = @($children).Count; non_loopback_tcp = ($remote.Keys -join ','); orphans_after_stop = @($orphans).Count
        raw = $meta.rawTranscript; cleaned = $meta.cleanedTranscript
    } | ConvertTo-Json -Compress
}
$after = JotProcs
"POST processes: " + (($after | ForEach-Object { "$($_.Name)#$($_.ProcessId) $($_.ExecutablePath)" }) -join ' ; ')

param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$ReportDirectory
)
$ErrorActionPreference='Stop'
$exe=(Resolve-Path -LiteralPath $Executable).Path
$report=[System.IO.Path]::GetFullPath($ReportDirectory)
if(Test-Path -LiteralPath $report){throw 'Utiliser un nouveau dossier de rapport.'}
$existing=@(Get-Process jot -ErrorAction SilentlyContinue)
if($existing.Count){$existing|Select-Object Id,Path|Format-Table;throw 'Fermer normalement Jot avant ce test.'}
New-Item -ItemType Directory -Path $report|Out-Null
$start=[System.Diagnostics.ProcessStartInfo]::new($exe)
$start.UseShellExecute=$false
$start.ArgumentList.Add('--hud-validate')
$start.ArgumentList.Add($report)
$proc=[System.Diagnostics.Process]::Start($start)
try {
    [pscustomobject]@{PID=$proc.Id;Executable=$exe;SHA256=(Get-FileHash -LiteralPath $exe).Hash}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $report 'controller.json')
    if(-not $proc.WaitForExit(55000)){throw 'Délai maximal atteint.'}
    $exitCode=$proc.ExitCode
    $rows=Get-Content -LiteralPath (Join-Path $report 'result.json') -Raw|ConvertFrom-Json
    $rows|Select-Object monitor,cycle,state,rendered,pill_hit_drawn_pixel,received_clicks,focus_ok,size_ok,placement_ok,animation_ok|Format-Table
    if($exitCode -ne 0){throw "Test échoué (code $exitCode). Consulter result.json."}
    Write-Output 'PASS : rendu, clics réels, focus, dimensions et animation.'
} finally {
    if(-not $proc.HasExited){$proc.Kill();$proc.WaitForExit()}
    [pscustomobject]@{PID=$proc.Id;Stopped=$proc.HasExited}|ConvertTo-Json|Set-Content -LiteralPath (Join-Path $report 'controller-stopped.json')
}


param([string]$Corpus,[string]$Out)
Add-Type -AssemblyName System.Speech
New-Item -ItemType Directory -Force $Out | Out-Null
$items = Get-Content -Raw -Encoding UTF8 $Corpus | ConvertFrom-Json
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000,[System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen,[System.Speech.AudioFormat.AudioChannel]::Mono)
foreach ($it in $items) {
  $s = New-Object System.Speech.Synthesis.SpeechSynthesizer
  $s.SelectVoice("Microsoft Hortense Desktop")
  $s.Rate = 1
  $s.SetOutputToWaveFile((Join-Path $Out ($it.id + ".wav")), $fmt)
  $s.Speak($it.say); $s.Dispose()
}
Get-ChildItem $Out | Select-Object Name,Length

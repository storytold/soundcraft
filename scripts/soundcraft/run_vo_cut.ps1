# SoundCraft VO cut — Whisper → cut plan → soundcraft-cli
# Bind to a hotkey via PowerToys Keyboard Manager or a desktop shortcut.
param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Rest
)

$ErrorActionPreference = "Stop"
$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
Set-Location $Root

$Py = if ($env:ZVUKACH_PYTHON) { $env:ZVUKACH_PYTHON } else { "python" }
$Script = Join-Path $PSScriptRoot "vo_cut_pipeline.py"

if (-not $Rest -or $Rest.Count -eq 0) {
    & $Py $Script --prefer-cached-asr --llm auto --apply
    exit $LASTEXITCODE
}

& $Py $Script @Rest
exit $LASTEXITCODE

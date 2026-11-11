param(
  [Parameter(Mandatory=$true)][string]$ExeSource,
  [Parameter(Mandatory=$true)][string]$ProductVersion,
  [Parameter(Mandatory=$true)][string]$OutDir
)

# Requires WiX v3.x (candle.exe, light.exe). Uses core PermissionEx only (no Util extension).
$ErrorActionPreference = "Stop"

if (!(Test-Path -LiteralPath $ExeSource)) {
  Write-Error "ExeSource not found: $ExeSource"
  exit 1
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$wxs = Join-Path $PSScriptRoot "grindvakt.wxs"
$wixobj = Join-Path $OutDir "grindvakt.wixobj"
$msi = Join-Path $OutDir ("grindvakt-" + $ProductVersion + "-windows-x64.msi")

$candle = Get-Command candle.exe -ErrorAction Stop
$light = Get-Command light.exe -ErrorAction Stop

& $candle.Source -nologo -arch x64 "-dExeSource=$ExeSource" "-dProductVersion=$ProductVersion" "-dRepoRoot=$repoRoot" -out $wixobj $wxs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& $light.Source -nologo -out $msi $wixobj
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if (!(Test-Path -LiteralPath $msi)) {
  Write-Error "light.exe reported success but MSI is missing: $msi"
  exit 1
}

Write-Host "MSI created at $msi"

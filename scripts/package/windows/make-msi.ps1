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

# The installer's License pane needs RTF; generate it from LICENSE so the two never drift.
$licenseSrc = Join-Path $repoRoot "LICENSE"
if (!(Test-Path -LiteralPath $licenseSrc)) { Write-Error "missing $licenseSrc"; exit 1 }
$licenseText = [IO.File]::ReadAllText($licenseSrc)
$escaped = foreach ($line in ($licenseText -replace "`r", "") -split "`n") {
  $l = $line.Replace('\', '\\').Replace('{', '\{').Replace('}', '\}')
  -join ($l.ToCharArray() | ForEach-Object { if ([int]$_ -gt 127) { "\u" + [int]$_ + "?" } else { [string]$_ } })
}
$licenseRtf = Join-Path ([IO.Path]::GetTempPath()) ("grindvakt-license-" + [guid]::NewGuid() + ".rtf")
$rtf = "{\rtf1\ansi\deff0{\fonttbl{\f0\fmodern Consolas;}}\f0\fs18 " + ($escaped -join "\par`r`n") + "}"
[IO.File]::WriteAllText($licenseRtf, $rtf, [Text.Encoding]::ASCII)

try {
  & $candle.Source -nologo -arch x64 -ext WixUIExtension "-dExeSource=$ExeSource" "-dProductVersion=$ProductVersion" "-dRepoRoot=$repoRoot" "-dLicenseRtf=$licenseRtf" -out $wixobj $wxs
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

  & $light.Source -nologo -ext WixUIExtension -cultures:en-us -out $msi $wixobj
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
  Remove-Item -LiteralPath $licenseRtf -Force -ErrorAction SilentlyContinue
}

if (!(Test-Path -LiteralPath $msi)) {
  Write-Error "light.exe reported success but MSI is missing: $msi"
  exit 1
}

# Check what was built: LICENSE.txt is installed and the License pane shows the license.
function Invoke-MsiQuery([string]$Path, [string]$Sql) {
  $installer = New-Object -ComObject WindowsInstaller.Installer
  $db = $installer.GetType().InvokeMember("OpenDatabase", "InvokeMethod", $null, $installer, @($Path, 0))
  $view = $db.GetType().InvokeMember("OpenView", "InvokeMethod", $null, $db, @($Sql))
  $view.GetType().InvokeMember("Execute", "InvokeMethod", $null, $view, $null) | Out-Null
  $rows = @()
  while ($true) {
    $rec = $view.GetType().InvokeMember("Fetch", "InvokeMethod", $null, $view, $null)
    if ($null -eq $rec) { break }
    $rows += $rec.GetType().InvokeMember("StringData", "GetProperty", $null, $rec, @(1))
    [void][Runtime.InteropServices.Marshal]::ReleaseComObject($rec)
  }
  $view.GetType().InvokeMember("Close", "InvokeMethod", $null, $view, $null) | Out-Null
  foreach ($o in @($view, $db, $installer)) { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($o) }
  [GC]::Collect(); [GC]::WaitForPendingFinalizers()
  return ,$rows
}
$files = Invoke-MsiQuery $msi "SELECT ``FileName`` FROM ``File`` WHERE ``File``='filLicense'"
if ($files.Count -ne 1 -or $files[0] -notlike "*LICENSE.txt") { Write-Error "MSI check failed: LICENSE.txt not in File table ($files)"; exit 1 }
$pane = Invoke-MsiQuery $msi "SELECT ``Text`` FROM ``Control`` WHERE ``Control``='LicenseText'"
if ($pane.Count -lt 1 -or -not ($pane -join "`n").Contains("BSD 3-Clause License")) { Write-Error "MSI check failed: License pane does not show the license"; exit 1 }
Write-Host "MSI checks passed: LICENSE.txt installed, License pane shows BSD 3-Clause"

Write-Host "MSI created at $msi"

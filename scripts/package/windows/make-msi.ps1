param(
  [Parameter(Mandatory=$true)][string]$ExeSource,
  [Parameter(Mandatory=$true)][string]$ProductVersion,
  [Parameter(Mandatory=$true)][string]$OutDir,
  [string]$WixVersion = "7.0.1"
)

# Requires the WiX Toolset 7 CLI (wix.exe) with the WixToolset.UI.wixext and
# WixToolset.Util.wixext extensions of the same version in the global cache:
#   wix extension add -acceptEula wix7 -g WixToolset.UI.wixext/7.0.1 WixToolset.Util.wixext/7.0.1
# Builds pass `-acceptEula wix7`: WiX 7 binaries are used under the Open Source Maintenance Fee
# EULA v1.1 (https://docs.firegiant.com/wix/osmf/), which asks no fee below US$10,000 a year of
# revenue from projects that use WiX. Michael accepted it for Grindvakt on 2026-11-12 (docs/RELEASES.md).
$ErrorActionPreference = "Stop"

if (!(Test-Path -LiteralPath $ExeSource)) {
  Write-Error "ExeSource not found: $ExeSource"
  exit 1
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$wxs = Join-Path $PSScriptRoot "grindvakt.wxs"
$msi = Join-Path $OutDir ("grindvakt-" + $ProductVersion + "-windows-x64.msi")

$wix = Get-Command wix.exe -ErrorAction Stop
$have = (& $wix.Source --version).Trim()
if (-not $have.StartsWith("$WixVersion")) { Write-Error "wix.exe is $have, expected $WixVersion"; exit 1 }

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
  & $wix.Source build -nologo -acceptEula wix7 -arch x64 `
    -ext "WixToolset.UI.wixext/$WixVersion" -ext "WixToolset.Util.wixext/$WixVersion" `
    -d "ExeSource=$ExeSource" -d "ProductVersion=$ProductVersion" -d "RepoRoot=$repoRoot" -d "LicenseRtf=$licenseRtf" `
    -culture en-US -pdbtype none -o $msi $wxs
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
} finally {
  Remove-Item -LiteralPath $licenseRtf -Force -ErrorAction SilentlyContinue
}

if (!(Test-Path -LiteralPath $msi)) {
  Write-Error "wix build reported success but the MSI is missing: $msi"
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
# The service's data dir must be %PROGRAMDATA%\Grindvakt (CommonAppDataFolder), never C:\Grindvakt.
$dirs = Invoke-MsiQuery $msi "SELECT ``Directory`` FROM ``Directory`` WHERE ``Directory_Parent``='TARGETDIR'"
if (-not ($dirs -contains "CommonAppDataFolder") -or ($dirs -contains "ProgramDataFolder")) { Write-Error "MSI check failed: data dir is not under CommonAppDataFolder ($dirs)"; exit 1 }
$svcArgs = Invoke-MsiQuery $msi "SELECT ``Arguments`` FROM ``ServiceInstall`` WHERE ``ServiceInstall``='svcInstall'"
if ($svcArgs.Count -ne 1 -or -not $svcArgs[0].Contains('--data-dir="[CommonAppDataFolder]Grindvakt"')) { Write-Error "MSI check failed: service arguments ($svcArgs)"; exit 1 }
$envPath = Invoke-MsiQuery $msi "SELECT ``Value`` FROM ``Environment`` WHERE ``Environment``='envPath'"
if ($envPath.Count -ne 1 -or -not $envPath[0].EndsWith("[INSTALLFOLDER]")) { Write-Error "MSI check failed: PATH entry ($envPath)"; exit 1 }
$svcAccount = Invoke-MsiQuery $msi "SELECT ``StartName`` FROM ``ServiceInstall`` WHERE ``ServiceInstall``='svcInstall'"
if ($svcAccount.Count -ne 1 -or $svcAccount[0] -ne "NT SERVICE\Grindvakt") { Write-Error "MSI check failed: service account ($svcAccount)"; exit 1 }
$sddl = Invoke-MsiQuery $msi "SELECT ``SDDLText`` FROM ``MsiLockPermissionsEx`` WHERE ``LockObject``='ProgramDataGrindvakt'"
$svcSid = "S-1-5-80-1499467798-2283939423-2764506236-959416072-1900373211"
if ($sddl.Count -ne 1 -or -not $sddl[0].Contains("(A;OICI;FA;;;$svcSid)") -or $sddl[0].Contains(";;;LS)")) { Write-Error "MSI check failed: data dir ACL ($sddl)"; exit 1 }
# util:ServiceConfig is the Wix4ServiceConfig table in WiX 4 and later.
foreach ($col in @("FirstFailureActionType", "SecondFailureActionType", "ThirdFailureActionType")) {
  $action = Invoke-MsiQuery $msi "SELECT ``$col`` FROM ``Wix4ServiceConfig`` WHERE ``ServiceName``='Grindvakt'"
  if ($action.Count -ne 1 -or $action[0] -ne "restart") { Write-Error "MSI check failed: service recovery $col ($action)"; exit 1 }
}
foreach ($ca in @("GvRememberRunning", "GvRepairAcl", "GvRestoreRunning")) {
  $seq = Invoke-MsiQuery $msi "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='$ca'"
  if ($seq.Count -ne 1) { Write-Error "MSI check failed: custom action $ca is not scheduled"; exit 1 }
}
function Get-Seq([string]$Action) { [int](Invoke-MsiQuery $msi "SELECT ``Sequence`` FROM ``InstallExecuteSequence`` WHERE ``Action``='$Action'")[0] }
# Nothing deferred may precede RemoveExistingProducts (error 2613), so GvRememberRunning is immediate.
$rememberType = [int](Invoke-MsiQuery $msi "SELECT ``Type`` FROM ``CustomAction`` WHERE ``Action``='GvRememberRunning'")[0]
if ($rememberType -band 0x400) { Write-Error "MSI check failed: GvRememberRunning must be immediate (type $rememberType)"; exit 1 }
if (-not ((Get-Seq "GvRememberRunning") -lt (Get-Seq "InstallInitialize") -and (Get-Seq "InstallInitialize") -lt (Get-Seq "RemoveExistingProducts") -and
          (Get-Seq "InstallFiles") -lt (Get-Seq "GvRepairAcl") -and (Get-Seq "GvRepairAcl") -lt (Get-Seq "StartServices") -and
          (Get-Seq "StartServices") -lt (Get-Seq "GvRestoreRunning") -and (Get-Seq "GvRestoreRunning") -lt (Get-Seq "InstallFinalize"))) {
  Write-Error "MSI check failed: upgrade custom actions are out of order"; exit 1
}
Write-Host "MSI checks passed: LICENSE.txt installed, License pane shows BSD 3-Clause, service data dir is %PROGRAMDATA%\Grindvakt, PATH entry present, restart on failure, runs as NT SERVICE\Grindvakt with a data dir ACL for that account only, upgrade actions in order"

Write-Host "MSI created at $msi"

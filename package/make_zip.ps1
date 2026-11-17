# Builds "DS3 Pause v<version>.zip" laid out exactly like the DS3 Game folder:
# players extract everything straight into DARK SOULS III\Game.
param([string]$Version = "1.0")
$ErrorActionPreference = "Stop"
$root = Split-Path $PSScriptRoot -Parent
$stage = Join-Path $root "dist\stage"
$zip = Join-Path $root "dist\DS3 Pause v$Version.zip"

if (Test-Path (Join-Path $root "dist")) { Remove-Item -Recurse -Force (Join-Path $root "dist") }
New-Item -ItemType Directory -Force $stage | Out-Null

Copy-Item (Join-Path $root "target\release\ds3pause.exe") (Join-Path $stage "DS3 Pause.exe")
Copy-Item (Join-Path $root "loader\target\release\dinput8.dll") $stage

# Text files with Windows line endings so Notepad shows them correctly.
function Write-Crlf($src, $dst) {
    $t = [IO.File]::ReadAllText($src) -replace "`r?`n", "`r`n"
    [IO.File]::WriteAllText($dst, $t, (New-Object Text.UTF8Encoding($false)))
}
# One text file: the READ ME with the license appended at the bottom.
$readme = [IO.File]::ReadAllText((Join-Path $PSScriptRoot "DS3 Pause - READ ME.txt")).TrimEnd()
$license = [IO.File]::ReadAllText((Join-Path $root "LICENSE")).TrimEnd()
$combined = "$readme`n`n`nLICENSE`n-------`n$license`n"
$tmp = Join-Path $stage "readme.tmp"
[IO.File]::WriteAllText($tmp, $combined)
Write-Crlf $tmp (Join-Path $stage "DS3 Pause - READ ME.txt")
Remove-Item $tmp

Compress-Archive -Path (Join-Path $stage "*") -DestinationPath $zip -Force
Remove-Item -Recurse -Force $stage
Write-Output $zip

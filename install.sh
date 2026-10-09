#!/bin/sh
# Build the Windows app and install it for the current Windows user:
#   %LOCALAPPDATA%\Programs\Unibox\unibox.exe, plus a Start Menu shortcut.
# Mail and settings live in %LOCALAPPDATA%\Unibox and are not touched.
set -e
cd "$(dirname "$0")"
./build.sh windows
SYS=/mnt/c/Windows/System32
LOCAL=$(wslpath "$($SYS/cmd.exe /c 'echo %LOCALAPPDATA%' 2>/dev/null | tr -d '\r')")
DEST="$LOCAL/Programs/Unibox"
mkdir -p "$DEST"
# a running copy keeps the file locked
$SYS/taskkill.exe /IM unibox.exe /F >/dev/null 2>&1 || true
sleep 1
cp target/x86_64-pc-windows-gnullvm/release/unibox.exe "$DEST/unibox.exe"
$SYS/WindowsPowerShell/v1.0/powershell.exe -NoProfile -Command '
  $exe = Join-Path $env:LOCALAPPDATA "Programs\Unibox\unibox.exe"
  $lnk = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Unibox.lnk"
  $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
  $s.TargetPath = $exe; $s.WorkingDirectory = Split-Path $exe; $s.Description = "Unified Gmail inbox"; $s.Save()
  Write-Output "Installed $exe"'

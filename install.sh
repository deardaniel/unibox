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
# A running copy keeps the file locked, and its window would go on showing the old interface.
$SYS/taskkill.exe /IM unibox.exe /F >/dev/null 2>&1 || true
$SYS/WindowsPowerShell/v1.0/powershell.exe -NoProfile -Command '
  Get-CimInstance Win32_Process |
    Where-Object { $_.Name -match "^(msedge|chrome)\.exe$" -and $_.CommandLine -like "*\Unibox\window*" } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }' >/dev/null 2>&1 || true
sleep 1
cp target/x86_64-pc-windows-gnullvm/release/unibox.exe "$DEST/unibox.exe"
# A second copy for other assistants to start with --mcp (see README). It only passes messages
# to the app, so it rarely needs replacing, and under its own name it is neither stopped above
# nor in the way of the next install while one of them has it running.
cp target/x86_64-pc-windows-gnullvm/release/unibox.exe "$DEST/unibox-mcp.exe" 2>/dev/null ||
  echo "unibox-mcp.exe is in use (an assistant has it open); kept the copy that is there."
# Microsoft's loader for the WebView2 runtime; the .exe will not start without it alongside.
LOADER=$(ls "$HOME"/.cargo/registry/src/*/webview2-com-sys-*/x64/WebView2Loader.dll | sort -V | tail -1)
# (left alone when it is already the same file: an assistant running unibox-mcp.exe holds it open)
cmp -s "$LOADER" "$DEST/WebView2Loader.dll" || cp "$LOADER" "$DEST/WebView2Loader.dll"
# the Edge profile from when the window was an Edge app window
rm -rf "$LOCAL/Unibox/window"
# An icon for the Start Menu shortcut (the window draws its own). Needs Pillow; skipped without it.
python3 -I - "$DEST/unibox.ico" <<'PY' 2>/dev/null || true
import sys
from PIL import Image, ImageDraw
S = 256
im = Image.new("RGBA", (S, S), (0, 0, 0, 0))
d = ImageDraw.Draw(im)
d.rounded_rectangle((0, 0, S - 1, S - 1), radius=56, fill=(0x5B, 0x2E, 0xE8, 255))
d.rounded_rectangle((56, 84, 200, 176), radius=6, outline="white", width=16)
d.line((60, 90, 128, 140, 196, 90), fill="white", width=16, joint="curve")
im.save(sys.argv[1], sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (256, 256)])
PY
$SYS/WindowsPowerShell/v1.0/powershell.exe -NoProfile -Command '
  $exe = Join-Path $env:LOCALAPPDATA "Programs\Unibox\unibox.exe"
  $lnk = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Unibox.lnk"
  $s = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
  $s.TargetPath = $exe; $s.WorkingDirectory = Split-Path $exe; $s.Description = "Unified Gmail inbox"
  $ico = Join-Path (Split-Path $exe) "unibox.ico"
  if (Test-Path $ico) { $s.IconLocation = $ico }
  $s.Save()
  Write-Output "Installed $exe"'

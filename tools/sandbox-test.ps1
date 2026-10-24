# Runs inside Windows Sandbox, started by tools/sandboxtest.sh: a clean Windows with no
# Visual C++ Runtime, no OBS Studio and no Laurel. It installs Laurel silently, shows
# its first run, runs the self-test, uninstalls it, and writes what it saw to
# C:\laurel\result.txt (the folder shared with the host), with the logs and a
# screenshot of the first run.

$ErrorActionPreference = 'Continue'
$dir = 'C:\laurel'
$out = Join-Path $dir 'result.txt'
function Note($text) { "$(Get-Date -Format HH:mm:ss) $text" | Out-File -Append -Encoding utf8 $out }
function VcRuntime {
    $k = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64' -ErrorAction SilentlyContinue
    if ($k) { "$($k.Major).$($k.Minor).$($k.Bld)" } else { 'missing' }
}
function WebView2 {
    $k = Get-ItemProperty 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}' -ErrorAction SilentlyContinue
    if ($k) { $k.pv } else { 'missing' }
}
function Screenshot($path) {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

$install = Join-Path $env:LOCALAPPDATA 'Laurel'
$data = Join-Path $env:LOCALAPPDATA 'dev.laurel.app'
$hook = Join-Path $env:ProgramData 'obs-studio-hook'
$layers = 'HKCU:\SOFTWARE\Khronos\Vulkan\ImplicitLayers'
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

Note "windows: $((Get-CimInstance Win32_OperatingSystem).Caption) $((Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').DisplayVersion); elevated: $admin"
Note "before: vc runtime $(VcRuntime); webview2 $(WebView2); obs installed $(Test-Path 'HKLM:\SOFTWARE\OBS Studio'); hook folder $(Test-Path $hook)"

$setup = Get-ChildItem $dir -Filter 'Laurel_*_x64-setup.exe' | Select-Object -First 1
$t = Get-Date
$p = Start-Process $setup.FullName -ArgumentList '/S' -Wait -PassThru
Note "install $($setup.Name): exit $($p.ExitCode) in $([int]((Get-Date) - $t).TotalSeconds) s; files $(@(Get-ChildItem $install -Recurse -File).Count); vc runtime $(VcRuntime); webview2 $(WebView2)"
Note "run value after a fresh install: $((Get-ItemProperty $run -ErrorAction SilentlyContinue).Laurel)"

# The first run as the user sees it: the window with the start-with-Windows banner.
Start-Process (Join-Path $install 'laurel-app.exe')
Start-Sleep 25
Screenshot (Join-Path $dir 'first-run.png')
Note "first run: processes $((Get-Process laurel-app, laurel-engine -ErrorAction SilentlyContinue | ForEach-Object Name) -join ', ')"
Get-Process laurel-app -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 5

$t = Get-Date
$p = Start-Process (Join-Path $install 'laurel-app.exe') -ArgumentList '--selftest' -Wait -PassThru
Note "selftest: exit $($p.ExitCode) in $([int]((Get-Date) - $t).TotalSeconds) s; $((Select-String -Path (Join-Path $data 'logs\app.log') -Pattern 'selftest: ' | Select-Object -Last 1).Line)"
New-Item -ItemType Directory -Force (Join-Path $dir 'logs') | Out-Null
Copy-Item (Join-Path $data 'logs\*') (Join-Path $dir 'logs')
Note "after recording: hook folder $(Test-Path $hook); vulkan layers for this user: $((Get-Item $layers -ErrorAction SilentlyContinue).Property -join ', ')"

$p = Start-Process (Join-Path $install 'uninstall.exe') -ArgumentList '/S' -Wait -PassThru
Start-Sleep 10
Note "uninstall: exit $($p.ExitCode); install dir $(Test-Path $install); hook folder $(Test-Path $hook); vulkan layers for this user: $((Get-Item $layers -ErrorAction SilentlyContinue).Property -join ', '); run value $((Get-ItemProperty $run -ErrorAction SilentlyContinue).Laurel); app data kept $(Test-Path $data); vc runtime $(VcRuntime)"
Note 'done'

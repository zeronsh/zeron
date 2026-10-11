# Capture only the offline fixture's client area using the existing HWND helper.
param(
    [Parameter(Mandatory = $true)][int]$FixtureProcessId,
    [Parameter(Mandatory = $true)][string]$OutputFile,
    [Parameter(Mandatory = $true)][string]$MetadataFile
)
$ErrorActionPreference = 'Stop'
$fixture = Get-Process -Id $FixtureProcessId
# MainWindowHandle excludes hidden windows. This fixture may be launched hidden.
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class AccountUsageFixtureWindow {
    public delegate bool EnumCallback(IntPtr hwnd, IntPtr param);
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumCallback callback, IntPtr param);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr hwnd, out Rect rect);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr hwnd, StringBuilder name, int length);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr hwnd, IntPtr after, int x, int y, int width, int height, uint flags);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr hwnd, int command);
    public static void RenderOffscreen(IntPtr hwnd) {
        // Keep the fixture away from the user's desktop and never activate it.
        SetWindowPos(hwnd, IntPtr.Zero, -10000, -10000, 0, 0, 0x0015);
        ShowWindow(hwnd, 4);
    }
    public static IntPtr Find(uint processId) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hwnd, param) => {
            uint owner; Rect rect;
            GetWindowThreadProcessId(hwnd, out owner);
            var name = new StringBuilder(256);
            GetClassName(hwnd, name, name.Capacity);
            if (owner == processId && name.ToString() == "Zed::Window" && GetClientRect(hwnd, out rect) && rect.Right > 100 && rect.Bottom > 100) {
                found = hwnd; return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@
$fixtureWindow = [AccountUsageFixtureWindow]::Find($fixture.Id)
if ($fixtureWindow -eq [IntPtr]::Zero) { throw 'Fixture has no client window' }
[AccountUsageFixtureWindow]::RenderOffscreen($fixtureWindow)
Start-Sleep -Milliseconds 700
& "$PSScriptRoot/test-windows-capture-client.ps1" -WindowHandle $fixtureWindow.ToInt64() -ExpectedProcessId $FixtureProcessId -OutputFile $OutputFile -MetadataFile $MetadataFile
# A successful PrintWindow call can still return a blank GPU surface.
$image = [Drawing.Bitmap]::new($OutputFile)
try {
    $darkest = 255
    $brightest = 0
    for ($y = 0; $y -lt $image.Height; $y += 4) {
        for ($x = 0; $x -lt $image.Width; $x += 4) {
            $color = $image.GetPixel($x, $y)
            $darkest = [math]::Min($darkest, $color.R)
            $brightest = [math]::Max($brightest, $color.R)
        }
    }
    if ($brightest - $darkest -lt 20) { throw 'Capture is blank; no rendered content' }
} finally { $image.Dispose() }

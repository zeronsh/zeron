param(
    [Parameter(Mandatory = $true)]
    [string]$PackagePath
)

$ErrorActionPreference = 'Stop'
$package = (Resolve-Path -LiteralPath $PackagePath).Path
$installDir = Join-Path $env:LOCALAPPDATA 'Programs\Glitch Flow'
$exe = Join-Path $installDir 'glitch-flow.exe'
$launcher = Join-Path $installDir 'launch-glitch-flow.vbs'
$desktopShortcut = Join-Path ([Environment]::GetFolderPath('DesktopDirectory')) 'Glitch Flow.lnk'
$menuShortcut = Join-Path ([Environment]::GetFolderPath('Programs')) 'Glitch Flow.lnk'
$schemeKey = 'HKCU:\Software\Classes\glitch-flow'

foreach ($path in @($installDir, $desktopShortcut, $menuShortcut, $schemeKey)) {
    if (Test-Path -LiteralPath $path) {
        throw "Install target already exists: $path"
    }
}

Expand-Archive -LiteralPath $package -DestinationPath $installDir
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
    throw "Package has no glitch-flow.exe: $package"
}

@'
Option Explicit
Dim shell, env, fs, codexRoot, folder, subfolder, candidate, newest, exe, dataDir, args, i
Set shell = CreateObject("WScript.Shell")
Set env = shell.Environment("PROCESS")
Set fs = CreateObject("Scripting.FileSystemObject")
exe = shell.ExpandEnvironmentStrings("%LOCALAPPDATA%") & "\Programs\Glitch Flow\glitch-flow.exe"
dataDir = shell.ExpandEnvironmentStrings("%LOCALAPPDATA%") & "\Glitch Flow"
codexRoot = shell.ExpandEnvironmentStrings("%LOCALAPPDATA%") & "\OpenAI\Codex\bin"
If fs.FolderExists(codexRoot) Then
    Set folder = fs.GetFolder(codexRoot)
    For Each subfolder In folder.SubFolders
        If fs.FileExists(subfolder.Path & "\codex.exe") Then
            Set candidate = fs.GetFile(subfolder.Path & "\codex.exe")
            If IsEmpty(newest) Then
                Set newest = candidate
            ElseIf candidate.DateLastModified > newest.DateLastModified Then
                Set newest = candidate
            End If
        End If
    Next
    If Not IsEmpty(newest) Then env("CODEX_EXECUTABLE") = newest.Path
End If
env("GLITCH_FLOW_DATA_DIR") = dataDir
env("ZERON_DATA_DIR") = dataDir
env("GLITCH_FLOW_IPC_PORT") = "27654"
env("ZERON_IPC_PORT") = "27654"
args = ""
For i = 0 To WScript.Arguments.Count - 1
    args = args & " " & Chr(34) & Replace(WScript.Arguments(i), Chr(34), "") & Chr(34)
Next
shell.Run Chr(34) & exe & Chr(34) & args, 1, False
'@ | Set-Content -LiteralPath $launcher -Encoding Unicode

$shell = New-Object -ComObject WScript.Shell
foreach ($shortcutPath in @($desktopShortcut, $menuShortcut)) {
    $shortcut = $shell.CreateShortcut($shortcutPath)
    $shortcut.TargetPath = Join-Path $env:WINDIR 'System32\wscript.exe'
    $shortcut.Arguments = '"{0}"' -f $launcher
    $shortcut.WorkingDirectory = $installDir
    $shortcut.IconLocation = "$exe,0"
    $shortcut.Description = 'Glitch Flow'
    $shortcut.Save()
}

New-Item -Path $schemeKey -Force | Out-Null
Set-Item -Path $schemeKey -Value 'URL:Glitch Flow Protocol'
New-ItemProperty -Path $schemeKey -Name 'URL Protocol' -Value '' -PropertyType String | Out-Null
$iconKey = Join-Path $schemeKey 'DefaultIcon'
New-Item -Path $iconKey -Force | Out-Null
Set-Item -Path $iconKey -Value "$exe,0"
$commandKey = Join-Path $schemeKey 'shell\open\command'
New-Item -Path $commandKey -Force | Out-Null
Set-Item -Path $commandKey -Value ('"{0}" "{1}" "%1"' -f (Join-Path $env:WINDIR 'System32\wscript.exe'), $launcher)

Write-Host "Installed Glitch Flow at $installDir"

# Zeron Windows installer.
#
#   irm https://zeron.sh/install.ps1 | iex
#
# Downloads the latest Windows release into %LOCALAPPDATA%\Programs\Zeron,
# names it zeron.exe, and writes zeron-update.json beside it. That pair is
# what `zeron update` replaces in place. The directory is added to the user
# PATH. No administrator account and no background service.
#
# Overrides (environment variables):
#   ZERON_BASE_URL       release mirror (default https://zeron.sh)
#   ZERON_VERSION        install this version instead of the current manifest
#   ZERON_INSTALL_DIR    directory that will contain zeron.exe (must stay writable)
#   ZERON_RELEASES_URL   update feed written into zeron-update.json
#
# The default feed matches the portable zip: GitHub's latest-release download
# URL, which serves manifest.json and zeron-<version>-windows-<arch>.exe.
# Set ZERON_BASE_URL to point both the download and the feed at another mirror.

# Invoked with `&` so `irm | iex` cannot leak variables into the caller, and so
# a failure throws instead of `exit` (which would close the user's terminal).
& {
    $ErrorActionPreference = 'Stop'
    if (Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue) {
        $PSNativeCommandUseErrorActionPreference = $false
    }

    function Write-Install([string]$Message) {
        Write-Host $Message
    }

    function Fail([string]$Message) {
        throw "zeron install: $Message"
    }

    function Get-ZeronArch {
        # 32-bit PowerShell on 64-bit Windows reports x86 in PROCESSOR_ARCHITECTURE.
        # The published binary matches the OS, not this host process.
        $arch = $env:PROCESSOR_ARCHITECTURE
        if ($env:PROCESSOR_ARCHITEW6432) { $arch = $env:PROCESSOR_ARCHITEW6432 }
        switch ($arch) {
            'AMD64' { return 'x86_64' }
            'ARM64' { return 'aarch64' }
            default { Fail "unsupported architecture '$arch'." }
        }
    }

    function Assert-HttpsBase([string]$Url, [string]$Label) {
        $parsed = $null
        if (-not [Uri]::TryCreate($Url, [UriKind]::Absolute, [ref]$parsed)) {
            Fail "$Label must be an https URL."
        }
        if ($parsed.Scheme -ne 'https' -or [string]::IsNullOrEmpty($parsed.Host)) {
            Fail "$Label must use https."
        }
        if ($parsed.UserInfo -or $parsed.Query -or $parsed.Fragment) {
            Fail "$Label must be a base URL without credentials, a query, or a fragment."
        }
        if ($Url -match '["\\]') {
            Fail "$Label contains characters that cannot be stored in zeron-update.json."
        }
    }

    function Invoke-Download([string]$Url, [string]$Destination, [switch]$Silent) {
        $curl = Get-Command -Name curl.exe -CommandType Application -ErrorAction SilentlyContinue
        if ($curl) {
            $curlArgs = @('-fL', '--retry', '3', '--retry-delay', '2')
            if ($Silent) { $curlArgs += '-fsS' } else { $curlArgs += '--progress-bar' }
            $curlArgs += @('-o', $Destination, $Url)
            & $curl.Source @curlArgs
            if ($LASTEXITCODE -ne 0) {
                Fail "download failed (${LASTEXITCODE}): $Url"
            }
            return
        }
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $Url -OutFile $Destination -UseBasicParsing
    }

    function Get-RemoteJson([string]$Url) {
        $file = Join-Path $TempDir ([IO.Path]::GetRandomFileName())
        Invoke-Download $Url $file -Silent
        return (Get-Content -LiteralPath $file -Raw | ConvertFrom-Json)
    }

    function Get-Checksum($Manifest, [string]$File) {
        if (-not $Manifest.files) { return $null }
        $property = $Manifest.files.PSObject.Properties[$File]
        if (-not $property) { return $null }
        $sha = [string]$property.Value.sha256
        if ($sha -notmatch '^[0-9a-fA-F]{64}$') {
            Fail "invalid checksum for $File."
        }
        return $sha.ToLowerInvariant()
    }

    function Get-ZeronVersion([string]$Path) {
        $probe = [Diagnostics.ProcessStartInfo]::new()
        $probe.FileName = $Path
        $probe.Arguments = '--version'
        $probe.UseShellExecute = $false
        $probe.CreateNoWindow = $true
        $probe.RedirectStandardOutput = $true
        $probe.RedirectStandardError = $true
        $process = [Diagnostics.Process]::Start($probe)
        try {
            $stdout = $process.StandardOutput.ReadToEndAsync()
            $stderr = $process.StandardError.ReadToEndAsync()
            if (-not $process.WaitForExit(15000)) {
                try { $process.Kill() } catch {}
                Fail 'version probe timed out.'
            }
            $match = [regex]::Match($stdout.Result.Trim(), '\Azeron (\d+\.\d+\.\d+)\z')
            if ($process.ExitCode -ne 0 -or -not $match.Success) {
                Fail "cannot read version of ${Path}: $($stderr.Result.Trim())"
            }
            return $match.Groups[1].Value
        } finally {
            $process.Dispose()
        }
    }

    function Test-PathEntry([string]$Existing, [string]$Dir) {
        if (-not $Existing) { return $false }
        $needle = $Dir.TrimEnd('\')
        foreach ($part in ($Existing -split ';')) {
            if ($part -and ($part.TrimEnd('\') -ieq $needle)) { return $true }
        }
        return $false
    }

    function Add-UserPath([string]$Dir) {
        $user = [EnvironmentVariableTarget]::User
        $machine = [EnvironmentVariableTarget]::Machine
        $userPath = [Environment]::GetEnvironmentVariable('Path', $user)
        $machinePath = [Environment]::GetEnvironmentVariable('Path', $machine)
        $changed = $false
        if (-not (Test-PathEntry $userPath $Dir) -and -not (Test-PathEntry $machinePath $Dir)) {
            if ($userPath) {
                $updated = $userPath.TrimEnd(';') + ';' + $Dir
            } else {
                $updated = $Dir
            }
            [Environment]::SetEnvironmentVariable('Path', $updated, $user)
            $changed = $true
        }
        if (-not (Test-PathEntry $env:Path $Dir)) {
            $env:Path = $env:Path.TrimEnd(';') + ';' + $Dir
        }
        return $changed
    }

    function Install-Executable([string]$Staged, [string]$InstallDir) {
        $installed = Join-Path $InstallDir 'zeron.exe'
        $backup = Join-Path $InstallDir 'zeron.exe.old'
        $incoming = Join-Path $InstallDir '.zeron-update-incoming.exe'
        Copy-Item -LiteralPath $Staged -Destination $incoming -Force
        Unblock-File -LiteralPath $incoming
        if (Test-Path -LiteralPath $backup) {
            Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
            if (Test-Path -LiteralPath $backup) {
                Fail 'quit Zeron and run the installer again (the previous executable is still in use).'
            }
        }
        if (Test-Path -LiteralPath $installed) {
            try {
                Rename-Item -LiteralPath $installed -NewName 'zeron.exe.old'
            } catch {
                Remove-Item -LiteralPath $incoming -Force -ErrorAction SilentlyContinue
                Fail 'could not replace zeron.exe. Quit Zeron and run the installer again.'
            }
        }
        try {
            Rename-Item -LiteralPath $incoming -NewName 'zeron.exe'
        } catch {
            if ((Test-Path -LiteralPath $backup) -and -not (Test-Path -LiteralPath $installed)) {
                Rename-Item -LiteralPath $backup -NewName 'zeron.exe'
            }
            Fail 'could not move the new executable into place. The previous zeron.exe was restored.'
        }
        Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
    }

    if ($env:OS -ne 'Windows_NT') {
        Fail "install.ps1 is for Windows. On Linux: curl -fsSL https://zeron.sh/install.sh | sh"
    }

    $base = if ($env:ZERON_BASE_URL) { $env:ZERON_BASE_URL.Trim().TrimEnd('/') } else { 'https://zeron.sh' }
    Assert-HttpsBase $base 'ZERON_BASE_URL'

    if ($env:ZERON_RELEASES_URL) {
        $releasesUrl = $env:ZERON_RELEASES_URL.Trim().TrimEnd('/')
    } elseif ($env:ZERON_BASE_URL) {
        $releasesUrl = "$base/releases"
    } else {
        $releasesUrl = 'https://github.com/zeronsh/zeron/releases/latest/download'
    }
    Assert-HttpsBase $releasesUrl 'ZERON_RELEASES_URL'

    $installDir = if ($env:ZERON_INSTALL_DIR) {
        $env:ZERON_INSTALL_DIR
    } else {
        Join-Path $env:LOCALAPPDATA 'Programs\Zeron'
    }
    if ([string]::IsNullOrWhiteSpace($installDir)) { Fail 'ZERON_INSTALL_DIR is empty.' }

    $arch = Get-ZeronArch
    $TempDir = Join-Path ([IO.Path]::GetTempPath()) ('zeron-install-' + [guid]::NewGuid().ToString('n'))
    New-Item -ItemType Directory -Path $TempDir | Out-Null
    try {
        $manifest = Get-RemoteJson "$base/releases/manifest.json"
        $version = [string]$manifest.version
        if ($version -notmatch '^\d+\.\d+\.\d+$') { Fail 'manifest.json has no usable version.' }
        if ($env:ZERON_VERSION) {
            $requested = $env:ZERON_VERSION.Trim().TrimStart('v')
            if ($requested -notmatch '^\d+\.\d+\.\d+$') { Fail "ZERON_VERSION '$requested' is not a dotted version." }
            if ($requested -ne $version) {
                $version = $requested
                $manifest = Get-RemoteJson "https://github.com/zeronsh/zeron/releases/download/v$version/manifest.json"
                if ([string]$manifest.version -ne $version) {
                    Fail "release manifest for $version does not match."
                }
            }
        }

        $file = "zeron-$version-windows-$arch.exe"
        $expected = Get-Checksum $manifest $file
        if (-not $expected) {
            if ($arch -eq 'aarch64') {
                Fail "no published windows-aarch64 build for $version yet. Published releases are x64 only."
            }
            Fail "manifest has no checksum for $file."
        }

        $installed = Join-Path $installDir 'zeron.exe'
        $already = $false
        if (Test-Path -LiteralPath $installed) {
            try {
                $already = (Get-ZeronVersion $installed) -eq $version
            } catch {
                $already = $false
            }
        }

        if (-not $already) {
            $urls = @("$base/releases/$file")
            if ($env:ZERON_VERSION) {
                $urls += "https://github.com/zeronsh/zeron/releases/download/v$version/$file"
            }
            $staged = Join-Path $TempDir 'zeron.exe'
            $downloaded = $false
            $downloadError = $null
            foreach ($url in $urls) {
                try {
                    if (Test-Path -LiteralPath $staged) { Remove-Item -LiteralPath $staged -Force }
                    Write-Install "downloading zeron $version (windows-$arch)..."
                    Invoke-Download $url $staged
                    $downloaded = $true
                    break
                } catch {
                    $downloadError = $_.Exception.Message -replace '^zeron install: ', ''
                }
            }
            if (-not $downloaded) { Fail $downloadError }

            $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $staged).Hash.ToLowerInvariant()
            if ($actual -ne $expected) {
                Fail "checksum mismatch for ${file}: expected $expected, got $actual."
            }
            $stagedVersion = Get-ZeronVersion $staged
            if ($stagedVersion -ne $version) {
                Fail "downloaded executable reports $stagedVersion, expected $version."
            }

            New-Item -ItemType Directory -Force -Path $installDir | Out-Null
            Install-Executable $staged $installDir
        } else {
            Write-Install "zeron $version already installed."
        }

        $configPath = Join-Path $installDir 'zeron-update.json'
        $config = '{"releases_url":"' + $releasesUrl + '"}' + "`n"
        $utf8 = New-Object System.Text.UTF8Encoding $false
        [IO.File]::WriteAllText($configPath, $config, $utf8)

        $pathAdded = Add-UserPath $installDir
        Write-Install ""
        Write-Install "zeron $version installed to $installed"
        if ($pathAdded) {
            Write-Install "PATH updated. Open a new terminal so ``zeron`` is found."
        }
        Write-Install ""
        Write-Install "  zeron            open the app"
        Write-Install "  zeron status     local/synced mode and engine status"
        Write-Install "  zeron update     update to the latest release"
    } finally {
        Remove-Item -LiteralPath $TempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

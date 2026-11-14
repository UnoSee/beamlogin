# Beamlogin installer for Windows.
#
#   irm https://raw.githubusercontent.com/UnoSee/beamlogin/main/install.ps1 | iex
#
# Downloads the Windows binary from GitHub Releases, checks it against SHA256SUMS, installs it
# as beamlogin.exe and adds its folder to your user PATH. No administrator rights needed.
#
# Settings (environment variables, set before running):
#   BEAMLOGIN_VERSION         release tag to install, such as v0.4.0-beta.1 (default: newest)
#   BEAMLOGIN_INSTALL_DIR     folder to install into
#                             (default: %LOCALAPPDATA%\Programs\beamlogin)
#   BEAMLOGIN_NO_MODIFY_PATH  set to 1 to leave your PATH alone
#
# Works in Windows PowerShell 5.1 and PowerShell 7.

# Everything is inside a script block, so a download cut off halfway runs nothing.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'   # the progress bar makes downloads very slow in 5.1
    $Repo = 'UnoSee/beamlogin'

    function Say([string]$Text = '') { Write-Host $Text }
    function Step([string]$Text) { Write-Host '  ' -NoNewline; Write-Host 'OK' -ForegroundColor Green -NoNewline; Write-Host " $Text" }
    function Warn([string]$Text) { Write-Host '  ! ' -ForegroundColor Yellow -NoNewline; Write-Host $Text }
    function Fail([string[]]$Lines) {
        Write-Host ''
        Write-Host "  x $($Lines[0])" -ForegroundColor Red
        $Lines | Select-Object -Skip 1 | ForEach-Object { Write-Host "    $_" }
        Write-Host ''
        throw 'Beamlogin was not installed.'
    }

    # Older Windows PowerShell may default to TLS 1.0, which GitHub refuses.
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    } catch { }

    # --- what system is this? ---------------------------------------------------------------
    if ([Environment]::OSVersion.Platform -ne 'Win32NT') {
        Fail @('This installer is for Windows. On Linux or macOS, run instead:',
               "  curl -fsSL https://raw.githubusercontent.com/$Repo/main/install.sh | sh")
    }
    if (-not [Environment]::Is64BitOperatingSystem) {
        Fail @('Beamlogin needs 64-bit Windows.')
    }
    $build = [Environment]::OSVersion.Version.Build
    if ($build -lt 17763) {
        Fail @("Beamlogin needs Windows 10 version 1809 or later (this is build $build).")
    }
    $cpu = $env:PROCESSOR_ARCHITEW6432
    if (-not $cpu) { $cpu = $env:PROCESSOR_ARCHITECTURE }
    $arch = 'x86_64'
    $note = ''
    if ($cpu -eq 'ARM64') { $note = ' (runs through x64 emulation)' }
    $asset = "beamlogin-$arch-windows.exe"

    # --- which release? ---------------------------------------------------------------------
    $version = $env:BEAMLOGIN_VERSION
    if (-not $version) {
        # The newest release, including pre-releases (GitHub's "latest" skips those).
        try {
            $releases = Invoke-RestMethod -UseBasicParsing "https://api.github.com/repos/$Repo/releases?per_page=1"
            $version = @($releases)[0].tag_name
        } catch { }
        if (-not $version) {
            Fail @("Couldn't find the newest release on GitHub.",
                   'Check the internet connection, or set the version first, for example:',
                   "  `$env:BEAMLOGIN_VERSION = 'v0.4.0-beta.1'")
        }
    }
    $base = "https://github.com/$Repo/releases/download/$version"

    # --- where to? --------------------------------------------------------------------------
    $dir = $env:BEAMLOGIN_INSTALL_DIR
    if (-not $dir) { $dir = Join-Path $env:LOCALAPPDATA 'Programs\beamlogin' }

    Say ''
    Say '  Beamlogin installer'
    Say "  System   Windows $cpu$note"
    Say "  Release  $version"
    Say "  Into     $dir"
    Say ''

    # --- download and check -----------------------------------------------------------------
    $tmp = Join-Path ([IO.Path]::GetTempPath()) ("beamlogin-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        $file = Join-Path $tmp $asset
        Step "Downloading $asset"
        try {
            Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $file
        } catch {
            Fail @("Download failed: $base/$asset", $_.Exception.Message)
        }

        $sums = $null
        try { $sums = (Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS").Content } catch { }
        if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }
        if (-not $sums) {
            Warn "This release has no SHA256SUMS file, so the download wasn't checked."
        } else {
            $line = ($sums -split "`n") | Where-Object { $_.Trim() -match "\s$([regex]::Escape($asset))$" } | Select-Object -First 1
            if (-not $line) {
                Warn "SHA256SUMS has no entry for $asset, so the download wasn't checked."
            } else {
                $want = ($line.Trim() -split '\s+')[0].ToLower()
                $got = (Get-FileHash -Algorithm SHA256 -Path $file).Hash.ToLower()
                if ($want -ne $got) {
                    Fail @("The download doesn't match its checksum, so it wasn't installed.",
                           'Try again. If it keeps happening, please report it.')
                }
                Step 'Checksum verified'
            }
        }

        # --- install ------------------------------------------------------------------------
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        $target = Join-Path $dir 'beamlogin.exe'
        try {
            Copy-Item -Force -Path $file -Destination $target
        } catch {
            Fail @("Couldn't write $target.", 'If Beamlogin is running, close it and try again.')
        }
    } finally {
        Remove-Item -Recurse -Force -Path $tmp -ErrorAction SilentlyContinue
    }
    $installed = (& $target --version 2>$null | Out-String).Trim()
    if (-not $installed) { $installed = 'beamlogin' }
    Step "Installed $installed to $target"

    # --- PATH -------------------------------------------------------------------------------
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $parts = @(($userPath -split ';') | Where-Object { $_ }) + @(($env:Path -split ';') | Where-Object { $_ })
    $onPath = $parts | Where-Object { $_.TrimEnd('\') -ieq $dir.TrimEnd('\') }
    Say ''
    if ($onPath) {
        Say '  Done. Run  beamlogin  to sign in to Claude Code with your phone.'
    } elseif ($env:BEAMLOGIN_NO_MODIFY_PATH -eq '1') {
        Say "  Done. $dir isn't on your PATH. Run it as:"
        Say "    & '$target'"
    } else {
        $newPath = if ($userPath) { "$dir;$userPath" } else { $dir }
        [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
        $env:Path = "$dir;$env:Path"
        Step 'Added the install folder to your user PATH'
        Say ''
        Say '  Done. Run  beamlogin  to sign in to Claude Code with your phone.'
        Say '  Other windows that are already open need restarting to see it.'
    }
    Say ''
}

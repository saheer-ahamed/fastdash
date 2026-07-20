<#
.SYNOPSIS
    Installs fastdash for the current user.

.DESCRIPTION
    Downloads the latest portable build from GitHub Releases, verifies its
    SHA256 against the published SHA256SUMS.txt, and extracts it to
    %LOCALAPPDATA%\fastdash with a Start Menu shortcut.

    No administrator rights are required and nothing is written outside your
    user profile.

    Note on SmartScreen: files fetched with Invoke-WebRequest do not receive the
    Mark-of-the-Web that browsers apply, so this install path does not raise the
    "Windows protected your PC" prompt. That is by design, not a bypass - the
    download is still verified by checksum below.

.EXAMPLE
    irm https://saheer-ahamed.github.io/fastdash/install.ps1 | iex

.EXAMPLE
    # Pin a specific version, or skip the post-install launch:
    & ([scriptblock]::Create((irm https://saheer-ahamed.github.io/fastdash/install.ps1))) -Version v0.1.0 -NoLaunch
#>
[CmdletBinding()]
param(
    # Release tag to install, e.g. "v0.1.0". Defaults to the newest release.
    [string] $Version = 'latest',

    # Where to install. Defaults to %LOCALAPPDATA%\fastdash.
    [string] $InstallDir = (Join-Path $env:LOCALAPPDATA 'fastdash'),

    # Do not start fastdash after installing.
    [switch] $NoLaunch
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$Repo = 'saheer-ahamed/fastdash'

function Write-Step { param([string] $Message) Write-Host "==> $Message" -ForegroundColor Cyan }
function Write-Ok   { param([string] $Message) Write-Host "    $Message" -ForegroundColor Green }

try {
    # PowerShell 5.1 can still default to TLS 1.0, which GitHub rejects.
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

    if ($PSVersionTable.PSVersion.Major -lt 5) {
        throw "PowerShell 5.0 or newer is required (found $($PSVersionTable.PSVersion))."
    }
    if ([Environment]::Is64BitOperatingSystem -ne $true) {
        throw 'fastdash ships 64-bit builds only; this looks like a 32-bit Windows.'
    }

    Write-Step 'Resolving release'
    $apiUrl = if ($Version -eq 'latest') {
        "https://api.github.com/repos/$Repo/releases/latest"
    } else {
        "https://api.github.com/repos/$Repo/releases/tags/$Version"
    }
    $headers = @{ 'User-Agent' = 'fastdash-installer'; 'Accept' = 'application/vnd.github+json' }
    $release = Invoke-RestMethod -Uri $apiUrl -Headers $headers

    $tag = $release.tag_name
    $zipAsset = $release.assets | Where-Object { $_.name -like '*_x64_portable.zip' } | Select-Object -First 1
    if (-not $zipAsset) {
        throw "Release $tag has no portable zip asset. Try the installer from https://saheer-ahamed.github.io/fastdash/"
    }
    $sumsAsset = $release.assets | Where-Object { $_.name -eq 'SHA256SUMS.txt' } | Select-Object -First 1
    Write-Ok "fastdash $tag"

    $work = Join-Path ([IO.Path]::GetTempPath()) ("fastdash-" + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work -Force | Out-Null

    try {
        Write-Step "Downloading $($zipAsset.name)"
        $zipPath = Join-Path $work $zipAsset.name
        Invoke-WebRequest -Uri $zipAsset.browser_download_url -OutFile $zipPath -Headers $headers -UseBasicParsing
        Write-Ok ('{0:N1} MB' -f ((Get-Item $zipPath).Length / 1MB))

        # Verify against the published checksums. A mismatch means a corrupted or
        # tampered download - refuse to install rather than "probably fine".
        if ($sumsAsset) {
            Write-Step 'Verifying checksum'
            $sumsPath = Join-Path $work 'SHA256SUMS.txt'
            Invoke-WebRequest -Uri $sumsAsset.browser_download_url -OutFile $sumsPath -Headers $headers -UseBasicParsing

            $expected = $null
            foreach ($line in Get-Content $sumsPath) {
                $parts = $line -split '\s+', 2
                if ($parts.Count -eq 2 -and $parts[1].Trim() -eq $zipAsset.name) { $expected = $parts[0].Trim().ToLower() }
            }
            if (-not $expected) { throw "SHA256SUMS.txt has no entry for $($zipAsset.name)." }

            $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash.ToLower()
            if ($actual -ne $expected) {
                throw "Checksum mismatch for $($zipAsset.name).`n  expected $expected`n  actual   $actual`nAborting - do not run this file."
            }
            Write-Ok "sha256 $actual"
        } else {
            Write-Warning 'Release published no SHA256SUMS.txt; skipping checksum verification.'
        }

        # A running instance holds a lock on fastdash.exe and would fail the copy.
        $running = Get-Process -Name 'fastdash' -ErrorAction SilentlyContinue
        if ($running) {
            Write-Step 'Closing running fastdash'
            $running | Stop-Process -Force
            Start-Sleep -Milliseconds 800
        }

        Write-Step "Installing to $InstallDir"
        $extract = Join-Path $work 'extract'
        Expand-Archive -Path $zipPath -DestinationPath $extract -Force
        if (-not (Test-Path (Join-Path $extract 'fastdash.exe'))) {
            throw 'Archive did not contain fastdash.exe.'
        }
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
        Copy-Item -Path (Join-Path $extract '*') -Destination $InstallDir -Recurse -Force

        Write-Step 'Creating Start Menu shortcut'
        $startMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
        $shortcut = Join-Path $startMenu 'fastdash.lnk'
        $shell = New-Object -ComObject WScript.Shell
        $lnk = $shell.CreateShortcut($shortcut)
        $lnk.TargetPath = Join-Path $InstallDir 'fastdash.exe'
        $lnk.WorkingDirectory = $InstallDir
        $lnk.Description = 'A super-fast desktop dashboard for Claude usage'
        $lnk.Save()

        # Record enough for a clean uninstall and for "am I up to date" checks.
        @{ version = $tag; installedAt = (Get-Date).ToString('o'); path = $InstallDir } |
            ConvertTo-Json | Set-Content -Path (Join-Path $InstallDir '.install.json') -Encoding utf8

        Write-Host ''
        Write-Host "fastdash $tag installed." -ForegroundColor Green
        Write-Host "  $InstallDir\fastdash.exe"
        Write-Host '  Find it in the Start Menu, or re-run this command to update.'
        Write-Host ''
        Write-Host '  To uninstall:' -ForegroundColor DarkGray
        Write-Host "    Remove-Item -Recurse -Force '$InstallDir'; Remove-Item '$shortcut'" -ForegroundColor DarkGray
        Write-Host ''

        if (-not $NoLaunch) { Start-Process (Join-Path $InstallDir 'fastdash.exe') }
    }
    finally {
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }
}
catch {
    Write-Host ''
    Write-Host "Install failed: $($_.Exception.Message)" -ForegroundColor Red
    Write-Host 'Report issues at https://github.com/saheer-ahamed/fastdash/issues' -ForegroundColor DarkGray
    exit 1
}

# Install the latest acts-server and acts-cli release binaries on Windows.
#
# Usage (PowerShell):
#   iwr -useb https://raw.githubusercontent.com/yaojianpin/acts/main/install.ps1 | iex
#   $env:ACTS_VERSION = "v0.24.0"; iwr -useb https://raw.githubusercontent.com/yaojianpin/acts/main/install.ps1 | iex
#
# The binaries land in ~\.acts\bin (override with ACTS_INSTALL_DIR).

param(
    [string]$Repo = "yaojianpin/acts",
    [string]$Version = "latest",
    [string]$InstallDir = ""
)

$ErrorActionPreference = "Stop"

if (-not $InstallDir) {
    $InstallDir = Join-Path $HOME ".acts\bin"
}

$tag = $Version
if ($tag -eq "latest") {
    Write-Host "resolving the latest release of $Repo"
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
    $tag = $release.tag_name
}
$version = $tag.TrimStart("v")

$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    "AMD64" { "x86_64" }
    "ARM64" { "aarch64" }
    default { throw "unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}

$asset = "acts-$version-windows-$arch.zip"
$url = "https://github.com/$Repo/releases/download/$tag/$asset"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "acts-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    Write-Host "downloading $url"
    Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $asset)
    Expand-Archive -Path (Join-Path $tmp $asset) -DestinationPath $tmp -Force

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item (Join-Path $tmp "acts-server.exe") $InstallDir -Force
    Copy-Item (Join-Path $tmp "acts-cli.exe") $InstallDir -Force

    Write-Host "installed acts-server $version and acts-cli $version to $InstallDir" -ForegroundColor Green
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    if ($current -notlike "*$InstallDir*") {
        Write-Host "add $InstallDir to your PATH, e.g.: setx PATH \"$InstallDir;%PATH%\""
    }
}
finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

<#
.SYNOPSIS
  Build the Windows x64 release zip in one step.

.EXAMPLE
  pwsh packaging/windows/build-release-local.ps1

  Writes dist/release/fieldcraft-<version>-windows-x64.zip.
  Needs rustup and the MSVC linker (Visual Studio Build Tools, "Desktop development with C++").
  The zip is unsigned.
#>
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') {
  throw 'this packages the Windows build and must run on Windows'
}

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
Set-Location $Root

foreach ($cmd in 'cargo', 'rustc') {
  if (-not (Get-Command $cmd -ErrorAction SilentlyContinue)) {
    throw "$cmd is not on PATH (install rustup)"
  }
}
if (Get-Command rustup -ErrorAction SilentlyContinue) {
  rustup target add x86_64-pc-windows-msvc
}

Write-Output '==> third-party licences'
cargo xtask licences
if ($LASTEXITCODE -ne 0) { throw "cargo xtask licences failed with exit code $LASTEXITCODE" }

Write-Output '==> package'
& (Join-Path $PSScriptRoot 'package.ps1')
if ($LASTEXITCODE -ne 0) { throw "package.ps1 failed with exit code $LASTEXITCODE" }

$ErrorActionPreference = "Stop"

if (-not $env:RUNNER_TEMP -or -not $env:GITHUB_PATH) {
    throw "This setup script is intended for a GitHub-hosted Windows runner."
}

$scoopRoot = Join-Path $env:RUNNER_TEMP "scoop"
$env:SCOOP = $scoopRoot

if (-not (Get-Command scoop -ErrorAction SilentlyContinue)) {
    Invoke-Expression "& {$(Invoke-RestMethod https://get.scoop.sh)} -RunAsAdmin -ScoopDir '$scoopRoot'"
}

$scoopShims = Join-Path $scoopRoot "shims"
$env:Path = "$scoopShims;$env:Path"
Add-Content -LiteralPath $env:GITHUB_PATH -Value $scoopShims

# Bun's Windows guide pins LLVM exactly. Install it separately because Scoop's
# LLVM manifest and the other tool manifests should not share one transaction.
scoop install llvm@21.1.8
if ($LASTEXITCODE -ne 0) {
    throw "Could not install LLVM 21.1.8."
}
$llvmBin = Join-Path $scoopRoot "apps\llvm\current\bin"
$env:Path = "$llvmBin;$env:Path"
Add-Content -LiteralPath $env:GITHUB_PATH -Value $llvmBin

foreach ($package in @("nasm", "perl", "ruby")) {
    scoop install $package
    if ($LASTEXITCODE -ne 0) {
        throw "Could not install $package."
    }
}

$requiredCommands = @(
    "bun",
    "clang-cl",
    "cmake",
    "go",
    "nasm",
    "ninja",
    "node",
    "perl",
    "ruby",
    "rustup"
)
foreach ($command in $requiredCommands) {
    if (-not (Get-Command $command -ErrorAction SilentlyContinue)) {
        throw "Required Bun build command is unavailable: $command"
    }
}

$clangVersion = (& clang-cl --version | Select-Object -First 1)
if ($clangVersion -notmatch "21\.1\.8") {
    throw "Expected clang-cl 21.1.8, found: $clangVersion"
}

Write-Host $clangVersion
Write-Host (& nasm -v)
Write-Host (& go version)
Write-Host (& ruby --version)

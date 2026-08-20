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

# Install Scoop's current LLVM separately from the other tool manifests.
scoop install llvm
if ($LASTEXITCODE -ne 0) {
    throw "Could not install LLVM."
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

Write-Host (& clang-cl --version | Select-Object -First 1)
Write-Host (& nasm -v)
Write-Host (& go version)
Write-Host (& ruby --version)

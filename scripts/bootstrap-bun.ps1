param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug"
)

$ErrorActionPreference = "Stop"
$projectRoot = Split-Path -Parent $PSScriptRoot
$sourceRoot = Join-Path $projectRoot ".poly\bun-src"
$pythonSource = Join-Path $projectRoot "crates\poly-python"
$pythonTarget = Join-Path $sourceRoot "src\poly_python"
$patchPath = Join-Path $projectRoot "patches\bun-in-process.patch"
$distRoot = Join-Path $projectRoot "dist"
$bunCommit = "e7ddfeb19e8bc714f6137aa2b1cd5a7bb56b93d7"

if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
    throw "Git is required."
}
if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
    throw "A released Bun binary is required to run Bun's build scripts."
}
if ($IsWindows -and $PSVersionTable.PSVersion.Major -lt 7) {
    throw "Bun's Windows build requires PowerShell 7 (pwsh.exe)."
}

if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot ".git"))) {
    New-Item -ItemType Directory -Force -Path $sourceRoot | Out-Null
    git -C $sourceRoot init
    git -C $sourceRoot config core.longpaths true
    git -C $sourceRoot config core.autocrlf false
    git -C $sourceRoot config core.eol lf
    git -C $sourceRoot remote add origin "https://github.com/oven-sh/bun.git"
    git -C $sourceRoot fetch --depth 1 origin $bunCommit
    git -C $sourceRoot checkout --detach FETCH_HEAD
}
git -C $sourceRoot config core.longpaths true
git -C $sourceRoot config core.autocrlf false
git -C $sourceRoot config core.eol lf

$actualCommit = (git -C $sourceRoot rev-parse HEAD).Trim()
if ($actualCommit -ne $bunCommit) {
    throw "Bun checkout is $actualCommit, expected $bunCommit."
}

$resolvedSource = [System.IO.Path]::GetFullPath($sourceRoot)
$resolvedTarget = [System.IO.Path]::GetFullPath($pythonTarget)
if (-not $resolvedTarget.StartsWith($resolvedSource, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace a Python target outside the pinned Bun checkout."
}
if (Test-Path -LiteralPath $pythonTarget) {
    Remove-Item -LiteralPath $pythonTarget -Recurse -Force
}
Copy-Item -LiteralPath $pythonSource -Destination $pythonTarget -Recurse

git -C $sourceRoot apply --check $patchPath 2>$null
if ($LASTEXITCODE -eq 0) {
    git -C $sourceRoot apply $patchPath
} else {
    git -C $sourceRoot apply --reverse --check $patchPath 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw "The Bun integration patch does not apply cleanly to the pinned commit."
    }
}

Push-Location $sourceRoot
try {
    if ($IsWindows) {
        . (Join-Path $sourceRoot "scripts\vs-shell.ps1")
    }

    $profile = $Configuration.ToLowerInvariant()
    bun install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) {
        throw "Bun dependency installation failed with exit code $LASTEXITCODE."
    }
    bun scripts/build.ts "--profile=$profile" --configure-only
    if ($LASTEXITCODE -ne 0) {
        throw "Bun $Configuration configuration failed with exit code $LASTEXITCODE."
    }

    # A fresh Bun checkout does not necessarily order generated headers after
    # every vendor-source fetch (zlib is one example). Fetch all source-backed
    # dependencies before Cargo metadata inspection and the full Ninja build.
    $buildDirectory = Join-Path "build" $profile
    $cloneTargets = @(
        ninja -C $buildDirectory -t targets all |
            ForEach-Object {
                if ($_ -match "^(clone-[^:]+):") {
                    $Matches[1]
                }
            } |
            Sort-Object -Unique
    )
    if ($LASTEXITCODE -ne 0) {
        throw "Could not enumerate Bun dependency source targets."
    }
    if ($cloneTargets.Count -eq 0) {
        throw "Bun configuration did not produce dependency source targets."
    }
    ninja -C $buildDirectory @cloneTargets
    if ($LASTEXITCODE -ne 0) {
        throw "Bun dependency source preparation failed with exit code $LASTEXITCODE."
    }

    # RustPython 0.5.0 expects the 0.9 malachite family. pymath 0.2 uses a
    # deliberately broad "0" requirement, so a large workspace may otherwise
    # select incompatible 0.9 and 0.10 BigInt types at the same time. Cargo can
    # resolve the complete Bun workspace only after source dependencies exist.
    cargo tree -p poly_python -i malachite-bigint@0.10.0 --depth 0 *> $null
    if ($LASTEXITCODE -eq 0) {
        cargo update -p malachite-bigint@0.10.0 --precise 0.9.1
        if ($LASTEXITCODE -ne 0) {
            throw "Could not unify RustPython's malachite dependency."
        }
    }

    if ($Configuration -eq "Release") {
        bun run build:release
    } else {
        bun run build
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Bun $Configuration build failed with exit code $LASTEXITCODE."
    }
} finally {
    Pop-Location
}

$binaryCandidates = if ($Configuration -eq "Release") {
    @(
        (Join-Path $sourceRoot "build\release\bun.exe"),
        (Join-Path $sourceRoot "build\release\bun")
    )
} else {
    @(
        (Join-Path $sourceRoot "build\debug\bun-debug.exe"),
        (Join-Path $sourceRoot "build\debug\bun-debug")
    )
}
$binary = $binaryCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $binary) {
    throw "Bun $Configuration build completed but its executable was not found."
}

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
$destination = if ($IsWindows) {
    Join-Path $distRoot "poly.exe"
} else {
    Join-Path $distRoot "poly"
}
Copy-Item -LiteralPath $binary -Destination $destination -Force
Write-Host "Built in-process polyglot runtime ($Configuration): $destination"

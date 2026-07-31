param(
    [ValidateSet("Debug", "Release")]
    [string]$Configuration = "Debug"
)

$ErrorActionPreference = "Stop"
$projectRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot "..\.."))
$distRoot = Join-Path $projectRoot "dist"

if (-not (Get-Command bun -ErrorAction SilentlyContinue)) {
    throw "A released Bun binary is required to run Bun's build scripts."
}
if ($IsWindows -and $PSVersionTable.PSVersion.Major -lt 7) {
    throw "Bun's Windows build requires PowerShell 7 (pwsh.exe)."
}

function Import-VisualStudioEnvironment {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path -LiteralPath $vswhere)) {
        throw "Visual Studio Installer's vswhere.exe was not found."
    }

    $installation = (& $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 `
        -property installationPath | Select-Object -First 1)
    if (-not $installation) {
        throw "A Visual Studio installation with the Desktop C++ toolchain was not found."
    }

    $devCmd = Join-Path $installation "Common7\Tools\VsDevCmd.bat"
    $command = "`"$devCmd`" -no_logo -arch=amd64 -host_arch=amd64 >nul && set"
    $environment = & cmd.exe /d /s /c $command
    if ($LASTEXITCODE -ne 0) {
        throw "Visual Studio's developer environment could not be loaded."
    }
    foreach ($line in $environment) {
        if ($line -match "^([^=]+)=(.*)$") {
            Set-Item -Path "Env:$($Matches[1])" -Value $Matches[2]
        }
    }
}

Push-Location $projectRoot
try {
    if ($IsWindows) {
        Import-VisualStudioEnvironment
    }

    $profile = $Configuration.ToLowerInvariant()
    bun install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) {
        throw "Dependency installation failed with exit code $LASTEXITCODE."
    }

    bun scripts/build.ts "--profile=$profile" --configure-only
    if ($LASTEXITCODE -ne 0) {
        throw "Poly $Configuration configuration failed with exit code $LASTEXITCODE."
    }

    # Fetch source-backed dependencies before Cargo resolves the full workspace.
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
    if ($LASTEXITCODE -ne 0 -or $cloneTargets.Count -eq 0) {
        throw "Poly configuration did not produce dependency source targets."
    }
    ninja -C $buildDirectory @cloneTargets
    if ($LASTEXITCODE -ne 0) {
        throw "Dependency source preparation failed with exit code $LASTEXITCODE."
    }

    if ($Configuration -eq "Release") {
        bun run build:release
    } else {
        bun run build
    }
    if ($LASTEXITCODE -ne 0) {
        throw "Poly $Configuration build failed with exit code $LASTEXITCODE."
    }
} finally {
    Pop-Location
}

$binaryCandidates = if ($Configuration -eq "Release") {
    @(
        (Join-Path $projectRoot "build\release\bun.exe"),
        (Join-Path $projectRoot "build\release\bun")
    )
} else {
    @(
        (Join-Path $projectRoot "build\debug\bun-debug.exe"),
        (Join-Path $projectRoot "build\debug\bun-debug")
    )
}
$binary = $binaryCandidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $binary) {
    throw "Poly $Configuration build completed but its executable was not found."
}

New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
$destination = if ($IsWindows) {
    Join-Path $distRoot "poly.exe"
} else {
    Join-Path $distRoot "poly"
}
Copy-Item -LiteralPath $binary -Destination $destination -Force
Write-Host "Built Poly ($Configuration): $destination"

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
$ManifestPath = Join-Path $RepositoryRoot "Cargo.toml"
$VersionMatch = Select-String -Path $ManifestPath -Pattern '^version = "([^"]+)"' | Select-Object -First 1
if (-not $VersionMatch) {
    throw "Could not read the Textify version from Cargo.toml"
}
$Version = $VersionMatch.Matches[0].Groups[1].Value
$DistDirectory = if ($env:TEXTIFY_DIST_DIR) { $env:TEXTIFY_DIST_DIR } else { Join-Path $RepositoryRoot "dist" }
$TargetDirectory = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $RepositoryRoot "target" }
$Executable = Join-Path $TargetDirectory "release\textify.exe"
$PortableName = "textify-$Version-windows-x64"
$PortableDirectory = Join-Path $DistDirectory $PortableName
$PortableArchive = Join-Path $DistDirectory "$PortableName.zip"

Push-Location $RepositoryRoot
try {
    & cargo build --locked --release --bin textify
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

    New-Item -ItemType Directory -Force -Path $DistDirectory | Out-Null
    if (Test-Path $PortableDirectory) { Remove-Item -Recurse -Force $PortableDirectory }
    if (Test-Path $PortableArchive) { Remove-Item -Force $PortableArchive }
    New-Item -ItemType Directory -Force -Path $PortableDirectory | Out-Null
    Copy-Item $Executable (Join-Path $PortableDirectory "Textify.exe")
    Copy-Item (Join-Path $RepositoryRoot "README.md") $PortableDirectory
    Compress-Archive -Path (Join-Path $PortableDirectory "*") -DestinationPath $PortableArchive -CompressionLevel Optimal

    $Iscc = Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe"
    if (-not (Test-Path $Iscc)) { throw "Inno Setup 6 was not found at $Iscc" }
    $InstallerScript = Join-Path $RepositoryRoot "packaging\windows\Textify.iss"
    & $Iscc "/DTextifyVersion=$Version" "/DSourceExe=$Executable" "/DOutputDir=$DistDirectory" $InstallerScript
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed with exit code $LASTEXITCODE" }

    Write-Host "Created $PortableArchive"
    Write-Host "Created $(Join-Path $DistDirectory "textify-$Version-windows-x64-setup.exe")"
}
finally {
    Pop-Location
}

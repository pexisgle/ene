$ErrorActionPreference = "Stop"

$metadata = cargo metadata --locked --no-deps --format-version 1 | ConvertFrom-Json
$marker = 'cfg\s*\(\s*(windows|target_os\s*=\s*"windows")|windows-sys|win32'
$discovered = @()

foreach ($package in $metadata.packages) {
    $root = Split-Path -Parent $package.manifest_path
    $files = @($package.manifest_path) + @(
        Get-ChildItem -Path $root -Recurse -Filter *.rs -File |
            Select-Object -ExpandProperty FullName
    )
    if (Select-String -Path $files -Pattern $marker -Quiet) {
        $discovered += $package.name
    }
}

$expected = @(
    "ene-desktop",
    "ene-registry",
    "ene-sandbox",
    "ene-stage",
    "ene-tool-app",
    "ene-tool-exec"
)
$missing = @($expected | Where-Object { $_ -notin $discovered })
$unexpected = @($discovered | Where-Object { $_ -notin $expected })

if ($missing.Count -gt 0 -or $unexpected.Count -gt 0) {
    throw "Windows CI inventory drifted. Missing expected packages: $($missing -join ', '); unexpected Windows packages: $($unexpected -join ', '). Update this inventory and the native Windows commands together."
}

Write-Host "Windows-specific package inventory: $($discovered -join ', ')"

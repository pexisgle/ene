<#
.SYNOPSIS
Downloads the VRoid VRMA motion pack and places it where ene looks for the
character's clips.

.DESCRIPTION
The motion pack is an install asset: it is not committed to this repository,
and its terms forbid redistributing the clips in an extractable form. The pack
is published by pixiv as a BOOTH download, so -Url is a direct link to the
archive on a source you are allowed to use.

.PARAMETER Url
Archive to download. May also be given as VRMA_MOTIONPACK_URL.

.PARAMETER Destination
Directory the clips are placed in. Defaults to <repository>/assets/motions, the
development location. Point it at the application data directory for an
installed build (apps/ene-body/README.md lists the full search order).

.PARAMETER Force
Overwrite clips that are already placed. Without it they are kept as they are.

.EXAMPLE
pwsh -File scripts/install-vrma-motionpack.ps1 -Url https://example.invalid/VRMA_MotionPack.zip

.EXAMPLE
$env:VRMA_MOTIONPACK_URL = 'https://example.invalid/VRMA_MotionPack.zip'
pwsh -File scripts/install-vrma-motionpack.ps1
#>
[CmdletBinding()]
param(
    [string]$Url = $env:VRMA_MOTIONPACK_URL,
    [string]$Destination,
    [switch]$Force
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.IO.Compression.FileSystem

# Every clip the archive ships; the first five back the activity hints.
$packClips = @(
    'VRMA_01.vrma',
    'VRMA_02.vrma',
    'VRMA_03.vrma',
    'VRMA_04.vrma',
    'VRMA_05.vrma',
    'VRMA_06.vrma',
    'VRMA_07.vrma'
)
# PoseHint -> clip, matching ene_body::motion::DEFAULT_POSE_CLIPS.
$hintClips = @('VRMA_06.vrma', 'VRMA_02.vrma', 'VRMA_01.vrma', 'VRMA_07.vrma', 'VRMA_03.vrma')

if (-not $Url) {
    throw 'no archive URL: pass -Url or set VRMA_MOTIONPACK_URL'
}
if (-not $Destination) {
    $repository = if ($PSScriptRoot) { Split-Path -Parent $PSScriptRoot } else { (Get-Location).Path }
    $Destination = Join-Path (Join-Path $repository 'assets') 'motions'
}

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) (
    'VRMA_MotionPack-{0}.zip' -f [System.IO.Path]::GetRandomFileName()
)
$clips = @{}
$placed = @()
$kept = @()
try {
    Write-Host "downloading $Url"
    $request = @{ Uri = $Url; OutFile = $temporary }
    if ($PSVersionTable.PSVersion.Major -lt 6) {
        $request['UseBasicParsing'] = $true
    }
    try {
        Invoke-WebRequest @request
    }
    catch {
        throw "download failed: $($_.Exception.Message)"
    }
    if (-not (Test-Path -LiteralPath $temporary -PathType Leaf)) {
        throw "$Url produced no download"
    }
    if ((Get-Item -LiteralPath $temporary).Length -eq 0) {
        throw "$Url produced an empty download"
    }

    try {
        $archive = [System.IO.Compression.ZipFile]::OpenRead($temporary)
    }
    catch [System.IO.InvalidDataException] {
        throw "the download is not a readable zip archive: $($_.Exception.Message)"
    }

    # Read the clips into memory and close the archive before writing anything,
    # so no open handle survives a failure below.
    try {
        foreach ($entry in $archive.Entries) {
            if ($entry.Name -notmatch '^VRMA_(\d{1,2})\.vrma$') {
                continue
            }
            # The archive nests the clips under VRMA_MotionPack/vrma/; only the
            # file name identifies them.
            $name = 'VRMA_{0:D2}.vrma' -f [int]$Matches[1]
            if ($clips.ContainsKey($name)) {
                continue
            }
            if ((Test-Path -LiteralPath (Join-Path $Destination $name)) -and -not $Force) {
                $kept += $name
                continue
            }
            $stream = $entry.Open()
            try {
                $buffer = New-Object System.IO.MemoryStream
                $stream.CopyTo($buffer)
                $clips[$name] = $buffer.ToArray()
            }
            finally {
                $stream.Dispose()
            }
        }
    }
    finally {
        $archive.Dispose()
    }

    if (($clips.Count + $kept.Count) -eq 0) {
        throw 'the download holds no VRMA_*.vrma entries; is this the VRMA_MotionPack archive?'
    }

    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    foreach ($name in $packClips) {
        if (-not $clips.ContainsKey($name)) {
            continue
        }
        # Write beside the target and rename, so a running body never reads a
        # half-written clip.
        $target = Join-Path $Destination $name
        $staging = "$target.part"
        [System.IO.File]::WriteAllBytes($staging, $clips[$name])
        Move-Item -LiteralPath $staging -Destination $target -Force
        $placed += $name
    }
}
finally {
    if (Test-Path -LiteralPath $temporary) {
        Remove-Item -LiteralPath $temporary -Force
    }
}

$missing = @($packClips | Where-Object { -not (Test-Path -LiteralPath (Join-Path $Destination $_)) })
Write-Host "motion directory: $Destination"
if ($placed.Count -gt 0) { Write-Host ('placed: ' + ($placed -join ', ')) }
if ($kept.Count -gt 0) { Write-Host ('kept (already present): ' + ($kept -join ', ')) }
if ($missing.Count -gt 0) { Write-Host ('missing from the download: ' + ($missing -join ', ')) }

$covered = @($hintClips | Where-Object { Test-Path -LiteralPath (Join-Path $Destination $_) })
if ($covered.Count -eq 0) {
    throw 'no activity-hint clip is placed; the body would keep its staged pose'
}
Write-Host ('activity hints covered: {0}/{1}' -f $covered.Count, $hintClips.Count)
Write-Host ('verify with: cargo run -p ene-body --bin ene-body-asset-probe -- <path to a VRM 1.0 avatar> "{0}"' -f $Destination)

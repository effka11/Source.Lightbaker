param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Map
)

$ErrorActionPreference = "Stop"
$root = "D:\Steam\steamapps\common\GarrysMod\garrysmod"

if ([System.IO.Path]::IsPathRooted($Map)) {
    $bsp = $Map
} else {
    $name = $Map
    if (-not $name.EndsWith(".bsp")) {
        $name += ".bsp"
    }
    $bsp = Join-Path $root "maps\$name"
    if (-not (Test-Path -LiteralPath $bsp)) {
        $bsp = Join-Path $root "download\maps\$name"
    }
}

if (-not (Test-Path -LiteralPath $bsp)) {
    Write-Error "Карта не найдена: $bsp"
}

Set-Location (Split-Path -Parent $PSScriptRoot)
& cargo run --release -p window -- $bsp
exit $LASTEXITCODE

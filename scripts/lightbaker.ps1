param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Map
)

$ErrorActionPreference = "Stop"
$maps = "D:\Steam\steamapps\common\GarrysMod\garrysmod\maps"

if ([System.IO.Path]::IsPathRooted($Map)) {
    $bsp = $Map
} else {
    $name = $Map
    if (-not $name.EndsWith(".bsp")) {
        $name += ".bsp"
    }
    $bsp = Join-Path $maps $name
}

if (-not (Test-Path -LiteralPath $bsp)) {
    Write-Error "Карта не найдена: $bsp"
}

Set-Location (Split-Path -Parent $PSScriptRoot)
& cargo run --release -p window -- $bsp
exit $LASTEXITCODE

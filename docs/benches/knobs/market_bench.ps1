# Market-lane bench (2026-09-04): run the profit harness over an
# origin x radius matrix and append one CSV row per run. The profit
# search is the lane's heaviest query; commodity-search timings come
# from the market-search tracing in the app log (kind/results/ms).
#
#   market_bench.ps1 -Database <edda.sqlite3> -Out <out.csv> [-Origins Wongi,Sol] [-Radii 60,120]
param(
    [Parameter(Mandatory)] [string]$Database,
    [Parameter(Mandatory)] [string]$Out,
    [string[]]$Origins = @("Wongi", "Sol"),
    [int[]]$Radii = @(60, 120),
    [int]$Cargo = 200,
    [int]$Range = 30,
    [int]$AgeHours = 48,
    [int]$Cap = 2500,
    [string]$Manifest = "$PSScriptRoot\..\..\..\Cargo.toml"
)
if (-not (Test-Path $Out)) {
    "ran_at,origin,radius_ly,cargo,age_h,cap,stations_considered,elapsed_s" | Set-Content $Out
}
$env:EDDA_DB = $Database
foreach ($origin in $Origins) {
    foreach ($radius in $Radii) {
        $lines = cargo run --manifest-path $Manifest -p ed-route --example profit --release -- `
            $origin $Cargo $Range any $AgeHours $radius $Cap 2>&1
        $summary = $lines | Select-String -Pattern '(\d+) stations considered in ([\d.]+)s' | Select-Object -First 1
        if ($summary) {
            $stations = $summary.Matches[0].Groups[1].Value
            $elapsed = $summary.Matches[0].Groups[2].Value
            "$(Get-Date -AsUTC -Format s)Z,$origin,$radius,$Cargo,$AgeHours,$Cap,$stations,$elapsed" | Add-Content $Out
            Write-Host "$origin @ $radius ly -> $stations stations, ${elapsed}s"
        } else {
            Write-Host "$origin @ $radius ly -> no summary line (unknown system?)"
        }
    }
}


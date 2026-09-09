# Build ed-api in WSL (native linux x86_64) and push it to the server;
# -Bootstrap also uploads this kit and runs bootstrap.sh (first bring-up
# or config change). SSH auth: an SSH key with root access on the box.
#
#   .\deploy\push-api.ps1               # binary only + service restart
#   .\deploy\push-api.ps1 -Bootstrap    # kit + bootstrap + binary
param(
    [switch]$Bootstrap,
    [string]$Server = $env:EDDA_DEPLOY_HOST
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$wslRoot = wsl -e wslpath -a $root

Write-Host "== building ed-api (WSL release) =="
wsl -e bash -lc "cd $wslRoot && CARGO_TARGET_DIR=$HOME/edda-target cargo build --release -p ed-api"
if ($LASTEXITCODE -ne 0) { throw "build failed" }
$binary = "\\wsl.localhost\Ubuntu\home\$env:EDDA_WSL_USER\edda-target\release\ed-api"

if ($Bootstrap) {
    Write-Host "== uploading kit =="
    ssh $Server "mkdir -p /root/edda-deploy"
    scp "$root\deploy\bootstrap.sh" "$root\deploy\Caddyfile" `
        "$root\deploy\edda-api.service" "$root\deploy\edda-daily.service" "$root\deploy\edda-daily.timer" `
        "$root\deploy\edda-weekly.service" "$root\deploy\edda-weekly.timer" "$root\deploy\edda-weekly.sh" "$root\deploy\edda-daily.sh" `
        "$root\deploy\edda-backup.service" "$root\deploy\edda-backup.timer" "$root\deploy\edda-backup.sh" `
        "${Server}:/root/edda-deploy/"
}

Write-Host "== uploading binary =="
scp $binary "${Server}:/usr/local/bin/ed-api.new"
ssh $Server "chmod 755 /usr/local/bin/ed-api.new && mv /usr/local/bin/ed-api.new /usr/local/bin/ed-api"

if ($Bootstrap) {
    Write-Host "== bootstrap =="
    ssh $Server "cd /root/edda-deploy && sed -i 's/\r$//' bootstrap.sh edda-daily.sh edda-weekly.sh edda-backup.sh && bash bootstrap.sh"
} else {
    ssh $Server "systemctl restart edda-api.service && systemctl --no-pager --lines 3 status edda-api.service"
}
Write-Host "== done: https://api.edda-app.com/healthz =="

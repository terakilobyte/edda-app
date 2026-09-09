# Release EDDA: build, sign, publish to the community API's app dir.
#
#   .\scripts\release-app.ps1 -Notes "what changed"
#
# THE VERSION COMES FROM src-tauri/Cargo.toml — the single source of
# truth (maintainer ruling 2026-09-05: "source the version from the toml
# files, not a magic floating number in the release script"; the old
# stamp-at-build-time flow left the committed toml at 0.2.0 through
# THREE releases, so dev builds and telemetry lied about their
# version). Releasing = bump the toml, commit, run this script.
# tauri.conf.json carries no version field: it inherits the toml's.
#
# The updater serves <ArtifactDir>\app\latest.json and the signed
# installer from /v1/app/; installed apps notice on their next check.
# Signing needs the private key (never in the repo).
param(
    [string]$ArtifactDir = $env:EDDA_ARTIFACT_DIR,
    [string]$ApiBase = "https://api.edda-app.com",
    [string]$Notes = "",
    [string]$KeyPath = $env:EDDA_UPDATER_KEY,
    # Escape hatch for a throwaway build; a real release is always tagged.
    [switch]$AllowUntagged,
    # Publish locally without touching prod. Nothing ships.
    [switch]$SkipUpload,
    [string]$RemoteHost = $env:EDDA_DEPLOY_HOST
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path $KeyPath)) { throw "signing key not found at $KeyPath" }

# 1. READ the version — never write it.
$toml = Get-Content "$root\src-tauri\Cargo.toml" -Raw
if ($toml -notmatch '(?m)^version = "(\d+\.\d+\.\d+)"') { throw "no x.y.z version in src-tauri/Cargo.toml" }
$Version = $Matches[1]
# A release builds committed source: a dirty tree means the artifacts
# would not match any commit anyone can check out.
$dirty = git -C $root status --porcelain -- src-tauri Cargo.lock
if ($dirty) { throw "src-tauri is dirty - commit (or discard) before releasing:`n$dirty" }
# Every release announces itself (maintainer, 2026-09-05): the bundled notes
# must open with this version's section, or there is nothing to splash.
# NOTE THE NAME. PowerShell variables are CASE-INSENSITIVE, so calling
# this `$notes` silently overwrote the `-Notes` PARAMETER with the whole
# file — and 0.2.6 shipped a manifest whose update splash was the file's
# preamble ("# EDDA release notes / Compiled into the app: …") instead of
# what changed. Caught by reading the published feed rather than trusting
# the script.
$notesFile = Get-Content "$root\src-tauri\RELEASE-NOTES.md" -Raw
if ($notesFile -notmatch [regex]::Escape("## $Version")) {
    throw "src-tauri/RELEASE-NOTES.md has no '## $Version' section - write the notes, then release"
}
# Default the splash to THIS version's blurb: the file's own convention is
# that every section leads with a one-paragraph summary, which is exactly
# what the updater and the website want.
if (-not $Notes) {
    $section = ($notesFile -split "(?m)^## ") | Where-Object { $_ -like "$Version*" } | Select-Object -First 1
    $body = ($section -replace "^$([regex]::Escape($Version))\s*", "")
    $Notes = ($body -split "(?m)^\s*$" | Where-Object { $_.Trim() } | Select-Object -First 1).Trim()
    if (-not $Notes) { throw "could not read the $Version blurb from RELEASE-NOTES.md" }
}
# A release is cut from a TAG, not from wherever HEAD has drifted to.
# Field case 2026-09-06: Item 52 A (fleet carriers) landed on main minutes
# after the 0.2.6 version bump, so HEAD carried an entire feature the maintainer
# had ruled OUT of the release — absent from its notes, never flown, and
# carrying its own schema migration. Nothing here would have noticed.
# `v$Version` makes the release candidate an explicit, checkable decision
# instead of "whatever main happened to be at".
$tag = "v$Version"
$headSha = (git -C $root rev-parse HEAD).Trim()
$tagSha = (git -C $root rev-parse --verify --quiet "$tag^{commit}")
if ($AllowUntagged) {
    Write-Warning "-AllowUntagged: shipping HEAD without checking it against $tag"
} elseif (-not $tagSha) {
    throw "no tag $tag - tag the exact commit you mean to ship (git tag $tag <sha>), then release. -AllowUntagged is for throwaway builds only."
} elseif ($tagSha.Trim() -ne $headSha) {
    $ahead = (git -C $root rev-list --count "$tag..HEAD").Trim()
    throw "HEAD is $ahead commit(s) past $tag - check out the tag to ship it (git checkout $tag), or move the tag if you truly mean to ship HEAD."
}
Write-Host "releasing $Version from $tag ($headSha) - committed, tagged, with notes"

# 2. Build with signing (the .sig lands beside the installer). This CLI
# reads the key CONTENT from TAURI_SIGNING_PRIVATE_KEY — the _PATH
# variant is silently ignored (field lesson: the 0.2.0 build bundled,
# then failed at the signing step).
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $KeyPath -Raw)
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = ""
$env:PATH = "$root\.data\tools\node;$env:PATH"
Set-Location $root
cargo tauri build
if ($LASTEXITCODE -ne 0) { throw "build failed" }

# 3. Collect the artifacts and MERGE into the published manifest —
# platforms is a map and the Linux/Mac release script owns its own
# entries: a same-version release preserves foreign platforms
# byte-for-byte; a version bump drops stale foreign entries loudly (an
# old-version entry preserved into a new manifest would hand that OS an
# old binary dressed as new).
# Cargo puts the bundle under CARGO_TARGET_DIR when it is set, and the
# house convention is that a worktree build sets it (so an assistant's
# build never fights the dev app's target dir). Assuming "$root\target"
# meant a release cut FROM a worktree built fine and then failed looking
# for its own installer.
$targetRoot = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "$root\target" }
$exe = Get-Item "$targetRoot\release\bundle\nsis\EDDA_${Version}_x64-setup.exe"
$sig = Get-Content "$($exe.FullName).sig" -Raw
$platforms = @{
    "windows-x86_64" = @{
        signature = $sig.Trim()
        url       = "$ApiBase/v1/app/EDDA_${Version}_x64-setup.exe"
    }
}
$appDir = Join-Path $ArtifactDir "app"
$latestPath = Join-Path $appDir "latest.json"
if (Test-Path $latestPath) {
    $existing = Get-Content $latestPath -Raw | ConvertFrom-Json
    foreach ($p in $existing.platforms.PSObject.Properties) {
        if ($p.Name -eq "windows-x86_64") { continue }
        if ($existing.version -eq $Version) {
            $platforms[$p.Name] = @{ signature = $p.Value.signature; url = $p.Value.url }
        } else {
            Write-Warning "dropping stale $($p.Name) entry from $($existing.version) - re-release it for $Version"
        }
    }
}
$latest = @{
    version  = $Version
    notes    = $Notes
    pub_date = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
    platforms = $platforms
}

# 4. Publish: installer first, manifest last (the pointer flips only once
# the package it names is in place).
New-Item -ItemType Directory -Force $appDir | Out-Null
Copy-Item $exe.FullName $appDir -Force
# The release-notes FEED ships beside the manifest: the website fetches it,
# so edda-app.com/notes cannot lag the binary the way it did at 0.2.6 (the
# app announced features the site had never heard of). Generated from the
# same RELEASE-NOTES.md the binary compiles in.
if (Test-Path "$root\site\build-notes.py") {
    & python3 "$root\site\build-notes.py" --out (Join-Path $appDir "notes.json")
    if ($LASTEXITCODE -ne 0) { throw "site/build-notes.py failed - the notes feed would lag the release" }
}
$latest | ConvertTo-Json -Depth 8 | Set-Content (Join-Path $appDir "latest.json") -Encoding utf8
Write-Host "published $($exe.Name) + latest.json -> $appDir"

# 5. UPLOAD BOTH FILES TOGETHER, or neither.
#
# This step used to be a human running two scp commands, and 0.2.6 shipped
# broken because of it: the script was re-run to fix the notes, which
# REBUILT and RE-SIGNED the installer, but only latest.json was uploaded.
# Prod then served the previous binary under the new signature and every
# auto-update failed verification. Installer FIRST (so the manifest never
# names bytes that are not there yet), manifest LAST, then verify the
# SERVED bytes — a "published" line is not evidence.
#
# SSH lives in WSL (the key is not reachable from Windows git), so the
# upload goes through it. -SkipUpload publishes locally only.
if ($SkipUpload) {
    Write-Warning "-SkipUpload: local publish only. Prod still serves the PREVIOUS release; nothing has shipped."
    return
}
$wslApp = "/mnt/c" + ($appDir -replace '^[A-Za-z]:', '' -replace '\\', '/')
$remote = "root@$RemoteHost"
$remoteDir = "/var/lib/edda/artifacts/app"
Write-Host "uploading installer then manifest to $remote..."
$up = @"
set -e
scp -o ConnectTimeout=30 '$wslApp/$($exe.Name)' $remote`:$remoteDir/$($exe.Name).new
ssh -o ConnectTimeout=30 $remote "mv $remoteDir/$($exe.Name).new $remoteDir/$($exe.Name) && chown edda:edda $remoteDir/$($exe.Name) && chmod 644 $remoteDir/$($exe.Name)"
scp -o ConnectTimeout=30 '$wslApp/notes.json' $remote`:$remoteDir/notes.json.new
ssh -o ConnectTimeout=30 $remote "mv $remoteDir/notes.json.new $remoteDir/notes.json && chown edda:edda $remoteDir/notes.json && chmod 644 $remoteDir/notes.json"
scp -o ConnectTimeout=30 '$wslApp/latest.json' $remote`:$remoteDir/latest.json.new
ssh -o ConnectTimeout=30 $remote "mv $remoteDir/latest.json.new $remoteDir/latest.json && chown edda:edda $remoteDir/latest.json && chmod 644 $remoteDir/latest.json"
"@
wsl -e bash -lc $up
if ($LASTEXITCODE -ne 0) { throw "upload failed - prod may be mid-flip; re-run before announcing anything" }

# 6. Verify what the API actually SERVES against what was signed.
Write-Host "verifying the published release..."
$root_wsl = "/mnt/c" + ($root -replace '^[A-Za-z]:', '' -replace '\\', '/')
wsl -e bash -lc "bash '$root_wsl/scripts/verify-release.sh' '$ApiBase' '$wslApp' 'windows-x86_64'"
if ($LASTEXITCODE -ne 0) { throw "PUBLISHED RELEASE FAILED VERIFICATION - do not announce it" }
Write-Host "installed apps will offer $Version on their next check"

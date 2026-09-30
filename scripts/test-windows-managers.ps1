# Native lifecycle tests. The 0.0.1 baseline is fixture metadata around the
# exact same release binary; no previous public Windows release is implied.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Version, [string]$Assets = 'dist')
$ErrorActionPreference = 'Stop'
$assetsPath = (Resolve-Path $Assets).Path
$version = $Version.TrimStart('v')
$root = Join-Path $env:RUNNER_TEMP "co-packages-$PID"
$user = "co-scoop-$PID"
$server = $null
$originalPath = $env:PATH
. "$PSScriptRoot\windows-manager-probe.ps1"
function Checked([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE) { throw "$Command failed ($LASTEXITCODE)" }
}
function Refresh-Path {
    $env:PATH = [Environment]::GetEnvironmentVariable('Path', 'User') + ';' + [Environment]::GetEnvironmentVariable('Path', 'Machine') + ';' + $originalPath
}
function Verify-Co([string]$Binary) {
    $actual = & co version
    if ($LASTEXITCODE -or $actual.Trim() -ne "co $version") { throw 'Installed co version mismatch' }
    $metadata = Get-Content "$root\new\windows-packages.json" -Raw | ConvertFrom-Json
    if ((Get-FileHash $Binary -Algorithm SHA256).Hash.ToLowerInvariant() -ne $metadata.binarySha256) { throw 'Installed binary does not match the tested source' }
    Checked co @('help')
}
try {
    New-Item -ItemType Directory -Path $root | Out-Null
    Copy-Item "$PSScriptRoot\windows-manager-probe.ps1" "$root\probe.ps1"
    $server = Start-Process python -ArgumentList 'scripts/windows-package-server.py', $assetsPath, "$root\port" -RedirectStandardOutput "$root\http.stdout.log" -RedirectStandardError "$root\http.stderr.log" -PassThru
    $deadline = (Get-Date).AddSeconds(20)
    while (!(Test-Path "$root\port")) {
        if ($server.HasExited -or (Get-Date) -gt $deadline) { throw 'Fixture download server failed' }
        Start-Sleep -Milliseconds 100
    }
    $origin = 'http://127.0.0.1:' + (Get-Content "$root\port" -Raw)
    Checked python @('scripts/windows-packages.py', 'manifests', $version, $assetsPath, "$root\old", '--fixture-origin', $origin, '--fixture-version', '0.0.1')
    Checked python @('scripts/windows-packages.py', 'manifests', $version, $assetsPath, "$root\new", '--fixture-origin', $origin)
    if (Get-Command co -ErrorAction SilentlyContinue) { throw 'Package smoke environment already has co on PATH' }

    # Scoop is tested in a clean standard-user profile, including its real shim.
    $password = ConvertTo-SecureString ([guid]::NewGuid().ToString() + 'aA1!') -AsPlainText -Force
    New-LocalUser -Name $user -Password $password | Out-Null
    Checked icacls @($root, '/grant', "${user}:(OI)(CI)F")
    $scoopScript = @'
param($Root, $Version)
$ErrorActionPreference = 'Stop'
function Checked($Command, $Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE) { throw "$Command failed ($LASTEXITCODE)" }
}
function Assert-ScoopPackageVersion([string]$ExpectedVersion) {
    $installed = Get-Content "$env:SCOOP\apps\co-codes-cli\current\scoop-manifest.json" -Raw | ConvertFrom-Json
    if ($installed.version -ne $ExpectedVersion) { throw "Scoop package version mismatch: expected $ExpectedVersion, got $($installed.version)" }
    Write-Output "Scoop installed package version: $($installed.version)"
}
try {
    . "$Root\probe.ps1"
    # The fresh standard user cannot write the CI parent's temporary directory.
    $env:TEMP = $Root
    $env:TMP = $Root
    # Scoop also reads XDG_CONFIG_HOME/USERPROFILE from the inherited environment.
    $env:XDG_CONFIG_HOME = "$Root\user-config"
    $env:SCOOP = "$Root\scoop"
    Invoke-WebRequest https://get.scoop.sh -UseBasicParsing -OutFile "$Root\install-scoop.ps1"
    & "$Root\install-scoop.ps1" -ScoopDir $env:SCOOP
    $env:PATH = "$env:SCOOP\shims;$env:PATH"
    $bucket = "$Root\bucket"
    New-Item -ItemType Directory "$bucket\bucket" | Out-Null
    # Keep fixture bytes stable when the credential probe isolates Git config.
    '* -text' | Set-Content "$bucket\.gitattributes" -Encoding ascii
    Copy-Item "$Root\old\co-codes-cli.json" "$bucket\bucket\co-codes-cli.json"
    Checked git @('init', '--quiet', $bucket)
    Checked git @('-C', $bucket, 'add', '.')
    Checked git @('-C', $bucket, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '--quiet', '-m', 'Fixture baseline')
    # Scoop validates a Git URI before cloning; a backslash directory is rejected.
    $bucketUri = ([uri]$bucket).AbsoluteUri
    Checked scoop @('bucket', 'add', 'co-windows-test', $bucketUri)
    Checked scoop @('install', 'co-windows-test/co-codes-cli')
    Assert-ScoopPackageVersion '0.0.1'
    $oldInstall = "$env:SCOOP\apps\co-codes-cli\0.0.1"
    if (!(Test-Path "$oldInstall\co.exe")) { throw 'Scoop fixture baseline install is missing' }
    if ((& co version).Trim() -ne "co $Version") { throw 'Scoop install version mismatch' }
    New-LinkProbe "$Root\scoop-link"
    Copy-Item "$Root\new\co-codes-cli.json" "$bucket\bucket\co-codes-cli.json" -Force
    Checked git @('-C', $bucket, 'add', '.')
    Checked git @('-C', $bucket, '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.test', 'commit', '--quiet', '-m', 'Fixture upgrade')
    # App-only update can skip bucket sync during Scoop's recent-update interval.
    Checked scoop @('update')
    $clonedManifest = Get-Content "$env:SCOOP\buckets\co-windows-test\bucket\co-codes-cli.json" -Raw | ConvertFrom-Json
    if ($clonedManifest.version -ne $Version) { throw 'Scoop fixture bucket did not refresh' }
    Checked scoop @('update', 'co-codes-cli')
    # Both packages contain the same binary, so observe Scoop's package metadata.
    Assert-ScoopPackageVersion $Version
    Checked scoop @('cleanup', 'co-codes-cli')
    if (Test-Path $oldInstall) { throw 'Scoop cleanup left the fixture baseline installed' }
    Write-Output 'Scoop fixture baseline removed; checking upgraded binary and Git helper.'
    $metadata = Get-Content "$Root\new\windows-packages.json" -Raw | ConvertFrom-Json
    if ((& co version).Trim() -ne "co $Version") { throw 'Scoop upgrade version mismatch' }
    if ((Get-FileHash "$env:SCOOP\apps\co-codes-cli\current\co.exe").Hash.ToLowerInvariant() -ne $metadata.binarySha256) { throw 'Scoop binary source mismatch' }
    Test-LinkProbe "$Root\scoop-link"
    Checked co @('help')
    Checked scoop @('uninstall', 'co-codes-cli')
    if (Get-Command co -ErrorAction SilentlyContinue) { throw 'Scoop left co on PATH after uninstall' }
    exit 0
} catch {
    Write-Output $_.Exception.ToString()
    Write-Output $_.ScriptStackTrace
    Write-Error $_
    exit 1
}
'@
    $scoopScript | Set-Content "$root\scoop.ps1"
    $credential = [pscredential]::new("$env:COMPUTERNAME\$user", $password)
    # Windows PowerShell must reconstruct its native module roots rather than
    # inherit incompatible PowerShell 7 modules from the CI parent process.
    $originalModulePath = $env:PSModulePath
    try {
        Remove-Item Env:PSModulePath -ErrorAction SilentlyContinue
        $process = Start-Process powershell.exe -Credential $credential -LoadUserProfile -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "$root\scoop.ps1", '-Root', $root, '-Version', $version -RedirectStandardOutput "$root\scoop.stdout.log" -RedirectStandardError "$root\scoop.stderr.log" -Wait -PassThru
    } finally { $env:PSModulePath = $originalModulePath }
    Get-Content "$root\scoop.stdout.log"
    Get-Content "$root\scoop.stderr.log"
    if ($process.ExitCode) { throw 'Native Scoop lifecycle failed' }

    # Official WinGet bootstrap supports Windows Server 2025 runners.
    if (!(Get-Command winget -ErrorAction SilentlyContinue)) {
        Install-PackageProvider -Name NuGet -Force | Out-Null
        Install-Module -Name Microsoft.WinGet.Client -Force -Repository PSGallery | Out-Null
        Repair-WinGetPackageManager -AllUsers
        Refresh-Path
    }
    Checked winget @('--version')
    Checked winget @('settings', '--enable', 'LocalManifestFiles')
    foreach ($manifest in @("$root\old\winget", "$root\new\winget")) { Checked winget @('validate', '--manifest', $manifest) }
    Checked winget @('install', '--manifest', "$root\old\winget", '--scope', 'user', '--silent', '--disable-interactivity', '--accept-package-agreements', '--accept-source-agreements')
    Refresh-Path
    $wingetBinary = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\CoCodes.Co*" -Recurse -Filter co.exe | Select-Object -First 1 -ExpandProperty FullName
    if (!$wingetBinary) { throw 'WinGet did not install co.exe' }
    Verify-Co $wingetBinary
    New-LinkProbe "$root\winget-link"
    Checked winget @('upgrade', '--manifest', "$root\new\winget", '--scope', 'user', '--silent', '--disable-interactivity', '--accept-package-agreements', '--accept-source-agreements')
    Refresh-Path
    $wingetBinary = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\CoCodes.Co*" -Recurse -Filter co.exe | Select-Object -First 1 -ExpandProperty FullName
    Verify-Co $wingetBinary
    Test-LinkProbe "$root\winget-link"
    Checked winget @('uninstall', '--id', 'CoCodes.Co', '--exact', '--scope', 'user', '--silent', '--disable-interactivity')
    Refresh-Path
    if (Get-Command co -ErrorAction SilentlyContinue) { throw 'WinGet left co on PATH after uninstall' }

    # Build and install genuine nupkg files through Chocolatey's standard helpers.
    $feed = "$root\feed"
    New-Item -ItemType Directory $feed | Out-Null
    foreach ($package in @("$root\old\chocolatey", "$root\new\chocolatey")) {
        Checked choco @('pack', "$package\co-codes-cli.nuspec", '--outputdirectory', $feed)
    }
    Checked choco @('install', 'co-codes-cli', '--version', '0.0.1', '--source', $feed, '--yes', '--no-progress')
    Refresh-Path
    Verify-Co "$env:ChocolateyInstall\lib\co-codes-cli\tools\co.exe"
    New-LinkProbe "$root\chocolatey-link"
    Checked choco @('upgrade', 'co-codes-cli', '--source', $feed, '--yes', '--no-progress')
    Refresh-Path
    Verify-Co "$env:ChocolateyInstall\lib\co-codes-cli\tools\co.exe"
    Test-LinkProbe "$root\chocolatey-link"
    Checked choco @('uninstall', 'co-codes-cli', '--yes', '--no-progress')
    if (Get-Command co -ErrorAction SilentlyContinue) { throw 'Chocolatey left co on PATH after uninstall' }
    Write-Output 'Native package install, fixture upgrade, uninstall and source-hash checks passed.'
} finally {
    $env:PATH = $originalPath
    foreach ($name in @('CO_CONFIG_DIR', 'CO_API_URL', 'CO_AGENT_ID', 'GIT_CONFIG_GLOBAL', 'GIT_CONFIG_NOSYSTEM', 'GIT_TERMINAL_PROMPT')) {
        Remove-Item "Env:$name" -ErrorAction SilentlyContinue
    }
    if ($server -and !$server.HasExited) { Stop-Process -Id $server.Id }
    if (Get-LocalUser -Name $user -ErrorAction SilentlyContinue) { Remove-LocalUser -Name $user }
}

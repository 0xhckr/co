# Exercise Git's real credential-helper protocol without contacting a live API.
function New-LinkProbe([string]$Directory) {
    New-Item -ItemType Directory -Path $Directory -Force | Out-Null
    $env:CO_CONFIG_DIR = Join-Path $Directory 'config'
    $env:CO_API_URL = 'http://127.0.0.1:1'
    Remove-Item Env:CO_AGENT_ID -ErrorAction SilentlyContinue
    & co logout | Out-Null
    if ($LASTEXITCODE) { throw 'Cannot initialize probe config' }
    '{"session_token":"fixture-only-session"}' | Set-Content "$env:CO_CONFIG_DIR\config.json" -Encoding ascii
    $env:GIT_CONFIG_GLOBAL = Join-Path $Directory 'empty.gitconfig'
    '' | Set-Content $env:GIT_CONFIG_GLOBAL
    $env:GIT_CONFIG_NOSYSTEM = '1'
    $env:GIT_TERMINAL_PROMPT = '0'
    & git init --quiet $Directory
    if ($LASTEXITCODE) { throw 'Cannot initialize Git probe' }
    Push-Location $Directory
    try {
        & co link co/fixture
        if ($LASTEXITCODE) { throw 'Cannot configure installed Git helper' }
        $local = & git config --local --list
        if ($local -match 'fixture-only-session') { throw 'Git config contains a credential' }
    } finally { Pop-Location }
    Test-LinkProbe $Directory
}

function Test-LinkProbe([string]$Directory) {
    $inputText = "protocol=https`nhost=git.co.codes`npath=co/fixture.git`n`n"
    $result = $inputText | & git -C $Directory credential fill 2>&1
    if ($LASTEXITCODE -or $result -notcontains 'password=fixture-only-session') { throw 'Installed Git helper failed after install or upgrade' }
    $env:CO_AGENT_ID = 'missing-fixture-agent'
    $previousPreference = $ErrorActionPreference
    try {
        # Windows PowerShell treats redirected native stderr as error records.
        # This invocation must fail; inspect its exit status without terminating.
        $ErrorActionPreference = 'Continue'
        $result = $inputText | & git -C $Directory credential fill 2>&1
        if ($LASTEXITCODE -eq 0 -or $result -match 'fixture-only-session') { throw 'Agent selection fell back to human credentials' }
    } finally {
        $ErrorActionPreference = $previousPreference
        Remove-Item Env:CO_AGENT_ID
    }
}

$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
$registered = $false
$taskName = 'local.mcp-gate.schema-fixture.' + [guid]::NewGuid().ToString()
$fixtureDirectory = Join-Path ([IO.Path]::GetTempPath()) $taskName
[IO.Directory]::CreateDirectory($fixtureDirectory) | Out-Null
$xmlPath = Join-Path $fixtureDirectory 'task.xml'
function XmlText([string]$value) { [Security.SecurityElement]::Escape($value) }
$template = [IO.File]::ReadAllText((Join-Path $PWD 'src/manage/windows-task.xml'))
$document = $template.Replace('{user}', (XmlText ((whoami.exe).Trim())))
$document = $document.Replace('{binary}', (XmlText (Join-Path $env:SystemRoot 'System32/cmd.exe')))
$document = $document.Replace('{arguments}', '/C exit 0')
$document = $document.Replace('{cwd}', (XmlText $fixtureDirectory))
try {
    [IO.File]::WriteAllText($xmlPath, $document, [Text.UTF8Encoding]::new($false))
    & schtasks.exe /Create /TN $taskName /XML $xmlPath
    if ($LASTEXITCODE -ne 0) { throw 'The production Task Scheduler XML contract was rejected' }
    $registered = $true
    & schtasks.exe /Run /TN $taskName
    if ($LASTEXITCODE -ne 0) { throw 'The current user cannot start the fixture logon task' }
    Write-Output 'Production Task Scheduler XML registered and started for the current user'
} finally {
    # An already completed action can make /End return an error. Always continue
    # to delete this fixture's registration, including when /Run failed.
    $ErrorActionPreference = 'Continue'
    if ($registered) {
        & schtasks.exe /End /TN $taskName 2>&1 | Out-Null
        & schtasks.exe /Delete /TN $taskName /F
        if ($LASTEXITCODE -ne 0) { throw "Cannot remove fixture task $taskName" }
    }
    Remove-Item -LiteralPath $fixtureDirectory -Recurse -Force
}

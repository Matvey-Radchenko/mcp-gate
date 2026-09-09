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
$document = $document.Replace('{arguments}', (XmlText '/D /C "echo completed > completed.txt"'))
$document = $document.Replace('{cwd}', (XmlText $fixtureDirectory))
try {
    [IO.File]::WriteAllText($xmlPath, $document, [Text.Encoding]::Unicode)
    & schtasks.exe /Create /TN $taskName /XML $xmlPath
    if ($LASTEXITCODE -ne 0) { throw 'The production Task Scheduler XML contract was rejected' }
    $registered = $true
    & schtasks.exe /Run /TN $taskName
    if ($LASTEXITCODE -ne 0) { throw 'The current user cannot start the fixture logon task' }
    # /Run only queues a launch. Observe the actual action before removing its cwd.
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (!(Test-Path (Join-Path $fixtureDirectory 'completed.txt')) -or
           (Get-ScheduledTask -TaskName $taskName).State -eq 'Running') {
        if ([DateTime]::UtcNow -ge $deadline) { throw 'Fixture task did not finish its action' }
        Start-Sleep -Milliseconds 100
    }
    Write-Output 'Production Task Scheduler XML registered and executed for the current user'
} finally {
    # An already completed action can make /End return an error. Always continue
    # to delete this fixture's registration, including when /Run failed.
    if ($registered) {
        & schtasks.exe /End /TN $taskName 2>&1 | Out-Null
        & schtasks.exe /Delete /TN $taskName /F
        if ($LASTEXITCODE -ne 0) { throw "Cannot remove fixture task $taskName" }
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while (Test-Path -LiteralPath $fixtureDirectory) {
        try { Remove-Item -LiteralPath $fixtureDirectory -Recurse -Force -ErrorAction Stop }
        catch {
            if ([DateTime]::UtcNow -ge $deadline) { throw }
            Start-Sleep -Milliseconds 100
        }
    }
}

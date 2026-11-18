# Native Task Scheduler preserves the task's identity, triggers and ACL on update.
$ErrorActionPreference = 'Stop'
$utf8 = [Text.UTF8Encoding]::new($false)
[Console]::InputEncoding = $utf8
[Console]::OutputEncoding = $utf8

function Read-Definition([string]$text) {
    $doc = [System.Xml.XmlDocument]::new()
    $doc.XmlResolver = $null
    $doc.LoadXml($text)
    return ,$doc
}

$mode = $env:MCP_GATE_TASK_MODE
if ($mode -eq 'rewrite') {
    $doc = Read-Definition ([Console]::In.ReadToEnd())
    $ns = [System.Xml.XmlNamespaceManager]::new($doc.NameTable)
    $ns.AddNamespace('t', $doc.DocumentElement.NamespaceURI)
    $command = $doc.SelectSingleNode('/t:Task/t:Actions/t:Exec/t:Command', $ns)
    if ($null -eq $command -or $command.InnerText -cne $env:MCP_GATE_TASK_OLD) {
        throw 'Task executable changed'
    }
    $command.InnerText = $env:MCP_GATE_TASK_BINARY
    [Console]::Write($doc.OuterXml)
    exit
}

$service = New-Object -ComObject 'Schedule.Service'
$service.Connect()
$folder = $service.GetFolder('\')
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
function Is-UserTask($task) {
    $principal = $task.Definition.Principal
    if ($principal.LogonType -ne 3) { return $false } # TASK_LOGON_INTERACTIVE_TOKEN
    $user = $principal.UserId
    if ($user -eq $sid) { return $true }
    if ([string]::IsNullOrEmpty($user) -or $user.StartsWith('S-1-')) { return $false }
    try {
        $account = [Security.Principal.NTAccount]::new($user)
        return $account.Translate([Security.Principal.SecurityIdentifier]).Value -eq $sid
    } catch [Security.Principal.IdentityNotMappedException] {
        return $false
    }
}

$label = $env:MCP_GATE_TASK_LABEL
if ($mode -in @('inspect', 'discover')) {
    $results = @(
        foreach ($task in $folder.GetTasks(1)) { # TASK_ENUM_HIDDEN
            if ($task.Name -notlike 'local.mcp-gate.*' -or ($label -and $task.Name -ne $label)) { continue }
            $actions = $task.Definition.Actions
            if ($actions.Count -ne 1 -or $actions.Item(1).Type -ne 0 -or !(Is-UserTask $task)) {
                if ($label) { throw 'Task has no supported per-user action' }
                continue
            }
            $action = $actions.Item(1)
            @{
                label = $task.Name
                binary = $action.Path
                arguments = $action.Arguments
                xml = if ($label) { (Read-Definition $task.Xml).OuterXml } else { $null }
            }
        }
    )
    [Console]::Write((ConvertTo-Json -Compress -InputObject $results))
    exit
}

$task = $folder.GetTask($label)
if (!(Is-UserTask $task)) { throw 'Task does not belong to the current user' }
switch ($mode) {
    'stop' {
        if ($task.GetInstances(0).Count -gt 0) { $task.Stop(0) }
        for ($attempt = 0; $attempt -lt 100; $attempt++) {
            if ($task.GetInstances(0).Count -eq 0) { exit }
            Start-Sleep -Milliseconds 100
        }
        throw 'Task is still stopping'
    }
    'publish' {
        $xml = [Console]::In.ReadToEnd()
        $principal = $task.Definition.Principal
        # TASK_UPDATE | TASK_DONT_ADD_PRINCIPAL_ACE | TASK_IGNORE_REGISTRATION_TRIGGERS
        $null = $folder.RegisterTask($task.Name, $xml, 52, $principal.UserId, $null, 3, $null)
    }
    'start' {
        if ($task.GetInstances(0).Count -eq 0) { $null = $task.Run($null) }
    }
    default { throw 'Unknown service manager operation' }
}

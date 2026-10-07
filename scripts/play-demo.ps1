<#
  Replay a reviewed script through the same MCP connection used by an assistant.
  Access must already be enabled in Kinetic PDF's Settings. No credential is saved.
  Example:
    .\scripts\play-demo.ps1 -Endpoint http://127.0.0.1:47831/mcp `
      -Token (Read-Host 'Session bearer token' -AsSecureString)
  Start window capture in your recorder separately; pointer cues are app overlays.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][uri]$Endpoint,
    [Parameter(Mandatory)][securestring]$Token,
    [string]$ScriptPath = (Join-Path $PSScriptRoot '../examples/demo/navigation.json')
)
$ErrorActionPreference = 'Stop'
if ($Endpoint.Scheme -ne 'http' -or $Endpoint.Host -ne '127.0.0.1' -or $Endpoint.AbsolutePath -ne '/mcp') {
    throw 'Use the exact local HTTP endpoint shown in Kinetic PDF Settings.'
}
$scriptData = Get-Content -LiteralPath $ScriptPath -Raw -Encoding UTF8 | ConvertFrom-Json
$credential = [System.Net.NetworkCredential]::new('', $Token)
$headers = @{
    Authorization = 'Bearer ' + $credential.Password
    Accept = 'application/json, text/event-stream'
    'MCP-Protocol-Version' = '2025-11-25'
}
$rpcId = 0
$startedDemo = $false
function Invoke-KineticRpc([string]$Method, [object]$Parameters) {
    $script:rpcId++
    $body = @{ jsonrpc='2.0'; id=$script:rpcId; method=$Method; params=$Parameters } | ConvertTo-Json -Depth 100 -Compress
    $result = Invoke-RestMethod -Uri $Endpoint -Method Post -Headers $headers -ContentType 'application/json; charset=utf-8' -Body ([Text.Encoding]::UTF8.GetBytes($body)) -TimeoutSec 35
    if ($result.error) { throw ('MCP request failed: ' + $result.error.message) }
    return $result.result
}
function Invoke-KineticCommand([object]$Command, [object]$Target) {
    $result = Invoke-KineticRpc 'tools/call' @{ name='kinetic_control'; arguments=@{ command=$Command; target=$Target } }
    if ($result.isError) { throw ('App rejected demo command: ' + $result.structuredContent.error.message) }
    return $result.structuredContent
}
try {
    $initialized = Invoke-KineticRpc 'initialize' @{ protocolVersion='2025-11-25'; capabilities=@{}; clientInfo=@{ name='kinetic-demo'; version='1' } }
    $headers['MCP-Protocol-Version'] = $initialized.protocolVersion
    $notification = @{ jsonrpc='2.0'; method='notifications/initialized' } | ConvertTo-Json -Compress
    $null = Invoke-RestMethod -Uri $Endpoint -Method Post -Headers $headers -ContentType 'application/json; charset=utf-8' -Body ([Text.Encoding]::UTF8.GetBytes($notification)) -TimeoutSec 10
    $state = Invoke-KineticCommand @{ command='inspect' } $null
    $null = Invoke-KineticCommand @{ command='run_demo'; script=$scriptData } $state.state.document
    $startedDemo = $true
    do {
        Start-Sleep -Milliseconds 200
        $status = Invoke-KineticCommand @{ command='demo_status' } $null
        $progress = $status.data.value
    } while ($progress.status -eq 'running')
    if ($progress.status -ne 'complete') { throw ('Demo ended: ' + $progress.status + ' ' + $progress.error.message) }
    Write-Host ('Demo complete: ' + $progress.total + ' steps.')
} finally {
    # Also handles Ctrl+C or a disconnected caller while playback remains active.
    if ($startedDemo) { try { $null = Invoke-KineticCommand @{ command='cancel_demo' } $null } catch {} }
    $headers.Clear()
    $credential = $null
}

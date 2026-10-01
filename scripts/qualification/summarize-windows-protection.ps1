param(
    [Parameter(Mandatory=$true)][string]$EvidenceRoot,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new summary directory.' }
$output = New-Item -ItemType Directory -Path $OutputDirectory
function Median($Values) {
    $sorted = @($Values | ForEach-Object { [double]$_ } | Sort-Object)
    if (-not $sorted.Count) { return $null }
    $middle = [int][math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2) { return $sorted[$middle] }
    return ($sorted[$middle - 1] + $sorted[$middle]) / 2
}
function PublicAddress([string]$Address) {
    # Fresh consent URLs can still contain transient session identifiers.
    # Keep raw evidence local and remove query/fragment fields from the report.
    if (-not $Address) { return $Address }
    return ([uri]$Address).GetLeftPart([UriPartial]::Path)
}
$summaries = @()
foreach ($directory in Get-ChildItem -LiteralPath $EvidenceRoot -Directory | Sort-Object Name) {
    if ($directory.Name -notmatch '^(settled-|idle-|final-(fixture|csp|coverage))') { continue }
    $resultPath = Join-Path $directory.FullName 'result.json'
    if (-not (Test-Path -LiteralPath $resultPath)) { continue }
    $result = Get-Content -Raw -LiteralPath $resultPath | ConvertFrom-Json
    $metadata = Get-Content -Raw -LiteralPath (Join-Path $directory.FullName 'metadata.json') | ConvertFrom-Json
    $samples = @(Import-Csv -LiteralPath (Join-Path $directory.FullName 'samples.csv'))
    # Only the corrected runner marks native teardown as non-idle. Earlier
    # fixture runs remain useful for correctness and sampled peak memory.
    $idle = @($samples | Where-Object { $_.idleInterval -eq 'True' -and [int]$_.processCount -gt 0 })
    $longIdle = $metadata.idleSeconds -ge 300
    $idleCpu = if ($longIdle) { ($idle.cpuSeconds | Measure-Object -Sum).Sum } else { $null }
    $idleWall = if ($longIdle) { ($idle.intervalSeconds | Measure-Object -Sum).Sum } else { $null }
    foreach ($page in $result.pages) {
        if ($page.document) {
            $page.document.url = PublicAddress $page.document.url
            if ($page.document.title -match '^https?://') {
                $page.document.title = PublicAddress $page.document.title
            }
        }
        if ($page.navigation) { $page.navigation.name = PublicAddress $page.navigation.name }
    }
    $summaries += [pscustomobject]@{
        run = $directory.Name; site = $result.site; mode = $result.mode
        counting = $result.counting; tabs = $result.tabs
        pageLoadMs = Median $result.pages.host_load_ms
        sampledPeakPrivateMiB = ($samples.privateMiB | ForEach-Object { [double]$_ } | Measure-Object -Maximum).Maximum
        sampledCpuSeconds = ($samples.cpuSeconds | Measure-Object -Sum).Sum
        retainedIdlePrivateMiB = $(if ($longIdle) { Median $idle.privateMiB } else { $null })
        idleCpuSeconds = $idleCpu; idleWallSeconds = $idleWall
        blocks = $result.installed_blocks
        title = @($result.pages.document.title | Where-Object { $_ })
        responseStatus = @($result.pages.navigation.responseStatus | Where-Object { $_ })
        callback = $result.callback; matcher = $result.matcher; diagnostics = $result.diagnostics
        sourceRevision = $metadata.sourceRevision; binarySha256 = $metadata.binarySha256
    }
    $destination = New-Item -ItemType Directory -Path (Join-Path $output.FullName $directory.Name)
    $result | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath (Join-Path $destination.FullName 'result.json')
    $metadata | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath (Join-Path $destination.FullName 'metadata.json')
    Copy-Item -LiteralPath (Join-Path $directory.FullName 'samples.csv') -Destination $destination.FullName
}
$summaries | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $output.FullName 'summary.json')
$summaries | Select-Object run,pageLoadMs,sampledPeakPrivateMiB,sampledCpuSeconds,retainedIdlePrivateMiB,idleCpuSeconds,idleWallSeconds,blocks | Format-Table -AutoSize

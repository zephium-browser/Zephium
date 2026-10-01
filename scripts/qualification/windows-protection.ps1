param(
    [Parameter(Mandatory=$true)][string]$Binary,
    [ValidateSet('fixture','clean','csp','coverage','yahoo','bloomberg','news','github','google')][string]$Site = 'fixture',
    [ValidateSet('off','on','paused')][string]$Mode = 'on',
    [ValidateSet('off','on')][string]$Counting = 'on',
    [ValidateSet(1,10,30)][int]$Tabs = 1,
    [ValidateRange(0,600)][int]$IdleSeconds = 0,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new evidence directory for each run.' }
$output = New-Item -ItemType Directory -Path $OutputDirectory
$stdout = Join-Path $output.FullName 'stdout.log'
$stderr = Join-Path $output.FullName 'stderr.log'
$env:ZEPHIUM_PROTECTION_SITE = $Site
$env:ZEPHIUM_PROTECTION_MODE = $Mode
$env:ZEPHIUM_PROTECTION_COUNTING = $Counting
$env:ZEPHIUM_PROTECTION_TABS = [string]$Tabs
$env:ZEPHIUM_PROTECTION_IDLE_SECONDS = [string]$IdleSeconds
$metadata = [ordered]@{
    startedUtc = [DateTime]::UtcNow.ToString('o')
    binarySha256 = (Get-FileHash -LiteralPath $binaryPath).Hash
    sourceRevision = (& git rev-parse HEAD)
    sourceDirty = [bool](& git status --porcelain)
    site = $Site; mode = $Mode; counting = $Counting; tabs = $Tabs; idleSeconds = $IdleSeconds
    cpu = (Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors)
    os = (Get-CimInstance Win32_OperatingSystem | Select-Object Caption,BuildNumber,TotalVisibleMemorySize)
    powerScheme = (& powercfg /getactivescheme)
    caveats = @('Native network adapter harness, not full browser chrome/cosmetics/startup.',
        'Host window is hidden; these are background native views, not foreground render-performance measurements.',
        'Sampled private bytes; working sets double-count shared pages.',
        'CPU is a lower bound: processes exiting between samples can be missed.',
        'CPU and process counts do not measure idle wakeups or battery energy.',
        'Fresh disposable profile per run; cold network and OS caches are not guaranteed.')
}
$metadata | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $output.FullName 'metadata.json')
$process = Start-Process -FilePath $binaryPath -ArgumentList @('native_protection_qualification','--ignored','--nocapture','--test-threads=1') -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
$handle = $process.Handle
$clock = [Diagnostics.Stopwatch]::StartNew()
$previous = @{}
$lastAt = 0.0
$lastIdle = $false
while (-not $process.HasExited) {
    $inventory = @(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,CreationDate)
    $births = @{}
    foreach ($entry in $inventory) { $births[[int]$entry.ProcessId] = $entry.CreationDate }
    $ids = [Collections.Generic.HashSet[int]]::new()
    [void]$ids.Add($process.Id)
    do {
        $added = $false
        foreach ($entry in $inventory) {
            $parent = [int]$entry.ParentProcessId
            if ($ids.Contains($parent) -and $null -ne $entry.CreationDate -and
                $null -ne $births[$parent] -and $entry.CreationDate -ge $births[$parent] -and
                $ids.Add([int]$entry.ProcessId)) { $added = $true }
        }
    } while ($added)
    $at = $clock.Elapsed.TotalSeconds
    $current = @{}
    $cpuDelta = 0.0
    $private = 0L
    $working = 0L
    foreach ($processId in $ids) {
        $child = Get-Process -Id $processId -ErrorAction SilentlyContinue
        if ($null -eq $child) { continue }
        try {
            $key = "$processId/$($child.StartTime.Ticks)"
            $cpu = $child.TotalProcessorTime.TotalSeconds
            if ($previous.ContainsKey($key)) {
                $cpuDelta += [Math]::Max([double]0,[double]($cpu - $previous[$key]))
            } else {
                # Fresh descendants belong to this disposable environment;
                # include their startup CPU before their first observation.
                $cpuDelta += $cpu
            }
            $current[$key] = $cpu
            $private += $child.PrivateMemorySize64
            $working += $child.WorkingSet64
        } catch { continue } # A child may exit during this sample.
    }
    # Read after sampling: discard intervals crossing native teardown rather
    # than misreporting destruction CPU and disappearing views as idle savings.
    $idle = [bool](Select-String -LiteralPath $stdout -SimpleMatch 'PROTECTION_IDLE_READY' -Quiet) -and
        -not [bool](Select-String -LiteralPath $stdout -SimpleMatch 'PROTECTION_IDLE_COMPLETE' -Quiet)
    [pscustomobject]@{
        elapsedSeconds = $at
        intervalSeconds = $at - $lastAt
        idleInterval = $idle -and $lastIdle
        processCount = $current.Count
        privateMiB = $private / 1MB
        workingSetMiB = $working / 1MB
        cpuSeconds = $cpuDelta
    } | Export-Csv -LiteralPath (Join-Path $output.FullName 'samples.csv') -NoTypeInformation -Append
    $previous = $current
    $lastAt = $at
    $lastIdle = $idle
    Start-Sleep -Milliseconds 1000
    $process.Refresh()
}
$process.WaitForExit()
if ($process.ExitCode -ne 0) {
    Get-Content -LiteralPath $stdout,$stderr -Tail 15
    throw "Native qualification failed: $($process.ExitCode)"
}
$resultLine = Get-Content -LiteralPath $stdout | Where-Object { $_ -match 'PROTECTION_RESULT ' } | Select-Object -Last 1
if ($null -eq $resultLine) { throw 'No qualification result was emitted.' }
$result = ($resultLine -replace '^.*PROTECTION_RESULT ', '') | ConvertFrom-Json
$result | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath (Join-Path $output.FullName 'result.json')
Write-Output $output.FullName

param(
    [Parameter(Mandatory=$true)][int]$RootProcessId,
    [ValidateRange(5,600)][int]$Seconds = 60,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference='Stop'
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a new evidence directory.' }
$output=New-Item -ItemType Directory -Path $OutputDirectory
$rootProcess=Get-Process -Id $RootProcessId
$rootBirth=$rootProcess.StartTime
$binaryPath=$rootProcess.Path
[ordered]@{
    binarySha256=(Get-FileHash -LiteralPath $binaryPath).Hash
    startedUtc=[DateTime]::UtcNow.ToString('o')
    sourceRevision=(& git rev-parse HEAD)
    sourceDirty=[bool](& git status --porcelain)
    rootProcessId=$RootProcessId
    cpu=(Get-CimInstance Win32_Processor | Select-Object Name,NumberOfCores,NumberOfLogicalProcessors)
    os=(Get-CimInstance Win32_OperatingSystem | Select-Object Caption,BuildNumber,TotalVisibleMemorySize)
    powerScheme=(& powercfg /getactivescheme)
    caveats=@('Full product process family, attached after launch; does not measure startup.', 'Sampled CPU lower bound; processes exiting between samples may be missed.', 'Private bytes are committed process memory; working sets double-count shared pages.', 'No wakeup or energy measurement. First CPU interval excluded.')
} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $output.FullName 'metadata.json')
$clock=[Diagnostics.Stopwatch]::StartNew()
$previous=@{}
$lastAt=0.0
$rows=[Collections.Generic.List[object]]::new()
while ($clock.Elapsed.TotalSeconds -lt $Seconds) {
    $rootProcess.Refresh()
    if ($rootProcess.HasExited -or $rootProcess.StartTime -ne $rootBirth) { throw 'Measured process exited.' }
    $inventory=@(Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,CreationDate)
    $births=@{}
    foreach($entry in $inventory) { $births[[int]$entry.ProcessId]=$entry.CreationDate }
    $ids=[Collections.Generic.HashSet[int]]::new()
    [void]$ids.Add($RootProcessId)
    do {
        $added=$false
        foreach($entry in $inventory) {
            $parent=[int]$entry.ParentProcessId
            if($ids.Contains($parent) -and $null -ne $entry.CreationDate -and $null -ne $births[$parent] -and $entry.CreationDate -ge $births[$parent] -and $ids.Add([int]$entry.ProcessId)) { $added=$true }
        }
    } while($added)
    $current=@{}; $cpuDelta=0.0; $private=0L; $working=0L
    foreach($processId in $ids) {
        $child=Get-Process -Id $processId -ErrorAction SilentlyContinue
        if($null -eq $child) {continue}
        try {
            # CIM truncates process creation to microseconds; Process.StartTime
            # retains 100 ns ticks. Compare at the inventory precision.
            if([Math]::Abs(($child.StartTime - $births[$processId]).Ticks) -ge 10) {continue}
            $key="$processId/$($child.StartTime.Ticks)"
            $cpu=$child.TotalProcessorTime.TotalSeconds
            if($previous.ContainsKey($key)) { $cpuDelta += [Math]::Max(0.0,$cpu-$previous[$key]) }
            elseif($rows.Count -gt 0) { $cpuDelta += $cpu }
            $current[$key]=$cpu
            $private += $child.PrivateMemorySize64
            $working += $child.WorkingSet64
        } catch {continue}
    }
    $at=$clock.Elapsed.TotalSeconds
    $rows.Add([pscustomobject]@{elapsedSeconds=$at; intervalSeconds=$at-$lastAt; processCount=$current.Count; privateMiB=$private/1MB; workingSetMiB=$working/1MB; cpuSeconds=$cpuDelta})
    $previous=$current; $lastAt=$at
    Start-Sleep -Milliseconds 1000
}
$rows | Export-Csv -LiteralPath (Join-Path $output.FullName 'samples.csv') -NoTypeInformation
$privateValues=@($rows.privateMiB | Sort-Object)
$summary=[ordered]@{samples=$rows.Count; durationSeconds=$lastAt-$rows[0].elapsedSeconds; medianPrivateMiB=$privateValues[[int][Math]::Floor($privateValues.Count/2)]; peakPrivateMiB=($rows.privateMiB | Measure-Object -Maximum).Maximum; cpuSeconds=($rows.cpuSeconds | Measure-Object -Sum).Sum; processCount=$rows[$rows.Count-1].processCount}
$summary | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $output.FullName 'summary.json')
$summary | ConvertTo-Json

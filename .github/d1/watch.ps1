# D1 DIAGNOSTIC (throwaway branch only): polls the explicit AppContainer entries
# of the directory and the AppContainer entries (inherited) of the probe tool
# and logs every change with a UTC timestamp, so that a start failure can be
# compared with what the files carried at that moment.
param(
    [string]$Dir,
    [string]$Exe,
    [string]$Out,
    [string]$Stop,
    [int]$PollMs = 40
)
$ErrorActionPreference = 'Continue'

function Stamp { [DateTime]::UtcNow.ToString('HH:mm:ss.fff') }

function Sids([string]$path, [bool]$explicitOnly) {
    try {
        $item = Get-Item -LiteralPath $path -Force
        $acl = [System.IO.FileSystemAclExtensions]::GetAccessControl($item)
        $rules = $acl.GetAccessRules($true, -not $explicitOnly, [System.Security.Principal.SecurityIdentifier])
        $set = @()
        foreach ($r in $rules) {
            $v = $r.IdentityReference.Value
            if ($v -like 'S-1-15-2-*') { $set += $v }
        }
        return ,@($set | Sort-Object -Unique)
    } catch {
        return $null
    }
}

$prevDir = $null
$prevExe = $null
$errCount = 0
"$(Stamp) watcher start dir=$Dir exe=$Exe" | Add-Content -Path $Out
while (-not (Test-Path -LiteralPath $Stop)) {
    $d = Sids $Dir $true
    $e = Sids $Exe $false
    if ($null -eq $d -or $null -eq $e) {
        $errCount++
        if ($errCount -le 20) { "$(Stamp) read error dir=$($null -eq $d) exe=$($null -eq $e)" | Add-Content -Path $Out }
    } else {
        if ($null -ne $prevDir) {
            $added = @($d | Where-Object { $prevDir -notcontains $_ })
            $removed = @($prevDir | Where-Object { $d -notcontains $_ })
            if ($added.Count -or $removed.Count) {
                "$(Stamp) DIR  count=$($d.Count) +$($added.Count) -$($removed.Count) added=[$($added -join ',')] removed=[$($removed -join ',')]" | Add-Content -Path $Out
            }
        } else {
            "$(Stamp) DIR  initial count=$($d.Count)" | Add-Content -Path $Out
        }
        if ($null -ne $prevExe) {
            $added = @($e | Where-Object { $prevExe -notcontains $_ })
            $removed = @($prevExe | Where-Object { $e -notcontains $_ })
            if ($added.Count -or $removed.Count) {
                $missing = @($d | Where-Object { $e -notcontains $_ })
                "$(Stamp) EXE  count=$($e.Count) +$($added.Count) -$($removed.Count) dir_entries_missing_on_exe=$($missing.Count) added=[$($added -join ',')] removed=[$($removed -join ',')]" | Add-Content -Path $Out
            }
        } else {
            "$(Stamp) EXE  initial count=$($e.Count)" | Add-Content -Path $Out
        }
        $prevDir = $d
        $prevExe = $e
    }
    Start-Sleep -Milliseconds $PollMs
}
"$(Stamp) watcher stop" | Add-Content -Path $Out

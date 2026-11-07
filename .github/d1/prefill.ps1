# D1 DIAGNOSTIC (throwaway branch only): adds N fake AppContainer entries to a
# directory in ONE write (inheritable), to test whether a large DACL matters.
param([string]$Dir, [int]$Count = 400)
$acl = Get-Acl -LiteralPath $Dir
for ($i = 1; $i -le $Count; $i++) {
    $sid = New-Object System.Security.Principal.SecurityIdentifier("S-1-15-2-1-1-1-1-1-1-$i")
    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule($sid, 'ReadAndExecute', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
    $acl.AddAccessRule($rule)
}
$sw = [Diagnostics.Stopwatch]::StartNew()
Set-Acl -LiteralPath $Dir -AclObject $acl
"prefill: $Count entries written to $Dir in $($sw.ElapsedMilliseconds) ms"

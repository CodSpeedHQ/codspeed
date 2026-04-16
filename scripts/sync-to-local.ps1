# Syncs the repo from the shared folder (Z:\) to a local working copy.
#
# Usage: powershell -ExecutionPolicy Bypass -File Z:\scripts\sync-to-local.ps1

$ErrorActionPreference = "Stop"

$syncs = @(
    @{ SRC = "Z:\codspeed";      DST = "C:\Users\vboxuser\codspeed" },
    @{ SRC = "Z:\codspeed-rust";  DST = "C:\Users\vboxuser\codspeed-rust" }
)

foreach ($sync in $syncs) {
    # /MIR = mirror (copy + delete extras), /XD = exclude dirs, /MT = multi-threaded
    robocopy $sync.SRC $sync.DST /MIR /MT /XD .git target target_win node_modules /NFL /NDL /NJH /NJS /NC /NS
    Write-Host "Synced $($sync.SRC) -> $($sync.DST)"
}

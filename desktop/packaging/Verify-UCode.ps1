param(
  [string]$Root = $PSScriptRoot
)

$ErrorActionPreference = "Stop"
$capsuleRoot = [System.IO.Path]::GetFullPath($Root)
$manifestPath = Join-Path $capsuleRoot "ucode-manifest.json"
if (!(Test-Path -LiteralPath $manifestPath -PathType Leaf)) { throw "UCode manifest is missing: $manifestPath" }
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.format -ne 2 -or $manifest.integrity.algorithm -ne "sha256") { throw "Unsupported UCode manifest format" }

$failures = [System.Collections.Generic.List[object]]::new()
$verifiedBytes = [int64]0
foreach ($entry in $manifest.integrity.files) {
  $relative = [string]$entry.path
  if (!$relative -or [System.IO.Path]::IsPathRooted($relative) -or $relative.Split('/') -contains '..') {
    $failures.Add([ordered]@{ path = $relative; error = "unsafe_path" })
    continue
  }
  $candidate = [System.IO.Path]::GetFullPath((Join-Path $capsuleRoot ($relative.Replace('/', '\'))))
  if (!$candidate.StartsWith($capsuleRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    $failures.Add([ordered]@{ path = $relative; error = "path_escape" })
    continue
  }
  if (!(Test-Path -LiteralPath $candidate -PathType Leaf)) {
    $failures.Add([ordered]@{ path = $relative; error = "missing" })
    continue
  }
  $file = Get-Item -LiteralPath $candidate
  if ([int64]$file.Length -ne [int64]$entry.size) {
    $failures.Add([ordered]@{ path = $relative; error = "size_mismatch"; expected = [int64]$entry.size; actual = [int64]$file.Length })
    continue
  }
  $digest = (Get-FileHash -LiteralPath $candidate -Algorithm SHA256).Hash.ToLowerInvariant()
  if ($digest -ne [string]$entry.sha256) {
    $failures.Add([ordered]@{ path = $relative; error = "digest_mismatch" })
    continue
  }
  $verifiedBytes += $file.Length
}

$result = [ordered]@{
  product = $manifest.product
  version = $manifest.version
  verified = $failures.Count -eq 0
  files = @($manifest.integrity.files).Count
  bytes = $verifiedBytes
  failures = $failures
}
$result | ConvertTo-Json -Depth 6
if ($failures.Count -gt 0) { exit 1 }

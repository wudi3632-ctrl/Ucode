param(
  [string]$Destination = "",
  [switch]$SkipRuntime
)

$ErrorActionPreference = "Stop"
$desktopRoot = Split-Path $PSScriptRoot -Parent
$pocketmindRoot = Split-Path $desktopRoot -Parent
if ([string]::IsNullOrWhiteSpace($Destination)) {
  $Destination = Join-Path $pocketmindRoot "build\UCode-Capsule-Candidate"
}
$sourceRoot = [System.IO.Path]::GetFullPath((Join-Path $pocketmindRoot "..\.."))
$sourceExe = Join-Path $desktopRoot "src-tauri\target\release\ucode-portable.exe"
$sourceIcon = Join-Path $desktopRoot "src-tauri\icons\icon.png"
$readme = Join-Path $desktopRoot "packaging\README-原型.md"
$verifier = Join-Path $desktopRoot "packaging\Verify-UCode.ps1"

foreach ($required in @($sourceExe, $sourceIcon, $readme, $verifier, (Join-Path $sourceRoot "Start-PocketMind-Research.ps1"), (Join-Path $sourceRoot "Stop-PocketMind.ps1"), (Join-Path $sourceRoot "Restart-PocketMind-Controller.ps1"), (Join-Path $sourceRoot "Stop-PocketMind-Controller.ps1"))) {
  if (!(Test-Path -LiteralPath $required -PathType Leaf)) { throw "Required release input is missing: $required" }
}
$destinationPath = [System.IO.Path]::GetFullPath($Destination)
if ($destinationPath -eq [System.IO.Path]::GetPathRoot($destinationPath)) { throw "Refusing to stage at a drive root" }
$runtimePack = Join-Path $destinationPath "runtime-pack"

function Copy-Directory([string]$Source, [string]$Target) {
  if (!(Test-Path -LiteralPath $Source -PathType Container)) { throw "Required source directory is missing: $Source" }
  New-Item -ItemType Directory -Force -Path $Target | Out-Null
  & robocopy.exe $Source $Target /E /R:1 /W:1 /NFL /NDL /NJH /NJS /NP /XD __pycache__ .pytest_cache | Out-Null
  if ($LASTEXITCODE -ge 8) { throw "Failed to copy $Source to $Target (robocopy $LASTEXITCODE)" }
}

function Copy-ReleaseFile([string]$Source, [string]$Target) {
  $parent = Split-Path $Target -Parent
  New-Item -ItemType Directory -Force -Path $parent | Out-Null
  Copy-Item -LiteralPath $Source -Destination $Target -Force
}

foreach ($directory in @("data\evidence", "data\webview", "model-packs", "skills", "workspaces", "worktrees", "logs", "licenses")) {
  New-Item -ItemType Directory -Force -Path (Join-Path $destinationPath $directory) | Out-Null
}

# Remove only the obsolete generated built-in profile from the first model-pack
# prototype. User-supplied model packs are never cleaned by staging.
$legacyProfile = [System.IO.Path]::GetFullPath((Join-Path $destinationPath "model-packs\qwen2.5-coder-1.5b-q4-k-m"))
$modelPacksBoundary = [System.IO.Path]::GetFullPath((Join-Path $destinationPath "model-packs")).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
if ($legacyProfile.StartsWith($modelPacksBoundary, [StringComparison]::OrdinalIgnoreCase) -and (Test-Path -LiteralPath $legacyProfile -PathType Container)) {
  $legacyManifestPath = Join-Path $legacyProfile "manifest.json"
  $legacyManifest = if (Test-Path -LiteralPath $legacyManifestPath -PathType Leaf) { Get-Content -LiteralPath $legacyManifestPath -Raw | ConvertFrom-Json } else { $null }
  if ($legacyManifest -and [string]$legacyManifest.id -ceq "qwen2.5-coder-1.5b-q4-k-m" -and [string]$legacyManifest.ggufRelativePath -ceq "runtime-pack/coder/model-gguf/qwen2.5-coder-1.5b-q4_k_m.gguf") {
    Remove-Item -LiteralPath $legacyProfile -Recurse -Force
  }
}

Copy-ReleaseFile $sourceExe (Join-Path $destinationPath "UCode.exe")
Copy-ReleaseFile $sourceIcon (Join-Path $destinationPath "UCode.png")
Copy-ReleaseFile $readme (Join-Path $destinationPath "README-原型.md")
Copy-ReleaseFile $verifier (Join-Path $destinationPath "Verify-UCode.ps1")

if (!$SkipRuntime) {
  Copy-Directory (Join-Path $sourceRoot "runtime\node") (Join-Path $runtimePack "runtime\node")
  Copy-Directory (Join-Path $sourceRoot "runtime\python") (Join-Path $runtimePack "runtime\python")
  Copy-Directory (Join-Path $sourceRoot "runtime\llama\b12211") (Join-Path $runtimePack "runtime\llama\b12211")
  Copy-Directory (Join-Path $pocketmindRoot "vendor\minimind\model") (Join-Path $runtimePack "app\pocketmind\vendor\minimind\model")
  Copy-Directory (Join-Path $sourceRoot "model\controller") (Join-Path $runtimePack "model\controller")
  Copy-ReleaseFile (Join-Path $sourceRoot "coder\model\candidate.json") (Join-Path $runtimePack "coder\model\candidate.json")
  Copy-ReleaseFile (Join-Path $sourceRoot "coder\model-gguf\qwen2.5-coder-1.5b-q4_k_m.gguf") (Join-Path $runtimePack "coder\model-gguf\qwen2.5-coder-1.5b-q4_k_m.gguf")
}

# Application code and startup policy are small release inputs and must refresh
# even when a previously staged immutable runtime is reused with -SkipRuntime.
# Otherwise a new desktop can silently call an older Agent protocol from the
# previous capsule, which invalidates both behavior and the integrity manifest.
Copy-Directory (Join-Path $pocketmindRoot "agent") (Join-Path $runtimePack "app\pocketmind\agent")
Copy-Directory (Join-Path $pocketmindRoot "config") (Join-Path $runtimePack "app\pocketmind\config")
Copy-ReleaseFile (Join-Path $sourceRoot "Start-PocketMind-Research.ps1") (Join-Path $runtimePack "Start-PocketMind-Research.ps1")
Copy-ReleaseFile (Join-Path $sourceRoot "Stop-PocketMind.ps1") (Join-Path $runtimePack "Stop-PocketMind.ps1")
Copy-ReleaseFile (Join-Path $sourceRoot "Restart-PocketMind-Controller.ps1") (Join-Path $runtimePack "Restart-PocketMind-Controller.ps1")
Copy-ReleaseFile (Join-Path $sourceRoot "Stop-PocketMind-Controller.ps1") (Join-Path $runtimePack "Stop-PocketMind-Controller.ps1")
Copy-ReleaseFile (Join-Path $pocketmindRoot "minimind\runtime\server.py") (Join-Path $runtimePack "app\pocketmind\minimind\runtime\server.py")
Copy-ReleaseFile (Join-Path $pocketmindRoot "minimind\eval\coder_server.py") (Join-Path $runtimePack "app\pocketmind\minimind\eval\coder_server.py")
Copy-ReleaseFile (Join-Path $pocketmindRoot "minimind\eval\llama_coder_adapter.py") (Join-Path $runtimePack "app\pocketmind\minimind\eval\llama_coder_adapter.py")

$builtInModel = Join-Path $runtimePack "coder\model-gguf\qwen2.5-coder-1.5b-q4_k_m.gguf"
if (Test-Path -LiteralPath $builtInModel -PathType Leaf) {
  $builtInProfileId = "qwen2-5-coder-1-5b-q4-k-m"
  $builtInProfileDir = Join-Path $destinationPath ("model-packs\" + $builtInProfileId)
  New-Item -ItemType Directory -Force -Path $builtInProfileDir | Out-Null
  $builtInModelFile = Get-Item -LiteralPath $builtInModel
  [ordered]@{
    format = 1
    id = $builtInProfileId
    name = "Qwen2.5-Coder 1.5B Q4_K_M"
    runtime = "llama.cpp"
    ggufRelativePath = "runtime-pack/coder/model-gguf/qwen2.5-coder-1.5b-q4_k_m.gguf"
    bytes = [int64]$builtInModelFile.Length
    sha256 = (Get-FileHash -LiteralPath $builtInModel -Algorithm SHA256).Hash.ToLowerInvariant()
    parameterClass = "1.5B"
    contextSize = 4096
    minimumRamBytes = [int64]2GB
    recommendedRamBytes = [int64]4GB
    taskSuitability = @("日常对话", "代码解释", "明确的小型单文件修改")
    releaseEligible = $false
  } | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $builtInProfileDir "manifest.json") -Encoding UTF8
}

if (Test-Path -LiteralPath $runtimePack -PathType Container) {
  foreach ($mutableName in @("data", "logs", "workspaces", "worktrees")) {
    $mutablePath = [System.IO.Path]::GetFullPath((Join-Path $runtimePack $mutableName))
    if (Test-Path -LiteralPath $mutablePath) {
      if (!$mutablePath.StartsWith($runtimePack + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to clean mutable path outside runtime-pack: $mutablePath" }
      Remove-Item -LiteralPath $mutablePath -Recurse -Force
    }
  }
  foreach ($cache in @(Get-ChildItem -LiteralPath $runtimePack -Recurse -Directory -Filter "__pycache__" -ErrorAction SilentlyContinue)) {
    $cachePath = [System.IO.Path]::GetFullPath($cache.FullName)
    if (!$cachePath.StartsWith($runtimePack + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to clean cache outside runtime-pack: $cachePath" }
    Remove-Item -LiteralPath $cachePath -Recurse -Force
  }
  Get-ChildItem -LiteralPath $runtimePack -Recurse -File -Filter "*.pyc" -ErrorAction SilentlyContinue | Remove-Item -Force
}

$immutable = @(
  Get-Item -LiteralPath (Join-Path $destinationPath "UCode.exe"), (Join-Path $destinationPath "UCode.png"), (Join-Path $destinationPath "Verify-UCode.ps1")
  Get-ChildItem -LiteralPath $runtimePack -Recurse -File
  Get-ChildItem -LiteralPath (Join-Path $destinationPath "model-packs") -Recurse -File
) | Sort-Object FullName
$integrityFiles = foreach ($file in $immutable) {
  $relative = $file.FullName.Substring($destinationPath.Length + 1).Replace('\', '/')
  $role = if ($relative -eq "UCode.exe" -or $relative -eq "UCode.png") { "application" } elseif ($relative -like "runtime-pack/model/*" -or $relative -like "runtime-pack/coder/model-gguf/*") { "model" } elseif ($relative -like "runtime-pack/runtime/*") { "runtime" } else { "code" }
  [ordered]@{
    path = $relative
    role = $role
    size = [int64]$file.Length
    sha256 = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
  }
}
$totalBytes = [int64](($integrityFiles | ForEach-Object { [int64]$_['size'] } | Measure-Object -Sum).Sum)
if ($totalBytes -gt 3GB) { throw "CPU capsule exceeds 3 GiB: $totalBytes bytes" }

$manifest = [ordered]@{
  format = 2
  product = "UCode Portable"
  version = "0.3.0-capsule"
  status = "capsule-candidate"
  release_eligible = $false
  architecture = "tauri2-rust-react-superagent"
  portable = $true
  runtime_layout = "embedded-runtime-pack"
  generated_at = (Get-Date).ToUniversalTime().ToString("o")
  integrity = [ordered]@{
    algorithm = "sha256"
    total_bytes = $totalBytes
    files = @($integrityFiles)
  }
}
$manifest | ConvertTo-Json -Depth 7 | Set-Content -LiteralPath (Join-Path $destinationPath "ucode-manifest.json") -Encoding UTF8

# A cold first launch must not spend tens of seconds hashing a multi-gigabyte
# capsule behind an apparently blank native window. Prime the exact same
# manifest + metadata-bound cache that the app validates on every startup.
# This moves the one-time full verification into the explicit staging step;
# changing any immutable file afterwards invalidates the cache as before.
$primeProcess = Start-Process -FilePath (Join-Path $destinationPath "UCode.exe") -ArgumentList "--prime-integrity-cache" -WorkingDirectory $destinationPath -WindowStyle Hidden -Wait -PassThru
if ($primeProcess.ExitCode -ne 0) { throw "Failed to prime the portable integrity cache (exit $($primeProcess.ExitCode))" }
$integrityCachePath = Join-Path $destinationPath "data\integrity-cache.json"
if (!(Test-Path -LiteralPath $integrityCachePath -PathType Leaf)) {
  throw "Portable integrity cache was not created"
}
$primedCache = Get-Content -LiteralPath $integrityCachePath -Raw | ConvertFrom-Json
$manifestSha256 = (Get-FileHash -LiteralPath (Join-Path $destinationPath "ucode-manifest.json") -Algorithm SHA256).Hash.ToLowerInvariant()
if ([string]$primedCache.manifestSha256 -cne $manifestSha256) {
  throw "Portable integrity cache does not match the staged manifest"
}

[ordered]@{
  path = $destinationPath
  files = @($integrityFiles).Count
  immutable_bytes = $totalBytes
  immutable_gib = [math]::Round($totalBytes / 1GB, 3)
  manifest = (Join-Path $destinationPath "ucode-manifest.json")
} | ConvertTo-Json

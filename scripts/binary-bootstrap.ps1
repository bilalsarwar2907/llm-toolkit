# binary-bootstrap.ps1
# Downloads and installs the pinned llama-server binary and its dependencies for Windows x64 CPU.

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# --- Pinned version ---
$LLAMA_VERSION   = "b10448"
$PLATFORM        = "win-cpu-x64"
$ARCHIVE_NAME    = "llama-$LLAMA_VERSION-bin-$PLATFORM.zip"
$DOWNLOAD_URL    = "https://github.com/ggml-org/llama.cpp/releases/download/$LLAMA_VERSION/$ARCHIVE_NAME"
$EXPECTED_SHA256 = $null  # Not published for this asset in release b10448

$INSTALL_DIR  = Join-Path $env:USERPROFILE ".llm-toolkit\bin"
$VERSION_LOCK = Join-Path $INSTALL_DIR "version.lock"
$BINARY_PATH  = Join-Path $INSTALL_DIR "llama-server.exe"
$IMPL_PATH    = Join-Path $INSTALL_DIR "llama-server-impl.dll"

# Files required for llama-server to operate
$REQUIRED_FILES = @(
    "llama-server.exe",
    "llama-server-impl.dll",
    "llama-common.dll",
    "llama-completion-impl.dll",
    "llama.dll",
    "ggml.dll",
    "ggml-base.dll",
    "ggml-rpc.dll",
    "mtmd.dll",
    "libomp140.x86_64.dll",
    "ggml-cpu-x64.dll",
    "ggml-cpu-sse42.dll",
    "ggml-cpu-sandybridge.dll",
    "ggml-cpu-ivybridge.dll",
    "ggml-cpu-haswell.dll",
    "ggml-cpu-piledriver.dll",
    "ggml-cpu-alderlake.dll",
    "ggml-cpu-skylakex.dll",
    "ggml-cpu-icelake.dll",
    "ggml-cpu-cascadelake.dll",
    "ggml-cpu-cooperlake.dll",
    "ggml-cpu-cannonlake.dll",
    "ggml-cpu-sapphirerapids.dll",
    "ggml-cpu-zen4.dll"
)

# --- Check if already installed at correct version ---
if ((Test-Path $VERSION_LOCK) -and (Test-Path $BINARY_PATH) -and (Test-Path $IMPL_PATH)) {
    $installed = (Get-Content $VERSION_LOCK -Raw).Trim()
    if ($installed -eq $LLAMA_VERSION) {
        Write-Host "[OK] llama-server $LLAMA_VERSION already installed at $INSTALL_DIR"
        exit 0
    }
    Write-Host "[INFO] Installed version ($installed) differs from pinned ($LLAMA_VERSION). Updating."
}

# --- Create install directory ---
New-Item -ItemType Directory -Force -Path $INSTALL_DIR | Out-Null

# --- Download ---
$tempDir     = [System.IO.Path]::GetTempPath()
$archivePath = Join-Path $tempDir $ARCHIVE_NAME

Write-Host "[INFO] Downloading $ARCHIVE_NAME..."
Invoke-WebRequest -Uri $DOWNLOAD_URL -OutFile $archivePath -UseBasicParsing
Write-Host "[OK] Download complete."

# --- Tier 1: SHA256 verification ---
if ($null -ne $EXPECTED_SHA256) {
    Write-Host "[INFO] Verifying SHA256..."
    $actual = (Get-FileHash -Algorithm SHA256 -Path $archivePath).Hash.ToLower()
    if ($actual -ne $EXPECTED_SHA256.ToLower()) {
        Write-Error "[FAIL] SHA256 mismatch. Expected: $EXPECTED_SHA256 Got: $actual"
        exit 1
    }
    Write-Host "[OK] SHA256 verified."
} else {
    Write-Host "[WARN] SHA256 not published for this asset in release $LLAMA_VERSION. Structural check only."
}

# --- Tier 3: Archive size sanity check ---
$archiveSize = (Get-Item $archivePath).Length
if ($archiveSize -lt 10MB) {
    Write-Error "[FAIL] Archive is suspiciously small ($archiveSize bytes). Aborting."
    exit 1
}
Write-Host "[OK] Archive size check passed ($([math]::Round($archiveSize / 1MB, 1)) MB)."

# --- Extract required files ---
Write-Host "[INFO] Extracting required files..."
Add-Type -AssemblyName System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::OpenRead($archivePath)

foreach ($fileName in $REQUIRED_FILES) {
    $entry = $zip.Entries | Where-Object { $_.Name -eq $fileName } | Select-Object -First 1
    if ($null -eq $entry) {
        $zip.Dispose()
        Write-Error "[FAIL] Required file not found in archive: $fileName"
        exit 1
    }
    $destPath = Join-Path $INSTALL_DIR $fileName
    [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $destPath, $true)
    Write-Host "  Extracted: $fileName ($([math]::Round($entry.Length / 1KB, 0)) KB)"
}

$zip.Dispose()
Write-Host "[OK] All required files extracted."

# --- Tier 3: Implementation DLL size sanity check ---
$implSize = (Get-Item $IMPL_PATH).Length
if ($implSize -lt 1MB) {
    Write-Error "[FAIL] llama-server-impl.dll is suspiciously small ($implSize bytes)."
    exit 1
}
Write-Host "[OK] llama-server-impl.dll size check passed ($([math]::Round($implSize / 1MB, 1)) MB)."

# --- Write version lock ---
Set-Content -Path $VERSION_LOCK -Value $LLAMA_VERSION
Write-Host "[OK] Version lock written: $LLAMA_VERSION"

# --- Cleanup ---
Remove-Item $archivePath -Force
Write-Host "[OK] Temp archive removed."

Write-Host ""
Write-Host "llama-server $LLAMA_VERSION installed successfully."
Write-Host "Install path: $INSTALL_DIR"
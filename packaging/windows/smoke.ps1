[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $ReleaseDirectory,

    [Parameter(Mandatory = $true)]
    [string] $Version,

    [switch] $RequireAuthenticode,

    [string] $PublisherSubject
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    throw "version must use MAJOR.MINOR.PATCH"
}

$releasePath = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
$archivePath = Join-Path $releasePath "heyfood-v$Version-x86_64-pc-windows-msvc.zip"
if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) {
    throw "Windows release archive is missing"
}

$staging = Join-Path ([System.IO.Path]::GetTempPath()) "heyfood-smoke-$([System.Guid]::NewGuid().ToString('N'))"
[System.IO.Directory]::CreateDirectory($staging) | Out-Null
try {
    [System.IO.Compression.ZipFile]::ExtractToDirectory($archivePath, $staging)
    $files = @(Get-ChildItem -LiteralPath $staging -File -Recurse)
    if ($files.Count -ne 1 -or $files[0].Name -ne "heyfood.exe") {
        throw "Windows release archive must install exactly heyfood.exe"
    }
    $binary = $files[0].FullName
    if ($RequireAuthenticode) {
        if ([string]::IsNullOrWhiteSpace($PublisherSubject)) {
            throw "expected Authenticode publisher subject is required"
        }
        $signature = Get-AuthenticodeSignature -LiteralPath $binary
        if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
            $signature.SignerCertificate.Subject -ne $PublisherSubject -or
            $null -eq $signature.TimeStamperCertificate) {
            throw "installed Windows executable is not signed and timestamped by the expected publisher"
        }
    }
    $observedVersion = (& $binary --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $observedVersion -ne "heyfood $Version") {
        throw "installed Windows executable returned an unexpected version"
    }
    & $binary --help | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "installed Windows executable help failed"
    }
    & $binary register --help | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "installed Windows registration help failed"
    }
    $completion = (& $binary completion power-shell | Out-String)
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($completion)) {
        throw "installed Windows PowerShell completion failed"
    }
}
finally {
    if (Test-Path -LiteralPath $staging) {
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
}

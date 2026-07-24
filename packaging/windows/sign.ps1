[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Binary,

    [Parameter(Mandatory = $true)]
    [string] $CertificateBase64,

    [Parameter(Mandatory = $true)]
    [string] $CertificatePassword,

    [Parameter(Mandatory = $true)]
    [string] $PublisherSubject,

    [Parameter(Mandatory = $true)]
    [string] $TimestampUrl
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
if ([string]::IsNullOrWhiteSpace($CertificateBase64) -or
    [string]::IsNullOrWhiteSpace($CertificatePassword) -or
    [string]::IsNullOrWhiteSpace($PublisherSubject) -or
    [string]::IsNullOrWhiteSpace($TimestampUrl)) {
    throw "Windows release-signing inputs must all be configured"
}

$timestamp = [System.Uri] $TimestampUrl
if ($timestamp.Scheme -notin @("http", "https") -or -not $timestamp.IsAbsoluteUri) {
    throw "Windows timestamp URL must be absolute HTTP(S)"
}

$signTool = Get-ChildItem `
    -LiteralPath "${env:ProgramFiles(x86)}\Windows Kits\10\bin" `
    -Filter signtool.exe `
    -File `
    -Recurse |
    Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
    Sort-Object -Property FullName -Descending |
    Select-Object -First 1
if ($null -eq $signTool) {
    throw "signtool.exe was not found in the Windows SDK"
}

$pfxPath = Join-Path $env:RUNNER_TEMP "heyfood-codesign-$([System.Guid]::NewGuid().ToString('N')).pfx"
$certificate = $null
$pfxThumbprints = @()
$preexistingThumbprints = [System.Collections.Generic.HashSet[string]]::new(
    [System.StringComparer]::OrdinalIgnoreCase
)
Get-ChildItem -LiteralPath Cert:\CurrentUser\My |
    ForEach-Object { [void] $preexistingThumbprints.Add($_.Thumbprint) }
try {
    [System.IO.File]::WriteAllBytes(
        $pfxPath,
        [System.Convert]::FromBase64String($CertificateBase64)
    )
    $password = ConvertTo-SecureString $CertificatePassword -AsPlainText -Force
    $pfxData = Get-PfxData -FilePath $pfxPath -Password $password
    $pfxThumbprints = @(
        @($pfxData.EndEntityCertificates) + @($pfxData.OtherCertificates) |
            ForEach-Object { $_.Thumbprint } |
            Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
            Sort-Object -Unique
    )
    if ($pfxThumbprints.Count -eq 0) {
        throw "PFX does not contain any certificates"
    }
    $collision = $pfxThumbprints |
        Where-Object { $preexistingThumbprints.Contains($_) } |
        Select-Object -First 1
    if ($null -ne $collision) {
        throw "PFX certificate already exists in the signing account certificate store"
    }

    $imported = @(
        Import-PfxCertificate `
            -FilePath $pfxPath `
            -CertStoreLocation Cert:\CurrentUser\My `
            -Password $password `
            -Exportable:$false
    )
    $certificate = $imported |
        Where-Object {
            $_.HasPrivateKey -and
            $_.EnhancedKeyUsageList.ObjectId.Value -contains "1.3.6.1.5.5.7.3.3"
        } |
        Select-Object -First 1
    if ($null -eq $certificate) {
        throw "PFX does not contain a code-signing identity with a private key"
    }
    if ($certificate.Subject -ne $PublisherSubject) {
        throw "code-signing certificate subject does not match the protected release identity"
    }

    & $signTool.FullName sign `
        /sha1 $certificate.Thumbprint `
        /s My `
        /fd SHA256 `
        /tr $timestamp.AbsoluteUri `
        /td SHA256 `
        /d "heyfood native CLI" `
        $binaryPath
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode signing failed"
    }

    & $signTool.FullName verify /pa /all /tw $binaryPath
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode verification failed"
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $binaryPath
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
        $signature.SignerCertificate.Subject -ne $PublisherSubject -or
        $null -eq $signature.TimeStamperCertificate) {
        throw "signed executable did not retain the expected trusted and timestamped identity"
    }
}
finally {
    foreach ($thumbprint in $pfxThumbprints) {
        if ($preexistingThumbprints.Contains($thumbprint)) {
            continue
        }
        $certificatePath = "Cert:\CurrentUser\My\$thumbprint"
        if (Test-Path -LiteralPath $certificatePath) {
            $importedCertificate = Get-Item -LiteralPath $certificatePath
            if ($importedCertificate.HasPrivateKey) {
                Remove-Item -LiteralPath $certificatePath -DeleteKey -Force
            }
            else {
                Remove-Item -LiteralPath $certificatePath -Force
            }
        }
    }
    if (Test-Path -LiteralPath $pfxPath) {
        Remove-Item -LiteralPath $pfxPath -Force
    }
}

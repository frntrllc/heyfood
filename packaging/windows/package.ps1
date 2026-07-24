[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Binary,

    [Parameter(Mandatory = $true)]
    [string] $Version,

    [Parameter(Mandatory = $true)]
    [string] $Target,

    [Parameter(Mandatory = $true)]
    [string] $OutputDirectory
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
    throw "version must use MAJOR.MINOR.PATCH"
}
if ($Target -ne "x86_64-pc-windows-msvc") {
    throw "unsupported Windows release target: $Target"
}

$binaryPath = (Resolve-Path -LiteralPath $Binary).Path
if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
    throw "Windows executable does not exist: $Binary"
}

$outputPath = [System.IO.Path]::GetFullPath($OutputDirectory)
[System.IO.Directory]::CreateDirectory($outputPath) | Out-Null
$archivePath = Join-Path $outputPath "heyfood-v$Version-$Target.zip"
$temporaryPath = "$archivePath.$([System.Guid]::NewGuid().ToString('N')).tmp"

try {
    $archiveStream = [System.IO.File]::Open(
        $temporaryPath,
        [System.IO.FileMode]::CreateNew,
        [System.IO.FileAccess]::ReadWrite,
        [System.IO.FileShare]::None
    )
    try {
        $archive = [System.IO.Compression.ZipArchive]::new(
            $archiveStream,
            [System.IO.Compression.ZipArchiveMode]::Create,
            $true
        )
        try {
            $entry = $archive.CreateEntry(
                "heyfood.exe",
                [System.IO.Compression.CompressionLevel]::Optimal
            )
            $entry.LastWriteTime = [System.DateTimeOffset]::new(
                1980,
                1,
                1,
                0,
                0,
                0,
                [System.TimeSpan]::Zero
            )
            $entry.ExternalAttributes = 0
            $entryStream = $entry.Open()
            try {
                $sourceStream = [System.IO.File]::OpenRead($binaryPath)
                try {
                    $sourceStream.CopyTo($entryStream)
                }
                finally {
                    $sourceStream.Dispose()
                }
            }
            finally {
                $entryStream.Dispose()
            }
        }
        finally {
            $archive.Dispose()
        }
    }
    finally {
        $archiveStream.Dispose()
    }

    [System.IO.File]::Move($temporaryPath, $archivePath, $true)
}
finally {
    if (Test-Path -LiteralPath $temporaryPath) {
        Remove-Item -LiteralPath $temporaryPath -Force
    }
}

$verificationStream = [System.IO.File]::OpenRead($archivePath)
try {
    $verificationArchive = [System.IO.Compression.ZipArchive]::new(
        $verificationStream,
        [System.IO.Compression.ZipArchiveMode]::Read
    )
    try {
        if ($verificationArchive.Entries.Count -ne 1) {
            throw "Windows archive must contain exactly one entry"
        }
        $entry = $verificationArchive.Entries[0]
        if ($entry.FullName -ne "heyfood.exe") {
            throw "Windows archive entry must be heyfood.exe"
        }
        if ($entry.Length -ne (Get-Item -LiteralPath $binaryPath).Length) {
            throw "Windows archive executable length does not match the source"
        }
    }
    finally {
        $verificationArchive.Dispose()
    }
}
finally {
    $verificationStream.Dispose()
}

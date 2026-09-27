# Downloads a Foundry Local nightly runtime plus matching ONNX Runtime / GenAI into native\.
# Windows counterpart of fetch-native-nightly.sh; keep the versions in sync.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # the progress bar slows Invoke-WebRequest down a lot

$Foundry = '2.0.0-dev.202609240625'
$Ort = '1.30.0'
$GenAI = '0.16.0'
$Rid = if ($env:RID) { $env:RID } else { 'win-x64' }

$Dir = Join-Path (Split-Path $PSScriptRoot -Parent) 'native'
$NuGet = 'https://api.nuget.org/v3-flatcontainer'
$Nightly = 'https://pkgs.dev.azure.com/aiinfra/PublicPackages/_packaging/ORT-Nightly/nuget/v3/flat2'

Add-Type -AssemblyName System.IO.Compression.FileSystem
New-Item -ItemType Directory -Force $Dir | Out-Null

function Fetch($Feed, $Id, $Version) {
    $tmp = New-TemporaryFile
    Write-Host "Downloading $Id $Version ..."
    Invoke-WebRequest "$Feed/$Id/$Version/$Id.$Version.nupkg" -OutFile $tmp
    $zip = [IO.Compression.ZipFile]::OpenRead($tmp)
    try {
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName -like "runtimes/$Rid/native/*" -and $entry.Name) {
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $Dir $entry.Name), $true)
            }
        }
    } finally {
        $zip.Dispose()
        Remove-Item $tmp
    }
}

Fetch $Nightly 'microsoft.ai.foundry.local.runtime' $Foundry
Fetch $NuGet 'microsoft.ml.onnxruntime' $Ort
Fetch $NuGet 'microsoft.ml.onnxruntimegenai.foundry' $GenAI
Get-ChildItem $Dir

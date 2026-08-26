# **このファイルは UTF-8 BOM 付きで保存する。** BOM が無いと Windows
# PowerShell は ANSI(CP932) として読み、下の日本語コメントが壊れて構文
# エラーになる。エディタで保存し直すときは BOM を落とさないこと。
# Windows 配布物の zip を作る。build-release.sh から呼ばれる。
#
# **Compress-Archive も ZipFile::CreateFromDirectory も使わない。**
#   - Compress-Archive は区切りをバックスラッシュで書き、日本語名を
#     UTF-8 のフラグ無しで格納する（0.1.4 で「はじめに.txt」が化けた）
#   - CreateFromDirectory も、文字コードを明示すると言語エンコードフラグ
#     (EFS) を立てない。Windows PowerShell は .NET Framework で動くので
#     区切りもバックスラッシュになる（0.3.0 のビルドで再発した）
#
# 1 件ずつ、名前を自分で「/」区切りで書き、文字コードは既定に任せる。
# 既定は「ASCII 外を含む名前は UTF-8 で書き、EFS を立てる」なので、
# どの OS で展開しても「はじめに.txt」が読める。
param(
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$Destination
)
$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.IO.Compression.FileSystem

$source = (Resolve-Path -LiteralPath $SourceDir).Path
$name = Split-Path -Leaf $source
if (Test-Path -LiteralPath $Destination) { Remove-Item -LiteralPath $Destination -Force }

$zip = [System.IO.Compression.ZipFile]::Open($Destination, 'Create')
try {
    foreach ($file in Get-ChildItem -LiteralPath $source -Recurse -File) {
        $relative = $file.FullName.Substring($source.Length + 1).Replace([char]92, [char]47)
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $zip, $file.FullName, "$name/$relative", 'Optimal') | Out-Null
    }
} finally {
    $zip.Dispose()
}

# **作ったものを読み直して確かめる。** 名前の壊れは、作る側では気づけず、
# 受け取った人の画面で初めて出る。読み直しに失敗する形（EFS が立って
# いない）なら、ここで CP437 の化けた名前になるので気づける。
$check = [System.IO.Compression.ZipFile]::OpenRead($Destination)
try {
    $names = $check.Entries | ForEach-Object { $_.FullName }
} finally {
    $check.Dispose()
}
$bad = $names | Where-Object { $_ -like '*\*' }
if ($bad) { throw "zip の中でパス区切りがバックスラッシュになっている: $bad" }
if (-not ($names -contains "$name/はじめに.txt")) {
    throw "zip の中の日本語ファイル名が壊れている: $names"
}

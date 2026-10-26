# ==============================================================================
#  ⚡ unisoc-tool-rs Windows Installer & Updater (PowerShell)
#  https://github.com/Milordick/unisoc-tool-rs
# ==============================================================================

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$ErrorActionPreference = "Stop"
$Repo = "Milordick/unisoc-tool-rs"
$InstallDir = "$env:LOCALAPPDATA\unisoc-tool"

Write-Host "⚡ ========================================================= ⚡" -ForegroundColor Cyan
Write-Host "               UNISOC-TOOL-RS WINDOWS INSTALLER                 " -ForegroundColor White
Write-Host "          Spreadtrum / Unisoc Service Tool (Rust)               " -ForegroundColor Yellow
Write-Host "⚡ ========================================================= ⚡" -ForegroundColor Cyan
Write-Host ""

$Arch = "x86_64"
if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64" -or $env:PROCESSOR_ARCHITEW6432 -eq "ARM64") {
    $Arch = "arm64"
}

Write-Host "[1/4] Поиск последнего релиза на GitHub ($Repo) для Windows ($Arch)..." -ForegroundColor Cyan
$ApiUrl = "https://api.github.com/repos/$Repo/releases/latest"
$Release = $null

try {
    $Release = Invoke-RestMethod -Uri $ApiUrl -Headers @{ "User-Agent" = "unisoc-installer" }
} catch {
    Write-Warning "Релизы на GitHub пока не найдены или превышен лимит API."
}

$ZipUrl = $null
$TagName = "latest"

if ($Release -and $Release.assets) {
    $TagName = $Release.tag_name
    # Search for matching architecture (windows-arm64 or windows-x86_64)
    $TargetAsset = "windows-$Arch.zip"
    $Asset = $Release.assets | Where-Object { $_.name -like "*$TargetAsset*" } | Select-Object -First 1
    if (-not $Asset -and $Arch -eq "arm64") {
        Write-Host "Нативная сборка Windows ARM64 не найдена, используем x86_64 с эмуляцией..." -ForegroundColor Yellow
        $Asset = $Release.assets | Where-Object { $_.name -like "*windows-x86_64.zip*" } | Select-Object -First 1
    }
    if ($Asset) {
        $ZipUrl = $Asset.browser_download_url
    }
}

if (-not $ZipUrl) {
    $ZipUrl = "https://github.com/$Repo/releases/download/$TagName/unisoc-tool-windows-$Arch.zip"
}

$TempZip = "$env:TEMP\unisoc-tool-windows.zip"

Write-Host "[2/4] Скачивание сборки $TagName ($ZipUrl)..." -ForegroundColor Cyan
try {
    Invoke-WebRequest -Uri $ZipUrl -OutFile $TempZip -UseBasicParsing
} catch {
    Write-Error "Не удалось скачать архив релиза: $_. Скачайте релиз вручную с https://github.com/$Repo/releases"
    return
}

Write-Host "[3/4] Установка файлов в $InstallDir..." -ForegroundColor Cyan
if (Test-Path $InstallDir) {
    # Delete old files but keep config if any
    Get-ChildItem -Path $InstallDir -Recurse | Remove-Item -Force -Recurse -ErrorAction SilentlyContinue
} else {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

Expand-Archive -Path $TempZip -DestinationPath $InstallDir -Force
Remove-Item -Path $TempZip -Force -ErrorAction SilentlyContinue

# Ensure unisoc.exe is named properly
if (Test-Path "$InstallDir\unisoc_tool.exe" -and -not (Test-Path "$InstallDir\unisoc.exe")) {
    Copy-Item "$InstallDir\unisoc_tool.exe" "$InstallDir\unisoc.exe" -Force
}

Write-Host "[4/4] Добавление $InstallDir в системный PATH..." -ForegroundColor Cyan
$UserPath = [Environment]::GetEnvironmentVariable("Path", [EnvironmentVariableTarget]::User)
if ($UserPath -notlike "*$InstallDir*") {
    $NewPath = "$UserPath;$InstallDir"
    [Environment]::SetEnvironmentVariable("Path", $NewPath, [EnvironmentVariableTarget]::User)
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Каталог успешно добавлен в переменную окружения PATH пользователя." -ForegroundColor Green
}

Write-Host ""
Write-Host "🎉 Установка успешно завершена!" -ForegroundColor Green
Write-Host "------------------------------------------------------------" -ForegroundColor DarkGray
Write-Host "Каталог программы:  $InstallDir" -ForegroundColor White
Write-Host "Основной файл:      $InstallDir\unisoc.exe" -ForegroundColor White
Write-Host "Загрузчики (FDL):   $InstallDir\loaders" -ForegroundColor White
Write-Host "------------------------------------------------------------" -ForegroundColor DarkGray
Write-Host ""
Write-Host "Для запуска графического интерфейса введите:" -ForegroundColor Yellow
Write-Host "  unisoc" -ForegroundColor Green
Write-Host ""
Write-Host "Для проверки обновлений:" -ForegroundColor Yellow
Write-Host "  unisoc update" -ForegroundColor White
Write-Host ""

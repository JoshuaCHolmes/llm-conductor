# Extract Outlier Playground cookies from Vivaldi/Chrome/Edge
# Run this in Windows PowerShell, then copy the output commands to WSL

param(
    [string]$Browser = "Vivaldi",
    [string]$Profile = "Default"
)

$ErrorActionPreference = "Stop"

# Map browser to cookie path
$paths = @{
    "Vivaldi" = "$env:LOCALAPPDATA\Vivaldi\User Data\$Profile\Network\Cookies"
    "Chrome" = "$env:LOCALAPPDATA\Google\Chrome\User Data\$Profile\Network\Cookies"
    "Edge" = "$env:LOCALAPPDATA\Microsoft\Edge\User Data\$Profile\Network\Cookies"
}

$cookiesPath = $paths[$Browser]

if (-not (Test-Path $cookiesPath)) {
    Write-Host "❌ $Browser cookies not found at: $cookiesPath" -ForegroundColor Red
    Write-Host "   Please log in to playground.outlier.ai in $Browser first" -ForegroundColor Yellow
    exit 1
}

# Copy to temp
$tempDb = "$env:TEMP\outlier_cookies_$(Get-Random).db"
Copy-Item $cookiesPath $tempDb -Force

Write-Host "🔍 Extracting cookies from $Browser..." -ForegroundColor Cyan

# Simple SQLite reader (no external dependencies needed)
$bytes = [System.IO.File]::ReadAllBytes($tempDb)
$content = [System.Text.Encoding]::ASCII.GetString($bytes)

# Find outlier.ai in the database (crude but works)
if ($content -notmatch "outlier\.ai") {
    Write-Host "❌ No Outlier cookies found" -ForegroundColor Red
    Write-Host "   Please log in to https://playground.outlier.ai first" -ForegroundColor Yellow
    Remove-Item $tempDb -Force
    exit 1
}

# For now, provide manual instructions
Write-Host ""
Write-Host "=" * 70
Write-Host ""
Write-Host "⚠  Automated extraction requires additional setup." -ForegroundColor Yellow
Write-Host ""
Write-Host "Manual method:" -ForegroundColor White
Write-Host "1. Open $Browser Developer Tools (F12)" -ForegroundColor Gray
Write-Host "2. Go to Application > Cookies > https://playground.outlier.ai" -ForegroundColor Gray
Write-Host "3. Copy the values for:" -ForegroundColor Gray
Write-Host "   - _session (full cookie string)" -ForegroundColor Gray
Write-Host "   - _csrf (CSRF token)" -ForegroundColor Gray
Write-Host ""
Write-Host "4. In WSL, run:" -ForegroundColor White
Write-Host "   llm-conductor config add-key outlier_cookie 'PASTE_COOKIE_STRING_HERE'" -ForegroundColor Green
Write-Host "   llm-conductor config add-key outlier_csrf 'PASTE_CSRF_HERE'" -ForegroundColor Green
Write-Host ""

Remove-Item $tempDb -Force

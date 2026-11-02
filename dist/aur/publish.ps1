# Lädt die AUR-Pakete (tuilnux, tuilnux-git) ins Arch User Repository hoch.
#
# Voraussetzung (einmalig): AUR-Konto auf https://aur.archlinux.org/register mit dem
# öffentlichen Schlüssel aus ~/.ssh/aur.pub unter "SSH Public Key".
#
#   pwsh dist/aur/publish.ps1                 # beide Pakete
#   pwsh dist/aur/publish.ps1 -Packages tuilnux
#   pwsh dist/aur/publish.ps1 -DryRun         # nur vorbereiten, nicht pushen
param(
    [string[]]$Packages = @('tuilnux', 'tuilnux-git'),
    [string]$Message,
    [switch]$DryRun
)
$ErrorActionPreference = 'Stop'

function Get-Field([string]$text, [string]$pattern) {
    $m = [regex]::Match($text, $pattern, 'Multiline')
    if ($m.Success) { $m.Groups[1].Value.Trim() } else { $null }
}

$work = Join-Path ([IO.Path]::GetTempPath()) "tuilnux-aur-$PID"
New-Item -ItemType Directory -Force $work | Out-Null
$name = git -C $PSScriptRoot config user.name
$email = git -C $PSScriptRoot config user.email
if (-not $name -or -not $email) { throw 'git user.name/user.email nicht gesetzt' }
try {
    foreach ($pkg in $Packages) {
        $src = Join-Path $PSScriptRoot $pkg
        if (-not (Test-Path "$src/PKGBUILD")) { throw "$pkg`: PKGBUILD fehlt in $src" }
        if (-not (Test-Path "$src/.SRCINFO")) { throw "$pkg`: .SRCINFO fehlt (makepkg --printsrcinfo > .SRCINFO)" }

        $build = Get-Content -Raw "$src/PKGBUILD"
        $info = Get-Content -Raw "$src/.SRCINFO"
        $ver = "$(Get-Field $build '^pkgver=(.+)$')-$(Get-Field $build '^pkgrel=(.+)$')"
        $infoVer = "$(Get-Field $info '^\s*pkgver = (.+)$')-$(Get-Field $info '^\s*pkgrel = (.+)$')"
        if ($ver -ne $infoVer) { throw "$pkg`: .SRCINFO ($infoVer) passt nicht zum PKGBUILD ($ver) – neu erzeugen" }

        $repo = Join-Path $work $pkg
        Write-Host "==> $pkg $ver"
        git clone -q -c core.autocrlf=false "ssh://aur@aur.archlinux.org/$pkg.git" $repo
        if ($LASTEXITCODE) { throw "$pkg`: git clone fehlgeschlagen (SSH-Schlüssel im AUR-Konto hinterlegt?)" }
        Push-Location $repo
        try {
            git config user.name $name
            git config user.email $email
            git checkout -q -B master
            foreach ($f in 'PKGBUILD', '.SRCINFO', 'tuilnux.install') {
                $text = (Get-Content -Raw (Join-Path $src $f)).Replace("`r`n", "`n")
                [IO.File]::WriteAllText((Join-Path $repo $f), $text)
            }
            git add PKGBUILD .SRCINFO tuilnux.install
            git diff --cached --quiet
            if (-not $LASTEXITCODE) { Write-Host "    keine Änderungen"; continue }
            $msg = if ($Message) { $Message } else { "Update to $ver" }
            git commit -q -m $msg
            if ($LASTEXITCODE) { throw "$pkg`: git commit fehlgeschlagen" }
            if ($DryRun) { Write-Host "    vorbereitet in $repo (DryRun, nicht gepusht)"; continue }
            git push -q origin master
            if ($LASTEXITCODE) { throw "$pkg`: git push fehlgeschlagen" }
            Write-Host "    https://aur.archlinux.org/packages/$pkg"
        } finally { Pop-Location }
    }
} finally {
    if (-not $DryRun) { Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue }
}

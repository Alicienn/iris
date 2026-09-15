# Compile les modules livrés avec Iris et les dispose comme l'application les attend.
#
# Sortie : packaging\plugins\<id>\{plugin.toml, plugin.wasm}
#
# C'est exactement la forme que le registre lit — un dossier par module, son manifeste
# et son binaire à côté. L'installateur recopie cet arbre tel quel, ce qui veut dire
# que la disposition n'existe qu'ici : si elle change, elle change à un seul endroit.
#
# Le nom du binaire vient du manifeste (`entry`) et non d'une constante de ce script :
# c'est le manifeste que le registre lit, et deux copies de la même vérité finissent
# par diverger.

$ErrorActionPreference = 'Stop'

$racine = Split-Path -Parent $PSScriptRoot
$sortie = Join-Path $PSScriptRoot 'plugins'

# Les caisses à compiler. Ajouter un module livré, c'est ajouter une ligne — et rien
# d'autre : l'identifiant et le nom du fichier se lisent dans son manifeste.
$caisses = @(
    'iris-plugin-vip',
    'iris-plugin-office-hours',
    'iris-plugin-filer'
)

# La cible WebAssembly n'est pas installée par défaut. Le dire vaut mieux que de
# laisser cargo échouer sur un message qui parle de composants.
$cibles = & rustup target list --installed
if ($cibles -notcontains 'wasm32-unknown-unknown') {
    throw "The wasm32-unknown-unknown target is missing. Run: rustup target add wasm32-unknown-unknown"
}

Write-Host "Building $($caisses.Count) plugins for wasm32-unknown-unknown…"

$arguments = @('build', '--release', '--target', 'wasm32-unknown-unknown')
foreach ($caisse in $caisses) { $arguments += @('-p', $caisse) }

& cargo @arguments
if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

# Table rase : un module retiré de la liste ci-dessus doit disparaître de la sortie,
# sans quoi l'installateur emporterait un module que plus personne ne compile.
if (Test-Path $sortie) { Remove-Item $sortie -Recurse -Force }
New-Item -ItemType Directory -Path $sortie | Out-Null

foreach ($caisse in $caisses) {
    $manifeste = Join-Path $racine "crates\$caisse\plugin.toml"
    if (-not (Test-Path $manifeste)) { throw "No plugin.toml in crates\$caisse" }

    $texte = Get-Content $manifeste -Raw

    # Une lecture de TOML à deux champs, faite à la main. Le format complet vaudrait une
    # dépendance ; ces deux clés sont en tête de fichier, sans table, et sur une ligne.
    $id = [regex]::Match($texte, '(?m)^\s*id\s*=\s*"([^"]+)"').Groups[1].Value
    $entree = [regex]::Match($texte, '(?m)^\s*entry\s*=\s*"([^"]+)"').Groups[1].Value
    if (-not $id -or -not $entree) { throw "crates\$caisse\plugin.toml has no id or entry" }

    # cargo remplace les tirets par des soulignés dans le nom du binaire.
    $binaire = Join-Path $racine ("target\wasm32-unknown-unknown\release\{0}.wasm" -f ($caisse -replace '-', '_'))
    if (-not (Test-Path $binaire)) { throw "Built nothing at $binaire" }

    $dossier = Join-Path $sortie $id
    New-Item -ItemType Directory -Path $dossier | Out-Null
    Copy-Item $manifeste (Join-Path $dossier 'plugin.toml')
    Copy-Item $binaire (Join-Path $dossier $entree)

    $ko = [math]::Round((Get-Item $binaire).Length / 1KB)
    Write-Host ("  {0,-14} {1,5} KB" -f $id, $ko)
}

Write-Host "Laid out in $sortie"

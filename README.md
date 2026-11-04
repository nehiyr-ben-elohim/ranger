# Ranger

Ranger remet les fichiers de ton dossier personnel à leur place : images, musique, vidéos, documents, archives et installeurs. Tu relis chaque déplacement avant qu'il ait lieu, et tu peux tout annuler.

## Ce qu'il fait

- **Analyse** ton dossier personnel, ou seulement les dossiers et fichiers que tu choisis.
- **Montre chaque déplacement** dans une table : le fichier, d'où il part, où il va. Rien ne bouge avant ta confirmation.
- **Annule** le dernier rangement, même après avoir fermé l'app.
- **Protège** ce qui ne doit pas bouger : dossiers système, projets de code (`.git`, `package.json`, `Cargo.toml`…), outils de dev (OrbStack, miniconda, `~/go`…), bibliothèques Photos, Musique et TV, disques montés.
- **IA (optionnelle)** : Claude propose des sous-dossiers par sujet (`Documents/Factures`, `Images/Captures d'écran`…). Seuls le nom, le dossier, la taille, la date et le début des fichiers texte sont envoyés ; jamais le contenu des images, vidéos, musiques ou PDF. Connexion par compte Anthropic (CLI `ant`) ou clé API, gardée dans le trousseau du système.

| Type | Destination macOS | Destination Linux |
|---|---|---|
| Images | `~/Pictures` | dossier XDG des images (ex. `~/Images`) |
| Musique | `~/Music` | dossier XDG de la musique |
| Vidéos | `~/Movies` | dossier XDG des vidéos (ex. `~/Vidéos`) |
| Documents | `~/Documents` | dossier XDG des documents |
| Archives | `~/Documents/Archives` | `Documents/Archives` |
| Installeurs | `~/Downloads/Installeurs` | `Téléchargements/Installeurs` |

## Installer

Télécharge la dernière version dans [Releases](../../releases) :

- **macOS** (Apple Silicon ou Intel) : le `.dmg`. L'app n'est pas signée : au premier lancement, clic droit sur Ranger puis **Ouvrir**.
- **Ubuntu x86_64** : `sudo apt install ./Ranger_*_amd64.deb`, ou l'`.AppImage` (`chmod +x`, puis lancer).

## Développer

Prérequis : Node 22, Rust stable, et sur Linux les paquets listés dans [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

```bash
npm ci
npm run tauri dev                         # lancer en développement
cd src-tauri && cargo test --lib          # tests
./scripts/build-linux.sh                  # paquets Ubuntu depuis un Mac (Docker ou OrbStack)
```

Publier une version : `git tag v0.1.0 && git push origin v0.1.0`. GitHub Actions construit les binaires dans un brouillon de release.

Stack : [Tauri 2](https://tauri.app), Rust, TypeScript sans framework.

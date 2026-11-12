# Jot 1.5.0 — rapport de la première version publique

Date : 12 novembre 2026. Dépôt publié : [`tadikwa/jot-gemini-transcribe-Windows`](https://github.com/tadikwa/jot-gemini-transcribe-Windows),
seule cible. Rien n’a été poussé vers `ubranch` ni `google-gemini`, aucune réécriture forcée.

## 1. Résumé

| Livrable | État |
|---|---|
| Étape « Comment voulez-vous déclencher la dictée ? » au premier lancement | Livrée (`c32af60`), testée sur le binaire distribué |
| Installateur `Jot-1.5.0-setup.exe` et `Jot-1.5.0-portable.zip` | Livrés, construits depuis le commit du tag |
| README français 1.5, About et sujets GitHub | Publiés |
| Vidéo `Jot-1.5.0-demo-FR.mp4` (62 s) et affiche | Jointes à la version, sources dans `video/` |
| Version `v1.5.0` stable avec notes françaises | [Publiée](https://github.com/tadikwa/jot-gemini-transcribe-Windows/releases/tag/v1.5.0) le 12 novembre 2026 : stable, « Latest », 5 fichiers, empreintes revérifiées après téléchargement |

## 2. État Git

| | Avant | Après |
|---|---|---|
| `origin/main` | `09ad593` (README 1.4) | `b43c06e` (avance rapide, sans forçage), puis ce rapport |
| tag `v1.5.0` | absent (local et distant) | annoté (`f6d3eec`), pointe sur `b43c06e`, poussé |
| Branche de travail `claude/v15-first-release` | `4b5a2a2` | `b43c06e` puis ce rapport |
| `main` local du dépôt principal | `33a8d98` | inchangé (seul `origin/main` est publié) |
| `dev` (dépôt principal) | `99e5c99`, modifications non commitées sur le HUD | **non touché** |

`main` a avancé en avance rapide (`09ad593` est un ancêtre du commit de version). Le commit du tag
`b43c06e` est celui depuis lequel les binaires ont été construits ; le présent rapport est commité
juste après, sur `main`, et ne change aucun fichier compilé.

**Commits de `dev` non inclus** : `1ae65d9` (afficher le HUD après sa création) et `99e5c99`
(cacher la fenêtre du HUD quand la pastille est masquée), du 10 novembre. La version publiée affiche
déjà le HUD avec `SW_SHOWNOACTIVATE` (équivalent du premier) ; le second n’y est pas. Les deux sont
liés au travail en cours non commité dans `E:\05 - IA\01 - GITHUB\JOT`, laissé tel quel. Le HUD de
1.5.0 est bien traversant au repos (§ 4.5).

## 3. Contrôles du code (commit `b43c06e`)

| Commande | Résultat |
|---|---|
| `cargo fmt --all --check` | OK |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | OK, aucun avertissement |
| `cargo test --workspace --locked` | OK : 215 (`jot-core`) + 101 (`jot`) + 16 (bibliothèque d’historique) réussis, 0 échec, 4 ignorés volontairement (mesure de performance sur 100 000 lignes, moteur et modèle réels, appel réel à l’API GitHub) |
| `scripts/package.ps1` | OK : `Jot-1.5.0-setup.exe` et `Jot-1.5.0-portable.zip`, non signés |
| `scripts/package.ps1 -TestBuild` | OK : `Jot-1.5.0-test-setup.exe` et `Jot-1.5.0-test-portable.zip`, même `jot.exe` (SHA-256 `76B2F489…FCCEA4`) avec le marqueur en plus |
| Vérification réelle de mise à jour (`cargo test -p jot-core -- --ignored reaches_github`) après publication | OK : `Available(1.5.0)`, page de la version, `prerelease: false` |

Le `jot.exe` testé au § 4 avant le commit final (`50787AD8…`) et le `jot.exe` publié (`76B2F489…`)
ont la même taille (25 930 752 octets) et ne diffèrent que de 24 octets : horodatage de l’en-tête PE,
horodatages du répertoire de débogage et GUID du PDB. Les commits intermédiaires ne touchent que la
documentation et la vidéo.

Le seul message d’erreur des journaux est « Accès refusé » lorsque cargo tente de nettoyer un fichier
de son cache (`icu_locale_core`) verrouillé par un autre processus ; sans effet sur la compilation.

## 4. Tests sur le binaire distribué

Tous faits avec le build de test (`package.ps1 -TestBuild`, même code, marqueur `jot-test-profile`)
dans des dossiers jetables `E:\JotReleaseTest\*` : aucun ne lit ni n’écrit `%LOCALAPPDATA%\Jot`.

### 4.1 Premier lancement (captures `PrintWindow` de chaque étape)

| Cas | Résultat |
|---|---|
| Profil neuf | Étape 2 « Comment voulez-vous déclencher la dictée ? », *Appuyer pour démarrer / arrêter* présélectionné |
| Local + Toggle, Local + Maintenir | Choix enregistré (`dictationActivationMode` = `toggle` / `hold`), visible dans *Paramètres → Dictée et raccourcis* |
| Gemini + Toggle, Gemini + Maintenir | Choix enregistré (`toggle` / `hold`, moteur `gemini`), puis étape « Connecter Gemini » (3/6) ; aucune vraie clé saisie |
| Les deux | Parcours jusqu’à « Connecter Gemini » (3/7) seulement ; le réglage écrit n’a pas été relu pour ce moteur |
| Bouton Retour | Revient à l’étape précédente en gardant la sélection, jusqu’au choix du moteur |
| Clavier | Tab, Espace, Entrée suffisent pour choisir et continuer |
| Petite fenêtre | Les deux cartes passent sur plusieurs lignes, rien n’est coupé |
| Redémarrage | Le réglage choisi est relu à la relance |
| Profil existant (onboarding déjà terminé, `hold`) | Assistant rouvert depuis la zone de notification : *Maintenir pour parler* reste sélectionné, réglage non modifié |

Tests unitaires associés : `activation_mode_survives_reopen_without_overwriting_the_hotkey` et la
lecture d’un ancien `settings.json` sans le champ (valeur par défaut, aucune migration destructive).

### 4.2 Moteur local, dictées longues (corpus `docs/v15/bench/smart_e2e.json`)

18 dictées fictives lues par la voix de synthèse Windows, déposées comme enregistrements interrompus
et transcrites par le vrai `jot.exe` (Qwen3-ASR 1.7B puis Qwen3.5 4B) : **10/18** conformes à la
mise en forme attendue. Les 8 autres : erreur de transcription (« lots », « Samy », « DMI »), point
d’interrogation manquant, ou texte brut inséré parce que le garde-fou a refusé la reformulation
(2 cas). Résultat obtenu en 5 à 6 s après le lancement, 2 serveurs `llama-server`, **aucune
connexion hors `127.0.0.1`**, **0 processus orphelin** après l’arrêt, dans les 18 cas.

### 4.3 Libération des modèles

Une dictée, puis mesure toutes les 15 s (RTX 5070 Ti) :

| Moment | `llama-server` | RAM llama | VRAM | RAM `jot.exe` |
|---|---|---|---|---|
| Avant | 0 | — | 1 918 Mio | — |
| Après la dictée (15 s → 303 s) | 2 | 4 985 Mo | ≈ 9 670 Mio | 87 Mo |
| 318 s | 0 | 0 | 1 901 Mio | 87 Mo |
| Arrêt de Jot | 0 orphelin | | 1 863 Mio | |

Les modèles sont libérés entre 303 et 318 s, soit 5 minutes après la dernière dictée. Jot reste
vivant ensuite.

### 4.4 Annonce des nouvelles versions

Faux serveur local (`JOT_TEST_RELEASES_URL`, honoré seulement dans un profil de test) :

| Scénario | Comportement observé |
|---|---|
| Aucune version publiée | `NoRelease`, rien affiché |
| Même version | `UpToDate` |
| 1.6.0 disponible | Annoncée une fois |
| 1.6.0 au lancement suivant | Connue, pas annoncée de nouveau |
| Préversion 1.6.0-beta.1, option désactivée | `UpToDate` |
| Préversion, option activée | Annoncée |
| Erreur HTTP 500 | Échec journalisé, rien affiché |
| Pas de réseau | Échec « offline » journalisé, rien affiché |

### 4.5 HUD, historique, dictionnaire

- **HUD au repos** : fenêtre `WS_EX_LAYERED | TRANSPARENT | NOACTIVATE | TOOLWINDOW | TOPMOST`,
  606 × 99 en bas d’écran ; `WindowFromPoint` sur 6 points de sa surface renvoie la fenêtre d’un
  autre processus : les clics la traversent.
- **Historique** : correction de « lots » en « logs » par sélection à la souris, ajout d’un retour à
  la ligne, enregistrement ; la base contient exactement le texte attendu sur deux lignes.
- **Dictionnaire** : la suggestion « lots → logs » (score 75 %) apparaît après « Enrichir à partir de
  mes corrections », puis « Règle acceptée ».

### 4.6 Installateur

Version : `jot.exe` 1.5.0, *Jot*, « Jot - voice dictation », x64 (machine `0x8664`) ; setup
« Jot 1.5.0 Setup ». Mutex `Local\com.ammaar.jot.dictation`, `PrivilegesRequired=lowest`.

| Scénario | Résultat |
|---|---|
| Installation silencieuse du setup publié, avec raccourci bureau et démarrage auto | Code 0 ; `%LOCALAPPDATA%\Programs\Jot` : `jot.exe`, `LICENSE`, `NOTICE-FR.md`, `THIRD_PARTY_NOTICES.md`, désinstalleur ; raccourcis menu Démarrer et bureau ; entrée « Jot 1.5.0 » dans Applications ; valeur `HKCU\…\Run\Jot` |
| Mise à niveau par-dessus, sans tâche cochée | Fichiers, raccourcis et entrée Run conservés |
| Désinstallation | Fichiers, raccourcis, entrée Run et inscription retirés ; données conservées par défaut |
| Build de test, entrée Run étrangère (`"C:\Other\notjot.exe" --x`) | Conservée telle quelle |
| Build de test, réinstallation propre | Entrée Run vers `Jot-test`, marqueur présent, `jot.exe` identique au build normal |
| Désinstallation du build de test avec `/PURGEDATA` | Dossier `Jot-test` supprimé entièrement |
| Arrêt de Jot avec le mode local actif | `llama-server` arrêté avec lui (objet job `KILL_ON_JOB_CLOSE`), 0 orphelin |

**Incident pendant ce test.** L’installateur considère qu’une valeur `Run\Jot` qui lance un
`jot.exe` appartient à Jot (même règle que `autostart.rs`) : cocher le démarrage automatique la
remplace. Entre deux prises d’empreinte, une valeur
`Jot="E:\05 - IA\01 - GITHUB\JOT\test-build\jot-test-v4\jot.exe"` avait été ajoutée au profil réel
(pas par cette session). Le test d’installation normale l’a remplacée, puis la désinstallation l’a
retirée. La restauration automatique a été refusée par les garde-fous de l’environnement ; elle
reste à faire à la main :

```powershell
New-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name Jot -PropertyType String -Value '"E:\05 - IA\01 - GITHUB\JOT\test-build\jot-test-v4\jot.exe"'
```

C’est la seule différence entre l’empreinte prise avant les tests et celle prise à la fin :
`%LOCALAPPDATA%\Jot` (chemins, tailles, dates, SHA-256) et la présence de la clé Gemini sont
identiques.

## 5. Fichiers publiés

Construits depuis `b43c06e`. `SHA256SUMS.txt` (joint à la version) :

```
2e7d8f8eb24e89631501757e272896bcfce3cf549ccbc2d4303c0c97ad6206d9  Jot-1.5.0-demo-FR.mp4
6d17d29be4c38dbb22a0eb09ae21e34ec4d8c345aed433cdadb249cd53d61bdc  Jot-1.5.0-demo-FR.png
4dd9973ddd697807f0cffc3f0d4a2b782b9a39da1720dd7c328b71b166e6e68b  Jot-1.5.0-portable.zip
f46b60a902555ac5154d768833be3dd6698c2faa8716c747ad7794ffff4d5ae6  Jot-1.5.0-setup.exe
```

Tailles : setup 9 761 733 o, zip 10 823 631 o, vidéo 28 078 262 o, affiche 1 110 669 o. Après
publication, les 5 fichiers ont été retéléchargés depuis GitHub et `sha256sum -c` les donne tous OK.

- **Signature** : aucun fichier n’est signé (pas de certificat de signature de code). SmartScreen
  avertira ; le README et les notes de version le disent et expliquent comment vérifier l’empreinte.
- **Modèles** : aucun GGUF ni runtime llama.cpp dans l’installateur ou le zip ; ils sont téléchargés
  au premier usage du mode local (5,30 Go), vérifiés par taille et SHA-256.
- **Licences** : Jot Apache 2.0 ; llama.cpp MIT ; Qwen3-ASR et Qwen3.5 Apache 2.0, affichées avant le
  téléchargement. Vidéo : voir `video/MEDIAS-ET-LICENCES.md` (musique originale, Inter OFL, Remotion
  Free License pour un particulier).
- **Contenu du zip portable** : `jot.exe`, `LICENSE`, `NOTICE-FR.md`, `README.md`, `THIRD_PARTY_NOTICES.md`, sans marqueur `jot-test-profile`,
  sans profil, journal, clé ni donnée.

## 6. Vidéo

`Jot-1.5.0-demo-FR.mp4` : 62,0 s, 1920 × 1080, 30 i/s, H.264 + AAC 48 kHz, sous-titres français
intégrés, musique originale, pas de voix off. Uniquement des captures réelles du build de test sur
des profils jetables (dictées fictives, aucune clé, ni bureau, ni notification). Sources Remotion et
générateur de musique dans `video/` ; le MP4 et le WAV ne sont pas dans Git.

## 7. Non testé ou limites

- **Pastille pendant l’enregistrement et insertion par la touche de dictée** : non testées sur le
  binaire publié. L’environnement n’avait accès ni à l’écran ni à `SendInput`, et filmer le bureau
  personnel était exclu. Les tests passent par des enregistrements déposés dans le profil.
- **Voix réelle** : seulement une voix de synthèse.
- **Autres cartes graphiques et processeur seul** : non mesurés (une seule machine, RTX 5070 Ti).
- **Mode Gemini de bout en bout** : pas de vraie clé utilisée.
- **SmartScreen** : l’avertissement n’a pas été observé sur une machine tierce, il est attendu pour un
  binaire non signé et sans réputation.
- La capture `assets/readme/accueil.png` vient de la version 1.4 (données fictives, interface
  identique pour cet écran).

## 8. Nettoyage

- Aucun `jot.exe` ni `llama-server.exe` de test en cours d’exécution. Le Jot personnel, fermé par
  son propriétaire pendant les tests, n’a pas été relancé.
- Installation de test `%LOCALAPPDATA%\Programs\Jot-test` désinstallée (`/PURGEDATA`) ; clé de test
  `HKCU\Software\Jot-installer-test` supprimée ; aucune installation normale laissée.
- **Reste** : le dossier jetable `E:\JotReleaseTest` (≈ 5,4 Go, surtout les modèles du profil de
  test). Sa suppression a été bloquée par les garde-fous de l’environnement ; il peut être effacé
  sans risque.
- **Reste** : l’entrée `Run\Jot` à restaurer (§ 4.6).
- Dépôt principal `E:\05 - IA\01 - GITHUB\JOT` (branche `dev`, modifications non commitées) : non
  touché. Le travail s’est fait dans un worktree séparé.

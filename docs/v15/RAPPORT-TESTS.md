# Jot 1.5.0 — rapport de tests (candidat local, 11/11/2026)

Tests exécutés par Claude Code dans le worktree `jot-v15` (branche `claude/v15`), sur la machine de développement (Windows 11, RTX 5070 Ti). Toutes les données sont fictives : profils isolés, échantillons audio de synthèse vocale et clé de registre de test. **Rien n'est publié**, ni release, ni tag, ni binaire en ligne.

Statuts :

- **OK** : exécuté et réussi.
- **NON VÉRIFIÉ** : impossible dans cette session, avec la raison.

## 1. Barrières de qualité

| Commande | Statut |
|---|---|
| `cargo fmt --all --check` | OK |
| `cargo clippy --all-targets --locked -- -D warnings` | OK (code 0) |
| `cargo test --locked` | OK : 98 + 202 + 16 réussis, 3 ignorés (tests réseau ou matériel réel, exécutés à part ci-dessous) |
| `cargo build --release -p jot --locked` | OK. `jot.exe` en version 1.5.0 (`jot/1.5.0` dans l'en-tête User-Agent) |
| `scripts/package.ps1 -TestBuild` | OK : enchaîne les quatre commandes ci-dessus, puis produit le ZIP et l'installateur de test |

## 2. Moteur 100 % local (test ignoré `real_engine_end_to_end`, matériel réel)

```
JOT_LOCAL_E2E=<racine de test> JOT_LOCAL_E2E_AUDIO=<wav fictifs> cargo test -p jot-core real_engine_end_to_end -- --ignored --nocapture
```

| Vérification | Résultat |
|---|---|
| Annulation au milieu d'un téléchargement | OK : arrêt après 1 620 815 octets, fichier `.part` supprimé |
| Installation du runtime llama.cpp (38 022 207 octets, GitHub, SHA-256) | OK en 1,2 s |
| Démarrage du moteur sur le GPU dédié | OK en 7,5 s (RTX 5070 Ti ; l'iGPU AMD est ignoré) |
| 6 échantillons fictifs | OK : 0,28 à 0,37 s chacun ; 1,35 s pour 27,8 s d'audio |
| Hésitations retirées, injection dictée conservée telle quelle | OK |
| Silence | OK : « transcription vide », sans texte inventé |
| Réseau pendant la dictée locale (`netwatch.ps1`) | OK : uniquement 127.0.0.1 |
| `unload` | OK : aucun `llama-server` restant |

## 3. Interface réelle (jot.exe compilé, profil de test, lecture par UI Automation)

| Vérification | Statut |
|---|---|
| Choix Gemini / 100 % local et Reformulation Désactivée / Rapide / Qualité | OK |
| Qualité → « Télécharger (5,68 Go) » → confirmation → « Pas maintenant » | OK : aucun téléchargement lancé |
| Mode Gemini : Dictée intelligente et Ton visibles | OK |
| Persistance dans `settings.json` du profil de test | OK |
| Confidentialité : « Rechercher les mises à jour » et « Inclure les préversions » | OK |
| Démarrage automatique en profil de test | Partiel : le bouton est présent et `HKCU\…\Run\Jot` reste absent. Le texte explicatif n'est pas exposé à UI Automation, donc **NON VÉRIFIÉ** visuellement |

## 4. Avis de mise à jour

| Vérification | Statut |
|---|---|
| Tests unitaires : ordre semver, stable / préversion / aucune, port fermé, limite de requêtes, ETag, cache frais, avis unique | OK |
| Test réel contre `api.github.com` (fork sans release) | OK : `NoRelease` |
| Faux serveur (v9.9.0 stable + v9.10.0-rc.1) : avis au démarrage | OK : 9.9.0 annoncée une fois, rien de plus au relancement |
| Affichage de la bulle et clic menant à À propos | **NON VÉRIFIÉ** : pas de capture d'écran possible dans cette session |

## 5. Installateur de test (`inst-test.ps1`, 31 vérifications)

Le script lance `Jot-1.5.0-test-setup.exe` en mode silencieux dans un dossier temporaire :

- application distincte, « Jot (test) » ;
- clé de démarrage de test `HKCU\Software\Jot-installer-test\Run` à la place de la vraie ;
- profil isolé.

| Vérification | Statut |
|---|---|
| Installation sans droits administrateur (code 0) | OK |
| Fichiers installés : exécutable, marqueur, licences, notice FR et désinstalleur | OK |
| Raccourci du menu Démarrer ; aucune icône de bureau (option décochée) | OK |
| Démarrage coché : la commande écrite est identique à celle de Jot (`"<app>\jot.exe" --background`) | OK |
| Entrée de désinstallation HKCU en version 1.5.0 | OK |
| Entrée « Jot » d'un autre programme | OK : jamais remplacée, mentionnée dans le journal |
| Jot installé lancé : profil créé à côté de l'exécutable | OK (arrêté par son PID exact) |
| Désinstallation pendant que Jot tourne | OK : refusée (`AppMutex`), fichiers intacts |
| Désinstallation silencieuse | OK : programme, raccourci, entrée de démarrage de cette installation et entrée de désinstallation retirés ; **profil conservé** |
| Réinstallation sans tâche | OK : aucune entrée de démarrage créée |
| `/PURGEDATA` | OK : profil supprimé |
| Vraie clé `Run\Jot` (empreinte) et `%LOCALAPPDATA%\Jot` | OK : inchangés du début à la fin |
| Processus `jot` restants | OK : 0 |
| `cmdkey /delete` (utilisé par la suppression des données) sur un identifiant générique fictif | OK : supprimé |
| Installateur normal `Jot-1.5.0-setup.exe` | Compilé (ISCC code 0), **jamais exécuté**, car il écrirait dans le vrai profil |
| Question interactive de suppression (Non par défaut) et message « entrée étrangère » | **NON VÉRIFIÉ** visuellement : pas d'affichage. La logique est couverte par les chemins silencieux ci-dessus |

## 6. Harnais HUD (`docs/hud/Validate-Hud.ps1`)

**NON VÉRIFIÉ.** Le harnais échoue avec `Accès refusé (0x80070005)` sur le binaire 1.5, avant de produire `result.json`.

Le témoin écarte une régression :

- le même harnais échoue de la même façon sur le binaire 1.4 qui l'avait réussi (`test-build\claude-v14-walkthrough\jot.exe`) ;
- la cause vient de cette session : le bureau interactif est inaccessible aux captures et aux clics réels, comme les captures d'écran (« Descripteur non valide ») ;
- la session Windows est pourtant active et non verrouillée.

Par rapport à la 1.4 validée (09ad593), `window_shell.rs` et `hud_validation.rs` sont identiques. `pill.rs` ne gagne que le message d'erreur du moteur local (2 lignes). **À relancer lors de votre test final :**

```powershell
.\docs\hud\Validate-Hud.ps1 -Executable <dossier>\jot.exe -ReportDirectory <nouveau dossier>
.\docs\hud\Validate-Hud.ps1 -Executable <dossier>\jot.exe -ReportDirectory <autre dossier> -InitiallyHidden
```

## 7. Livrables locaux (dans `test-build\v15\`, ignoré par Git)

| Fichier | Taille | SHA-256 |
|---|---|---|
| `Jot-1.5.0-test-portable.zip` | 11 148 266 | `4BE632FCA5135CEB99F38AEB31DE52E25AD8490941AFE9E519239780691D9F43` |
| `Jot-1.5.0-test-setup.exe` | 9 727 196 | `DF942920703C4E27039F4B9D475501E051C1A1A1D9816368CACEE2C288822249` |
| `jot.exe` (contenu dans les deux) | 25 695 744 | `BEA943213126173978739914E92B23841185E589CEC8E2D2ADD6621D16E3BB63` |
| `Jot-1.5.0-setup.exe` (normal, dans `target\dist`, non exécuté) | 9 727 205 | `E1279F98D55DBD8FD8FFC41474D4322E85DCA45D8268E2B8B03ACE32CCA6B4ED` |

Contenu du ZIP : `jot.exe`, `jot-test-profile`, `LICENSE`, `THIRD_PARTY_NOTICES.md`, `README.md` et `NOTICE-FR.md`. Il ne contient ni profil, ni journal, ni audio, ni clé. Aucun fichier n'est signé : SmartScreen avertira, comme le documente la notice.

## 8. Restes connus

- L'autocorrection « enfin je veux dire » n'est pas appliquée par la reformulation locale.
- Une vraie dictée au raccourci en mode local, faite dans l'interface, n'a pas été testée. Le moteur a été testé de bout en bout, mais par le test intégré.
- `check_updates` est activé par défaut. Ce choix vous revient.
- L'étape d'installateur n'est pas ajoutée à la CI (Inno Setup n'est pas garanti sur les machines GitHub).

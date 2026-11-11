# Jot 1.4.0 — reprise Claude Code (11/11/2026)

Candidat de test **non publié** : aucun push, PR, tag, release ni remplacement de l’installation principale. À valider par l’utilisateur.

## État Git
- Worktree : `<worktrees>/jot-dictionary-v14\JOT`, branche locale `codex/dictionary-v14`, base `e6e502d` (Jot 1.3).
- `a9141b1` — travail Codex V1.4 (lots A–D), commité tel quel après relecture du diff et recherche de secrets.
- `3dceb76` — corrections de cette reprise (ci-dessous). **Le binaire livré est compilé depuis cet arbre.**
- Commit suivant : ce rapport (documentation seule).
- Non commité volontairement : `docs/v14/test-integration.log` (ancien journal Codex, version 1.3.0, chemins locaux).
- Le checkout principal `<checkout>` (branche `dev`, modifications non commitées) n’a pas été touché, sauf ajouts dans `test-build\` et le dossier `target` partagé.

## Corrections apportées
| Défaut | Cause | Correction | Preuve |
|---|---|---|---|
| Recherche du Dictionnaire affichait « Rechercher dans les dictées » | réutilisait `Msg::Search` de l’Historique | `Msg::SearchDictionary` FR/EN (`messages.rs`, `views/dictionary.rs`) | capture GUI après rebuild |
| Interrupteur « Deux appuis… » hors de sa carte | la colonne texte de `widgets::toggle` ne pouvait pas rétrécir ; le texte 1.4, plus long, poussait l’interrupteur | `flex_1().min_w_0()` sur la colonne texte (`views/widgets.rs`) | capture GUI après rebuild |
| Purge des suggestions orphelines non couverte par le chemin Historique | — | test `deleting_a_dictation_purges_its_pending_suggestion_but_keeps_accepted_rules` (`tests/history_library.rs`) | `cargo test` |

## Commandes et résultats (relancés sur l’arbre final)
`CARGO_TARGET_DIR=<checkout>/target`, journaux dans `test-build\claude-v14-evidence\*-final.log`.

| Commande | Résultat |
|---|---|
| `cargo fmt --all --check` | 0 |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | 0 |
| `cargo test --workspace --locked` | 0 — **308 réussis** (96 + 196 + 16), 2 ignorés (`dashboard_large_history`, `update::reaches_github_and_parses_the_reply`). Codex annonçait 307 ; non vérifié. |
| `cargo build --release -p jot --locked` | 0 — `jot.exe` SHA-256 `C399D8F9A237EEFFDC22EBBAB6080C9F9F59243D34E196069A7C3056E8FC93AB` |
| `Validate-Hud.ps1 -Executable test-build\codex-dictionary-v14\jot.exe` | PASS, 2 cycles × 12 états (`hud-visible-final`) |
| idem `-InitiallyHidden` | PASS (`hud-hidden-final`) |
| `Validate-Icons.ps1` (PE puis `-ProcessId`) | PASS : 9 tailles 16–256 px ; fenêtre 16/32 px à 96 DPI |

## Vérifications GUI (profil isolé, binaire packagé, clavier simulé via SendInput qui traverse le vrai hook)
- **A — Dictionnaire** : correction dans Historique → suggestion en attente sans règle active ; accepter → règle active ; rescan, refus, rescan → 0 nouvelle suggestion, refus persistant ; désactiver/activer ; modifier ; suppression d’une dictée (unitaire, avec confirmation) → suggestion orpheline purgée **sans ouvrir Dictionnaire** ; suppression groupée de la source d’une règle acceptée → règle conservée ; redémarrage → règle persistée.
- **B — Raccourcis** : capture puis Annuler sans effet ; Ctrl+Alt+Espace → conflit Windows réel signalé, ancien raccourci conservé ; Ctrl+Maj+D enregistré (`customHotkey {modifiers:6, vk:68}`) ; Toggle : 2 cycles, 5 répétitions de touche → un seul démarrage, Échap annule ; Maintenir : relâcher D ou Ctrl d’abord finalise, combinaison partielle ou avec modificateur en trop ignorée ; Rétablir par défaut → Ctrl droit en Maintenir fonctionne, l’ancienne combinaison ne déclenche plus rien.
- **C — Lien Gemini** : clic dans Paramètres → Confidentialité et conservation → le navigateur par défaut affiche « API keys | Google AI Studio ». Aucune connexion. L’assistant utilise la même fonction `open_gemini_key_page`.
- **D — Icônes** : icône visible dans la barre de titre ; HUD `WS_EX_TOOLWINDOW` (absent de la barre des tâches) ; fenêtre principale `WS_EX_APPWINDOW`.
- Sans clé, la dictée échoue localement avec « Ajoutez une clé API valide dans les paramètres » ; aucun appel réseau.
- ZIP extrait et lancé : l’historique se reconstruit depuis `meta.json` (3 dictées fictives).

## Livrables
- Dossier portable : `<checkout>\test-build\codex-dictionary-v14\` (`jot.exe` final, `jot-test-profile`, profil fictif Codex, `LIRE-MOI.md` mis à jour, `VALIDATION.md`). L’ancien binaire Codex est conservé : `codex-dictionary-v14-evidence\jot-codex-60EDF637.exe`.
- ZIP : `<checkout>\test-build\Jot-1.4.0-test-portable.zip` (SHA-256 `AD0BB330D4CF02786EF8C24C11BFBFD5DA2EEE75E126275538E16D03EAEC0BD6`). Contient binaire, marqueur, licences, notes, et le profil fictif sans SQLite, journaux, audio ni clé.
- Preuves : `test-build\claude-v14-evidence\`. Parcours GUI : `test-build\claude-v14-walkthrough\`. Ce dossier contient de **courts enregistrements audio réels de la pièce** captés pendant les tests de raccourcis. Ne pas le partager ; il est exclu du ZIP.

## Limites
- Pas de dictée Gemini réelle (aucune clé manipulée) ni d’insertion dans une application tierce.
- Raccourcis testés par SendInput, pas au clavier physique.
- Barre des tâches, Alt+Tab et zone de notification : contrôle par API uniquement. Aucune capture faite, car elle aurait montré les autres fenêtres de l’utilisateur.
- Thème clair, interface anglaise, fenêtre étroite, DPI > 96 et multi-écrans : non vérifiés à l’écran (chaque texte a sa version EN : la macro de `messages.rs` exige les deux langues).
- Un raccourci en conflit au démarrage reste inactif jusqu’à ce qu’il soit réenregistré (comportement documenté, non modifié).
- Observé hors périmètre : « Temps économisé estimé −2 s » au tableau de bord (statistiques 1.3).

## Étapes manuelles pour l’utilisateur
1. Quitter tout Jot en cours, extraire le ZIP, lancer `jot.exe`.
2. Coller sa clé (Paramètres → Confidentialité et conservation) et faire 3–4 vraies dictées dans ses applications.
3. Enregistrer son propre raccourci au clavier physique ; essayer Toggle, Maintenir et Échap.
4. Corriger une dictée et accepter la suggestion dans Dictionnaire ; vérifier qu’elle s’applique à la dictée suivante.
5. Regarder l’icône dans la barre des tâches, Alt+Tab et la zone de notification ; essayer thème clair et anglais.
6. Si tout convient, décider vous-même de la fusion, du tag et de la publication.

## Feuille de route V1.5 (notée, non implémentée)
- Étude ASR français local et LLM local de reformulation (comparaison qualité/latence/poids, avec Opus 5.5).
- Installateur Windows.
- Notification de mise à jour basée sur les releases GitHub.

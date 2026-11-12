# Jot 1.5.1 (candidat de test) — rapport

Branche locale `claude/v151`, base 1df6dc8 (1.5.0). Rien n'a été poussé, tagué, publié ni installé. La release 1.5.0, le dépôt principal (`dev`) et le profil réel de l'utilisateur n'ont pas été touchés. Dans ce rapport, `<worktree>` désigne le dossier du worktree.

| Commit | Objet |
|---|---|
| 47ad8a1 | A/B : timeouts CPU de la reformulation |
| 5bc16cb | E : raccourcis toujours modifiables, plus de formes enregistrables |
| a264691 | C : suppression de l'indicateur au repos et de son réglage |
| c717b80 | D : note de faisabilité iGPU Intel / NPU |
| 3ab6e4f | version 1.5.1 |

## 1. Avant / après

1. **Reformulation sur CPU.**
   - Avant : `llama-server` avait plusieurs slots. Une requête abandonnée envoyait la suivante sur un slot froid et vide : 4,1 s de prefill mesurées, et +230 Mo par slot. Juste après « LLM ready », la première dictée payait le prefill froid de la consigne (~800 tokens). Le budget était fixe, `10 s + 0,25 s/mot`, plafonné à 90 s.
   - Après :
     - un seul slot (`-np 1`) ;
     - la consigne système est amorcée dès le chargement, sous le verrou de démarrage ;
     - le budget s'adapte aux vitesses mesurées sur la machine : 2 × le temps attendu + 5 s, entre 15 et 120 s. Les vitesses sont divisées par deux après un timeout.
   - Les contrôles de fidélité sont inchangés. Aucun repli vers Gemini, silencieux ou non : en cas d'échec, le brut est inséré.
2. **Historique.**
   - Avant : un seul statut, « timeout ».
   - Après, des statuts distincts :
     - le contrôle de fidélité qui a rejeté le texte ;
     - le timeout ;
     - l'échec de démarrage ;
     - l'erreur du moteur ;
     - et dans chaque cas, la mention que le brut a été conservé.
   - Les journaux par phase ne contiennent que des durées et des nombres de tokens, jamais de texte.
3. **Modèle 2B optionnel.** Non ajouté : aucun artefact n'a été fourni avec licence, SHA-256 et benchmark.
4. **Indicateur au repos.**
   - Avant : une barre de 56×5 px au repos, et le réglage « Afficher l'indicateur au repos ».
   - Après : rien n'est dessiné au repos, après une annulation ni à la fin de l'affichage du résultat (1,6 s). Le réglage est supprimé.
   - Le HUD reste affiché pendant l'écoute, le chargement et la transcription, sans prendre le focus ni bloquer les clics.
5. **Ancien `settings.json`.**
   - La clé `showIdleIndicator` est ignorée au chargement. Toutes les autres valeurs sont conservées, et la clé disparaît à la prochaine écriture.
   - C'est prouvé par le test `a_removed_idle_indicator_key_keeps_every_other_setting`, avec la clé à `true` puis à `false`.
6. **Raccourci impossible à changer : quatre causes, toutes corrigées.**
   1. Le bouton Enregistrer exigeait l'état Idle ou Done. Après un Échap ou une erreur, il restait désactivé jusqu'à la dictée suivante. C'est le cas que vous avez constaté.
   2. Un relâchement de touche perdu sur le bureau sécurisé (Win+L, Ctrl+Alt+Suppr, UAC) laissait un modificateur bloqué. Les modificateurs que Windows ne signale plus comme enfoncés sont maintenant relâchés.
   3. Une touche seule appuyée avant de passer à un accord perdait son relâchement.
   4. L'interface affichait « enregistré » sans vérifier que le raccourci était actif. Elle affiche maintenant l'état réel : Enregistré, Actif ou Échec.
7. **Formes de raccourci.**
   - Avant : la capture était limitée.
   - Après :
     - aperçu en direct ;
     - Confirmer seulement une fois toutes les touches relâchées ;
     - chaque refus est expliqué (Alt+Espace, Win+touche, Alt+F4, Ctrl+Alt+Suppr, touches non prises en charge) ;
     - accords de modificateurs seuls acceptés (Alt droit + Maj droite…) ;
     - ponctuation, pavé numérique et touches de navigation acceptés ;
     - le faux Ctrl d'AltGr est mis à part.
8. **iGPU / NPU Intel.** Voir `docs/v151/NOTE-GPU-INTEL-NPU.md` : rien n'est intégré.
   - Le NPU n'est pas exploitable : Qwen3.5 échoue sur NPU selon la doc llama.cpp OpenVINO, et le backend ne traite que du texte, donc pas l'ASR.
   - L'iGPU par Vulkan est déjà fourni, mais il n'est choisi que si son nom contient « Arc ». Il reste à mesurer sur le PC cible.
   - Aucun benchmark n'a été fait : ce poste n'a pas de matériel Intel.
9. **Documentation.** README, `docs/hud/REPORT.md` et `Validate-Hud.ps1` sont à jour. L'option `--initially-hidden` est retirée.

## 2. Contrôles de qualité

| Contrôle | Résultat |
|---|---|
| `cargo fmt --all --check` | OK |
| `cargo clippy --all-targets --locked -- -D warnings` | OK |
| `cargo test` | 111 (jot) + 219 (jot-core, 4 ignorés) + 16 + 0 : tout passe |
| build release + `scripts/package.ps1 -TestBuild` | OK |

## 3. E2E sur le binaire extrait du ZIP final (profil isolé)

Script : il extrait le ZIP, prépare un `settings.json`, lance `--test-settings`, navigue avec UIA, prend des captures, envoie les touches avec SendInput, puis lit les états dans les journaux du profil de test. Résultats dans `<worktree>\test-build\v151\e2e-4\`. L'audio de test est supprimé après chaque cas.

Seuls des raccourcis à gauche ont été envoyés, pour ne jamais déclencher le Jot réel. Ctrl droit n'a pas été pressé.

| Cas | Vérifié | Résultat |
|---|---|---|
| E1 profil | Racine, réglages et historique sous `app\profile`. Écriture du démarrage automatique refusée. Contrôle d'instance OK. | OK |
| E2 migration | Fichier 1.5.0 avec `showIdleIndicator:true` : mode Basculer et thème Sombre conservés. Pas de bascule d'indicateur dans « Apparence et langue ». Ligne d'état « Actif : Ctrl droit (par défaut) ». Fichier ni réinitialisé ni réécrit. | OK |
| E3 accord de modificateurs seuls | Ctrl gauche + Alt gauche, en mode Basculer. Ligne d'état « Actif : Ctrl gauche + Alt gauche ». États : Warming › Recording › Recording (verrouillé) › Finalizing › Transcribing › Failed. | OK* |
| E4 accord avec touche | Ctrl + Alt + F9. Ligne d'état « Actif : Ctrl + Alt + F9 ». Même séquence d'états. | OK* |
| HUD | `docs/hud/Validate-Hud.ps1` sur le même exe : 2 cycles × 11 états. Rendu, focus, taille, position et animation OK sur les 22 lignes, avec 2 clics reçus par ligne. L'état initial est vide. | PASS |

\* `Failed` est attendu : le profil de test n'a pas de clé API. La ligne d'état n'est pas exposée à UIA (limite connue de GPUI). Elle a donc été vérifiée sur les captures `*-appearance.png`.

## 4. Livrables

Dossier : `<worktree>\target\dist\`, avec une copie dans `<worktree>\test-build\v151\`.

| Fichier | SHA-256 |
|---|---|
| `Jot-1.5.1-test-portable.zip` | `715D30C011D1318F6A68B3E6085350D805BFEA4AFD38A1CDBE95A3B9CC0B7358` |
| `Jot-1.5.1-test-setup.exe` | `7C5A4FE8B8E025F63D8B62DEA58AB17AE2821695F260A6C0C71D80BDA66D0AAA` |
| `jot.exe` (dans le ZIP, 25 964 032 octets, `jot/1.5.1`) | `4881ECBB6E0ED1FEE18A388779B11F53E56DD51A8D287574A51E15A079B98424` |

Contenu du ZIP :
- `jot-test-profile` : le marqueur qui place les données dans `profile\` à côté de l'exe ;
- `jot.exe` ;
- `LICENSE`, `NOTICE-FR.md`, `README.md`, `THIRD_PARTY_NOTICES.md`.

Le contrôle d'instance unique est global : quittez Jot 1.5.0 avant de lancer le ZIP de test.

## 5. À vérifier sur le PC cible (non couvert ici)

1. Ctrl droit (le raccourci par défaut) en dictée réelle, avec la clé API : insertion dans un vrai champ.
2. Reformulation CPU : plus aucun timeout sur des dictées de 30 à 150 mots, première dictée juste après le lancement comprise. Comparer les durées par phase dans `profile\logs`.
3. AltGr : il ne doit pas déclencher un raccourci Ctrl.
4. Win+Alt : l'accord passe, et le menu Démarrer ne s'ouvre pas.
5. Raccourci après Win+L puis déverrouillage, et après une invite UAC : il fonctionne toujours et se modifie encore.
6. Après Échap pendant une dictée, puis après une erreur : le bouton Enregistrer est actif.
7. Au repos : plus aucune barre visible en bas de l'écran.
8. iGPU / NPU : les commandes du § 1 et le benchmark du § 2 de la note GPU, sans téléchargement.

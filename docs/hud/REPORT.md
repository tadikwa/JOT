# Correctif du HUD Windows — validation

Base : `33a8d98`, avant les essais `1ae65d9`, `99e5c99` et les modifications V2/V3/V4 non committées. Ces essais restent dans le checkout initial, branche `dev`. Travail isolé sur `codex/hud-click-through`.

## Cause démontrée

Le HWND est récupéré par `raw_window_handle` auprès du vrai `Window` GPUI. Sur la base originale, son style étendu était `0x082001A8` : transparent/noactivate/toolwindow/topmost/no-redirection-bitmap, sans layered. Le rendu était présent dans les captures écran, mais la cible d’un autre processus recevait **0/40 clics**, y compris dans les zones visuellement vides.

`WS_EX_TRANSPARENT` seul ne garantit pas le passage des clics entre processus : sa définition générale concerne l’ordre de peinture. Le comportement d’entrée explicitement documenté pour une fenêtre layered transparente exige aussi `WS_EX_LAYERED`. Une nouvelle fenêtre layered doit être initialisée pour devenir visible.

Correctif : conserver le HUD GPUI, ajouter `WS_EX_LAYERED` et appeler `SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA)`, en vérifiant le résultat et les styles réellement appliqués. Ne pas supprimer `WS_EX_NOREDIRECTIONBITMAP`. Aucun changement de classe, aucun traitement spéculatif de fenêtre `#32770`, aucun remplacement du moteur de dictée.

Le GPUI épinglé à `7733b9922665f103abda7c6a3fde6b9dfdc8eba9` utilise `CreateSwapChainForComposition` et un visual DirectComposition. Il n’appelle pas `UpdateLayeredWindow` : la restriction de Microsoft après `SetLayeredWindowAttributes` ne rencontre donc pas ce chemin de rendu. La compatibilité effective a été vérifiée sur cette session par captures et clics réels ; elle n’est pas déduite de la documentation seule.

Les dimensions originales sont appliquées avec `show: true`. Dans cette version de GPUI, `show: false` conserve seulement `initial_placement`, sans appliquer immédiatement les dimensions. Un `ShowWindow` Win32 direct ne consomme pas ce placement interne : cela explique le défaut de dimensions introduit par les essais précédents. Les valeurs 3840×1026 rapportées auparavant ne sont pas reprises comme preuve : elles n’ont pas été reproduites ici.

## Modifications

- Appels critiques Win32 vérifiés ; `GetLastError` réinitialisé pour les fonctions dont zéro peut être un succès. `ShowWindow` retourne la visibilité précédente : la visibilité après appel est vérifiée séparément. Les attributs DWM d’apparence non disponibles restent facultatifs.
- Topmost appliqué par `SetWindowPos`, puis vérifié dans les styles réels. Focus interdit par `WS_EX_NOACTIVATE` et affichage sans activation.
- Après un déplacement entre domaines DPI, relire les dimensions physiques et le DPI après `WM_DPICHANGED`, puis recentrer dans la zone de travail de l’écran de la fenêtre au premier plan.
- En cas d’échec d’initialisation, fermer le HUD créé et refuser le démarrage.
- Portable : marqueur `jot-test-profile` à côté de l’exécutable ; paramètres/dictionnaire/SQLite/enregistrements/logs dans `profile`. Le marqueur s’applique aussi au double-clic sans variable d’environnement. `JOT_TEST_PROFILE=1` active le même mécanisme.
- Credential Manager : espace distinct par dossier portable, aucune copie de clé personnelle. Démarrage automatique désactivé, écritures Run refusées en profil test.
- Mutex commun aux nouvelles versions ; en profil test, détection supplémentaire des anciens processus `jot.exe`, avec PID et chemin. Refuser la deuxième instance avant les services et hooks.

## Banc reproductible

`jot.exe --hud-validate <nouveau-dossier>` ouvre le vrai `open_hud` et le vrai `PillView`, sans construire Services, sans microphone, clé, hook, tray ou données personnelles. Une cible Win32 dans un autre processus compte les vrais `WM_LBUTTONDOWN` après `SendInput`. Le contrôle ne poste jamais artificiellement ces événements à la cible.

Les captures viennent de l’écran composé (`BitBlt` + `CAPTUREBLT`, après `DwmFlush`) dans une zone entièrement couverte par la cible de couleur connue. La visibilité exige des pixels dessinés, un fond transparent conservé et l’absence de grand rectangle opaque. L’animation compare deux captures espacées de 240 ms. Le clic sur la pilule est sélectionné à partir d’un pixel effectivement dessiné dans la colonne centrale ; coordonnées et couleur BGR sont consignées. Le second clic est dans le pourtour transparent.

**Correction de validation :** les premiers essais utilisaient `bottom-7`, dans la marge transparente de 8 points. Leurs clics ne démontraient pas le passage sur la pilule. Ces essais intermédiaires sont conservés pour traçabilité et ne fondent pas la conclusion finale sur les pixels dessinés.

États : premier affichage, contrôle fenêtre cachée, repos, état caché avec HWND affiché, warming, recording, verrouillage, processing, succès, notice, erreur et retour caché ; deux cycles par lancement. `--initially-hidden` permet de tester le premier affichage sans indicateur au repos.

`processes.json` identifie les deux PID et l’exécutable. `result.json` indique HWND, classe réellement lue, parent/owner, PID/thread, styles, rectangle, DPI, placement, pixels, clics, focus et animation. La cible est fermée normalement puis attendue ; un Job Object tue la cible si le contrôleur disparaît. La cible surveille aussi son parent et possède sa propre échéance de 80 secondes ; le contrôleur est borné à 90 secondes. Le script `Validate-Hud.ps1` conserve son processus et le ferme en `finally` sans toucher aux autres Jot.

## R�sultats effectivement obtenus

| Niveau | R�sultat |
|---|---|
| Compilation Windows Release | `cargo build --release --bin jot --locked` : code 0 |
| Tests du workspace | `cargo test --workspace --locked` sous `JOT_TEST_PROFILE=1` : 64 + 156 = **220 r�ussis**, aucun �chec ; 1 test r�seau GitHub volontairement ignor� par le projet |
| R�gression graphique/Win32 du livrable exact | Deux lancements du script fourni, repos initial activ� puis d�sactiv� : **48 cas, 96/96 clics gauche re�us**, dont **34 sur des pixels dessin�s** ; tous les contr�les rendu/focus/topmost/dimensions/placement/animation valides |
| Dimensions | Client 600�96 pixels � 96 DPI ; rectangle ext�rieur GPUI 606�99, bordures comprises ; conserver cette distinction |
| Param�tres r�els | Fen�tre affich�e ; clics sur � Show the resting indicator � : fichier isol� pass� � false puis true |
| Isolation | Routage v�rifi� sans variable d�environnement gr�ce au marqueur ; appels autostart d�clar�s inactifs et �criture refus�e ; tests de s�paration des cibles Credential Manager |
| Coexistence | Mutex : deuxi�me instance refus�e ; ancien processus sans mutex : refus avec PID et chemin ; apr�s fermeture, nouvelle acquisition possible |
| Nettoyage | Fermeture normale des contr�leurs et cibles v�rifi�e ; interruption volontaire du contr�leur : cible automatiquement arr�t�e |
| Donn�es personnelles | Comparaison SHA-256 de tous les fichiers de `%LOCALAPPDATA%\Jot`, tailles et dates, et comparaison de l�entr�e Run avant/apr�s : identiques ; aucun jot.exe restant � l�inventaire final |
| Dict�e r�elle | Non test�e : aucun envoi Gemini ni insertion de texte r�elle revendiqu�s |

Le dernier ex�cutable portable valid� a pour SHA-256 :

`25CDC125D7C1C7062EDDF2D58357AB4356EB966417FCCFAA21E16A7DB765DBE2`

Preuves locales (hors Git et hors portable, sans capture du bureau personnel) :

`E:\05 - IA\01 - GITHUB\JOT\test-build\codex-hud-evidence`

- `baseline/result.json` : reproduction sur la base originale.
- `packaged-runner/` et `packaged-hidden-runner/` : contr�leurs, empreinte exacte du livrable, r�sultats, captures et confirmation d�arr�t.
- `HUD-STATES.png` : montage de captures finales repos, enregistrement, traitement, succ�s, erreur et �tat vide.
- `workspace-tests.txt`, `release-build.txt` : sorties r�elles des commandes.
- `profile-check.json`, `duplicate-check.json`, `legacy-instance-check.json`, `after-settings-exit-check.json`, `settings-toggle-off.json`, `settings-toggle-on.json`, `interruption-cleanup.json`, `final-integrity.json` : contr�les associ�s. Le contr�le `duplicate-check` est celui effectu� pendant que la premi�re fen�tre Settings �tait encore ouverte ; celui apr�s sa fermeture acquiert correctement le mutex.

Cargo a �mis un avertissement lors de son nettoyage automatique du cache global (acc�s refus� � un ancien fichier de d�pendance). Aucune tentative de nettoyage de ce cache n�a �t� faite ; compilation et tests ont termin� avec le code 0. La revue ind�pendante a conduit aux trois corrections de validation/s�curit� d�crites ci-dessus et n�a plus relev� de probl�me concret dans ces corrections.

## Limites de la preuve

La session graphique ne présente qu’un écran à 96 DPI. Le banc énumère les écrans disponibles, mais aucun déplacement réel entre deux écrans ou DPI hétérogènes n’a pu être validé. Les coordonnées négatives ont un test arithmétique ; elles n’ont pas été testées sur un second écran réel.

Le banc force les états visuels et ne constitue pas une dictée Gemini réelle. Microphone, envoi, insertion dans une application tierce et plusieurs dictées vocales réelles nécessitent encore validation manuelle avec une clé saisie dans le profil test.

## Sources primaires consultées le 10 novembre 2026

- [Microsoft — Window Features / Layered Windows](https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features#layered-windows) : initialisation et entrée des fenêtres layered.
- [Microsoft — SetLayeredWindowAttributes, mise à jour 22 février 2024](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setlayeredwindowattributes) : retour et restriction sur UpdateLayeredWindow.
- [Microsoft — Extended Window Styles](https://learn.microsoft.com/en-us/windows/win32/winmsg/extended-window-styles) : styles de peinture, activation et redirection.
- [Microsoft — WM_NCHITTEST](https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-nchittest) : `HTTRANSPARENT` est limité aux fenêtres du même thread ; ce retour seul ne constitue pas le correctif.
- [Source GPUI épinglée — fenêtre](https://github.com/zed-industries/zed/blob/7733b9922665f103abda7c6a3fde6b9dfdc8eba9/crates/gpui_windows/src/window.rs), [rendu DirectX](https://github.com/zed-industries/zed/blob/7733b9922665f103abda7c6a3fde6b9dfdc8eba9/crates/gpui_windows/src/directx_renderer.rs), consultées dans le cache Cargo correspondant à cette révision.
- [Issue d’origine #2](https://github.com/ubranch/jot-gemini-transcribe-Windows/issues/2), consultée par le connecteur GitHub.

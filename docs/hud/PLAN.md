# Plan HUD Windows

Base : 33a8d98 ; les essais dev et modifications locales restent dans le checkout initial.

1. Reproduire avec le vrai open_hud/PillView, sans Services, données, clé, hook ou tray.
2. Cible Win32 dans un autre processus : compter WM_LBUTTONDOWN après SendInput, vérifier le focus et capturer uniquement sa zone sous le HUD. Refuser un résultat sans pixels visibles.
3. Tester un seul changement de composition ; ne conserver GPUI que si visibilité et passage des clics sont mesurés simultanément.
4. Vérifier dimensions, topmost, HWND/PID/thread/styles, états caché/repos/enregistrement/verrouillage/traitement/succès/erreur et répétitions ; écrans présents et redémarrage.
5. Isoler le portable (données, Credential Manager, Run, mutex et raccourci), compiler Release, tests workspace, livrer rapport et commit local sans push.

Points de vigilance : DPI différents, coordonnées négatives, tout premier affichage, démarrage masqué, fermeture d'un processus de test en cas d'échec. Les dictées réelles, microphone et Gemini ne sont pas simulés comme validés.

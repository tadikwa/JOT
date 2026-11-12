# Captures de l’application

Les images de `../public/stills/` sont des captures réelles de Jot 1.5.0. Pour les refaire :

1. Construire le build de test : `scripts/package.ps1 -TestBuild`. Il contient le marqueur
   `jot-test-profile`, donc tout (réglages, historique, modèles, journaux) va dans `profile\` à côté
   de `jot.exe`. Le vrai profil `%LOCALAPPDATA%\Jot` n’est jamais lu ni écrit.
2. Copier ce dossier ailleurs, lancer `jot.exe` et passer le premier lancement (sans clé Gemini).
3. Pour l’historique, générer les dictées fictives avec `docs/v15/bench/tts.ps1` (voix de synthèse
   Windows, textes de `docs/v15/bench/smart_e2e.json`) et les faire traiter par ce `jot.exe` en mode
   100 % local.
4. Capturer chaque fenêtre avec `win.ps1`, qui n’agit que sur les fenêtres d’un PID donné :

   ```powershell
   powershell -File win.ps1 list <pid>                  # fenêtres visibles de ce processus
   powershell -File win.ps1 cap <pid> 0 C:\tmp\h1.png   # PrintWindow : la fenêtre seule
   powershell -File win.ps1 click <pid> 0 120 340       # clic posté (coordonnées client)
   ```

   `PrintWindow` rend la fenêtre seule, jamais le bureau ; les clics et touches sont postés dans sa
   file de messages, la vraie souris ne bouge pas.

Ces captures ne montrent ni la pastille de dictée ni l’insertion dans une autre application : il
aurait fallu filmer le bureau.

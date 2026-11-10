# Tester Jot — mode Toggle

1. Quittez Jot depuis la zone de notification s’il est déjà ouvert. Lancez `jot.exe` en gardant le fichier `jot-test-profile` à côté. Le dossier `profile` et la clé Gemini de ce portable sont indépendants. Ajoutez une clé de test pour vérifier la transcription.
2. Dans **Settings → General → Mode d’activation de la dictée**, sélectionnez **Appuyer pour démarrer / arrêter**. Relancez Jot et vérifiez que le choix est conservé.
3. Dans le Bloc-notes, appuyez brièvement sur la touche configurée (Ctrl droit par défaut). Relâchez, attendez quelques secondes et parlez : le HUD doit rester en écoute. Appuyez à nouveau : le HUD passe au traitement, puis le texte s’insère. Cliquez à travers le HUD et vérifiez que la fenêtre au-dessous reçoit le clic.
4. Recommencez en gardant la touche enfoncée un instant à chaque appui : les répétitions ne doivent pas basculer le mode. Essayez Échap pendant l’écoute et pendant le traitement, puis redictez. Un appui pendant le traitement ne lance pas d’autre dictée.
5. Sélectionnez **Maintenir pour parler** : maintenez la touche pour parler et relâchez pour transcrire. En changeant le mode pendant une dictée, vérifiez que la dictée en cours se termine selon son mode initial.

La limite existante de dix minutes par dictée s’applique aux deux modes.

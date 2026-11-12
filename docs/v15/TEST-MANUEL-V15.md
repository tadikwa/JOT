# Test manuel rapide — Jot 1.5 (paquet de test portable)

Fermez d'abord votre Jot habituel : une seule instance peut tourner. Décompressez `Jot-1.5.0-test-portable.zip` dans un dossier neuf. Le paquet utilise son propre profil (marqueur `jot-test-profile`).

1. **Premier lancement** : lancez `jot.exe` et choisissez « 100 % local ». Vérifiez que la taille, la licence et le SHA-256 s'affichent avant le téléchargement, et que rien ne démarre sans confirmation.
2. **Mémoire** : après le lancement, le Gestionnaire des tâches ne montre aucun `llama-server.exe`. Maintenez la touche de dictée : les modèles se chargent pendant que vous parlez. Dans les réglages, cliquez « Libérer la mémoire maintenant » : les `llama-server.exe` disparaissent.
3. **Prose longue** : dictez un paragraphe avec « euh », « en fait » et une phrase répétée plus loin. Le texte doit sortir propre, avec les mêmes faits.
4. **Liste d'étapes** : dictez « Pour installer le serveur, d'abord tu télécharges l'image, ensuite tu la copies sur la clé, et enfin tu redémarres la machine. » On attend « Pour installer le serveur : » suivi de 3 lignes numérotées.
5. **Liste à puces** : dictez « Pour samedi il faut du pain, des tomates, du fromage et deux bouteilles d'eau. » On attend l'introduction puis 4 puces. Vérifiez que les retours à la ligne restent dans l'application cible, dans l'historique et après un copier-coller.
5 bis. **Fidélité** : dictez « Pour la réunion il faut parler du budget, et peut-être du déménagement, je ne sais pas. » puis « À acheter : deux câbles HDMI et une multiprise. » Le « peut-être » et le « deux » doivent rester. Si Jot n'a pas pu reformuler sans perdre un de ces éléments, il insère le texte brut : c'est voulu.
6. **Vocabulaire** : ajoutez « Ansible » et « Jot » au dictionnaire, puis dictez « Lance le playbook Ansible sur Jot ». Contre-exemple : « Le contrôle qualité est passé » ne doit pas changer.
7. **Historique** : corrigez un mot mal entendu. Jot propose « Mémoriser « X » dans mon vocabulaire ». Testez aussi le glisser, le double-clic, le triple-clic, Maj+clic et Ctrl+X dans l'éditeur.
8. **Fermeture** : quittez Jot. Aucun `llama-server.exe` ne doit rester.

Signalez les cas ratés avec le texte brut et le texte obtenu, sans audio personnel.

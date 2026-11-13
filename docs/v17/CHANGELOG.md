# Jot 1.7.0 — changelog candidat

Base : v1.6.0 (`b070742`). Candidat de test, non publié.

## Nouveautés

- **Mises à jour et À propos** : une page dans les Paramètres, aussi accessible depuis l'icône de la barre des tâches. Elle affiche :
  - la version installée et la dernière version publiée ;
  - le canal et la date de la dernière vérification ;
  - les notes de version, en texte simple.
- La vérification ne se fait que sur demande. Les cas hors ligne, limite GitHub, cache et « à jour » sont indiqués explicitement.
- **Téléchargement vérifié** : seul `Jot-<version>-setup.exe` de la release est récupéré. Sa taille et son SHA-256 sont comparés à `SHA256SUMS.txt` de la même release. Le téléchargement est refusé si :
  - le fichier provient d'un autre hôte ;
  - `SHA256SUMS.txt` est absent ;
  - la version est plus ancienne.
- Le téléchargement affiche sa progression et peut être annulé ou relancé. Le fichier est gardé dans `%LOCALAPPDATA%\Jot\updates`. Il n'est jamais téléchargé automatiquement.
- **Installation seulement au clic** : le bouton « Installer » ne devient actif qu'après la vérification.
  - Jot demande avant de se fermer.
  - Il revérifie qu'aucune dictée n'est en cours et que le fichier n'a pas changé.
  - Il lance ensuite l'assistant d'installation normal, jamais en silence, et ne redémarre rien de force.
- **Fermeture accompagnée** : l'installateur demande au Jot ouvert de son dossier de se fermer, via un évènement nommé, puis attend un temps borné.
  - Un Jot occupé (dictée en cours) reste ouvert.
  - Une copie plus ancienne ou étrangère reçoit un message de fermeture manuelle, avec Réessayer ou Annuler.
  - Aucun processus n'est tué : jamais de `taskkill`, de `TerminateProcess` ni d'arrêt par nom.
  - `AppMutex` reste le garde-fou contre le remplacement de l'exécutable actif.
- **À propos** : le texte décrit désormais les deux modes, local (tout sur ce PC) et Gemini (votre clé, envoi direct à l'API).
- **Expérimental, désactivé par défaut : sauvetage partiel des longues reformulations locales.**
  - Il se déclenche seulement si le contrôle refuse la reformulation entière, ou si elle dépasse le délai, sur une dictée de 60 mots ou plus.
  - Jot retente alors la reformulation par groupes de phrases :
    - il ne coupe qu'entre deux phrases autonomes, jamais au milieu d'une négation, d'un « si… alors », d'un « non, plutôt », d'une citation, d'une adresse IP, d'un chemin ou d'un nombre suivi de son unité ;
    - il vérifie chaque partie, puis l'ensemble ;
    - il garde brute toute partie refusée.
  - Le sauvetage est borné à 8 appels, 60 s et deux fois le nombre de mots de la dictée.
  - L'Historique indique « complet », « partiel » ou la transcription brute conservée.
  - Le détail par partie ne va que dans le diagnostic local optionnel, avec sa durée de conservation. Les journaux ne contiennent jamais le texte.
  - Voir le rapport : la fidélité mesurée ne justifie pas de l'activer par défaut.

## Corrections

- **Dictionnaire** : les corrections sont apprises sur les dictées longues, jusqu'à 6 000 caractères au lieu de 1 000.
  - La lecture se limite au segment autour du curseur.
  - Le texte collé n'attend plus comme s'il était tapé : on observait 13,5 s sur une dictée de 317 mots dans Brave/ChatGPT.
  - Les paragraphes et les espaces insécables des éditeurs web ne cassent plus l'ancrage.
  - Une nouvelle faute d'un terme connu (« Jiotte → Jot ») est proposée comme variante à valider.
- L'icône de la barre des tâches disparaît bien à la fermeture.

## Inchangé

- Comportement par défaut de la reformulation : identique à 1.6.0.
- Réglages, Historique, modèles, presse-papiers et raccourci sont conservés à la mise à jour.
- Les téléchargements de modèles restent séparés des mises à jour.
- Aucun réseau pendant une dictée locale.

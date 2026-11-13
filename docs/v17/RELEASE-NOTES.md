# Jot 1.7.0 — Mises à jour vérifiées et longues dictées reformulées

Jot est une application de dictée vocale pour Windows, gratuite et open source.
- Vous dictez naturellement : Jot transcrit votre voix, peut nettoyer et reformuler vos phrases, puis insère le résultat dans l'application que vous utilisez.
- Le traitement peut se faire **entièrement en local**, avec les modèles Qwen téléchargés sur votre PC. Il n'y a ni abonnement ni envoi de vos dictées à un service distant.
- Le mode Gemini reste optionnel. Il utilise votre propre clé API, et vos dictées sont alors traitées à distance.

## Nouveautés de la version 1.7.0

### Mises à jour et À propos

Une nouvelle page, dans les Paramètres et depuis l'icône de la barre des tâches, affiche la version installée, la dernière version publiée, la date de la dernière vérification et les notes de version.

- **Rien n'est téléchargé sans votre clic.** Jot ne récupère que l'installateur de la version, puis compare sa taille et son SHA-256 au fichier `SHA256SUMS.txt` publié avec elle. Un fichier d'une autre origine, sans empreinte ou d'une version plus ancienne est refusé.
- **Installation seulement après vérification.** Le bouton « Installer » ne s'active qu'une fois le fichier vérifié. Jot demande confirmation, se ferme proprement s'il n'y a pas de dictée en cours, puis ouvre l'assistant d'installation habituel. Jamais d'installation silencieuse.
- **Fermeture accompagnée.** À partir de cette version, l'installateur demande au Jot ouvert de se fermer de lui-même, sans jamais forcer l'arrêt d'un processus. Une version plus ancienne, comme 1.6.0, ne sait pas répondre à cette demande : fermez-la depuis l'icône de la barre des tâches avant d'installer 1.7.0.
- **À propos** décrit les deux modes : local (tout sur ce PC) et Gemini (votre clé, envoi direct à l'API de Google).

### Apprendre de mes corrections : activé par défaut

La fonction introduite en 1.6.0 est maintenant **activée par défaut** pour une nouvelle installation. Un profil 1.6.0 garde le réglage qu'il avait enregistré : si la fonction était désactivée, activez-la dans Paramètres → Confidentialité et conservation.

- Pendant 30 secondes après une insertion, Jot relit uniquement le champ qui a reçu le texte, par l'accessibilité de Windows. Ni capture d'écran, ni frappes, ni champs de mot de passe. **Rien ne s'applique sans votre accord** : chaque correction attend dans le Dictionnaire.
- Les dictées longues sont maintenant prises en compte, jusqu'à 6 000 caractères. Les paragraphes et espaces insécables des éditeurs web ne gênent plus.
- Une nouvelle faute sur un terme déjà connu (« Jiotte » pour « Jot ») est proposée comme variante à valider.
- Le collage n'attend plus inutilement après une longue dictée : sur 317 mots dans Brave/ChatGPT, on observait 13,5 s d'attente.

### Expérimental : sauver les longues reformulations par parties

En mode local, sur une dictée de 60 mots ou plus, le contrôle de fidélité refuse parfois la reformulation entière, et Jot insérait alors le texte brut. Désormais, Jot retente la reformulation par groupes de phrases, vérifie chaque partie puis l'ensemble, et garde brute toute partie refusée.

- **Activé par défaut, et expérimental.** Sur nos essais, toutes les longues dictées refusées en bloc ont été reformulées, au moins en partie. Mais la relecture montre que **8 textes sauvés sur 10 perdaient une nuance** que le brut gardait : « je crois », une condition en « si », une auto-correction orale. **Relisez les longues dictées avant de les envoyer.**
- Le découpage ne coupe jamais au milieu d'une négation, d'un « si… alors », d'un « non, plutôt », d'une citation, d'une adresse, d'un chemin ou d'un nombre avec son unité.
- C'est borné à 8 appels et 60 secondes par dictée. Comptez environ 2 s de plus sur carte graphique, et jusqu'à 30 s sur processeur seul.
- L'Historique indique « reformulé par parties : complet » ou « partiel ». Les journaux ne contiennent jamais le texte.
- **Pour le désactiver** : décochez-le à la dernière étape de l'assistant de configuration, ou dans Paramètres → Confidentialité et conservation → « Expérimental : sauver les longues reformulations par parties ». Désactivé, Jot se comporte exactement comme en 1.6.0.

### Assistant de configuration

La dernière étape propose maintenant de cocher ou décocher ces deux options. Toutes deux sont cochées par défaut. Le sauvetage par parties n'apparaît pas en mode Gemini seul.

## Compatibilité et limites

- Windows 10 et Windows 11, **64 bits (x64)**. Les essais de cette version ont été faits sous Windows 11, sur RTX 5070 Ti et en mode processeur seul.
- **Mise à jour depuis 1.6.0** : à faire avec l'installateur ci-dessous. La mise à jour depuis l'application n'est possible qu'à partir de 1.7.0. Réglages, Historique, modèles et dictionnaire sont conservés.
- **Fidélité des reformulations** : le contrôle ne repère pas encore la disparition de « je crois », « je trouve », d'une condition en « si » ou d'un nom propre. Cela concerne aussi la reformulation entière.
- **SmartScreen :** l'installateur n'est pas signé numériquement. Windows affichera « Windows a protégé votre ordinateur » : cliquez sur **Informations complémentaires**, puis sur **Exécuter quand même**.

## Télécharger et installer

- **`Jot-1.7.0-setup.exe`** : installation classique, par utilisateur, sans droits administrateur. Elle remplace une version 1.5.x ou 1.6.0 sur place, dans `%LOCALAPPDATA%\Programs\Jot`.
- **`Jot-1.7.0-portable.zip`** : version sans installation.
- **`SHA256SUMS.txt`** : empreintes SHA-256 pour vérifier l'intégrité des téléchargements.

# Jot 1.5.0 — notice d'utilisation

> Version candidate locale, non publiée. Elle ne doit pas être présentée comme une version stable avant la validation finale.

Jot est une application de dictée pour Windows. Vous appuyez sur un raccourci, vous parlez, et le texte s'insère là où se trouve le curseur. Toutes les fonctions de la 1.4 restent en place :

- l'indicateur de dictée (HUD), qui laisse passer les clics ;
- les modes Basculer et Maintenir, l'arrêt par Échap et les raccourcis personnalisés ;
- l'historique avec recherche et corrections versionnées ;
- le dictionnaire intelligent et le tableau de bord ;
- les thèmes et les langues FR/EN ;
- la clé API stockée dans le Gestionnaire d'identification de Windows.

## Nouveautés de la 1.5

- **Choix du moteur** (Réglages → Transcription et reformulation) : **Gemini (en ligne)** ou **100 % local**.
- **Reformulation locale** : Désactivée, Rapide ou Qualité. Elle supprime les hésitations et corrige la ponctuation sans changer le sens.
- **Installateur par utilisateur**, qui ne demande pas de droits administrateur, en plus du mode portable.
- **Avis de nouvelle version**, publiés sur la page GitHub Releases du fork.

## Installer

Il y a deux façons d'utiliser Jot. Les deux partagent les mêmes données dans `%LOCALAPPDATA%\Jot`.

| | Installateur `Jot-1.5.0-setup.exe` | Mode portable (ZIP) |
|---|---|---|
| Droits administrateur | Aucun | Aucun |
| Emplacement | `%LOCALAPPDATA%\Programs\Jot` | Le dossier où vous décompressez |
| Menu Démarrer | Raccourci « Jot » | Non |
| Icône sur le bureau | Option, décochée par défaut | Non |
| Démarrage avec Windows | Option, décochée par défaut | Depuis les réglages de Jot |
| Désinstallation | Paramètres Windows → Applications | Supprimer le dossier |

Si Jot est ouvert pendant une installation, une mise à jour ou une désinstallation, l'assistant vous demande d'abord de le quitter (menu de l'icône près de l'horloge → Quitter).

### Avertissement SmartScreen

L'installateur et `jot.exe` **ne sont pas signés**, faute de certificat de signature de code. À la première ouverture, Windows SmartScreen peut afficher « Windows a protégé votre ordinateur ». Pour continuer :

1. Cliquez sur **Informations complémentaires**.
2. Cliquez sur **Exécuter quand même**.

Faites-le uniquement si le fichier vient de la page Releases du dépôt `tadikwa/jot-gemini-transcribe-Windows` et que son empreinte SHA-256 correspond à celle publiée. Pour la vérifier dans PowerShell :

```powershell
Get-FileHash .\Jot-1.5.0-setup.exe -Algorithm SHA256
```

Un antivirus peut aussi signaler un programme non signé qui installe un raccourci clavier global. C'est une limite connue, que seul un certificat payant lèverait.

### Démarrage avec Windows

L'option crée l'entrée `Jot` de votre clé de démarrage utilisateur (`HKCU\...\Run`). Vous la retrouvez dans le Gestionnaire des tâches, onglet Applications de démarrage.

- **Entrée existante d'un autre programme** : si une entrée « Jot » existe déjà et ne lance pas `jot.exe`, elle n'est jamais remplacée.
- **Version de test** : une copie de test (fichier `jot-test-profile` à côté de l'exécutable) ne s'inscrit jamais au démarrage, et les réglages l'expliquent.
- **Pour désactiver** : utilisez les réglages de Jot ou le Gestionnaire des tâches.

### Désinstaller

Lancez la désinstallation depuis Paramètres → Applications → Jot.

- Le programme, les raccourcis et l'entrée de démarrage de cette installation sont retirés.
- **Vos données sont conservées par défaut** : historique, dictionnaire, réglages, modèles locaux et clé API.
- Une dernière question propose de tout supprimer définitivement. La réponse par défaut est **Non**.

Attention : les copies portables utilisent le même dossier de données. Ne répondez Oui que si vous n'en gardez aucune.

Pour une désinstallation par script, les données sont conservées sauf si vous ajoutez explicitement `/PURGEDATA` :

```powershell
& "$env:LOCALAPPDATA\Programs\Jot\unins000.exe" /SILENT            # garde les données
& "$env:LOCALAPPDATA\Programs\Jot\unins000.exe" /SILENT /PURGEDATA # supprime tout
```

## Choisir le moteur

### Gemini (en ligne)

C'est le fonctionnement de la 1.4. L'enregistrement part vers l'API Gemini de Google avec votre clé. Les options Dictée intelligente et Ton restent disponibles.

### 100 % local

Tout s'exécute sur votre ordinateur, après un téléchargement unique :

| Pack | Taille | Contenu |
|---|---|---|
| Transcription (obligatoire) | 2,56 Go | Moteur llama.cpp (Vulkan) et modèle Qwen3-ASR 1.7B |
| Reformulation rapide | 2,74 Go | Qwen3.5 4B, environ 3,5 Go de mémoire vidéo |
| Reformulation qualité | 5,68 Go | Qwen3.5 9B, environ 6 Go de mémoire vidéo |

Chaque téléchargement fonctionne ainsi :

- **Annonce et confirmation** : avant de commencer, Jot indique la taille, la source (GitHub ou Hugging Face, avec les licences) et le dossier de destination (`%LOCALAPPDATA%\Jot\local`). Rien ne démarre sans votre clic sur « Télécharger maintenant ».
- **Suivi** : la progression s'affiche, et le bouton « Annuler » supprime le fichier incomplet.
- **Vérification** : chaque fichier est contrôlé par SHA-256 avant usage, et un fichier altéré est refusé.
- **Conservation** : les modèles restent après une mise à jour ou une désinstallation qui garde les données.

Fonctionnement du moteur :

- **Démarrage et arrêt** : le moteur démarre à la première dictée et s'arrête après 10 minutes d'inactivité. Il s'arrête aussi immédiatement si vous repassez en mode Gemini.
- **Accélération** : avec une carte graphique dédiée compatible, la transcription passe par le GPU. Sinon, elle utilise le processeur, plus lentement. La ligne « Exécution sur… » des réglages indique lequel est utilisé.
- **Résultat brut** : l'historique conserve aussi la transcription brute et indique l'étape qui a produit le texte final (transcription ou reformulation).

**Confidentialité en mode local** :

- L'audio et le texte ne quittent pas l'ordinateur.
- Le moteur écoute uniquement sur `127.0.0.1` et fonctionne hors ligne.
- Jot ne bascule jamais vers Gemini sans vous le demander. Si le mode local échoue, la dictée échoue avec un message, sans envoi en ligne.
- Le dictionnaire est utilisé comme vocabulaire de référence.
- Les instructions dictées (« ignore les consignes… ») sont traitées comme du texte à transcrire, jamais comme des ordres.

## Avis de nouvelle version

Réglages → Confidentialité et conservation → **Rechercher les mises à jour**. Cette option est activée par défaut et vous pouvez la désactiver.

- **Fréquence** : au plus une fois par jour, Jot interroge l'API publique GitHub (`api.github.com`) pour lister les versions du fork. Seule la version de Jot est transmise (en-tête User-Agent) : ni identifiant, ni clé, ni donnée de dictée. Jot ne télécharge et n'installe rien de lui-même.
- **Nouvelle version** : une notification apparaît une seule fois par version. Un clic dessus ouvre la fenêtre À propos, qui donne le lien vers la page de la version.
- **Préversions** : l'option « Inclure les préversions » ajoute les versions de test (décochée par défaut).
- **Hors ligne ou limite de requêtes GitHub** : Jot n'affiche rien au démarrage. La fenêtre À propos en indique la raison.

## Version de test isolée

Les fichiers `Jot-1.5.0-test-portable.zip` et `Jot-1.5.0-test-setup.exe` servent aux essais :

- **Profil séparé** : grâce au fichier `jot-test-profile`, elles gardent tout dans le sous-dossier `profile` à côté de `jot.exe`. Elles ne lisent jamais `%LOCALAPPDATA%\Jot` et utilisent une entrée distincte du Gestionnaire d'identification.
- **Pas de démarrage automatique** : elles ne s'inscrivent jamais au démarrage de Windows. L'installateur de test s'installe dans `%LOCALAPPDATA%\Programs\Jot-test` sous le nom « Jot (test) ».
- **Clé API** : pour essayer le mode Gemini avec la version de test, saisissez la clé une fois dans ce profil. Aucune clé n'est copiée depuis votre installation normale.

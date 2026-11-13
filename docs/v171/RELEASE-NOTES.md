# Jot 1.7.1 — Mises à jour après le renommage du dépôt

Jot est une application de dictée vocale pour Windows, gratuite et open source. Vous dictez, Jot transcrit, peut reformuler, puis insère le texte là où se trouve votre curseur. Le traitement peut se faire entièrement en local, ou avec Gemini et votre propre clé.

## Depuis 1.7.0 : mettre à jour à la main, une seule fois

Le dépôt s'appelle désormais **`tadikwa/Jot`**. Jot 1.7.0 voit bien cette version dans *Paramètres → Mises à jour*, mais **il refuse de la télécharger**, avec le message « Le fichier n'est pas hébergé dans la version attendue du dépôt Jot ». C'est le contrôle d'origine qui fonctionne : 1.7.0 n'accepte que l'ancien nom du dépôt.

1. Téléchargez `Jot-1.7.1-setup.exe` et `SHA256SUMS.txt` ci-dessous.
2. Facultatif : vérifiez l'empreinte dans PowerShell avec `Get-FileHash .\Jot-1.7.1-setup.exe -Algorithm SHA256`. Elle doit être identique à celle de `SHA256SUMS.txt`.
3. Lancez l'installateur. Il demande à Jot 1.7.0 de se fermer. Si une dictée est en cours, terminez-la avant.

Réglages, Historique, dictionnaire et modèles locaux sont conservés. À partir de 1.7.1, la mise à jour depuis l'application fonctionne de nouveau.

## Nouveautés

### Mises à jour

- Jot ne fait confiance qu'aux fichiers publiés dans les versions de `github.com/tadikwa/Jot`. Les contrôles de taille, de version et de SHA-256 sont inchangés.
- La réponse mise en cache par 1.7.0 est ignorée au premier lancement, ce qui force une vérification fraîche. L'annonce déjà vue n'est pas répétée.
- Les liens de l'installateur, de la page Mises à jour et du README pointent vers le nouveau nom.

### Sauvetage des longues reformulations : désactivé par défaut

En 1.7.0, cette option expérimentale était activée par défaut. Sur nos essais, **8 textes sauvés sur 10 perdaient une nuance** (« je crois », une condition, une auto-correction).

- Une nouvelle installation l'a désormais **désactivée**, y compris à la dernière étape de l'assistant. L'option reste disponible, marquée « Expérimental », dans *Paramètres → Confidentialité et conservation*.
- Désactivée, aucun appel de sauvetage n'est fait : Jot se comporte comme en 1.6.0. Activée, rien ne change par rapport à 1.7.0.
- **Si vous venez de 1.7.0, votre réglage est conservé.** Jot 1.7.0 enregistrait tous les réglages, donc on ne peut pas distinguer un choix délibéré de l'ancienne valeur par défaut. Si vous ne l'avez jamais activée volontairement, décochez « Expérimental : sauver les longues reformulations par parties ».

### Nombres, dates et versions (mode local)

La reformulation locale écrit plus souvent en chiffres les dates, années, prix, mesures, numéros de version et adresses : « version un point sept point un » devient « version 1.7.1 », et « vingt dollars » devient « 20 dollars ». « Un » article et « un point important » restent en lettres.

- Le contrôle de fidélité refuse désormais un nombre réordonné, par exemple 192.168.1.50 réécrit en 192.168.50.1. Il refuse toujours un nombre modifié, comme 1.7 → 1.8 ou 20 → 200.
- Mesuré sur 156 textes, anglais compris : 130 corrects au lieu de 126, sans nouveau refus à tort.
- Certains montants et heures longues restent en lettres, par exemple « trois cent quarante-neuf euros » ou « quinze heures trente ».
- Une adresse IP dictée en lettres est encore insérée brute.

## Compatibilité et limites

- Windows 10 et 11, 64 bits (x64). Essais faits sous Windows 11.
- **Depuis 1.6.0 ou plus ancien** : utilisez l'installateur, comme pour 1.7.0.
- La fidélité des reformulations n'est pas parfaite. Le contrôle ne repère pas toujours la disparition d'une nuance ou d'un mot dans une transcription déjà mal reconnue. Relisez les textes importants.
- **SmartScreen** : l'installateur et `jot.exe` **ne sont pas signés numériquement**. Windows affichera « Windows a protégé votre ordinateur » : cliquez sur **Informations complémentaires**, puis sur **Exécuter quand même**.

## Télécharger

- **`Jot-1.7.1-setup.exe`** : installation par utilisateur, sans droits administrateur. Elle remplace 1.5.x, 1.6.0 ou 1.7.0 sur place.
- **`Jot-1.7.1-portable.zip`** : version sans installation.
- **`SHA256SUMS.txt`** : empreintes SHA-256 des deux fichiers.

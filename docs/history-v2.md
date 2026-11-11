# Jot 1.3.0 — Bibliothèque de dictées

Base : `615d531`, branche `codex/history-v2`. Version portable de validation Windows, sans publication ni push.

## Utilisation

Historique affiche des pages de 100 aperçus, regroupées par jour local. Cliquer sur une entrée ouvre son détail ; sur une fenêtre étroite, le détail remplace la liste. Les filtres de période, d’application et de statut se combinent avec la recherche.

La recherche utilise FTS5 réellement créé par la version SQLite embarquée. Elle ignore la casse et les accents, recherche les mots et leurs débuts, et exige la présence de tous les mots saisis. Elle couvre les textes brut, nettoyé et corrigé actuels. Une saisie est temporisée de 180 ms ; les requêtes travaillent en arrière-plan et les réponses périmées sont ignorées. Le bouton Effacer rétablit la recherche vide en conservant les filtres.

Dans le détail : Copier, Modifier, Enregistrer, Annuler, suppression et reprise admissible. Entrée ajoute une ligne dans l’éditeur ; Ctrl+Entrée enregistre. Les champs disposent de sélection, copier/coller et annulation/rétablissement. Un brouillon empêche de quitter l’historique ou de fermer la fenêtre. Une confirmation est nécessaire avant de l’abandonner en ouvrant une autre dictée.

Les onglets Brut et Nettoyé donnent accès aux sorties du moteur. Corrections présente les changements avant/après et leur date ; rétablir une version ajoute une nouvelle correction, sans effacer les précédentes. La suppression confirmée inclut la session, ses versions et son audio. Les suppressions multiples valident tous les identifiants et chemins avant de commencer.

## Sources et comptage

- `meta.json` conserve les métadonnées et textes du moteur.
- `corrections.json`, dans le même dossier, est un journal versionné des corrections explicites : identifiant, avant, après et date. L’écriture est synchronisée puis remplacée atomiquement. Un fichier illisible ou d’une version inconnue provoque une erreur, jamais un remplacement silencieux.
- SQLite est un index dérivé : la réindexation relit les deux fichiers. La dernière correction est prioritaire sur le texte nettoyé, puis le brut. Les écritures du moteur ne touchent pas le journal utilisateur.
- Une garde optimiste refuse une sauvegarde si le texte a changé depuis son ouverture. Une correction vide est refusée.
- Les statistiques utilisent le nombre de mots Unicode du texte affiché. Une correction ne crée aucune nouvelle dictée. Les statuts admissibles, le calcul pondéré de vitesse et la formule de temps économisé de 1.2 sont conservés ; seule la valeur du nombre de mots suit la correction. Les suppressions retirent la session des agrégats. Les notifications existantes rafraîchissent le tableau de bord.
- Les statuts sont ceux du moteur. La reprise réutilise `RetryQueue`, sa protection contre les traitements concurrents et ses résultats explicites ; elle est proposée seulement si le statut et l’audio conservé le permettent. Les erreurs de clé, quota ou modèle peuvent être relancées explicitement après correction du problème ; elles restent exclues des reprises automatiques.

Le journal prépare des analyses futures ; aucune différence n’est transformée automatiquement en règle de dictionnaire. Aucun apprentissage ni surveillance clavier supplémentaire n’a été ajouté.

## Vérification du 11 novembre 2026

- `cargo fmt --all --check`.
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`.
- `cargo test --workspace --locked --offline` : 85 tests d’interface, 172 tests moteur et 12 tests d’intégration réussis ; deux tests ignorés dans cette commande.
- `cargo test -p jot-core dashboard_large_history --locked --offline -- --ignored --nocapture` : test supplémentaire réussi sur 100 000 sessions fictives. L’autre test ignoré est une vérification réseau de mise à jour GitHub.
- `cargo build --release -p jot --locked --offline` : Windows x64 Release.
- Harness HUD existant, démarrage visible puis masqué : 24 cas réussis par série, avec contrôle réel des pixels, du focus et du passage des clics. Les processus cible sont bornés et fermés par le harness.
- Tests GPUI : éditeur multiligne/Unicode/IME simulé, pagination au-delà de 500, recherche différée, protection du brouillon, conservation de l’onglet et blocage de l’édition pendant un rafraîchissement, et fenêtre 540 × 480 avec une transcription dépassant 100 000 caractères.
- Parcours Windows réel sur un profil de 120 sessions entièrement fictives : français/sombre puis anglais/clair, consultation longue, correction multiligne enregistrée avec Ctrl+Entrée, original conservé, recherche et surlignage, copie comparée au texte attendu et correction retrouvée après redémarrage. Le total de mots passe de 4 194 à 1 204 en remplaçant la dictée de 3 000 mots par sa correction de 10 mots, avec 113 dictées réussies conservées.
- Suppression multiple réelle de deux autres sessions fictives après confirmation : historique à 118 entrées, tableau de bord à 1 184 mots et 111 dictées réussies sans redémarrage ; contrôle SQLite et dossiers concordant. Le bouton Effacer et les détails d’une erreur conservée ont également été vérifiés. Toutes les instances de test sont arrêtées ; Jot 1.2.0 n’a pas été relancé, conformément à la demande utilisateur.

## Mesures séparées sur données fictives

`cargo run --release -p jot-core --example history_benchmark --locked --offline` construit quatre index indépendants. Les schémas, déclencheurs FTS et méthodes de requête sont ceux de production. Préparation exclue, 31 échantillons chauds par opération, médiane/p95 en millisecondes. Les pages mesurées contiennent 50 aperçus et le nombre total de résultats.

| Sessions | Première page | Recherche mots/préfixes | Recherche + filtres | Dernière page |
|---:|---:|---:|---:|---:|
| 100 | 0,062 / 0,068 | 0,206 / 0,233 | 0,309 / 0,353 | 0,062 / 0,068 |
| 1 000 | 0,077 / 0,153 | 0,490 / 0,575 | 0,533 / 0,586 | 0,086 / 0,099 |
| 10 000 | 0,202 / 0,215 | 1,514 / 1,651 | 1,667 / 1,745 | 0,327 / 0,356 |
| 100 000 | 4,868 / 6,217 | 24,036 / 32,531 | 15,017 / 19,242 | 9,295 / 10,338 |

À 100 000 sessions, la liste des applications mesure 10,499 / 12,094 ms ; l’agrégation d’activité locale mesure 137,538 / 161,445 ms. Les plans de requête ont confirmé l’utilisation de l’index partiel dédié. Ces nombres mesurent le stockage et les requêtes, pas le rendu complet de l’interface ni le démarrage ou la réindexation des dossiers.

## Limites de validation

Pas de nouvel essai de dictée Gemini avec une clé réelle, ni de validation matérielle microphone/IME ; les protections de reprise et modes Toggle/Maintenir sont couverts par les tests existants. Le surlignage vise les accents latins français ; l’indexation FTS reste la référence des résultats. L’éditeur prend en charge la sélection clavier et Maj+clic, mais pas la sélection par glissement ni le déplacement par mots. Pas de période personnalisée dans cette version.

Une suppression est irréversible après confirmation. Si le nettoyage physique échoue après retrait de l’index, les fichiers restent dans un dossier `.deleted-*` sous recordings et une erreur est signalée ; ces dossiers ne sont pas réindexés. Le livrable contient un profil neuf, aucune clé ni transcription personnelle, et aucun dossier de diagnostics ou de sauvegardes. L’exécutable est non signé.

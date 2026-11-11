# Dictionnaire 1.4 — fonctionnement et preuves locales

## Architecture

Le store existant reste la source des termes et des remplacements. `dictionary.json` accepte l’ancienne liste JSON et écrit désormais une enveloppe version 2 (`entries`, `suggestions`) à la première modification. Les champs ajoutés aux entrées sont `enabled` (true par défaut) et `provenance` facultative. La provenance conserve seulement l’identifiant de session et la date de la correction explicite. Le journal `corrections.json` et `meta.json` ne sont ni migrés ni réécrits par ce module.

Une suggestion comporte les deux expressions courtes, son statut (`pending`, `accepted`, `rejected`), ses sources et un score lexical. Aucune phrase complète avant/après n’est copiée dans le dictionnaire. Aucun appel réseau n’est ajouté. Une proposition reste inactive jusqu’à acceptation explicite. Les refus et les acceptations constituent des décisions durables : une relecture identique ne les repropose pas, même après suppression de l’entrée acceptée. Les sources des seules propositions en attente peuvent être purgées. Une règle acceptée survit volontairement à la suppression de sa session source.

Les mutations préparent une copie sous verrou, écrivent un fichier temporaire voisin, synchronisent le fichier, puis remplacent atomiquement la destination avant d’actualiser l’état en mémoire. Un échec laisse les règles vivantes intactes et remonte une erreur via les méthodes `*_checked` utilisées par la nouvelle UI. Une mutation identique ne réécrit pas le fichier. Un fichier JSON illisible ou une version inconnue bloque les écritures ; son contenu est conservé et aucune erreur de parsing contenant du vocabulaire privé n’est journalisée.

Plusieurs fautes distinctes peuvent cibler le même terme exactement orthographié sans écraser une entrée manuelle. Le vocabulaire est dédoublonné avant son plafond existant de 100 termes. Les conflits de forme erronée et les chaînes/cycles sont refusés ; les termes canoniques restent stables. Pour les anciens fichiers contradictoires, les règles manuelles sont prioritaires et les règles dangereuses sont exclues à la fois des remplacements et des indices de correction du prompt. Les limites de vocabulaire et sa sanitization restent en place. Désactiver une entrée la retire immédiatement des entrées de pipeline.

Le moteur de remplacement calcule les occurrences sur le texte d’origine : aucune cascade sur du texte nouvellement produit. Les expressions les plus longues ont priorité ; les limites de mots couvrent aussi les marques Unicode combinantes. La casse explicite et la propagation de casse existantes sont conservées.

## Extraction déterministe et limites

L’analyse aligne les mots Unicode d’une correction explicite, en retirant les préfixes et suffixes communs puis en calculant une sous-séquence commune bornée. Elle privilégie les substitutions de 1 à 3 mots, avec une proximité orthographique mesurée sur les graphèmes ; un squelette consonantique fournit un second indice pour des noms comme Klod/Claude. Un suffixe de nom propre commun permet de proposer `Klod Code → Claude Code` plutôt qu’une règle générale sur Klod.

Le score affiché est un score lexical, **pas une probabilité de justesse**. Les réécritures générales, ajouts, suppressions, changements de ponctuation ou de casse seuls et plusieurs préfixes de négation sont écartés. Cela ne prouve pas la conservation du sens : une personne doit relire chaque proposition. L’analyse n’est pas une compréhension sémantique ni un apprentissage d’un modèle.

Bornes prudentes : au plus 4096 mots par version, une région modifiée de 24 mots, 3 groupes de substitutions et des expressions de 60 caractères maximum. Au-delà, aucune proposition n’est produite. Certaines corrections légitimes ne seront donc pas suggérées ; l’ajout manuel reste possible. Les représentations Unicode canoniquement équivalentes NFC/NFD ne font pas l’objet d’une normalisation générale supplémentaire.

Le CSV conserve `term,misspelling` et ajoute une troisième colonne facultative `enabled`. Les anciens CSV à deux colonnes restent importables ; les règles désactivées restent désactivées après export/import. Le CSV est un transfert des entrées, pas une sauvegarde complète des décisions/provenances ; conserver le JSON pour cela. Ne pas réutiliser un profil 1.4 avec un ancien binaire qui ne connaît pas l’enveloppe version 2.

## Tests réellement exécutés sur données fictives

Depuis le worktree dédié, avec `CARGO_TARGET_DIR=<checkout>/target` :

- `cargo test -p jot-core --lib --locked dictionary -- --nocapture` : **28 tests sélectionnés réussis**, aucun échec (inclut les tests d’extraction et un test de prompt existant).
- `cargo test -p jot-core --lib --locked replacement -- --nocapture` : **13 tests sélectionnés réussis**, aucun échec (un test est commun avec le filtre dictionary).
- `cargo clippy -p jot-core --lib --locked -- -D warnings` : réussi, code de sortie 0.
- `rustfmt --edition 2024 crates/jot-core/src/dictionary.rs crates/jot-core/src/replacement.rs` et `git diff --check` ciblé : réussis.

Les tests rouges ont été observés avant les correctifs pour les conflits/cycles, l’écrasement d’un JSON invalide, les cascades, les marques combinantes, l’extraction absente, les conflits des fichiers migrés, le roundtrip de `enabled`, les variantes et la casse Unicode sigma. Les tests vérifient aussi l’acceptation/le rejet persistants, le redémarrage, l’absence de phrases privées dans le JSON, l’édition/l’activation, la suppression, la purge partielle des sources, les migrations et l’échec de sauvegarde sans modification de l’état vivant.

Mesures indicatives en build de test debug, machine de cette session, sans seuil temporel fragile dans les assertions :

- 2000 corrections fictives distinctes en lot puis seconde analyse : environ **0,75 seconde** au dernier passage.
- 1000 sessions fictives analysées individuellement : **5,341 secondes** au premier passage ; **0,361 seconde** à la relecture. Le test confirme que la date de modification du JSON n’a pas changé lors de la relecture.
- 1000 entrées CSV fictives : plafond de vocabulaire de 100 et budget de 2048 octets vérifiés.

Cargo a signalé un nettoyage automatique de cache refusé (`icu_locale_core`), sans échec des commandes ci-dessus. Deux essais intermédiaires ont été temporairement bloqués par des fichiers hotkey/settings en cours d’écriture concurrente ; les relances après correction de leurs fins de lignes ont réussi.

La validation complète du workspace, du binaire Release, du HUD et du parcours graphique est consignée par le rapport d’intégration. Ces tests unitaires ne prouvent pas la qualité d’une vraie transcription Gemini ni l’insertion dans une application externe.

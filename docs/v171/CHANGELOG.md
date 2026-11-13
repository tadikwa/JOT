# Jot 1.7.1 — changelog

Base : v1.7.0.

## Corrections

- **Mises à jour après le renommage du dépôt en `tadikwa/Jot`.**
  - Liste des versions : `https://api.github.com/repos/tadikwa/Jot/releases?per_page=30`.
  - Seuls les fichiers sous `https://github.com/tadikwa/Jot/releases/download/` sont acceptés, avec la casse exacte. Toute autre origine est refusée (`UnexpectedSource`) : ancien nom, `JOT` ou `jot`, autre propriétaire, `http`, chemin en `..`.
  - Les contrôles de version, taille et SHA-256 sont inchangés.
  - `CACHE_SCHEMA` passe de 2 à 3. Le cache écrit par 1.7.0 est relu sans son ETag, puis remplacé. L'annonce déjà vue (`notified`) est conservée.
  - Liens mis à jour dans `jot.iss` (éditeur, support, mises à jour), sur la page Mises à jour et dans le README.
- **Jot 1.7.0 ne peut pas se mettre à jour seul** vers 1.7.1. Son contrôle d'origine attend l'ancien nom, alors que GitHub renvoie les fichiers sous le nouveau. La première mise à jour depuis 1.7.0 doit donc se faire à la main (voir les notes de version).
- **Fidélité** : une suite de nombres séparés par des points (version, adresse IP) réordonnée par la reformulation est refusée (`number_changed`).

## Changements

- **Sauvetage partiel des longues reformulations : désactivé par défaut** pour une nouvelle installation, un profil 1.6.0 et l'assistant de configuration.
  - Un réglage enregistré par 1.7.0 est conservé tel quel, faute de pouvoir le distinguer de l'ancien défaut.
  - Désactivé, aucun appel de sauvetage n'a lieu.
- **Prompt local** : les heures, dates, années, prix, quantités, mesures, versions et adresses sont écrits en chiffres, avec des garde-fous pour « un » article et « un point ». Mesures et limites : [bench/RESULTATS.md](bench/RESULTATS.md).

## Tests ajoutés

- `update` :
  - `only_the_exact_current_repository_is_trusted_for_assets` ;
  - `a_1_7_0_cache_is_refetched_without_its_etag_and_keeps_the_notice` ;
  - `official_update_endpoints_use_current_repository`, mis à jour.
- `settings` : `partial_rescue_is_off_for_new_and_1_6_profiles_and_a_saved_choice_is_kept`.
- `local` :
  - `a_disabled_rescue_is_never_attempted` ;
  - `prompts_match_the_benchmarked_variants`, qui porte désormais sur les variantes v171.
- `fidelity` :
  - `dotted_numbers_keep_their_order` ;
  - `spoken_numbers_written_in_digits_pass_and_altered_ones_do_not`.

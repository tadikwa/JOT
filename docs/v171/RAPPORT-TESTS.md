# Jot 1.7.1 — rapport de tests

Windows 11 Pro, RTX 5070 Ti, toolchain Rust 1.97.1. Données synthétiques ou fictives uniquement.

## Contrôles

| Commande | Résultat |
|---|---|
| `cargo fmt --all --check` | OK |
| `cargo clippy --all-targets --locked -- -D warnings` | OK |
| `cargo test --locked` | 393 réussis, 0 échec, 4 ignorés |
| `cargo build --release -p jot --locked` | OK |

Les 4 tests ignorés :

- `update::tests::reaches_github_and_parses_the_reply` : lancé à la main contre le vrai GitHub, réussi.
- `local::tests::real_engine_end_to_end` : lancé à la main sur 20 WAV synthétiques, réussi (voir [bench/RESULTATS.md](bench/RESULTATS.md)).
- `local::tests::reformulation_bench` : lancé à la main sur 156 cas (voir [bench/RESULTATS.md](bench/RESULTATS.md)).
- `history::tests` (mesure de performance sur 100 000 lignes) : non relancé, aucun changement dans l'historique.

## Mises à jour

### Contre le vrai GitHub

- L'ancienne URL d'API (`tadikwa/jot-gemini-transcribe-Windows`) répond par une redirection 301 vers `/repositories/1452998716`. Les fichiers de chaque version y sont listés sous `https://github.com/tadikwa/Jot/releases/download/v<version>/<nom>`, exactement.
- Jot 1.7.0 attend donc l'ancien préfixe et refuse le fichier avec `UnexpectedSource`. Cela confirme la mise à jour manuelle depuis 1.7.0.
- 1.7.1 exige l'URL exacte `<préfixe>v<version>/<nom>`. Vérifié sur les fichiers publiés de v1.5.0, v1.6.0 et v1.7.0 : aucun écart.

### Faux serveur local

Le faux serveur répond sur `127.0.0.1` et publie une version v1.7.2 avec un faux installateur de 10 Mo, envoyé par blocs de 128 Ko. Le code testé est celui de l'application, `update::check` puis `update::download`, avec `JOT_TEST_PROFILE=1`, `JOT_TEST_RELEASES_URL` et `JOT_TEST_DOWNLOADS_URL`. Les fichiers vont dans le dossier `updates` du profil isolé.

| Scénario | Résultat |
|---|---|
| Vérification | `Available 1.7.2`, date 2026-11-13. Notes réduites à du texte : titre, puces, et la balise `<script>` retirée |
| Annulation à 20 % | `Cancelled`, aucun fichier ni `.part` restant |
| Nouvel essai | `Verified`, 10 485 760 octets, SHA-256 conforme, `still_intact` vrai, progression de 0 à 97 % par paliers d'environ 4 % |
| Mauvaise empreinte dans `SHA256SUMS.txt` | `HashMismatch`, fichier supprimé |
| Installateur hébergé hors du préfixe autorisé | `UnexpectedSource`, **aucune requête** vers l'hôte étranger (journal du serveur) |
| Version égale à la version installée | `NotNewer` |

Le téléchargement n'exécute jamais rien. L'installation reste un clic séparé, après confirmation.

### Tests unitaires ajoutés

- Origine : la casse exacte `tadikwa/Jot` est acceptée. Sont refusés `JOT`, `jot`, l'ancien nom, `ubranch`, un autre propriétaire, `http`, `v1.7.0/../` et `v1.7.1/../`.
- Cache 1.7.0 (schéma 2, ETag `"old"`) : ignoré au démarrage. Il est relu sans `If-None-Match`, réécrit en schéma 3, et l'annonce déjà vue est gardée. L'appel suivant envoie l'ETag et un 304 conserve la liste.
- Déjà couverts : 304, ETag, limite GitHub, réponse incomplète, hors ligne, HTTP 503 au téléchargement.

## Sauvetage partiel

| Cas | Valeur |
|---|---|
| Nouvelle installation (`Settings::default`) | désactivé |
| Profil 1.6.0, champ absent | désactivé |
| Profil 1.7.0, `true` enregistré | conservé à `true` |
| Enregistrer puis relire, `true` et `false` | conservé |
| Désactivé : appel de sauvetage | aucun (`a_disabled_rescue_is_never_attempted`) |
| Assistant, dernière étape (mode local) | la case affiche `settings.local_partial_rescue`, donc décochée sur un profil neuf ; libellé « Expérimental » et avertissement de fidélité (lu dans le code, pas encore vu à l'écran) |
| Mode Gemini seul | option absente de l'assistant, comme en 1.7.0 |

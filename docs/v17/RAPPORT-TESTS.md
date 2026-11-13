# Jot 1.7.0 (candidat) — rapport de tests comparatif

Branche locale `claude/v17`, base v1.6.0 (`b070742`). Rien n'a été poussé, tagué ni publié. Le tag v1.6.0, sa release, l'installation réelle et le profil réel n'ont pas été touchés. Tous les essais d'installation ont utilisé la variante isolée `/DTestBuild` :
- un autre AppId ;
- le dossier `%LOCALAPPDATA%\Programs\Jot-test` ;
- un profil dans `{app}\profile`.

| Commit | Objet |
|---|---|
| b0fccef | Dictionnaire : dictées longues, nouvelles fautes de termes connus |
| ccd3494 | Chantier 1 : page Mises à jour et À propos, installation vérifiée au clic, fermeture accompagnée |
| bec5c39 | Version 1.7.0, texte À propos (modes local et Gemini) |
| 11f34ad | Chantier 2 : sauvetage partiel expérimental, désactivé par défaut |
| 416aa20 | Les deux options activées par défaut et proposées dans l'assistant de configuration |

Tests automatiques : 386 réussis, 0 échec :
- `jot-core` : 258, plus 4 ignorés qui demandent les vrais modèles ;
- `jot` : 112 ;
- intégration : 16.

`cargo fmt --check` et `cargo clippy --workspace --all-targets` passent sans remarque.

## 1. Chantier 1 — mises à jour et fermeture accompagnée

| Essai | 1.6.0 | 1.7.0 |
|---|---|---|
| Setup lancé avec Jot ouvert | Message Inno générique | Un Jot 1.6.0 n'écoute pas la demande : message de fermeture manuelle, puis Annuler, code 1, rien modifié. Un Jot 1.7.0 se ferme de lui-même 0,3 s après la demande : setup code 0, Jot code 0, aucun `jot` ni `llama-server` résiduel |
| Mise à niveau 1.6.0 → 1.7.0 | — | Code 0. Réglages, Historique, modèles et fichier témoin identiques avant et après |
| Vérifier les mises à jour | Bandeau | Page dédiée. Contre un faux serveur local : 1.7.1 disponible, notes affichées en texte (une balise `<script>` reste du texte) |
| Télécharger | — | Barre de progression (« 8 % (0,8 Mo / 9,9 Mo) »). « Annuler » supprime le fichier partiel et propose « Réessayer ». « Installer » reste grisé jusqu'à « Vérifié : taille et SHA-256 conformes » |
| Installer | — | Confirmation, puis Jot se ferme et l'assistant normal s'ouvre (`/JOTWAIT`), jamais en silence. Registre en 1.7.1. « Exécuter Jot » relance l'application |

Aucun `taskkill`, `TerminateProcess` ni arrêt par nom. `AppMutex` est conservé comme garde-fou.

Non testé en réel :
- le refus de fermeture d'un Jot occupé, faute de dictée possible pendant l'essai (couvert par les tests unitaires) ;
- « Réessayer » après annulation ;
- la fermeture accompagnée par le désinstalleur, qui garde le message Inno d'origine.

## 2. Chantier 2 — sauvetage partiel des longues reformulations

### Méthode

- **Corpus privé** : les 26 dictées réelles de 60 mots ou plus de l'Historique local, parmi 90 enregistrements. Elles font de 62 à 557 mots : 12 Gemini, 9 locales acceptées, 5 locales refusées. Elles sont lues sur ce PC et rejouées comme texte brut. Aucune n'est copiée dans le dépôt ; ce rapport n'en donne que des comptes.
- **Corpus public** : `docs/v151/bench/fidelity_cases.json`, 28 cas, dont 3 de 60 mots ou plus.
- **Banc** : `reformulation_bench` avec `JOT_RESCUE=1`, pack rapide (Qwen3.5 4B Q4_K_M).
- **Matériel** :
  - GPU : Vulkan sur RTX 5070 Ti ;
  - CPU : `GGML_VK_VISIBLE_DEVICES=-1`.
- **Fidélité** : relecture manuelle de chaque partie réécrite face à son brut. Pour le corpus public, s'y ajoutent les contrôles du corpus.

### Résultats (GPU / CPU)

| | GPU | CPU |
|---|---|---|
| Reformulation entière acceptée (privé, 26) | 17 | 18 |
| Sauvetage tenté | 9 | 8 |
| → complet / partiel / brut | 5 / 4 / 0 | 4 / 4 / 0 |
| Parties réécrites / refusées | 29 / 5 | 27 / 5 |
| Appels par sauvetage, moyenne (max) | 4,7 (8) | 5,0 (8) |
| Durée du sauvetage, moyenne (max) | 1,7 s (2,9 s) | 18,2 s (28,4 s) |
| Attente totale (entière + sauvetage), max | ≤ 5,5 s | 41,7 s |
| Refus du contrôle global après recomposition | 0 | 0 |

Corpus public : un seul cas déclenche le sauvetage, `r2-retractation`. Il est complet (2/2), mais le contrôle du corpus relève `MISSING: je pense`, et l'auto-correction « le serveur était un peu lent, enfin pas vraiment le serveur, c'était plutôt l'application » devient « Le serveur n'était pas vraiment lent ». Le score final reste de 24/28, comme en 1.6.0.

### Fidélité (relecture)

10 textes ont été sauvés sur GPU (9 privés, 1 public). 8 d'entre eux contiennent au moins une perte que le contrôle n'a pas vue :

| Défaut non détecté | Occurrences |
|---|---|
| Nuance d'opinion supprimée (« je crois », « je trouve », « je pense ») | 5 |
| Auto-correction mal rendue (« enfin pas vraiment X, plutôt Y ») | 2 |
| Condition en « si » supprimée (affirmation devenue inconditionnelle) | 1 |
| Sens inversé (sujet et objet permutés) | 1 |
| Nom propre de logiciel tronqué | 1 |

Seuls 2 textes sur 10 sont fidèles. Sur CPU, les sorties diffèrent légèrement, mais les défauts sont du même type, avec en plus des phrases de fin supprimées.

### Conclusion

- **Gain** : sur les longues dictées refusées en bloc, la part reformulée passe de 0/9 à 9/9, au moins partiellement.
- **Coût** : 8 textes sur 10 perdent une nuance, une condition ou le sens que le brut gardait. La fidélité est donc **dégradée** par rapport au brut inséré par 1.6.0. Sur CPU, l'attente atteint jusqu'à environ 40 s.

Recommandation issue de la mesure : ne pas l'activer par défaut.

**Décision produit retenue : activé par défaut**, à la demande du mainteneur. Il est proposé, coché, à la dernière étape de l'assistant de configuration, et reste réglable dans Paramètres → Confidentialité : « Expérimental : sauver les longues reformulations par parties ». Décoché, le comportement est identique à 1.6.0 : mêmes appels, même texte inséré. Le risque mesuré ci-dessus s'applique donc par défaut, sur les dictées locales de 60 mots ou plus refusées en bloc.

Les défauts relevés viennent surtout du contrôle de fidélité, pas du découpage. Le contrôle ne connaît pas :
- « je crois » ni « je trouve » ;
- les subordonnées en « si » ;
- la conservation des noms propres.

Ces angles morts touchent aussi la reformulation entière. Les combler est un chantier à part, préalable à toute activation par défaut.

## 3. Build Draft

Voir `SHA256-DRAFT.txt` à côté de ce fichier : installateur et zip non signés, construits par `scripts/package.ps1`. Rien n'est publié.

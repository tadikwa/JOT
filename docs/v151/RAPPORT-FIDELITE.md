# Jot 1.5.1 — correctifs de fidélité et audit CORS

Candidat de test, non publié. Branche locale `claude/v151` : code au commit `7fd3b03`, ce rapport au commit suivant. Rien n'a été poussé, tagué ni publié. La release `v1.5.0` n'est pas touchée.

## 1. Ce qui change

| Zone | Changement |
|---|---|
| Consigne (prompt `v151j` / `v151n`) | Une version retirée (« enfin pas X, plutôt Y ») n'est plus affirmée, même en partie. Les restrictions (« uniquement si », « seulement si », « sauf si », « à condition que », *only if*, *unless*) et les nuances (« pas forcément », « à peu près », *not necessarily*) font partie de ce qu'il faut garder. Les rappels FR/EN qui suivent la dictée reprennent ces trois points. |
| Garde-fou (`fidelity.rs`) | `restriction_lost` : le brut contient une restriction et la reformulation n'en a plus aucune. `qualifier_lost` : le brut contient « pas forcément / pas vraiment / pas complètement… » et la reformulation n'en a plus. Une nuance que l'auteur retire lui-même (« enfin, pas vraiment… ») est ignorée. Dans les deux cas, c'est le texte brut qui est inséré. |
| Serveurs locaux (`local.rs`) | Une clé API aléatoire est générée à chaque lancement et passée par l'environnement (`LLAMA_API_KEY`). Ajout de `--cors-origins localhost`. Jot envoie la clé en `Bearer`. `/health` reste ouvert : c'est le contrôle de disponibilité. |

Aucune règle globale NBAR → Unbound. Raccourcis, HUD, installeur, modèles, fournisseurs, budget adaptatif et réglages : inchangés.

## 2. Méthode

- **Corpus** `docs/v151/bench/fidelity_cases.json`, 28 cas, écrit avant tout passage du modèle. Il contient :
  - les 8 phrases du terrain : 4 défauts et 4 contrôles ;
  - une dictée de 172 mots, d'environ 60 s ;
  - des phrases nouvelles pour chaque défaut ;
  - des phrases de contraste qui ne doivent **pas** déclencher la règle : « enfin » de conclusion, « plutôt » adverbe, « si » simple, tics seuls, NBAR légitime.
- Les attendus sont des contraintes (`must`, `must_not`, `once`, `format`, `order`), pas des sorties exactes.
- **Non-régression** : les 96 cas texte de la v1.5 (`docs/v15/bench/*.json`).
- **Scoreur** `docs/v151/bench/score_fidelity.py`. Il sépare :
  - LLM correct (le texte du modèle, avant garde-fou) ;
  - refus ;
  - faux positif (refus d'un texte correct) ;
  - faux négatif (texte faux accepté) ;
  - final correct (le LLM si accepté, sinon le brut) ;
  - reformulations réellement appliquées.
- **Moteur** : llama-server b12289, Qwen3.5-4B Q4_K_M, décodage glouton, même requête et même garde-fou qu'en production (test `reformulation_bench`).
  - Corpus de fidélité : sur **CPU**, GPU masqué par `GGML_VK_VISIBLE_DEVICES=-1`.
  - Non-régression : sur GPU, plus rapide. Le texte produit est le même à précision près.

## 3. Avant / après (texte)

### Corpus de fidélité, CPU

| Mesure | Avant (`2a21264`) | Après (`7fd3b03`) |
|---|---:|---:|
| LLM correct | 21/28 | 23/28 |
| Refus du garde-fou | 1 | 4 |
| dont faux positifs | 0 | 0 |
| Faux négatifs (texte faux inséré) | 6 | **1** |
| Texte final correct | 21/28 | **24/28** |
| Reformulations réellement appliquées | 20/28 | 19/28 |
| Latence LLM moyenne / max (CPU 9800X3D) | 2,8 s / 11,9 s | 3,4 s / 12,9 s |

### Les 4 défauts et les 4 contrôles

| Cas | Avant | Après | Texte inséré après |
|---|---|---|---|
| 2 rétractation serveur → application | faux, accepté | LLM toujours faux (« le serveur était lent »), **refusé** (`hedge_lost`) | brut, rétractation lisible telle que dite |
| 5 « uniquement si » | perdu, accepté | LLM le perd encore, **refusé** (`restriction_lost`) | brut, condition intacte |
| 6 Unbound | correct côté texte | correct | voir § 4 : l'erreur NBAR vient de l'ASR |
| 8 « pas forcément très puissant » | nuance perdue | **correct** | reformulation |
| 1 phrase correcte | inchangée | inchangée | reformulation = phrase dite |
| 3 procédure | ordre et numérotation OK | OK | reformulation |
| 4 courses sans fromage | liste mal formée | **correct** | reformulation |
| 7 IP / port / Go / vCPU (forme orale) | LLM corrompt l'IP, refusé | idem, refusé (`number_changed`) | brut |

Sur les phrases nouvelles :
- rétractations, restrictions et nuances : toutes correctes avant comme après, sauf `n-uniquement-repete`, désormais refusé ;
- contrastes : tous corrects, sans faux positif. « enfin » de conclusion, « plutôt » adverbe, « si » simple, NBAR légitime et tics seuls restent reformulés.

### Non-régression v1.5 (96 cas)

| Mesure | Avant | Après |
|---|---:|---:|
| Texte final correct | 83/96 | 83/96 |
| Refus / faux positifs | 2 / 0 | 4 / 0 |
| Reformulations appliquées | 70/96 | 69/96 |

Différences cas par cas :
- **gagné** : `f-commande-chemin` ;
- **rattrapé par le garde-fou** : `d-ordre-ambigu` (`hedge_lost`) ;
- **perdu** : `non-porteur` (« Non, je ne viendrai pas demain. » perd son « Non » ; la négation reste) et `h-long-bug` (une date retirée, « la semaine prochaine », reste à côté de « dans quinze jours »).

Deux variantes ont été essayées puis écartées :
- un exemple supplémentaire rétractation + « uniquement si » : 82/96, avec un faux positif ;
- les rappels sans « ou retire » : 82/96. `non-porteur` n'est pas corrigé, donc ce n'était pas la cause.

Le garde-fou seul, sans changement de consigne, fait 83/96 sans aucune différence, mais seulement 22/28 en fidélité. Il ne corrige pas les cas 4 et 8. Conformément à la consigne, j'ai arrêté les retouches là.

## 4. E2E sur le binaire extrait du ZIP (CPU, profil isolé)

Conditions :
- `jot.exe` extrait de `Jot-1.5.1-test-portable.zip`, avec le marqueur `jot-test-profile` : toutes les données sont dans `app\profile`. Le profil réel n'est pas touché.
- GPU masqué (`GGML_VK_VISIBLE_DEVICES=-1`, et le serveur ASR reçoit `--device none`), mode Rapide.
- Script `docs/v15/bench/reformulation/e2e.ps1` : chaque WAV est déposé comme dictée interrompue, puis la reprise au lancement le traite par le vrai pipeline. Un lancement à froid par cas : chargement des modèles compris.
- Audio : voix de synthèse Windows (Hortense), pas d'audio personnel. Les phrases sont celles du terrain.

| Cas | Audio | Lancement → résultat | ASR exact | Garde-fou | Texte final |
|---|---:|---:|---|---|---|
| 1 phrase correcte | 4 s | 9,4 s | oui | accepté | correct, identique |
| 2 rétractation | 26 s | 19,1 s | oui | `hedge_lost` | brut inséré (rétractation lisible) |
| 3 procédure | 14 s | 15,9 s | « restores » | accepté | correct : sauvegarde → Windows → logiciels → réseau → documents, intro + 1 à 5 |
| 4 courses | 10 s | 15,0 s | oui | accepté | fidèle (« Pas de fromage, il en reste. »), mais en phrase et non en liste |
| 5 « uniquement si » | 21 s | 16,3 s | oui | `restriction_lost` | brut inséré, condition intacte |
| 6 termes | 23 s | 20,0 s | **non** : Unbound → « Ansible », Jot → « Yeot », Claude → « Cloud » | accepté | l'erreur vient de l'ASR, le LLM ne l'aggrave pas |
| 7 IP / port | 19 s | 16,0 s | oui (forme orale) | `number_changed` | brut inséré, nombres en toutes lettres |
| 8 « pas forcément » | 40 s | 24,5 s | « Yiote » pour Jot | accepté | **correct**, « pas forcément très puissant » conservé |
| 60 s, 172 mots | 62 s | 34,4 s | oui | accepté | « je pense » perdu, « pas sûr », « à peu près », « uniquement après » et la rétractation pare-feu → proxy conservés |

Contrôles du même passage :
- **Timeouts** : aucun sur les 12 dictées. Marge la plus faible : ASR 15,1 s pour un budget de 107 s, et LLM 12,8 s pour un budget de 120 s, sur la dictée de 60 s. Le budget adaptatif n'a pas changé.
- **Réseau** : aucune connexion hors boucle locale. Aucun processus orphelin après l'arrêt. Deux serveurs par lancement.
- **Clé API de bout en bout** : Jot obtient ses réponses ASR et LLM avec la clé. La ligne de commande des serveurs contient `--cors-origins localhost` et aucune clé. Aucune chaîne de la forme d'une clé dans `llama-*.log` ni `jot.log`. Aucun avertissement « no API key ».
- **Journaux** : un refus du garde-fou est journalisé avec sa raison seule, jamais le texte.

### Cas 6 avec « Unbound » dans le dictionnaire du profil de test

| Cas | ASR | Texte final |
|---|---|---|
| r6 (même audio) | « Ansible » toujours, mais « PowerShell » désormais juste : le contexte est bien reçu | « Unbound » absent ; le LLM ne remplace pas « Ansible » par « Unbound », et c'est voulu |
| contraste « on a activé NBAR » sur Cisco | exact | NBAR conservé |
| contraste « Unbound … et NBAR classe le trafic » | « nbaron » pour Unbound | inchangé, NBAR conservé |

Conclusion pour le cas 6 :
- Le défaut est **acoustique, côté ASR**. Le brut contient déjà le mauvais mot avant la reformulation : « NBAR » sur le terrain, « Ansible » ou « nbaron » avec la voix de synthèse.
- Le dictionnaire aide l'ASR sur certains termes (PowerShell), mais ne suffit pas pour celui-ci.
- Une règle texte NBAR → Unbound casserait les phrases de contraste. Elle n'a donc pas été ajoutée.
- Si vous ne dites jamais « NBAR », vous pouvez ajouter dans le Dictionnaire l'entrée « Unbound » avec la faute « NBAR ». C'est un remplacement que vous choisissez, limité à votre profil.

### Cycle de libération mémoire (`localUnload: immediately`)

| Moment | Serveurs llama | Mémoire des serveurs | jot.exe |
|---|---:|---:|---:|
| chargement | 1 puis 2 | jusqu'à 8,7 Go | 80 Mo |
| résultat écrit (9,2 s) | 2 | 8,7 Go | 81 Mo |
| +1 s | **0** | 0 | 81 Mo |
| +20 s | 0 | 0 | 81 Mo |
| après arrêt | 0 orphelin | — | — |

Non couvert : le rechargement après libération dans le même processus, qui demande une vraie dictée au raccourci. Chaque lancement de l'E2E recharge à froid sans échec.

## 5. Audit CORS / clé API (llama-server b12289)

Le serveur a été lancé comme Jot le lance, avec `LLAMA_API_KEY` dans l'environnement et `--cors-origins localhost`.

| Requête | Résultat |
|---|---|
| `GET /health` sans clé | 200 : le contrôle de disponibilité de Jot marche toujours |
| `POST /v1/chat/completions` sans clé | 401 |
| idem en `text/plain`, comme un formulaire d'une autre page | 401 |
| mauvaise clé | 401 |
| bonne clé (`Bearer`) | 200 |
| `GET /v1/models`, `GET /props` sans clé | 401 |
| préflight `Origin: https://evil.example` | aucun `Access-Control-Allow-Origin` (journal : « skip non-localhost origin ») |
| préflight `localhost:3000`, `127.0.0.1:8080` | origine renvoyée : seul le local est autorisé |
| clé dans la ligne de commande ou le journal | absente |
| avertissement « no API key is set and CORS allows all origins » | disparu |

Risque couvert : une page web ouverte dans le navigateur, ou un site qui fait du DNS rebinding, ne peut plus utiliser les modèles locaux ni lire leurs réponses.

Non couvert : un programme local qui tourne sous le même compte peut lire l'environnement du processus. Ce n'est pas l'objet de la mesure.

« audio input is in experimental stage » est un avertissement amont du serveur ASR. Ce n'est pas une erreur.

## 6. Défauts résiduels (limites connues)

1. **Rétractation longue (cas 2)** : le 4B garde encore « le serveur était lent ». Ce cas n'est plus inséré faux : il est refusé, et c'est le brut qui est inséré, rétractation comprise. Sur des rétractations plus courtes (`n-retract-*`), la reformulation est correcte.
2. **Restriction répétée (cas 5, `n-uniquement-repete`)** : le modèle supprime la restriction. Le garde-fou insère le brut. Le texte est fidèle mais non nettoyé, et il garde la répétition dite.
3. **Dictée de 60 s** : « je pense » et « à peu près » peuvent disparaître alors que « pas sûr » reste. Le garde-fou ne le voit pas, parce qu'une incertitude reste. Le sens général est conservé, pas chaque nuance.
4. **Nombres dits à l'oral** (IP, port) : le 4B les corrompt parfois. Le garde-fou le détecte toujours (`number_changed`) et insère le brut.
5. **Non-régression** : `non-porteur` (« Non » de réponse perdu, la négation reste) et `h-long-bug` (date retirée conservée).
6. **Latence** : mesurée sur un Ryzen 9800X3D, plus rapide que le PC cible.
   - Sur les mêmes phrases, le terrain donnait 2,5 à 3 fois les durées LLM d'ici : 4,8 s contre 1,5 s pour le test 1, 21,3 s contre 8,5 s pour le test 8.
   - La nouvelle consigne ajoute environ 20 % de latence LLM moyenne sur ce corpus (3,4 s contre 2,8 s), surtout sur les phrases courtes. Sur la dictée de 172 mots, c'est 12,9 s contre 11,9 s.
   - Le budget adaptatif n'a pas changé.

7. **Courses (cas 4) avec l'audio réel** : quand l'ASR coupe « Ah et du fromage. Non, finalement… » en deux phrases, le modèle garde une phrase au lieu d'une liste. Le texte reste fidèle (pas de fromage), mais ce n'est pas une liste. Sur le texte du terrain, la liste est correcte.
8. **Termes rares (cas 6)** : c'est une limite de l'ASR, voir le § 4.

## 7. Contrôles

| Contrôle | Résultat |
|---|---|
| `cargo fmt --all --check` | OK |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | OK |
| `cargo test --workspace --locked` | 111 (jot) + 220 (jot-core, 4 ignorés) + 16 + 0 : tout passe |
| `scripts/package.ps1 -TestBuild` (refait fmt, lints et tests) | OK |
| E2E sur le binaire du ZIP | § 4 |

Tests ajoutés :
- dans `fidelity.rs` : `restriction_lost` et `qualifier_lost` refusent ce qu'ils doivent refuser, et 8 reformulations légitimes passent : « Si, et seulement si », « ne… que si », « pas nécessairement », rétractation FR et EN ;
- `prompts_match_the_benchmarked_variants` fige la consigne sur `docs/v151/bench/variants/v151{j,n}.json`.

## 8. Livrables

Dossier : `<worktree>\target\dist\`. Copie dans `E:\JotReleaseTest\v151-final\`. L'ancien candidat (`715D30C0…`) est déplacé dans `E:\JotReleaseTest\v151-previous-candidate\`.

| Fichier | SHA-256 |
|---|---|
| `Jot-1.5.1-test-portable.zip` | `F133E760BBB885B16D98E2BB025E8D4D62DADF3FA9CAE069AC6A216A23A5A0A4` |
| `Jot-1.5.1-test-setup.exe` | `3A97DAE2546806B40CBA72FA4981D50F5CD1E658F2B3151BC8CC1F4D06CA0752` |
| `jot.exe` (dans le ZIP, 25 929 216 octets) | `7CB14F285ACB8EDD288294D6EDE76512E9D3C69C71877CFC26102834CC8F0CF2` |

État Git :
- branche locale `claude/v151` ; binaire construit depuis `7fd3b03`, puis ce rapport en commit séparé, sans code ;
- rien de poussé, aucun tag, `main` et `v1.5.0` non modifiés ;
- les modifications non commitées de votre dépôt principal (branche `dev`) ne sont pas touchées.

Le contrôle d'instance unique est global : quittez Jot 1.5.0 avant de lancer le ZIP de test.

### À vérifier sur le PC cible

1. Les cas 2, 5 et 8 dictés à la voix.
   - Cas 2 et 5 : le texte inséré doit être le brut, avec la rétractation et « uniquement si » ; `model_id` se termine par `:rejected/…` dans l'historique.
   - Cas 8 : « pas forcément » doit être conservé.
2. Les durées par phase dans `profile\logs` : `inference_ms` doit rester sous `budget_s`.
3. Cas 6 : si « NBAR » revient toujours pour « Unbound », essayez l'entrée de dictionnaire décrite au § 4.

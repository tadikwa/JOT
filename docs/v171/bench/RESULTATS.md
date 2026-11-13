# Jot 1.7.1 — nombres et versions dans la reformulation locale

Moteur réel : Qwen3-ASR 1.7B + Qwen3.5 4B (pack rapide), RTX 5070 Ti, Windows 11.
Données synthétiques uniquement (aucune dictée réelle).

## Changement retenu

Dans `PROMPT_MIDDLE` (`crates/jot-core/src/local.rs`), la règle
« heures et quantités en chiffres (10 h, 4 Go). » devient :

> heures, dates, années, prix, quantités et mesures en chiffres (10 h, 12 mars,
> 2026, 20 dollars, 16 Go) ; numéros de version et adresses en chiffres séparés
> par des points (version 2.4.1, 10.0.0.1), sans changer un chiffre ni ajouter
> d'unité ; « un » article et « un point » sans nombre restent en lettres.

Aucune substitution déterministe : seul le modèle convertit, et la porte de
fidélité vérifie. La porte (`fidelity.rs`) refuse désormais aussi une suite
pointée réordonnée (192.168.1.50 → 192.168.50.1, 1.7.1 → 1.1.7) ; les
conversions équivalentes (un point sept → 1.7, vingt dollars → 20 dollars)
passent, les altérations (1.7 → 1.8, 20 → 200) restent refusées.

Variantes figées : `variants/v171j.json`, `variants/v171n.json`
(test `prompts_match_the_benchmarked_variants`).

## Texte injecté — 9 corpus, 156 cas (anglais compris)

`number_cases.json` (20 nouveaux cas) + `v151/bench/fidelity_cases.json` +
corpus v15 (reformulation, holdout, holdout2, smart_control/dev/holdout, llm).

| Variante | LLM correct | Refusés | Faux positifs | Final correct | Réellement reformulés |
|---|---|---|---|---|---|
| Avant (1.7.0) | 122 | 10 | 0 | 126 | 105 |
| p1 : exemples « 1.7.1 », « 192.168.1.50 » | 127 | 11 | 1 | 130 | 103 |
| **p2 : retenue** | **126** | **9** | **0** | **130** | **107** |
| p3 : p2 + rappel dans le message | 124 | 14 | 2 | 127 | 107 |

p1 rejetée : la valeur d'exemple fuit dans le cas anglais et n08 régresse.
p3 rejetée : régressions sur sept cas historiques (r4-courses, correction-ambigue,
h-long-bug, c-long-450, d-long-retour-produit, x02, x11).

Différences p2 / avant, cas par cas : gains n02 (« Jot 1.5 » au lieu de « 1,5 »),
n03 (« 1.7.1 »), n05 (« 20 dollars »), x08 ; f-liste-doublon passe de refusé à
accepté avec une ligne HDMI répétée, mais son texte final était déjà faux avant.
Aucune régression du texte final sur les corpus historiques.

## Audio synthétique — 20 WAV (SAPI, 16 kHz), ASR réel puis LLM réel

Résultats déterministes (deux passages identiques).

| Cas | Avant | Après |
|---|---|---|
| n02 « yott un point cinq » | « yott, un point cinq » | « yott 1.5 » |
| n03 « y un point sept point un » | refusé `number_changed`, brut | « à 1.7.1 » (le « y » mal reconnu devient « à ») |
| n05 « vingt dollars » | lettres | « 20 dollars » |
| n16 anglais | « which costs » (sens modifié) | brut conservé |
| n20 long (25 s) | refusé `number_changed`, brut | accepté : 1.7.1, 1.7.0, 2.0, 2027 |
| n06 « seize gigas » | « seize gigaoctets » | « seize Go » (mixte) |
| n09 « Il nie à aucun point à discuter » (ASR déjà faux) | conservé | « Il ne nie à aucun point. » — **perd « à discuter »** |

Inchangés : n01, n04, n08, n10–n15, n17–n19 ; n07 (adresse IP) reste au brut.

Bilan : 5 gains, 2 pertes, dont une perte de contenu (n09) sur une
transcription déjà incompréhensible. Retenu pour 1.7.1 au vu du gain sur le
texte (126 → 130, 0 faux positif) ; n09 reste une limite connue.

## Limites connues

- n07 : « 192.168.1.50 » correct est refusé par `validation`
  (`content_divergence_c0.43_t0.46`) car la mesure de divergence ne lit pas les
  nombres français écrits en lettres ; le brut est inséré. Même texte final
  qu'en 1.7.0 (alors refusé par `number_changed`). Non corrigé en 1.7.1.
- Restent en lettres : « trois cent quarante-neuf euros », « douze novembre à
  quinze heures trente », « twenty dollars », reprises de montants.
- Le prompt ne change rien en mode Gemini.

## Reproduire

```
JOT_LOCAL_E2E=<racine contenant local/models et local/runtime>
JOT_REFORM_PACK=fast
JOT_REFORM_CASES=<chemins absolus des corpus séparés par ;>
cargo test --locked -p jot-core --lib reformulation_bench -- --ignored --nocapture > bench.txt
python docs/v151/bench/score_fidelity.py "<mêmes chemins>" bench.txt

JOT_LOCAL_E2E_AUDIO=<dossier de WAV 16 kHz mono>
cargo test --locked -p jot-core --lib real_engine_end_to_end -- --ignored --nocapture
```

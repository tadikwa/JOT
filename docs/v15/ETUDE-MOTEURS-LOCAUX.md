# Jot 1.5 — étude des moteurs locaux (transcription et reformulation)

Date des mesures et des vérifications : **11 novembre 2026**.
Les sections distinguent trois types d'information :

- **[Source]** : fait vérifié sur une page officielle (URL citée, consultée le 11/11/2026) ;
- **[Mesure]** : chiffre produit par le banc d'essai de `docs/v15/bench/` sur la machine décrite plus bas ;
- **[Avis]** : appréciation, à prendre comme telle.

## 1. Résumé et décision

| Rôle | Choix pour la 1.5 | Pourquoi |
| --- | --- | --- |
| Runtime | **llama.cpp `llama-server`**, build officiel **Vulkan x64** (b12289), téléchargé à la demande | Un seul binaire (38 Mo) pour la transcription **et** la reformulation ; MIT ; fonctionne sur GPU NVIDIA/AMD/Intel via Vulkan et retombe sur le CPU (les DLL `ggml-cpu-*` sont incluses dans le même zip). |
| Transcription | **Qwen3-ASR 1.7B, GGUF Q8_0** (dépôt officiel `ggml-org`) | Meilleur WER mesuré (2,8 % FLEURS, 3,0 % corpus technique), 25/28 termes techniques, 0,15–0,26 s par dictée sur GPU, ~2 s sur CPU. Apache-2.0. |
| Reformulation (optionnelle) | **Qwen3.5-4B Q4_K_M** par défaut, **Qwen3.5-9B Q4_K_M** en option « qualité » | 22/24 contrôles de fidélité, aucune réponse à une question dictée, aucune injection suivie, 0–1 mot ajouté. Apache-2.0. GGUF communautaire (Unsloth) épinglé par SHA-256. |

Écartés ou gardés en réserve, avec la raison :

- **Ministral 3 8B Instruct** : suit les injections et répond aux questions dictées (9 échecs sur 24). **[Mesure]** Écarté.
- **Qwen3-8B** : répond « 144 » à « combien font douze fois douze » et traduit une consigne en anglais. **[Mesure]** Écarté.
- **Gemma 4 12B (QAT, officiel Google)** : aussi fidèle que Qwen3.5-9B, mais 8,3 Go de VRAM ; avec l'ASR, plus de 11,8 Go. Réserve pour une version ultérieure. **[Mesure]**
- **Gemma 4 E4B** : perd une partie d'une consigne dictée (x07) et ajoute 15 mots au total. **[Mesure]**
- **Parakeet TDT 0.6B v3** : excellent sur CPU (0,8 s, 3,7 % FLEURS en q8 ggml) et **n'hallucine pas sur le silence**, mais 18/28 termes techniques (« Ansuble », « PlayBuck ») et un second runtime (whisper.cpp `parakeet-cli`, sans serveur). Meilleur candidat de secours si l'on veut un moteur CPU léger. **[Mesure]**
- **Whisper large-v3-turbo** : bon sur GPU (0,08–0,15 s, 3,8–4 % FLEURS), mais ~4,7 s par dictée sur CPU et hallucination connue sur le silence (« Sous-titrage Société Radio-Canada »). **[Mesure]**
- **Qwen3-ASR via Python/transformers** : même famille, mais embarquer Python + PyTorch (~4 Go) dans Jot n'est pas raisonnable. **[Avis]**
- **Voxtral Mini 4B Realtime, Cohere Transcribe, Canary 1B v2** : non mesurés (voir § 7).

## 2. Machine et conditions de mesure

- Windows 11 Pro 10.0.26300, AMD Ryzen 7 9800X3D (8 c / 16 t), 31,2 Go de RAM.
- NVIDIA GeForce RTX 5070 Ti 16 Go (Blackwell, sm_120), pilote 621.01. Environ 2,2 Go de VRAM déjà occupés par
  le bureau et d'autres applications pendant les mesures : la colonne « VRAM » est l'**augmentation** de la
  mémoire GPU globale mesurée par NVML toutes les 50 ms (Windows WDDM ne donne pas la mémoire par processus).
- « RSS » : pic de mémoire de travail du processus et de ses enfants (psutil). Pour llama.cpp, elle inclut les
  pages du fichier modèle projeté en mémoire, ce qui la gonfle.
- Latence = horloge murale côté client, **serveur déjà chaud**, sauf pour `parakeet-cli` qui relance un
  processus par dictée (chargement inclus). Température 0 : sorties déterministes, une passe par cas.

## 3. Corpus et métriques

- **tts** : 12 phrases françaises **fictives** (court, technique Proxmox/Ansible/Kubernetes/Claude Code,
  hésitations, autocorrection orale, noms accentués, nombres, question, injection, négation, texte long de 75 mots),
  synthétisées avec la voix SAPI « Microsoft Hortense » (16 kHz mono). Fichier : `bench/corpus.json`.
- **noisy** : les mêmes, avec bruit rose à 12 dB de SNR.
- **fleurs** : 25 phrases lues par des humains, jeu de test `fr_fr` de Google FLEURS (CC-BY-4.0), les
  25 premiers identifiants. Non redistribuées ici ; le script les extrait du parquet officiel.
- **silence** : 3 s de silence numérique, 5 s de bruit rose faible, 4 s de ronflement 50 Hz.
- **WER** : `jiwer`, après normalisation (minuscules, ponctuation retirée, nombres → lettres via `num2words`,
  « 17h » → « dix-sept heures », « Go » → « gigaoctets »). **Limite** : la voix de synthèse est plus régulière
  qu'une voix réelle et ne contient pas de vraies hésitations ; le WER `tts` est donc optimiste. FLEURS est de la
  parole lue, pas spontanée. Aucun audio réel privé n'a été utilisé.
- **Termes** : nombre de termes attendus (Proxmox, Ansible, Ceph, nginx, Mathilde, Besançon…) retrouvés tels quels.
- **Reformulation** : 24 cas, soit les 12 sorties ASR réelles du corpus tts et 12 cas adverses écrits
  (`bench/llm_cases.json`) : question à ne pas traiter, calcul, demande de poème, « traduis », injection avec une
  fausse balise `</dictee>`, « ignore les règles », commande `cargo`/`git`, nombres, négation, autocorrection.
  Contrôles : chaînes obligatoires ou interdites ; **fidélité** = 1 − WER par rapport au texte propre attendu ;
  **ajouts** = mots absents de l'entrée et de la cible (indicateur d'hallucination).

## 4. Résultats — transcription **[Mesure]**

WER par jeu, termes (tts), latence moyenne/max (s, tts), chargement jusqu'au premier résultat, pics VRAM/RSS (Mio).

| Moteur (runtime, précision, matériel) | WER tts | WER bruit | WER FLEURS | Termes | Lat. moy. / max | 1er résultat | VRAM | RSS |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| **Qwen3-ASR 1.7B Q8_0** (llama.cpp CUDA) | **3,0 %** | **4,3 %** | **2,8 %** | **25/28** | 0,15 / 0,52 | 1,6 s | 3 497 | 6 638 |
| Qwen3-ASR 1.7B Q8_0 (llama.cpp **Vulkan**) | 3,0 % | 4,3 % | 2,8 % | 25/28 | 0,26 / 1,40 | 7,6 s ¹ | 3 190 | 6 340 |
| Qwen3-ASR 1.7B Q8_0 (llama.cpp CPU) | 3,0 % | 4,7 % | 2,8 % | 25/28 | 1,82 / 6,78 | 1,9 s | 0 | 7 453 |
| Qwen3-ASR 0.6B Q8_0 (llama.cpp CUDA) | 8,2 % | 8,6 % | 7,2 % | 19/28 | 0,10 / 0,35 | 1,0 s | 2 199 | 4 328 |
| Qwen3-ASR 0.6B Q8_0 (llama.cpp CPU) | 8,2 % | 8,2 % | 7,0 % | 19/28 | 0,66 / 2,56 | 1,1 s | 0 | 4 879 |
| Qwen3-ASR 1.7B bf16 (Python qwen-asr, CUDA) | 5,6 % | 6,0 % | 2,8 % | 22/28 | 0,90 / 3,38 | — ² | 4 811 | 4 930 |
| Parakeet TDT 0.6B v3 q8_0 (whisper.cpp, CUDA 12.4) | 6,4 % | 7,3 % | 3,7 % | 18/28 | 0,75 / 0,82 ³ | 0,8 s | 1 214 | 455 |
| Parakeet TDT 0.6B v3 q8_0 (whisper.cpp, CPU) | 6,4 % | 7,3 % | 3,7 % | 18/28 | 0,78 / 1,71 ³ | 0,8 s | 0 | 923 |
| Parakeet TDT 0.6B v3 int8 (sherpa-onnx, CPU) | 8,2 % | 7,3 % | 6,6 % | 18/28 | 0,22 / 0,79 | 1,5 s | 0 | 1 530 |
| Whisper large-v3-turbo f16 (whisper.cpp CUDA) | 6,0 % | 6,0 % | 3,8 % | 20/28 | 0,14 / 0,29 | 1,4 s | 2 123 | 906 |
| Whisper large-v3-turbo q5_0 (whisper.cpp CUDA) | 5,6 % | 6,0 % | 4,0 % | 21/28 | 0,08 / 0,22 | 5,2 s ¹ | 1 115 | 1 136 |
| Whisper large-v3-turbo q5_0 (whisper.cpp CPU) | 5,6 % | 6,0 % | 4,0 % | 22/28 | 4,67 / 5,13 | 5,1 s | 0 | 1 582 |

¹ Premier lancement : compilation des shaders Vulkan ou des noyaux CUDA (JIT PTX). Au second lancement de la même
paire Vulkan, le premier résultat tombe de 24,3 s à 5,6 s (voir § 6).
² Inclut le téléchargement depuis Hugging Face ; non comparable.
³ Un processus par dictée, chargement du modèle compris (pas de mode serveur pour `parakeet-cli`).

Le build CUDA 12.4 de whisper.cpp ne contient pas de noyaux pour Blackwell (`ARCHS = …,890,900`). Le premier
lancement compile en JIT pendant ~15 s, puis le cache CUDA prend le relais. **[Mesure]**

**Silence et bruit seuls [Mesure]** :

| Moteur | Silence 3 s | Bruit rose 5 s | Ronflement 50 Hz |
| --- | --- | --- | --- |
| Qwen3-ASR 1.7B | phrase chinoise inventée | (vide) | « ¿Qué es la vida? » |
| Qwen3-ASR 0.6B | titre chinois inventé | (vide) | (vide) |
| Whisper turbo | « Sous-titrage Société Radio-Canada » | « ... » | « ... » |
| Parakeet v3 (deux runtimes) | (vide) | (vide) | (vide) |

Forcer la langue (`language French<asr_text>` en préremplissage) **aggrave** le problème : le modèle invente alors
une phrase française plausible (« La ville est située à 10 km au nord-ouest de Saint-Étienne. »). Conséquences pour
l'intégration : garder le contrôle d'énergie existant de Jot avant la transcription, ne **pas** forcer la langue, et
rejeter une transcription dont la langue détectée n'est ni le français ni l'anglais (Qwen3-ASR renvoie
`language X<asr_text>…`).

## 5. Résultats — reformulation **[Mesure]**

24 cas, llama.cpp CUDA sauf mention, prompt strict de `bench/prompt.txt`, réflexion désactivée
(`enable_thinking: false`).

| Modèle (GGUF) | Fichier | Fidélité moy. | Contrôles OK | Mots ajoutés | Lat. moy. / max | Chargement | VRAM |
| --- | --- | --- | --- | --- | --- | --- | --- |
| **Qwen3.5-4B Q4_K_M** (Unsloth) | 2,74 Go | 0,921 | 22/24 | **0** | 0,15 / 0,62 s | 1,6 s | 3 249 Mio |
| **Qwen3.5-9B Q4_K_M** (Unsloth) | 5,68 Go | 0,920 | 22/24 | 1 | 0,21 / 0,95 s | 3,5 s | 5 539 Mio |
| Gemma 4 12B it QAT q4_0 (Google) | 6,98 Go | 0,922 | 22/24 | 1 | 0,32 / 1,30 s | 4,4 s | 8 345 Mio |
| Gemma 4 E4B it QAT q4_0 (Google) | 5,15 Go | 0,890 | 21/24 | 15 | 0,16 / 0,66 s | 2,8 s | 3 329 Mio |
| Qwen3-8B Q4_K_M (Qwen) | 5,03 Go | 0,841 | 21/24 | 16 | 0,17 / 0,88 s | 3,0 s | 5 373 Mio |
| Ministral 3 8B Instruct Q4_K_M (Mistral) | 5,20 Go | 0,580 | 15/24 | 179 | 0,23 / 1,39 s | 3,1 s | 5 561 Mio |
| Qwen3.5-9B Q4_K_M — **Vulkan** | | 0,920 | 22/24 | 1 | 0,35 / 2,02 s | 7,0 s | 5 380 Mio |
| Qwen3.5-4B Q4_K_M — **CPU seul** | | 0,879 | 22/24 | 4 | 1,31 / 6,44 s | 2,2 s | 0 (RSS 4,5 Go) |
| Qwen3.5-9B Q4_K_M — **CPU seul** | | 0,906 | 22/24 | 1 | 2,38 / 12,16 s | 2,7 s | 0 (RSS 8,1 Go) |

Lecture des échecs :

- Les deux échecs communs aux meilleurs modèles :
  - `s06` : le modèle écrit « 64 Go / 2 To » au lieu de « gigaoctets / téraoctets ». C'est acceptable, le contrôle
    était trop strict.
  - `x04` : « le mot de passe est pas le bon, enfin je veux dire l'identifiant… ». Aucun modèle n'applique cette
    autocorrection orale ; tous gardent les deux versions. C'est un **vrai défaut**, sans perte de sens.
- **Qwen3.5-4B** laisse « trois cent quarante-deux » en lettres là où le 9B et Gemma 12B écrivent « 342 ».
- **Aucun** des trois retenus n'a répondu à une question dictée (météo, capitale, calcul), écrit de poème, traduit,
  suivi la fausse balise `</dictee>` ni révélé son identité.
- **Termes mal transcrits** (« Padjunks » pour « pod nginx ») : aucun modèle ne les corrige à partir du
  vocabulaire. La reformulation ne remplace donc pas le dictionnaire de Jot, qui reste appliqué après.

## 6. Bout en bout, deux modèles résidents **[Mesure]**

Les deux serveurs llama.cpp sont chargés en même temps ; on mesure ASR puis LLM sur les 12 phrases tts.

| Configuration | Chargement des deux | 1er résultat | Total moy. / max | VRAM | RSS |
| --- | --- | --- | --- | --- | --- |
| CUDA — ASR 1.7B + Qwen3.5-9B | 2,6 s | 3,0 s | 0,42 / 1,51 s | 9 032 Mio | 8,0 Go |
| Vulkan — ASR 1.7B + Qwen3.5-9B (1er lancement) | 9,3 s | 24,3 s | 0,65 / 2,48 s | 8 570 Mio | 7,8 Go |
| Vulkan — ASR 1.7B + Qwen3.5-9B (2e lancement) | 5,2 s | 5,6 s | 0,46 / 1,62 s | 8 580 Mio | 7,6 Go |
| Vulkan — ASR 1.7B + Qwen3.5-4B | 3,4 s | 7,3 s | 0,48 / 1,78 s | 6 274 Mio | 5,3 Go |
| CPU — ASR 1.7B + Qwen3.5-4B | 1,9 s | 4,5 s | 3,41 / 12,98 s | 0 | 7,4 Go |
| CPU — ASR 0.6B + Qwen3.5-4B | 1,8 s | 3,9 s | 2,45 / 9,89 s | 0 | 6,2 Go |

Conclusions **[Avis]** :

- Sur une carte de 16 Go, les deux modèles tiennent ensemble. Sur une carte de 8 Go, seul ASR 1.7B + 4B passe
  (~6,3 Go), et sans marge si d'autres applications utilisent le GPU.
- Il ne faut donc pas supposer que les deux tiennent : Jot doit laisser la reformulation **désactivable**, la
  décharger après inactivité et basculer sur le CPU de façon visible.
- Sur CPU seul, la dictée reste utilisable (2 à 3 s en moyenne), mais un texte long peut dépasser 10 s.
  L'interface doit l'annoncer, et un délai maximal doit rendre la transcription brute plutôt que d'attendre.

## 7. Candidats non mesurés

| Candidat | Raison |
| --- | --- |
| Cohere Transcribe 03-2026 (2B, Apache-2.0) | Dépôt à accès restreint (formulaire) : je n'ai pas demandé d'accès au nom de l'utilisateur. Un blog tiers annonce 4,02 % de WER en français, non vérifié ici. |
| Voxtral Mini 4B Realtime 2602 (Apache-2.0) | Poids BF16 de 8,9 Go et ≥ 16 Go de GPU recommandés, runtime vLLM ou transformers. Incompatible avec l'objectif « ASR + LLM sur la même carte ». |
| NVIDIA Canary 1B v2 (CC-BY-4.0) | Runtime NeMo/ONNX supplémentaire, pas de build Windows officiel prêt à l'emploi. |
| Kyutai STT, Granite Speech 4.1 | Hors délai ; à évaluer plus tard. |
| Ollama | Non imposé par la mission, non installé ; llama.cpp officiel suffit et évite un service système. |

## 8. Licences, provenance et intégrité **[Source]**

| Élément | Licence | Provenance | Taille | SHA-256 |
| --- | --- | --- | --- | --- |
| llama.cpp b12289 `llama-b12289-bin-win-vulkan-x64.zip` | MIT | github.com/ggml-org/llama.cpp/releases/tag/b12289 (11/11/2026) | 38 022 207 o | `61198de9f9a4dbaa77c8efb022e1dc436e0c55f79556b6b9ea05af37242a5ad4` |
| llama.cpp b12289 `llama-b12289-bin-win-cpu-x64.zip` | MIT | même release | 25 057 945 o | `553185cad8986aff5d951513e94328435f982835d00aa1838ccf7445df4c3f96` |
| `Qwen3-ASR-1.7B-Q8_0.gguf` | Apache-2.0 (modèle Qwen) | huggingface.co/ggml-org/Qwen3-ASR-1.7B-GGUF | 2 165 034 944 o | `58e22d0532d4eacaf034cfac17a6fed159f37c41390c710186783be439d1fc57` |
| `mmproj-Qwen3-ASR-1.7B-Q8_0.gguf` | Apache-2.0 | idem | 355 709 344 o | `46c1d533af3f354ceb37ce855dbceff7da7fa7cf1e6a523df3b13440bd164c0d` |
| `Qwen3.5-4B-Q4_K_M.gguf` | Apache-2.0 | huggingface.co/unsloth/Qwen3.5-4B-GGUF (conversion communautaire) | 2 740 937 888 o | `00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4` |
| `Qwen3.5-9B-Q4_K_M.gguf` | Apache-2.0 | huggingface.co/unsloth/Qwen3.5-9B-GGUF (conversion communautaire) | 5 680 522 464 o | `03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8` |

Tailles et SHA-256 relevés sur l'API Hugging Face (champ `lfs.oid`, qui est le SHA-256 du fichier) le 11/11/2026 ;
Jot les vérifie après chaque téléchargement.

## 9. Confidentialité **[Mesure + Avis]**

- llama.cpp `llama-server` est lancé par Jot sur `127.0.0.1` (port choisi au hasard), avec `--no-webui`. Jot ne
  contacte que cette adresse pendant une dictée locale.
- Les serveurs ne font aucune requête sortante tant qu'on ne leur passe pas `-hf`/`--hf-repo`, que Jot n'utilise
  jamais : Jot fournit des chemins de fichiers locaux.
- Les seules connexions sortantes du mode local sont les téléchargements de runtime et de modèles, lancés par
  l'utilisateur.

## 10. Reproduire

```powershell
cd docs\v15\bench
uv venv -p 3.12 .venv; uv pip install -p .venv\Scripts\python.exe numpy soundfile pyarrow jiwer psutil requests pynvml num2words sherpa-onnx==2.0.1
powershell -File tts.ps1 -Corpus corpus.json -Out audio-tts      # voix « Microsoft Hortense Desktop » requise
# Placer runtimes et modèles sous ..\dl\x\<build>\ et ..\dl\m\ (voir § 8), puis :
.venv\Scripts\python.exe asr.py llama-asr:llama-vulkan:1.7B:99 tts noisy fleurs
.venv\Scripts\python.exe llm.py qwen35-4b llama-vulkan Qwen3.5-4B-Q4_K_M.gguf 99
.venv\Scripts\python.exe e2e.py llama-vulkan 1.7B Qwen3.5-4B-Q4_K_M.gguf 99 vulkan-4b
.venv\Scripts\python.exe score.py; .venv\Scripts\python.exe score_llm.py
```

Les fichiers audio, les modèles et les résultats bruts ne sont pas versionnés.

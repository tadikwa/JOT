# Banc d'essai des moteurs locaux

Scripts utilisés pour `../ETUDE-MOTEURS-LOCAUX.md`. Aucun audio, modèle ni résultat n'est versionné.

- `corpus.json` : 12 phrases fictives (texte dit, référence, texte propre attendu, termes).
- `tts.ps1` : synthèse SAPI (voix « Microsoft Hortense Desktop », 16 kHz mono) vers `audio-tts/`.
- `asr.py <moteur> <jeux…>` : transcription, WER, latence, pics RAM/VRAM → `results/asr-*.jsonl`.
- `llm.py <label> <build> <gguf> <ngl>` : 24 cas de reformulation (`llm_cases.json` + sorties ASR) avec `prompt.txt`.
- `e2e.py <build> <taille ASR> <gguf LLM> <ngl> <label>` : ASR + LLM résidents ensemble.
- `score.py`, `score_llm.py` : tableaux récapitulatifs.

Les serveurs `llama-server` sont lancés sur 127.0.0.1 et arrêtés par PID à la fin de chaque script.

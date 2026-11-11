# Jot 1.5 — Mémoire des modèles locaux et premier lancement

## Conception

- **A1, chargement** : rien n'est chargé au démarrage. Les serveurs llama (ASR et LLM) démarrent à l'appui sur la touche de dictée, pendant que l'utilisateur parle.
- **A2, libération** : par défaut, 5 minutes après la fin de la dernière dictée. `LocalEngine` tient une garde RAII `busy` pendant la capture, le traitement et la file d'attente. Un « reaper » vérifie toutes les 5 s (`REAPER_TICK`) et ne libère jamais tant que la garde est tenue. Les modèles sont aussi libérés au passage en mode Gemini (après la dictée en cours) et à la fermeture de Jot : les serveurs vivent dans un job Windows `KillOnClose`.
- **A3, réglage** `localUnload` : Immédiatement, 1, 5 (par défaut), 10, 15 ou 30 minutes, ou Garder chargés. Le bouton « Libérer la mémoire maintenant » libère tout de suite. La ligne « Modèles : … » indique l'état et le matériel.
- **B, premier lancement** : l'assistant propose Gemini, 100 % local ou les deux. Avant tout téléchargement, il affiche la taille, la licence et le SHA-256 des modèles, et demande confirmation. Le choix « Les deux » démarre sur Gemini ; le passage au local reste un choix explicite dans les réglages, sans bascule silencieuse. Les utilisateurs existants (`hasCompletedOnboarding`) ne revoient pas l'assistant.

## Mesures sur le binaire de test

Conditions : profil isolé, RTX 5070 Ti, VRAM de base 1851 MiB, une dictée synthétique. Le script `mem.ps1` n'arrête que ses propres PID.

| Situation | Résultat |
|---|---|
| Démarrage, sans dictée | 0 llama-server, RSS de jot 86 MiB, VRAM +12 MiB |
| Dictée, `immediately` | traitée en 5,2 s ; pic RSS jot+llama 5070 MiB, pic VRAM +7769 MiB ; 12 s après : 0 enfant, VRAM +12 MiB |
| Dictée, `fiveMinutes` | 12 s après : modèles toujours chargés (2 enfants, VRAM +7769 MiB), comme attendu |
| Arrêt de jot | 0 orphelin ; VRAM revenue à +0 après tous les essais |
| Chargement à chaud des modèles | environ 4,8 à 5,6 s |

Le pic mesuré inclut l'ASR 1.7B Q8 et le LLM 4B Q4_K_M chargés ensemble. Le 9B ajoute environ 3 Go (taille du fichier).

## Non vérifié

- L'expiration réelle des 5 minutes sur le binaire (couverte par le test unitaire `models_are_freed_only_after_the_delay_and_never_during_a_dictation`).
- Le chargement déclenché par la vraie touche.
- Le 9B sur le cycle complet.
- Une machine sans GPU dédié.

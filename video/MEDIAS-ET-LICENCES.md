# Médias utilisés et licences

Vidéo : `Jot-1.5.0-demo-FR.mp4` et son affiche `Jot-1.5.0-demo-FR.png`, jointes à la version
[v1.5.0](https://github.com/tadikwa/jot-gemini-transcribe-Windows/releases/tag/v1.5.0). Elles ne sont
pas dans l’historique Git ; ce dossier contient de quoi les refaire.

| Élément | Origine | Licence | Remarque |
|---|---|---|---|
| Images de l’application (`public/stills/*.png`) | Jot 1.5.0, build de test isolé (`scripts/package.ps1 -TestBuild`), fenêtres capturées une à une par `PrintWindow` (voir `capture/README.md`) | Apache 2.0 (le projet) | Captures réelles, sans retouche ni montage d’interface. Profils jetables : aucune clé Gemini (étape laissée vide), aucun historique réel. Les dictées visibles sont 18 textes fictifs lus par une voix de synthèse Windows, puis passés dans le vrai `jot.exe` en mode local. Ni bureau, ni notification, ni nom de périphérique audio n’apparaissent. |
| Ce qui n’est **pas** montré | — | — | La pastille de dictée et l’insertion dans une autre application ne sont pas filmées (la capture d’écran du bureau n’était pas accessible depuis l’environnement de test, et filmer le bureau personnel était exclu). La vidéo le dit en sous-titre. |
| Musique (`public/audio/score.wav`, générée, non versionnée) | Composition originale synthétisée note par note par `music/score.py`, adapté du générateur écrit par le même auteur pour la vidéo de LocalChords | Aucun droit tiers | Ni échantillon ni boucle externe. |
| Logo (`public/logo.png`) | Extrait de `crates/jot/assets/jot.ico` (image 256 px) avec ffmpeg | Apache 2.0 (le projet) | |
| Police Inter | Google Fonts, chargée par `@remotion/google-fonts` au rendu | SIL Open Font License 1.1 | |
| Remotion 4.0.542 (et `@remotion/*`) | npm | Remotion License, « Free License » | Gratuite pour un particulier, une entreprise de 3 salariés au plus ou une association. Au-delà, une Company License est obligatoire (https://www.remotion.dev/docs/license). |
| React 19 | npm | MIT | |
| FFmpeg / libx264 (rendu) | Binaire fourni par Remotion | GPL (libx264) | Concerne l’outil, pas la vidéo. La diffusion gratuite de vidéos H.264 sur Internet est exemptée de redevance. |

Pas de voix off. Les sous-titres sont intégrés à l’image, en français.

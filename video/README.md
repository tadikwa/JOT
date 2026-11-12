# Vidéo de présentation de Jot 1.5

`Jot-1.5.0-demo-FR.mp4` : 62 s, 1920 × 1080, 30 i/s, H.264 + AAC, sous-titres français intégrés, pas
de voix off. L’affiche `Jot-1.5.0-demo-FR.png` sert de vignette. Les deux fichiers sont joints à la
version [v1.5.0](https://github.com/tadikwa/jot-gemini-transcribe-Windows/releases/tag/v1.5.0) et ne
sont pas dans Git (`out/` est ignoré).

## Montage

Une mesure de la musique dure 60 images (120 BPM) ; chaque partie commence sur une barre de mesure.

| Images | Temps | Partie | Images de l’application |
|---|---|---|---|
| 0–180 | 0–6 s | Logo, « Appuyez. Parlez. C’est écrit. » | — |
| 180–540 | 6–18 s | Premier lancement : moteur, puis mode de déclenchement | `o1`, `o2` |
| 540–900 | 18–30 s | Historique : la liste mise en forme, puis le texte brut | `h1`, `h2`, `h3` |
| 900–1260 | 30–42 s | Dictionnaire : une correction, la règle proposée, acceptée | `d0`, `d1`, `d2` |
| 1260–1680 | 42–56 s | Mode 100 % local : modèles à télécharger, réglages | `o3`, `s1` |
| 1680–1860 | 56–62 s | Fin : lien GitHub, licence, binaire non signé | — |

Le code est dans `src/` : `Presentation.tsx` (montage et affiche), `Shot.tsx` (caméra sur une capture,
cercles de mise en avant en pixels de la capture), `Caption.tsx` (sous-titres), `theme.ts` (couleurs
de Jot). Les origines et licences de chaque média sont dans [MEDIAS-ET-LICENCES.md](MEDIAS-ET-LICENCES.md),
la façon de refaire les captures dans [capture/README.md](capture/README.md).

## Refaire la vidéo

Node 20.19 ou plus récent, Python 3 avec numpy, scipy et soundfile pour la musique.

```bash
npm ci
python music/score.py public/audio/score.wav   # musique originale, 62 s (non versionnée)
npx remotion studio                            # aperçu interactif
npx remotion render Presentation out/Jot-1.5.0-demo-FR.mp4 --codec=h264 --audio-codec=aac
npx remotion still Poster out/Jot-1.5.0-demo-FR.png
```

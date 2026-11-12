# GPU Intel / NPU : faisabilité, preuves, limites, prochaines pistes (Jot 1.5.1)

Note non bloquante. Rien n'est intégré dans 1.5.1 : aucun backend, aucune option, aucun téléchargement.

Statut de chaque affirmation :

- **[doc]** : écrit dans une documentation officielle, avec le lien ;
- **[théorie]** : déduction non vérifiée ;
- **[mesuré]** : benchmark réel. Il n'y en a **aucun** ici, car la machine de développement (Ryzen + RTX + iGPU AMD) n'a ni GPU ni NPU Intel.

## 1. Matériel cible : ce que l'on sait, et ce qu'il reste à relever

Captures et journaux :

- un « Intel Graphics » intégré, partageant une RAM de 31,4 Go ;
- un « NPU Intel AI Boost » à 0 % au repos ;
- Jot 1.5.0 démarre en `--device none`, donc sur le CPU.

Le libellé « AI Boost » n'apparaît que sur les Core Ultra Series 1/2/3. C'est la liste des plateformes du pilote NPU Intel [doc] ([pilote NPU Intel](https://www.intel.com/content/www/us/en/download/794734/intel-npu-driver-windows.html)). En revanche, **le modèle exact n'est pas connu**.

À relever sur le PC cible. Ces commandes sont en lecture seule : elles n'installent rien et ne modifient aucune politique.

```powershell
Get-CimInstance Win32_Processor | Select-Object Name, NumberOfCores, NumberOfLogicalProcessors
Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, AdapterRAM
Get-PnpDevice -FriendlyName '*AI Boost*','*NPU*' | Select-Object FriendlyName, InstanceId, Status
Get-CimInstance Win32_PnPSignedDriver | Where-Object DeviceName -match 'AI Boost|NPU|Graphics' | Select-Object DeviceName, DriverVersion, DriverDate
[System.Environment]::OSVersion.Version; (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion').DisplayVersion
& "$env:LOCALAPPDATA\Jot\local\runtime\b12289\llama-server.exe" --list-devices
```

La dernière ligne indique comment Vulkan nomme l'iGPU. C'est elle qui décide de ce que fait Jot (voir le § 2).

## 2. iGPU Intel par Vulkan : déjà fourni, jamais choisi, à mesurer

- Jot embarque `llama-b12289-bin-win-vulkan-x64.zip`. Utiliser l'iGPU **ne demande donc aucun téléchargement**.
- `pick_device()` (`crates/jot-core/src/local.rs`) ne retient une carte Intel que si son nom Vulkan contient « arc ».
  - Un iGPU annoncé « Intel(R) Graphics » reste donc sur le CPU. C'est cohérent avec les journaux.
  - Un iGPU « Intel(R) Arc(TM) Graphics » (Core Ultra H avec mémoire bicanal) serait en revanche **déjà sélectionné**, en RAM partagée. Ce cas n'a jamais été mesuré.
- llama.cpp ne promet rien sur l'iGPU Intel en Vulkan. `build.md` montre seulement un exemple de journal `Using Intel(R) Graphics (ADL GT2) | uma: 1` [doc] ([build.md](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md)).
- Avec la RAM partagée, la génération de tokens est limitée par la bande passante mémoire, qui est la même que pour le CPU. Le gain attendu porte plutôt sur le traitement du prompt (consigne + brut) et sur la charge CPU que sur la vitesse de génération [théorie].

**Benchmark proposé** (PC cible, profil jetable, sans téléchargement) :

1. Lancer `llama-bench` ou `llama-server` du runtime fourni, avec `--device none`, puis avec `--device VulkanN`.
2. Utiliser le même GGUF 4B, `-c 4096 -np 1`, et 5 dictées fictives de référence.
3. Mesurer :
   - le temps du premier lancement (compilation des shaders) ;
   - les tokens/s du prompt et de la génération ;
   - la RAM et le commit maximum ;
   - la charge CPU ;
   - le texte produit et le passage des contrôles de fidélité.
4. Critère d'intégration : un gain reproductible sur au moins un scénario réel et aucune régression. Sinon, statu quo.

## 3. SYCL et OpenVINO sur l'iGPU : possibles, mais trop lourds pour 1.5.1

- **SYCL** :
  - documenté pour les iGPU Intel de Core 11e génération et plus, et vérifié sur les « built-in Arc GPU in Meteor Lake, Arrow Lake, Lunar Lake » sous Windows 11 [doc] ([SYCL.md](https://github.com/ggml-org/llama.cpp/blob/master/docs/backend/SYCL.md)) ;
  - la même page avertit qu'en dessous de 80 EU la vitesse sera « likely be too slow for practical use » [doc] ;
  - le runtime `llama-b12289-bin-win-sycl-x64.zip` est un second runtime à télécharger.
- **OpenVINO** :
  - backend llama.cpp ciblant les CPU, iGPU et NPU Intel, « actively under development » [doc] ([OPENVINO.md](https://github.com/ggml-org/llama.cpp/blob/master/docs/backend/OPENVINO.md)) ;
  - runtime `llama-b12289-bin-win-openvino-2026.4.1-x64.zip` ([release b12289](https://github.com/ggml-org/llama.cpp/releases/tag/b12289)).

Ces deux pistes demandent un runtime de plusieurs centaines de Mo, que ce poste ne permet pas de tester. Il faudra donc d'abord le benchmark Vulkan du § 2, puis une autorisation explicite avant tout téléchargement.

## 4. NPU AI Boost : non exploitable avec le pipeline actuel

D'après le tableau de compatibilité du backend OpenVINO de llama.cpp, la seule voie GGUF vers le NPU [doc] ([OPENVINO.md](https://github.com/ggml-org/llama.cpp/blob/master/docs/backend/OPENVINO.md)) :

- **LLM** :
  - **Qwen3.5 Q4_K_M échoue sur NPU (✗) en 0.8B, 2B, 4B et 9B**, alors qu'il passe sur CPU et GPU. C'est la famille utilisée par Jot.
  - Qwen3 0.6B à 8B passe sur NPU. En faire le moteur de reformulation serait un **changement de modèle**, à évaluer contre les garde-fous de fidélité. Il ne doit jamais être silencieux.
- **ASR** :
  - le backend ne prend en charge que les « text-only models » ;
  - Qwen3-ASR (audio) n'y figure pas, ni aucun modèle ASR ;
  - l'ASR resterait donc sur le CPU ou le GPU.
- **Limites NPU documentées** :
  - mode « stateless » uniquement ;
  - quantification principale `Q4_0` ;
  - contexte à garder petit (l'exemple utilise `-c 512`) ;
  - `-np > 1` non pris en charge.
- **Validation** : un seul poste, Core Ultra 5 238V (Lunar Lake). La page cite « Intel NPU Driver 1.38.0 », alors que la page Intel numérote ses pilotes en 32.0.100.x (dernier : 32.0.100.5540). Nous ne savons pas faire correspondre ces deux numérotations.
- **Non vérifié** : la voie OpenVINO GenAI (modèles en IR OpenVINO, pas en GGUF). Sa page NPU officielle renvoyait 404 lors de la consultation (2026-11-12). Elle impliquerait de toute façon d'autres artefacts de modèle, donc une licence, un SHA-256 et un benchmark de fidélité à fournir.

**Conclusion** : le NPU n'est pas exploitable par Jot 1.5.1, ni pour le LLM (Qwen3.5 non supporté) ni pour l'ASR (texte seulement).

## 5. Prochaines pistes, par ordre de coût

1. **[gratuit]** Relever le matériel avec le § 1. Lancer `--list-devices` pour savoir si l'iGPU s'appelle « Arc » et serait donc déjà choisi.
2. **[gratuit]** Benchmark Vulkan iGPU contre CPU (§ 2) avec le runtime déjà présent.
3. **[téléchargement, sur autorisation]** Faire le même benchmark avec le runtime OpenVINO, en mode GPU.
4. **[changement de modèle, sur décision explicite]** Qwen3 4B sur NPU par OpenVINO pour la seule reformulation : artefact exact, licence, SHA-256, benchmark de fidélité FR/EN (négations, nombres, incertitudes). Option visible uniquement, jamais automatique.

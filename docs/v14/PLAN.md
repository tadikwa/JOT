# Jot 1.4.0 Implementation Plan

Goal: livrer une portable Windows depuis e6e502d, dictionnaire validé humainement, raccourcis personnalisés, lien Gemini et icônes.
Architecture: prolonger les stores et UI existants. Extraction déterministe locale et conservatrice, propositions persistées sans phrases privées. Les corrections.json restent la vérité de l’historique. Aucun modèle local ni appel cloud supplémentaire.
Tech Stack: Rust, GPUI, Win32, JSON, SQLite dérivé.
Spec: mission utilisateur du 11 novembre 2026.

## Contraintes
Profil portable jot-test-profile/profile exclusivement. Aucun push/tag/release ni migration réelle. Préserver HUD Win32. FR/EN. Données fictives. Branche codex/dictionary-v14.

## Tâches et interfaces
- [ ] Dictionnaire core : dictionary.rs, replacement.rs, nouveau dictionary/learning.rs si utile. Tests avant code : Proxmocks/Proxmox, Klod Code/Claude Code, Unicode, réécriture, ponctuation, conflits/cycles, persistence, migration, rejet/idempotence, suppression source, CSV, pipeline active. DictionaryStore exposera suggestions(), learn_from_corrections(&[Correction]), accept_suggestion(id), reject_suggestion(id), update_entry(id,term,wrong), set_enabled(id,bool), purge_orphan_suggestions(live_sources). Chaque mutation doit être durable ou signaler une erreur. Les noms exacts seront consignés dans PROGRESS avant UI.
- [ ] UI dictionnaire/historique : recherche, filtres manuelles/validées/en attente, édition en place via formulaire existant, accepter/refuser/activer/supprimer, scan explicite en arrière-plan des corrections historiques. Génération après enregistrement de correction. Mise à jour sans redémarrage. Tests intégration corrections/reindex/suppression.
- [ ] Raccourcis : hotkey.rs, settings.rs, UI settings, i18n si nécessaire, main routing. Migrer anciens hotkey_key; capturer/confirmer/annuler/reset. Tests de séquences et configuration figée pendant geste. Détecter échecs/conflicts. Lien clé officiel dans settings (et onboarding si écran clé), erreur navigateur honnête.
- [ ] Icônes : ressources build Release, helper fenêtres interactives partagé, ne pas toucher overlay. Vérifier ressources PE et WM_GETICON tailles. Tests/harnais ciblés.
- [ ] Intégration : version 1.4.0, cargo fmt/clippy/test/build, HUD binaire livré, parcours UI réel possible, isolation profil et processus, docs notice/validation, archive portable vide de données privées. Commits locaux.

## Review Focus
Échec disque: aucune confirmation trompeuse. JSON inconnu/corrompu: pas d’écrasement. Corrections reformulées: pas de règle agressive. Modificateurs relâchés hors ordre: pas de session bloquée. Fenêtre petite: actions accessibles.

## Vérifications
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release -p jot --locked
docs/hud/Validate-Hud.ps1 -Executable <portable>/jot.exe -ReportDirectory <nouveau dossier>

Exécution autonome explicitement demandée: aucune étape de validation de design supplémentaire.

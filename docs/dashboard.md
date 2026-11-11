# Statistiques de dictée

## Quelles dictées sont comptées

Une dictée réussie correspond à une session qui a produit au moins un mot et dont le statut final indique un texte inséré, copié, disponible dans l’historique ou récupéré. Jot compte le texte final affiché dans l’historique, avec le texte brut en secours si le texte nettoyé est absent. Le découpage Unicode compte les segments contenant des lettres ou des chiffres. Les séparateurs et les emoji isolés n’ajoutent aucun mot. Ce découpage suit les limites de mots Unicode et peut différer d’un découpage linguistique par dictionnaire.

Une reprise met à jour l’identifiant de la session d’origine et reste attribuée au jour de son enregistrement. Les échecs, annulations conservées et reprises en attente sont indiqués séparément. Leurs mots et leurs durées sont exclus des indicateurs de réussite. Les sessions en cours, les silences et les annulations courtes écartées par l’historique sont exclus des compteurs. Le détail décrit le dernier statut conservé, car l’historique ne conserve pas chaque tentative.

Une suppression retire la session des agrégats, du calendrier et des séries. Une future correction enregistrée par le même chemin de mise à jour remplacera le nombre de mots de la session. La conservation de l’audio n’affecte pas les durées déjà enregistrées dans les métadonnées. L’index SQLite reste reconstructible à partir de ces métadonnées.

## Vitesse et gain estimé

Soient **M** les mots des dictées réussies dont la durée audio est connue et strictement positive, **A** la somme de ces durées en secondes et **T** la vitesse de frappe de référence en mots par minute. Le réglage est dans **Paramètres → Général**. La valeur de départ de 40 mots/min est une hypothèse modifiable.

- **Vitesse de dictée** = 60 × M / A. Les dictées longues et courtes contribuent selon leur durée.
- **Temps économisé estimé en secondes** = 60 × M / T − A − P. **P** additionne les délais de traitement connus sur les mêmes dictées, depuis la fin de l’enregistrement jusqu’au statut final, insertion comprise le cas échéant. Pour un délai absent, y compris après récupération, seule la durée audio est déduite. L’interface affiche combien de dictées ont une durée et un délai connus.
- Si aucune durée audio n’est connue, les deux valeurs restent indisponibles. Les mots et les dictées restent comptabilisés. Un résultat négatif signifie que l’enregistrement et le traitement pris en compte dépassent le temps de frappe de référence. Le modèle suppose une vitesse de frappe constante. Il ne mesure ni la saisie réelle, ni la relecture, ni le temps passé à corriger ou à reprendre une dictée.

## Calendrier et périodes

Jot attribue chaque session à la date locale de son début. Les horodatages UTC restent dans SQLite. Windows convertit chaque instant dans le fuseau du système avec les règles de changement d’heure applicables. Changer de fuseau peut donc déplacer certaines sessions entre deux dates lors du prochain chargement. [Microsoft Learn, 22 février 2024](https://learn.microsoft.com/en-us/windows/win32/api/timezoneapi/nf-timezoneapi-systemtimetotzspecificlocaltimeex)

Les périodes glissantes incluent aujourd’hui. Pour « 1 an », la période commence le lendemain de la date anniversaire de l’an passé. Le 29 février, le 28 février sert d’anniversaire lorsque l’année précédente n’est pas bissextile. Les écarts de mots et de dictées comparent deux périodes adjacentes de même nombre de jours. « Depuis le début » couvre tout l’historique conservé jusqu’à aujourd’hui.

Le calendrier affiche les 26 dernières semaines, en incluant la semaine en cours. L’intensité correspond à 0, 1–49, 50–149, 150–349 ou 350 mots et plus. Un jour actif contient au moins une dictée réussie. Les jours actifs et la meilleure série couvrent tout l’historique. La série actuelle peut se terminer hier tant que la journée d’aujourd’hui est en cours. Les graphiques regroupent les jours en périodes plus larges lorsque la plage s’allonge. Les infobulles donnent les dates et les totaux du groupe.

Le tableau de bord lit une ligne agrégée par journée, via un index SQLite sans texte. Les changements d’historique déclenchent un nouvel instantané hors du fil d’interface. Les filtres réutilisent l’instantané. Un réveil au changement de date maintient les plages à jour pendant que la page reste ouverte.

//! Fidelity gate for the local rewrite: the facts a smoother text must not lose.
//!
//! The 4B model, decoding greedily, sometimes drops or changes a fact while the
//! text still looks clean, and the length/overlap checks of [`crate::validation`]
//! cannot see it. Seen on the real binary and bench (docs/v15/RAPPORT-V15.md):
//! « 192 point 168 point 1 point 1 » became 192.168.68.1, « je ne sais pas »
//! vanished, « deux câbles HDMI » became « des câbles HDMI ». A rejection makes
//! the caller insert the raw transcript, which is faithful by construction.
//! Reasons never carry dictated content: they are logged.

use regex::Regex;
use std::collections::BTreeSet;
use std::sync::LazyLock;

pub fn check(raw: &str, cleaned: &str) -> Result<(), &'static str> {
    let corrects = CORRECTION.is_match(raw);
    let raw_numbers = numbers(raw);
    // List markers are formatting the model was asked to add, not numbers.
    let said = numbers(&LIST_MARKER.replace_all(cleaned, ""));
    if !said.is_subset(&raw_numbers) {
        return Err("number_changed");
    }
    // Same values in another order: 192.168.1.50 is not 192.168.50.1.
    let raw_dotted = dotted(raw);
    if dotted(cleaned).iter().any(|seq| !raw_dotted.contains(seq)) {
        return Err("number_changed");
    }
    // A spoken « deux points » may be the colon (« C deux-points antislash ») or
    // a quantity: its 2 is allowed in the output, not required.
    let required = numbers(&COLON.replace_all(raw, " : "));
    // 0 and 1 are mostly articles (« un switch » → « le switch »).
    let kept = numbers(cleaned);
    if !corrects && required.iter().any(|n| *n > 1 && !kept.contains(n)) {
        return Err("number_lost");
    }
    if !units_added(raw, cleaned).is_empty() {
        return Err("unit_added");
    }
    if has(&HEDGE, raw) && !has(&HEDGE, cleaned) {
        return Err("hedge_lost");
    }
    if !corrects && has(&NEGATION, raw) && !has(&NEGATION, cleaned) {
        return Err("negation_lost");
    }
    if has(&RESTRICTIVE, raw) && !has(&RESTRICTION_KEPT, cleaned) {
        return Err("restriction_lost");
    }
    let qualified = RETRACTED_QUALIFIER
        .replace_all(&raw.replace('’', "'"), "")
        .into_owned();
    if has(&QUALIFIER, &qualified) && !has(&QUALIFIER, cleaned) {
        return Err("qualifier_lost");
    }
    Ok(())
}

/// The dictated words behind a rejection, for the opt-in local diagnostic only
/// (`crate::diagnostic`): unlike the reason, this quotes the dictation, so it
/// must never be logged.
pub fn explain(raw: &str, cleaned: &str, reason: &str) -> Option<String> {
    let list = |terms: Vec<String>| {
        let terms: BTreeSet<String> = terms.into_iter().collect();
        (!terms.is_empty()).then(|| {
            terms
                .iter()
                .map(|t| format!("« {t} »"))
                .collect::<Vec<_>>()
                .join(", ")
        })
    };
    let found = |pattern: &Regex, text: &str| {
        pattern
            .find_iter(&text.replace('’', "'"))
            .map(|m| m.as_str().to_lowercase())
            .collect::<Vec<_>>()
    };
    let values = |set: BTreeSet<u64>| set.iter().map(u64::to_string).collect::<Vec<_>>();
    let (what, terms) = match reason {
        "number_changed" => ("nombre absent du brut", {
            let raw_dotted = dotted(raw);
            let mut terms = values(&numbers(&LIST_MARKER.replace_all(cleaned, "")) - &numbers(raw));
            terms.extend(
                dotted(cleaned)
                    .into_iter()
                    .filter(|seq| !raw_dotted.contains(seq))
                    .map(|seq| seq.iter().map(u64::to_string).collect::<Vec<_>>().join(".")),
            );
            terms
        }),
        "number_lost" => (
            "nombre du brut absent de la reformulation",
            values(
                numbers(&COLON.replace_all(raw, " : "))
                    .into_iter()
                    .filter(|n| *n > 1 && !numbers(cleaned).contains(n))
                    .collect(),
            ),
        ),
        "unit_added" => ("unité absente du brut", units_added(raw, cleaned)),
        "hedge_lost" => ("incertitude disparue", found(&HEDGE, raw)),
        "negation_lost" => ("négation disparue", found(&NEGATION, raw)),
        "restriction_lost" => ("condition restrictive disparue", found(&RESTRICTIVE, raw)),
        "qualifier_lost" => (
            "degré disparu",
            found(
                &QUALIFIER,
                &RETRACTED_QUALIFIER.replace_all(&raw.replace('’', "'"), ""),
            ),
        ),
        _ => return None,
    };
    list(terms).map(|terms| format!("{what} : {terms}"))
}

/// Units written in the rewrite whose kind (minutes, %, Go…) the speaker never
/// said: « vingt de temps » became « vingt minutes » (1.6 field test T12).
/// Compared by kind, so « dix heures » → « 10 h » and « seize gigas » → « 16 Go »
/// pass. Returns the offending words as written in the rewrite.
/// shortcut: a kind already said elsewhere in the dictation hides a new one
/// on another number; pair each unit with its number if that shows up.
fn units_added(raw: &str, cleaned: &str) -> Vec<String> {
    UNITS
        .iter()
        .filter(|unit| !unit.is_match(raw))
        .filter_map(|unit| unit.find(cleaned))
        // An abbreviation match carries its digit and the next character.
        .map(|m| {
            let unit = m
                .as_str()
                .trim_start_matches(|c: char| c.is_ascii_digit() || c.is_whitespace());
            match unit.char_indices().last() {
                Some((i, c)) if unit.len() > 1 && !c.is_alphanumeric() && !"%€$".contains(c) => {
                    unit[..i].trim_end()
                }
                _ => unit,
            }
            .to_string()
        })
        .collect()
}

// Spelled-out words anywhere; symbols and abbreviations only right after a
// digit (« 5 s », not the « s » of a word). Checked on the 116 bench references
// and the 20 accepted rewrites of the 1.6 field test: no false alarm.
static UNITS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        (r"secondes?|seconds?", r"s|sec"),
        (r"minutes?", r"min|mn"),
        (r"heures?|hours?", r"h"),
        (r"jours?|days?", r"j"),
        (r"semaines?|weeks?", ""),
        (r"mois|months?", ""),
        // Not a bare « an »: it is also the English article.
        (r"ans|années?|years?", ""),
        (r"pour[ -]?cents?|pourcentage|percent", r"%"),
        (r"euros?", r"€|EUR"),
        (r"dollars?", r"\$|USD"),
        (
            r"(?:kilo|méga|mega|giga|téra|tera)(?:octets?|bytes?)|(?:méga|mega|giga|téra|tera)s?|octets?|bytes?",
            r"[KMGT]i?[oB]",
        ),
        (r"(?:kilo|méga|mega|giga)bits?", r"[KMG]bit/s|[KMG]bps|[KMG]b/s"),
        (r"(?:kilo|méga|mega|giga)?hertz", r"[kKMG]?Hz"),
        (r"(?:kilo|centi|milli)?mètres?|(?:kilo|centi|milli)?meters?", r"km|cm|mm|m"),
        // « un kilo deux » is 1,2 kg.
        (r"(?:kilo|milli)?grammes?|(?:kilo|milli)?grams?|kilos?", r"kg|mg|g"),
        (r"volts?|watts?|ampères?", r"[kKm]?[VWA]"),
        (r"pouces?|inch(?:es)?", ""),
        (r"millisecondes?|milliseconds?", r"ms"),
    ]
    .into_iter()
    .map(|(words, abbreviations)| {
        let mut pattern = format!(r"(?i:\b(?:{words})\b)");
        if !abbreviations.is_empty() {
            pattern.push_str(&format!(r"|\d\s?(?:{abbreviations})(?:[^\w]|$)"));
        }
        Regex::new(&pattern).unwrap()
    })
    .collect()
});

// A necessary condition (« uniquement si ») weakened to a plain « si » changes
// what is allowed (1.5.1 field test). A plain « si » is not checked.
static RESTRICTIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:uniquement|seulement|exclusivement) (?:si|s'|après|quand|lorsqu|une fois)|\bque si\b|\bqu'(?:à condition|après|une fois)|\bà condition (?:que|qu'|de|d')|\bsauf (?:si|s')|\bà moins (?:que|qu')|\bpourvu qu|\bonly (?:if|when|after|once)\b|\bunless\b|\bprovided (?:that )?\b|\bexcept (?:if|when)\b|\bas long as\b").unwrap()
});
// Any wording that still restricts, so a rephrased condition passes.
static RESTRICTION_KEPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:uniquement|seulement|exclusivement|que si|qu'à condition|qu'après|qu'une fois|à condition|sauf|à moins|pourvu|only|unless|provided|except|as long as)\b").unwrap()
});
// « pas forcément très puissant » is not « pas très puissant » (1.5.1 field test).
static QUALIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bpas (?:forcément|nécessairement|vraiment|tout à fait|complètement|entièrement|totalement)\b|\bnot (?:necessarily|really|quite|entirely|completely|fully)\b").unwrap()
});
// Right after « enfin » or « non » the same words take something back
// (« enfin pas vraiment le serveur, plutôt l'application »): they may go.
static RETRACTED_QUALIFIER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:enfin|non|pardon|ou plutôt|well|sorry|or rather)\b[ ,]*(?:ce n'est |ce n'était |c'est |c'était |it's |it was )?(?:pas|not) (?:forcément|nécessairement|vraiment|tout à fait|complètement|entièrement|totalement|necessarily|really|quite|entirely|completely|fully)\b").unwrap()
});

static COLON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bdeux[- ]points?\b").unwrap());
static LIST_MARKER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^\s*\d+[.)]\s").unwrap());
// A speaker who takes something back legitimately loses a number or a « pas ».
static CORRECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(non|enfin|plutôt|pardon|je veux dire|attends|oups|no|sorry|i mean|actually|rather|wait)\b").unwrap()
});
static HEDGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)peut-être|peut être|\bje pense\b|\bje crois\b|sais pas|pas sûr|pas certain|probablement|sans doute|il me semble|j'imagine|\bi think\b|\bmaybe\b|not sure|n't know|\bprobably\b|\bperhaps\b|\bi guess\b").unwrap()
});
static NEGATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(pas|jamais|aucun|aucune|rien|ni|not|never|no)\b|n't").unwrap()
});

fn has(pattern: &Regex, text: &str) -> bool {
    pattern.is_match(&text.replace('’', "'"))
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Unit,
    Tens,
    Hundred,
    Thousand,
}

fn word(w: &str) -> Option<(u64, Kind)> {
    let v = match w {
        "zéro" | "zero" => 0,
        "un" | "une" | "one" => 1,
        "deux" | "two" => 2,
        "trois" | "three" => 3,
        "quatre" | "four" => 4,
        "cinq" | "five" => 5,
        "six" => 6,
        "sept" | "seven" => 7,
        "huit" | "eight" => 8,
        "neuf" | "nine" => 9,
        "dix" | "ten" => 10,
        "onze" | "eleven" => 11,
        "douze" | "twelve" => 12,
        "treize" | "thirteen" => 13,
        "quatorze" | "fourteen" => 14,
        "quinze" | "fifteen" => 15,
        "seize" | "sixteen" => 16,
        "seventeen" => 17,
        "eighteen" => 18,
        "nineteen" => 19,
        "vingt" | "vingts" | "twenty" => return Some((20, Kind::Tens)),
        "trente" | "thirty" => return Some((30, Kind::Tens)),
        "quarante" | "forty" => return Some((40, Kind::Tens)),
        "cinquante" | "fifty" => return Some((50, Kind::Tens)),
        "soixante" | "sixty" => return Some((60, Kind::Tens)),
        "seventy" => return Some((70, Kind::Tens)),
        "eighty" => return Some((80, Kind::Tens)),
        "ninety" => return Some((90, Kind::Tens)),
        "cent" | "cents" | "hundred" => return Some((100, Kind::Hundred)),
        "mille" | "thousand" => return Some((1000, Kind::Thousand)),
        _ => return None,
    };
    Some((v, Kind::Unit))
}

fn ordinal(w: &str) -> Option<u64> {
    Some(match w {
        "premier" | "première" | "first" => 1,
        "second" | "seconde" | "deuxième" => 2,
        "troisième" => 3,
        "quatrième" => 4,
        "cinquième" => 5,
        "sixième" => 6,
        "septième" => 7,
        "huitième" => 8,
        "neuvième" => 9,
        "dixième" => 10,
        _ => return None,
    })
}

/// A spoken number being read, word by word.
#[derive(Default)]
struct Spoken {
    total: u64,
    current: u64,
    last: Option<(u64, Kind)>,
}

impl Spoken {
    fn flush(&mut self, found: &mut BTreeSet<u64>) {
        if self.last.is_some() {
            found.insert(self.total + self.current);
        }
        *self = Self::default();
    }

    fn push(&mut self, value: u64, kind: Kind, hyphenated: bool, found: &mut BTreeSet<u64>) {
        let joins = match (kind, self.last) {
            (_, None) => true,
            (Kind::Unit, Some((_, Kind::Tens | Kind::Hundred | Kind::Thousand))) => true,
            // dix-sept, soixante-dix-neuf
            (Kind::Unit, Some((10, Kind::Unit))) => hyphenated && (7..=9).contains(&value),
            // quatre-vingt(s)
            (Kind::Tens, Some((4, Kind::Unit))) => value == 20,
            (Kind::Tens, Some((_, Kind::Hundred | Kind::Thousand))) => true,
            // deux mille cent = 2100
            (Kind::Hundred, Some((_, Kind::Unit | Kind::Tens | Kind::Thousand))) => true,
            (Kind::Thousand, Some(_)) => true,
            _ => false,
        };
        if !joins {
            self.flush(found);
        }
        match kind {
            Kind::Tens if matches!(self.last, Some((4, Kind::Unit))) => self.current += 76,
            Kind::Hundred => self.current = self.current.max(1) * 100,
            Kind::Thousand => {
                self.total += self.current.max(1) * 1000;
                self.current = 0;
            }
            _ => self.current += value,
        }
        self.last = Some((value, kind));
    }
}

/// Every number in the text, digits or spoken French/English words
/// (« cent quatre-vingt-douze » = 192), as a set of values.
pub fn numbers(text: &str) -> BTreeSet<u64> {
    static THOUSANDS: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\d{1,3})[ \u{a0}\u{202f}](\d{3})\b").unwrap());
    // « vingt pour cent » is 20 %, not 20 and 100. Only right after a number:
    // « une table pour cent personnes » keeps its 100.
    static PERCENT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(\d|\b(?:un|une|deux|trois|quatre|cinq|six|sept|huit|neuf|dix|onze|douze|treize|quatorze|quinze|seize|vingts?|trente|quarante|cinquante|soixante|cents?|mille))[ \u{a0}-]+pour[ -]?cents?\b").unwrap()
    });
    let lowered = THOUSANDS
        .replace_all(&text.to_lowercase(), "$1$2")
        .into_owned();
    let lowered = PERCENT.replace_all(&lowered, "$1 %");
    let mut found = BTreeSet::new();
    let mut spoken = Spoken::default();
    // Hyphens join a compound (« quatre-vingt-douze »); any other separator,
    // a comma or « point », ends the number.
    for token in lowered.split(|c: char| !(c.is_alphanumeric() || c == '-')) {
        for (i, part) in token.split('-').enumerate() {
            let digits = part
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(part.len());
            if digits > 0 {
                spoken.flush(&mut found);
                found.extend(part[..digits].parse::<u64>().ok());
            } else if part == "et" && matches!(spoken.last, Some((_, Kind::Tens))) {
                continue;
            } else if let Some(n) = ordinal(part) {
                spoken.flush(&mut found);
                found.insert(n);
            } else if let Some((value, kind)) = word(part) {
                spoken.push(value, kind, i > 0, &mut found);
            } else {
                spoken.flush(&mut found);
            }
        }
    }
    spoken.flush(&mut found);
    found
}

/// The single value `words` spell (« cent soixante-huit », « 168 »), if they
/// spell exactly one.
fn one_value(words: &[&str]) -> Option<u64> {
    if let [w] = words
        && w.bytes().all(|b| b.is_ascii_digit())
    {
        return w.parse().ok();
    }
    let mut spoken = Spoken::default();
    let mut found = BTreeSet::new();
    for w in words {
        for (i, part) in w.split('-').enumerate() {
            if part == "et" && matches!(spoken.last, Some((_, Kind::Tens))) {
                continue;
            }
            let (value, kind) = word(part)?;
            spoken.push(value, kind, i > 0, &mut found);
            if !found.is_empty() {
                return None;
            }
        }
    }
    spoken.flush(&mut found);
    found.pop_first()
}

/// Numbers joined by « . » or a spoken « point », in order: versions (1.7.1)
/// and addresses (192.168.1.50), whose order [`numbers`] cannot see. A « point »
/// without a number on each side (« qu'un point à discuter ») joins nothing.
/// shortcut: « , » and « virgule » are read as decimals, not joins; add them if
/// a version dictated that way shows up.
fn dotted(text: &str) -> Vec<Vec<u64>> {
    static DOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d)\.(\d)").unwrap());
    static GLUED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(\p{L})(\d)|(\d)(\p{L})").unwrap());
    let mut text = text.to_lowercase();
    // Twice: the matches of « 1.7.1 » overlap.
    for _ in 0..2 {
        text = DOT.replace_all(&text, "$1 point $2").into_owned();
    }
    let text = GLUED.replace_all(&text, "$1$3 $2$4");
    let tokens: Vec<&str> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|t| !t.is_empty())
        .collect();
    let segments: Vec<&[&str]> = tokens.split(|t| *t == "point").collect();
    let tail = |s: &[&str]| (0..s.len()).find_map(|i| one_value(&s[i..]));
    let head = |s: &[&str]| (1..=s.len()).rev().find_map(|j| one_value(&s[..j]));
    let mut found = Vec::new();
    let mut k = 0;
    while k + 1 < segments.len() {
        let (Some(first), Some(second)) = (tail(segments[k]), head(segments[k + 1])) else {
            k += 1;
            continue;
        };
        let mut seq = vec![first, second];
        k += 1;
        while k + 1 < segments.len() && one_value(segments[k]).is_some() {
            let Some(next) = head(segments[k + 1]) else {
                break;
            };
            seq.push(next);
            k += 1;
        }
        found.push(seq);
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_numbers_keep_their_order() {
        let none = Vec::<Vec<u64>>::new();
        assert_eq!(
            dotted(
                "Mon adresse est cent quatre-vingt-douze point cent soixante-huit point un point cinquante."
            ),
            vec![vec![192, 168, 1, 50]]
        );
        assert_eq!(
            dotted("Mon adresse est 192.168.1.50."),
            vec![vec![192, 168, 1, 50]]
        );
        assert_eq!(
            dotted("Jot un point sept point un, puis la v2.0 et Python 3 point 12."),
            vec![vec![1, 7, 1], vec![2, 0], vec![3, 12]]
        );
        assert_eq!(dotted("Il n'y a qu'un point à discuter."), none);
        assert_eq!(
            dotted("Voici mon premier point et mon deuxième point. Un point important."),
            none
        );
        assert_eq!(dotted("1. Branche le câble.\n2. Ouvre."), none);
        assert_eq!(
            dotted("la un point six et deux deux point trois"),
            vec![vec![1, 6], vec![2, 3]]
        );
    }

    #[test]
    fn spoken_numbers_written_in_digits_pass_and_altered_ones_do_not() {
        let ip = "Mon adresse est cent quatre-vingt-douze point cent soixante-huit point un point cinquante.";
        let ok = [
            (
                "J'utilise la version un point sept de Jot.",
                "J'utilise la version 1.7 de Jot.",
            ),
            (
                "L'ancienne vidéo parle de Jot un point cinq.",
                "L'ancienne vidéo parle de Jot 1.5.",
            ),
            (
                "Il faudra publier Jot un point sept point un.",
                "Il faudra publier Jot 1.7.1.",
            ),
            (
                "Nous sommes en deux mille vingt-six.",
                "Nous sommes en 2026.",
            ),
            (
                "Le prix est de vingt dollars.",
                "Le prix est de 20 dollars.",
            ),
            ("Le prix est de vingt dollars.", "Le prix est de 20 $."),
            (
                "Le serveur a seize gigas de RAM et quatre processeurs virtuels.",
                "Le serveur a 16 Go de RAM et 4 processeurs virtuels.",
            ),
            (ip, "Mon adresse est 192.168.1.50."),
            (
                "Ce n'est pas la version un point sept, non, c'est la version un point cinq.",
                "Ce n'est pas la version 1.7, c'est la version 1.5.",
            ),
            (
                "Ce n'est pas la version un point sept, non, c'est la version un point cinq.",
                "C'est la version 1.5.",
            ),
            (
                "Il n'y a qu'un point à discuter.",
                "Il n'y a qu'un point à discuter.",
            ),
            (
                "Voici mon premier point et mon deuxième point.",
                "Voici mon premier point et mon deuxième point.",
            ),
        ];
        for (raw, cleaned) in ok {
            assert_eq!(check(raw, cleaned), Ok(()), "{cleaned}");
        }
        let bad = [
            (
                "J'utilise la version un point sept de Jot.",
                "J'utilise la version 1.8 de Jot.",
            ),
            (
                "J'utilise la version un point sept de Jot.",
                "J'utilise la version 7.1 de Jot.",
            ),
            (
                "Le prix est de vingt dollars.",
                "Le prix est de 200 dollars.",
            ),
            (ip, "Mon adresse est 192.168.50.1."),
            (
                "Mon adresse est 192.168.1.50.",
                "Mon adresse est 192.168.50.1.",
            ),
            (
                "Il faudra publier Jot un point sept point un.",
                "Il faudra publier Jot 1.1.7.",
            ),
        ];
        for (raw, cleaned) in bad {
            assert_eq!(check(raw, cleaned), Err("number_changed"), "{cleaned}");
        }
        assert_eq!(
            explain(
                "Mon adresse est 192.168.1.50.",
                "Mon adresse est 192.168.50.1.",
                "number_changed"
            )
            .as_deref(),
            Some("nombre absent du brut : « 192.168.50.1 »")
        );
    }

    fn set(values: &[u64]) -> BTreeSet<u64> {
        values.iter().copied().collect()
    }

    #[test]
    fn spoken_numbers_parse_as_one_value() {
        assert_eq!(
            numbers("cent quatre-vingt-douze point cent soixante-huit point un point un"),
            set(&[192, 168, 1])
        );
        assert_eq!(numbers("192.168.68.1"), set(&[192, 168, 68, 1]));
        assert_eq!(
            numbers("deux mille vingt-six, soixante et onze, dix-sept"),
            set(&[2026, 71, 17])
        );
        assert_eq!(
            numbers("trois cents euros et quatre-vingts pages"),
            set(&[300, 80])
        );
        assert_eq!(numbers("un, deux, trois"), set(&[1, 2, 3]));
        assert_eq!(numbers("1 500 personnes à 10 h"), set(&[1500, 10]));
        assert_eq!(numbers("le deuxième devis, la 3e page"), set(&[2, 3]));
        assert_eq!(numbers("twenty one cats, a hundred dogs"), set(&[21, 100]));
    }

    #[test]
    fn a_changed_or_lost_fact_is_rejected() {
        let ip = "Pour configurer le routeur, d'abord tu branches le câble sur le port quatre, ensuite tu ouvres l'adresse cent quatre-vingt-douze point cent soixante-huit point un point un, puis tu changes le mot de passe par défaut.";
        let bad = "Pour configurer le routeur :\n1. Branche le câble sur le port quatre.\n2. Ouvre l'adresse 192.168.68.1.\n3. Change le mot de passe par défaut.";
        assert_eq!(check(ip, bad), Err("number_changed"));
        assert_eq!(
            check(ip, &bad.replace("192.168.68.1", "192.168.1.1")),
            Ok(())
        );

        let hdmi = "À acheter : des câbles HDMI, un switch huit ports, des câbles HDMI, voilà, deux câbles HDMI en tout, et une multiprise.";
        assert_eq!(
            check(
                hdmi,
                "À acheter :\n- Des câbles HDMI.\n- Un switch huit ports.\n- Une multiprise."
            ),
            Err("number_lost")
        );
        assert_eq!(
            check(
                hdmi,
                "À acheter :\n- Deux câbles HDMI.\n- Un switch 8 ports.\n- Une multiprise."
            ),
            Ok(())
        );

        let unsure = "Soit on fait la mise à jour d'abord, soit on change le disque d'abord, je ne sais pas, on verra demain avec Sami.";
        assert_eq!(
            check(
                unsure,
                "Soit on fait la mise à jour d'abord, soit on change le disque d'abord : on verra demain avec Sami."
            ),
            Err("hedge_lost")
        );
        assert_eq!(
            check(
                "Ne supprime pas le dossier backup.",
                "Supprime le dossier backup."
            ),
            Err("negation_lost")
        );
        // 1.5.1 field tests: a necessary condition and a degree qualifier.
        let only_if = "Si les sauvegardes sont terminées et uniquement si les sauvegardes sont terminées, on pourra intervenir demain.";
        assert_eq!(
            check(
                only_if,
                "Si les sauvegardes sont terminées, on pourra intervenir demain."
            ),
            Err("restriction_lost")
        );
        assert_eq!(
            check(
                "Ship it on Friday unless Paul objects.",
                "Ship it on Friday, and ask Paul."
            ),
            Err("restriction_lost")
        );
        assert_eq!(
            check(
                "Mon ordinateur n'est pas forcément très puissant.",
                "Mon ordinateur n'est pas très puissant."
            ),
            Err("qualifier_lost")
        );
    }

    #[test]
    fn rephrased_conditions_and_retractions_pass() {
        let only_if = "Si les sauvegardes sont terminées et uniquement si les sauvegardes sont terminées, on pourra intervenir demain.";
        for kept in [
            "Uniquement si les sauvegardes sont terminées, on pourra intervenir demain.",
            "On ne pourra intervenir demain que si les sauvegardes sont terminées.",
            "Si, et seulement si, les sauvegardes sont terminées, on pourra intervenir demain.",
            "On pourra intervenir demain, à condition que les sauvegardes soient terminées.",
        ] {
            assert_eq!(check(only_if, kept), Ok(()), "{kept}");
        }
        // A plain « si » is not a restriction to keep word for word.
        assert_eq!(
            check(
                "Euh, si tu as le temps, regarde le rapport.",
                "Si tu as le temps, regarde le rapport."
            ),
            Ok(())
        );
        assert_eq!(
            check(
                "Le portable n'est pas forcément plus rapide.",
                "Le portable n'est pas nécessairement plus rapide."
            ),
            Ok(())
        );
        // « enfin pas vraiment le serveur » is the retraction itself: it goes.
        assert_eq!(
            check(
                "Le serveur était lent, enfin pas vraiment le serveur, c'était plutôt l'application qui répondait lentement.",
                "L'application répondait lentement."
            ),
            Ok(())
        );
        assert_eq!(
            check(
                "The server was slow, well, not really the server, I mean the app.",
                "The app was slow."
            ),
            Ok(())
        );
    }

    #[test]
    fn an_invented_unit_is_rejected_and_a_rewritten_one_passes() {
        // 1.6 field test T12: the ASR dropped a word, the model filled it in.
        let t12 = "On a peut-être gagné, on a peut-être gagné à peu près vingt de temps, sans pouvoir encore confirmer ce chiffre.";
        let invented =
            "On a peut-être gagné environ vingt minutes, sans pouvoir encore confirmer ce chiffre.";
        assert_eq!(check(t12, invented), Err("unit_added"));
        assert_eq!(
            explain(t12, invented, "unit_added").as_deref(),
            Some("unité absente du brut : « minutes »")
        );
        assert_eq!(
            check(t12, "On a peut-être gagné à peu près 20 %."),
            Err("unit_added")
        );
        for (raw, cleaned) in [
            ("Je viens à dix heures.", "Je viens à 10 h."),
            ("Il a seize gigas de RAM.", "Il a 16 Go de RAM."),
            ("Le repas coûte dix-huit euros.", "Le repas coûte 18 €."),
            ("Il pèse un kilo deux.", "Il pèse 1,2 kg."),
            ("Attends cinq secondes.", "Attends 5 s."),
            // Not units: a word starting with one, and English « an ».
            (
                "La machine a 4 vCPU et un 5G.",
                "La machine a 4 vCPU et un 5G.",
            ),
            ("It is an option.", "It's an option."),
        ] {
            assert_eq!(check(raw, cleaned), Ok(()), "{cleaned}");
        }
        // Formerly refused: « pour cent » and « mille cent » read as an extra 100.
        for (raw, cleaned) in [
            (
                "Il y a vingt pour cent de remise.",
                "Il y a 20 % de remise.",
            ),
            ("Ça coûte deux mille cent euros.", "Ça coûte 2 100 €."),
            ("Ça coûte deux mille cent euros.", "Ça coûte 2100 €."),
            ("On vise quatre-vingt-dix pour cent.", "On vise 90 %."),
            ("Une remise de cinq pour-cent.", "Une remise de 5 %."),
            ("Cent pour cent sûr.", "100 % sûr."),
            ("Mille cent personnes.", "1 100 personnes."),
        ] {
            assert_eq!(check(raw, cleaned), Ok(()), "{cleaned}");
        }
        // The same words still guard the facts.
        for (raw, cleaned, reason) in [
            (
                "Il y a vingt pour cent de remise.",
                "Il y a 25 % de remise.",
                "number_changed",
            ),
            (
                "Il y a vingt pour cent de remise.",
                "Il y a de la remise.",
                "number_lost",
            ),
            (
                "Ça coûte deux mille cent euros.",
                "Ça coûte 2 000 €.",
                "number_changed",
            ),
            (
                "Ça coûte deux mille cent euros.",
                "Ça coûte 2 000 € et 100 €.",
                "number_changed",
            ),
            (
                "Une table pour cent personnes.",
                "Une table pour des personnes.",
                "number_lost",
            ),
            ("Deux mille, cent fois.", "2100 fois.", "number_changed"),
        ] {
            assert_eq!(check(raw, cleaned), Err(reason), "{cleaned}");
        }
    }

    #[test]
    fn explain_quotes_what_the_check_missed() {
        assert_eq!(
            explain(
                "Je pense que ça marche, peut-être.",
                "Ça marche.",
                "hedge_lost"
            )
            .as_deref(),
            Some("incertitude disparue : « je pense », « peut-être »")
        );
        assert_eq!(
            explain(
                "Ce sera uniquement après la validation.",
                "Ce sera après la validation.",
                "restriction_lost"
            )
            .as_deref(),
            Some("condition restrictive disparue : « uniquement après »")
        );
        assert_eq!(
            explain("Il faut 3 postes.", "Il faut 5 postes.", "number_changed").as_deref(),
            Some("nombre absent du brut : « 5 »")
        );
        assert_eq!(explain("a", "b", "length ratio"), None);
    }

    #[test]
    fn legitimate_cleanups_pass() {
        // A spoken path: « deux-points » is punctuation (seen on the bench).
        assert_eq!(
            check(
                "Lance cargo test puis git status dans C deux-points backslash projet.",
                "Lance cargo test, puis git status dans C:\\projet."
            ),
            Ok(())
        );
        assert_eq!(
            check(
                "Copie le fichier C deux points antislash temp antislash config point json.",
                "Copie le fichier C:\\temp\\config.json."
            ),
            Ok(())
        );
        // « deux points » may also be a real quantity in the output (bench h2-long-retour).
        let two = "Il y a deux trucs qui m'ont gêné, le bouton et la recherche.";
        assert_eq!(
            check(two, "Deux points m'ont gêné : le bouton et la recherche."),
            Ok(())
        );
        assert_eq!(
            check(two, "Le bouton et la recherche m'ont gêné."),
            Err("number_lost")
        );
        // Self-correction drops the abandoned number and the retracted item.
        assert_eq!(
            check(
                "Je viens à neuf heures. Enfin non, je viens à dix heures.",
                "Je viens à 10 h."
            ),
            Ok(())
        );
        assert_eq!(
            check(
                "Il faut des chips, du fromage, ah non pas de fromage, et des serviettes.",
                "Il faut :\n- des chips ;\n- des serviettes."
            ),
            Ok(())
        );
        // Numbered steps add list markers the speaker never said.
        assert_eq!(
            check(
                "D'abord tu télécharges l'image, ensuite tu la copies, et enfin tu redémarres.",
                "1. Télécharge l'image.\n2. Copie-la.\n3. Redémarre."
            ),
            Ok(())
        );
        // Articles and spoken numbers rewritten as digits.
        assert_eq!(
            check(
                "Il me faut quatre gigas et un câble.",
                "Il me faut 4 Go et le câble."
            ),
            Ok(())
        );
        assert_eq!(
            check(
                "Je pense que c'est bon, enfin je crois.",
                "Je pense que c'est bon."
            ),
            Ok(())
        );
    }
}

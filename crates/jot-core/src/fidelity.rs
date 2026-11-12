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
    // A spoken « deux points » may be the colon (« C deux-points antislash ») or
    // a quantity: its 2 is allowed in the output, not required.
    let required = numbers(&COLON.replace_all(raw, " : "));
    // 0 and 1 are mostly articles (« un switch » → « le switch »).
    let kept = numbers(cleaned);
    if !corrects && required.iter().any(|n| *n > 1 && !kept.contains(n)) {
        return Err("number_lost");
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
            (Kind::Hundred, Some((_, Kind::Unit | Kind::Tens))) => true,
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
    let lowered = THOUSANDS
        .replace_all(&text.to_lowercase(), "$1$2")
        .into_owned();
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

#[cfg(test)]
mod tests {
    use super::*;

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

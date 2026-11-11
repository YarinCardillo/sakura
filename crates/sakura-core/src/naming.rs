//! The name of a lavoro, made from what the person requested: nobody is asked to invent one. A short slug
//! of the first meaningful words of the sentence, in `box`'s own rule for names (lowercase letters and
//! digits joined by single hyphens, at most forty characters), unique among the lavori that exist.

use crate::validate;

/// The most words a name is made of.
const MAX_WORDS: usize = 4;
/// The most characters before a number is added to tell it from another lavoro.
const MAX_LENGTH: usize = 32;
/// The name when the sentence has no letter or digit at all.
const FALLBACK: &str = "task";
/// Words that say nothing of the work, in the two languages of the window.
const FILLER: [&str; 89] = [
    "il", "lo", "la", "i", "gli", "le", "un", "uno", "una", "di", "a", "da", "in", "con", "su",
    "per", "tra", "fra", "e", "ed", "o", "che", "del", "dello", "della", "dei", "degli", "delle",
    "al", "allo", "alla", "ai", "agli", "alle", "dal", "dalla", "nel", "nello", "nella", "nei",
    "sul", "sulla", "mi", "ti", "ci", "si", "non", "come", "vorrei", "voglio", "puoi", "potresti",
    "favore", "questo", "questa", "the", "an", "of", "to", "on", "for", "with", "and", "or",
    "that", "this", "is", "are", "be", "please", "can", "could", "would", "want", "you", "it",
    "its", "me", "my", "we", "our", "so", "at", "by", "as", "from", "into", "should", "need",
];

/// A name for the lavoro requested with `sentence`, that none of `taken` has.
pub fn name_for(sentence: &str, taken: &[&str]) -> String {
    let words = words(sentence);
    let meaningful: Vec<&String> = words.iter().filter(|word| is_meaningful(word)).collect();
    let chosen: Vec<&String> = match meaningful.is_empty() {
        true => words.iter().collect(),
        false => meaningful,
    };
    let stem = joined(&chosen);
    unique(&stem, taken)
}

/// The words of `sentence`: runs of letters and digits, lowercase, accents taken off.
fn words(sentence: &str) -> Vec<String> {
    let plain: String = sentence.chars().flat_map(plain_letters).collect();
    plain
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// A character as lowercase ASCII letters, a space for what is none.
fn plain_letters(c: char) -> Vec<char> {
    let lowered = c.to_lowercase().next().unwrap_or(c);
    let plain = match lowered {
        'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => "a",
        'è' | 'é' | 'ê' | 'ë' => "e",
        'ì' | 'í' | 'î' | 'ï' => "i",
        'ò' | 'ó' | 'ô' | 'ö' | 'õ' => "o",
        'ù' | 'ú' | 'û' | 'ü' => "u",
        'ç' => "c",
        'ñ' => "n",
        'ß' => "ss",
        c if c.is_ascii_alphanumeric() => return vec![c],
        _ => " ",
    };
    plain.chars().collect()
}

fn is_meaningful(word: &str) -> bool {
    word.len() > 1 && !FILLER.contains(&word)
}

/// The first words that fit the length, joined by hyphens; one word too long is cut.
fn joined(words: &[&String]) -> String {
    let mut name = String::new();
    for word in words.iter().take(MAX_WORDS) {
        let needed = name.len() + usize::from(!name.is_empty()) + word.len();
        if needed > MAX_LENGTH {
            break;
        }
        if !name.is_empty() {
            name.push('-');
        }
        name.push_str(word);
    }
    match (name.is_empty(), words.first()) {
        (false, _) => name,
        (true, Some(first)) => first.chars().take(MAX_LENGTH).collect(),
        (true, None) => FALLBACK.to_string(),
    }
}

/// `stem`, or `stem` with the first number from 2 up that no lavoro has.
fn unique(stem: &str, taken: &[&str]) -> String {
    if !taken.contains(&stem) {
        return stem.to_string();
    }
    let numbered = (2..).map(|number| format!("{stem}-{number}"));
    let mut free = numbered.filter(|name| !taken.contains(&name.as_str()));
    let name = free.next().unwrap_or_else(|| stem.to_string());
    debug_assert!(validate::task(&name).is_ok());
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_the_first_meaningful_words_of_the_sentence() {
        assert_eq!(
            name_for("Create a file hello.txt with a line", &[]),
            "create-file-hello-txt"
        );
        assert_eq!(
            name_for("Fix the login page: it's broken on Safari", &[]),
            "fix-login-page-broken"
        );
        assert_eq!(name_for("Add the dark mode", &[]), "add-dark-mode");
        assert_eq!(
            name_for("  Please, could you rename the API?  ", &[]),
            "rename-api"
        );
    }

    #[test]
    fn a_name_that_a_lavoro_has_gets_the_first_free_number() {
        let taken = ["create-file-hello-txt"];
        assert_eq!(
            name_for("create a file hello.txt", &taken),
            "create-file-hello-txt-2"
        );
        let taken = ["create-file-hello-txt", "create-file-hello-txt-2"];
        assert_eq!(
            name_for("create a file hello.txt", &taken),
            "create-file-hello-txt-3"
        );
    }

    #[test]
    fn a_sentence_with_nothing_meaningful_still_gives_a_name() {
        assert_eq!(name_for("the an", &[]), "the-an");
        assert_eq!(name_for("???", &[]), "task");
        assert_eq!(name_for("", &["task"]), "task-2");
        assert_eq!(name_for("🙂 ✨", &[]), "task");
    }

    #[test]
    fn a_long_word_or_a_long_sentence_stays_within_the_length() {
        let word = "x".repeat(60);
        assert_eq!(name_for(&word, &[]).len(), MAX_LENGTH);
        let name = name_for(
            "reorganization administration documentation internationalization",
            &[],
        );
        assert_eq!(name, "reorganization-administration");
    }

    #[test]
    fn every_name_is_one_box_takes() {
        let sentences = [
            "Create a file hello.txt with a line",
            "--force -rf /",
            "UPPER case AND Ünïcödé",
            "a",
            "1 2 3",
            "tab\there\nnew line",
            &"word ".repeat(40),
        ];
        let taken = ["a", "1-2-3", "task"];
        for sentence in sentences {
            let name = name_for(sentence, &taken);
            assert!(validate::task(&name).is_ok(), "{sentence:?} gave {name:?}");
            assert!(!taken.contains(&name.as_str()));
        }
    }
}

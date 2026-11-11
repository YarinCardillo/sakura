//! A strict lexer for one simple shell command. It returns the words a POSIX shell would hand to the
//! program only when the text needs no expansion, no substitution, no escape and no operator to be
//! understood; anything else is `None`, "cannot say", and the command goes to the person. It never tries to
//! work out what an expansion would produce.
//!
//! Accepted: words of plain characters separated by spaces or tabs, single-quoted text, double-quoted text.
//! Refused anywhere, quoted or not: `$`, a backtick, a backslash, a control character (a newline is one).
//! Refused outside quotes: every character that is not a letter, a digit or one of [`PLAIN_PUNCTUATION`],
//! which leaves out the control operators (`;`, `&`, `|`, parentheses, braces), the redirections (`<`,
//! `>`), the globs (`*`, `?`, brackets), `!` and `#`; and a `~` where a shell would expand it (at the start
//! of a word, or after `=` or `:`), and a `=` at the start of a word (zsh expands `=name` to a program's
//! path). An unclosed quote is refused.

/// Punctuation that is literal in a shell word outside quotes. `~` is handled apart.
const PLAIN_PUNCTUATION: [char; 10] = ['_', '-', '.', '/', ':', ',', '+', '@', '%', '='];
/// Characters that make a command unreadable wherever they stand, also inside single quotes: the policy
/// does not approve a command it would have to reason about.
const NEVER_PLAIN: [char; 3] = ['$', '`', '\\'];
/// Characters after which a shell may expand a `~`.
const TILDE_PREFIXES: [char; 2] = ['=', ':'];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

/// The words of `text` as a shell would pass them, or `None` when the text is anything but one plain
/// simple command.
pub(super) fn words(text: &str) -> Option<Vec<String>> {
    let is_refused = |c: char| NEVER_PLAIN.contains(&c) || (c.is_control() && c != '\t');
    if text.chars().any(is_refused) {
        return None;
    }
    let mut lexer = Lexer::default();
    for c in text.chars() {
        lexer.read(c)?;
    }
    lexer.finish()
}

#[derive(Default)]
struct Lexer {
    words: Vec<String>,
    /// The word being read; `Some("")` after an empty pair of quotes.
    word: Option<String>,
    quote: Option<Quote>,
}

impl Lexer {
    fn read(&mut self, c: char) -> Option<()> {
        match self.quote.unwrap_or(Quote::None) {
            Quote::None => self.read_plain(c),
            Quote::Single if c == '\'' => self.close_quote(),
            Quote::Double if c == '"' => self.close_quote(),
            // History expansion reaches inside double quotes.
            Quote::Double if c == '!' => None,
            Quote::Single | Quote::Double => self.push(c),
        }
    }

    fn read_plain(&mut self, c: char) -> Option<()> {
        match c {
            ' ' | '\t' => {
                self.words.extend(self.word.take());
                Some(())
            }
            '\'' => self.open_quote(Quote::Single),
            '"' => self.open_quote(Quote::Double),
            '~' if self.is_tilde_expanded() => None,
            '=' if self.word.is_none() => None,
            '~' => self.push(c),
            c if c.is_alphanumeric() || PLAIN_PUNCTUATION.contains(&c) => self.push(c),
            _ => None,
        }
    }

    /// Whether a `~` here is one a shell would expand.
    fn is_tilde_expanded(&self) -> bool {
        let before = self.word.as_ref().and_then(|word| word.chars().last());
        before.is_none_or(|c| TILDE_PREFIXES.contains(&c))
    }

    fn open_quote(&mut self, quote: Quote) -> Option<()> {
        self.quote = Some(quote);
        self.word.get_or_insert_with(String::new);
        Some(())
    }

    fn close_quote(&mut self) -> Option<()> {
        self.quote = None;
        Some(())
    }

    fn push(&mut self, c: char) -> Option<()> {
        self.word.get_or_insert_with(String::new).push(c);
        Some(())
    }

    fn finish(mut self) -> Option<Vec<String>> {
        if self.quote.is_some() {
            return None;
        }
        self.words.extend(self.word.take());
        Some(self.words)
    }
}

#[cfg(test)]
mod tests {
    use super::words;

    fn lexed(text: &str) -> Option<Vec<&'static str>> {
        let words = words(text)?;
        Some(words.into_iter().map(|word| &*word.leak()).collect())
    }

    #[test]
    fn words_split_on_spaces_and_tabs() {
        assert_eq!(
            lexed("cargo  test\tfolders "),
            Some(vec!["cargo", "test", "folders"])
        );
        assert_eq!(lexed(""), Some(vec![]));
        assert_eq!(lexed("   "), Some(vec![]));
    }

    #[test]
    fn words_quotes_join_their_neighbours_and_keep_spaces() {
        assert_eq!(
            lexed("git commit -m \"a b; c\""),
            Some(vec!["git", "commit", "-m", "a b; c"])
        );
        assert_eq!(lexed("cu\"r\"l x"), Some(vec!["curl", "x"]));
        assert_eq!(lexed("c''argo 'te'st"), Some(vec!["cargo", "test"]));
        assert_eq!(lexed("echo '' \"\""), Some(vec!["echo", "", ""]));
        assert_eq!(
            lexed("echo 'it is \"quoted\"'"),
            Some(vec!["echo", "it is \"quoted\""])
        );
    }

    #[test]
    fn words_plain_punctuation_and_a_tilde_inside_a_word_are_literal() {
        let text = "git diff HEAD~1 --stat=80,40 a/b.rs user@host:path 50%";
        let expected = vec![
            "git",
            "diff",
            "HEAD~1",
            "--stat=80,40",
            "a/b.rs",
            "user@host:path",
            "50%",
        ];
        assert_eq!(lexed(text), Some(expected));
    }

    #[test]
    fn words_anything_a_shell_would_expand_or_run_is_refused() {
        for text in [
            "a $b", "a ${b}", "a $(b)", "a `b`", "a '$b'", "a \"$b\"", "a \\b", "a 'b\\c'", "a;b",
            "a ; b", "a&&b", "a & b", "a|b", "a>b", "a <b", "a 2>&1", "(a)", "{ a; }", "a{b,c}",
            "a *", "a ?", "a [b]", "a !b", "a \"!b\"", "a #b", "~", "~/a", "a ~b", "a b=~/c",
            "a b:~/c", "a\nb", "a\rb", "a\u{0}b", "a\u{1b}b", "a\u{7f}b", "a 'b", "a \"b", "a ^b",
            "a =b", "=a",
        ] {
            assert_eq!(lexed(text), None, "{text:?}");
        }
    }
}

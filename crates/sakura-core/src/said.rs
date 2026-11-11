//! What the core has to tell the person, without words. The core holds no sentence in any language: it
//! names what is to be said by a key of the window's locale files and gives the values the sentence is
//! said with. The window finds the words for the language in force, writes numbers its way and picks the
//! singular or the plural from the value named `count`.

use serde::Serialize;

/// A key of the window's locale files: the name of a word or a sentence, never the words themselves. It
/// is its own type so that a key cannot be put on screen in place of what it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct Key(pub &'static str);

/// The name of the value that decides between singular and plural.
pub const COUNT: &str = "count";

/// One thing to say: the key of its sentence and the values it is said with, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Said {
    pub key: Key,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<(&'static str, Value)>,
}

/// A value of a sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Value {
    /// Text that is data: a name, a path, a line `box` wrote. Shown as it is.
    Text(String),
    /// A whole number.
    Count(u64),
    /// A number with decimals, written with a dot ("10.4"): the window writes it its language's way.
    Decimal(String),
    /// Another thing to say, inside this one.
    Said(Box<Said>),
}

impl Said {
    /// What `key` says, with no value.
    pub fn new(key: &'static str) -> Self {
        Self {
            key: Key(key),
            args: Vec::new(),
        }
    }

    /// With the text `value` under `name`.
    pub fn text(self, name: &'static str, value: impl Into<String>) -> Self {
        self.with(name, Value::Text(value.into()))
    }

    /// With the whole number `value` under `name`.
    pub fn number(self, name: &'static str, value: u64) -> Self {
        self.with(name, Value::Count(value))
    }

    /// With `value` as the count that decides between singular and plural.
    pub fn count(self, value: u64) -> Self {
        self.number(COUNT, value)
    }

    /// With `value` under `name`, written with `places` decimals.
    pub fn decimal(self, name: &'static str, value: f64, places: usize) -> Self {
        self.with(name, Value::Decimal(format!("{value:.places$}")))
    }

    /// With `value` under `name` to one decimal, and none when it is a whole number: `3.2`, `4`.
    pub fn tenths(self, name: &'static str, value: f64) -> Self {
        let tenths = (value * 10.0).round() / 10.0;
        let places = usize::from(tenths.fract() != 0.0);
        self.decimal(name, tenths, places)
    }

    /// With another thing to say under `name`.
    pub fn said(self, name: &'static str, value: Said) -> Self {
        self.with(name, Value::Said(Box::new(value)))
    }

    /// The value under `name`, if there is one.
    pub fn arg(&self, name: &str) -> Option<&Value> {
        let found = self.args.iter().find(|(arg, _)| *arg == name);
        found.map(|(_, value)| value)
    }

    fn with(mut self, name: &'static str, value: Value) -> Self {
        self.args.push((name, value));
        self
    }
}

impl From<Key> for Said {
    fn from(key: Key) -> Self {
        Said::new(key.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thing_to_say_is_its_key_and_its_values_by_name() {
        let said = Said::new("take.reason.dirty").count(2).text("file", "a.rs");
        assert_eq!(said.key, Key("take.reason.dirty"));
        assert_eq!(said.arg(COUNT), Some(&Value::Count(2)));
        assert_eq!(said.arg("file"), Some(&Value::Text("a.rs".into())));
        assert_eq!(said.arg("other"), None);
    }

    #[test]
    fn a_decimal_is_kept_with_a_dot_and_its_places() {
        let said = Said::new("machine.load").decimal("load", 10.44, 1);
        assert_eq!(said.arg("load"), Some(&Value::Decimal("10.4".into())));
        let whole = Said::new("machine.load").tenths("load", 4.02);
        assert_eq!(whole.arg("load"), Some(&Value::Decimal("4".into())));
        let part = Said::new("machine.load").tenths("load", 3.24);
        assert_eq!(part.arg("load"), Some(&Value::Decimal("3.2".into())));
    }
}

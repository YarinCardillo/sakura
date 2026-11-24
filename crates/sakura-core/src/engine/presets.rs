//! The presets the engine offers, as `sakura preset list --json` prints them: a list of presets, each with its
//! name, its name in words, its description and whether it is the one chosen.

use super::snapshot::capitalised;
use serde::Deserialize;

/// One preset that can be chosen.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PresetChoice {
    pub name: String,
    /// The name in words (`Lean`), as the engine says it.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Whether it is the preset chosen for new lanes.
    #[serde(default)]
    pub chosen: bool,
}

impl PresetChoice {
    /// A preset with the engine's label, for a list the engine did not give: the name with a capital.
    pub fn named(name: &str, label: Option<&str>) -> PresetChoice {
        let label = label.map_or_else(|| capitalised(name), str::to_string);
        PresetChoice {
            name: name.to_string(),
            label,
            description: String::new(),
            chosen: false,
        }
    }

    /// The preset in words: the engine's label, else its name with a capital.
    pub fn words(&self) -> String {
        match self.label.is_empty() {
            true => capitalised(&self.name),
            false => self.label.clone(),
        }
    }
}

/// The presets that can be chosen, in the engine's order.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(transparent)]
pub struct PresetList {
    pub presets: Vec<PresetChoice>,
}

impl PresetList {
    /// Reads the listing the engine prints.
    pub fn parse(text: &[u8]) -> Result<PresetList, serde_json::Error> {
        serde_json::from_slice(text)
    }

    /// The names, in the engine's order.
    pub fn names(&self) -> Vec<String> {
        self.presets
            .iter()
            .map(|preset| preset.name.clone())
            .collect()
    }

    /// The preset marked as chosen, when one is.
    pub fn chosen(&self) -> Option<&str> {
        let chosen = self.presets.iter().find(|preset| preset.chosen);
        chosen.map(|preset| preset.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::PresetList;

    const LISTING: &str = r#"[
      {"name": "solo", "label": "Solo", "description": "One agent.", "chosen": false},
      {"name": "night-shift", "label": "Night shift", "description": "At night.", "chosen": true},
      {"name": "lean", "description": "Fewer.", "chosen": false}]"#;

    #[test]
    fn a_listing_gives_the_names_in_order_and_the_chosen_one() {
        let list = PresetList::parse(LISTING.as_bytes()).expect("a listing");
        assert_eq!(list.names(), ["solo", "night-shift", "lean"]);
        assert_eq!(list.chosen(), Some("night-shift"));
    }

    #[test]
    fn a_preset_is_said_by_the_engine_s_label_else_by_its_name_with_a_capital() {
        let list = PresetList::parse(LISTING.as_bytes()).expect("a listing");
        let words: Vec<String> = list.presets.iter().map(|preset| preset.words()).collect();
        assert_eq!(words, ["Solo", "Night shift", "Lean"]);
    }

    #[tokio::test]
    async fn the_cli_reads_the_listing_the_engine_prints() {
        use crate::engine::cli::EngineCli;
        let dir = tempfile::tempdir().expect("a folder");
        let script = dir.path().join("sakura");
        let body = "#!/bin/sh\necho \"$*\" > \"$(dirname \"$0\")/args\"\nprintf '[{\"name\": \"solo\", \"label\": \"Solo\", \"description\": \"One.\", \"chosen\": false}, {\"name\": \"lean\", \"label\": \"Lean\", \"description\": \"Less.\", \"chosen\": true}]'\n";
        crate::test_support::write_script(&script, body);
        let list = EngineCli::at(script).presets().await.expect("the presets");
        assert_eq!((list.names().len(), list.chosen()), (2, Some("lean")));
        let args = std::fs::read_to_string(dir.path().join("args")).expect("the arguments");
        assert_eq!(args.trim(), "preset list --json");
    }

    #[test]
    fn a_listing_with_none_chosen_has_no_chosen_preset() {
        let list = PresetList::parse(br#"[{"name": "solo"}]"#).expect("a listing");
        assert_eq!(list.chosen(), None);
    }
}

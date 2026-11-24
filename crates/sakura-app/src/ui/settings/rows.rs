//! A section of Settings as rows: what the search reads of each and the kit's item that draws it.
//! The rows are grouped; a group may have a title and words under its last row. The kit draws each
//! row as a group of its own, so a hairline ends every setting; a group's title stands over its
//! first row. A part that is not one setting draws its own lines, if any.

use crate::theme::Theme;
use gpui::{AnyElement, App, Axis, IntoElement, SharedString, Styled as _, Window};
use gpui_component::setting::{SettingField, SettingGroup, SettingItem};
use std::rc::Rc;

/// What draws words under a group.
type Footer = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// One row: a setting with its title, the sentence that says what it changes and its control, or a
/// part of the section that is not one setting (a list, a form, the brand's page).
pub struct Row {
    /// Everything the search reads: the title, the sentence and the words a person may search by.
    words: Vec<SharedString>,
    item: SettingItem,
    /// Whether a hairline ends the row: a setting's does.
    is_ruled: bool,
}

impl Row {
    /// A setting: its `title`, the one sentence that says what changes with it, the `keywords` a
    /// person may search it by besides those, and its control.
    pub fn setting<E: IntoElement + 'static>(
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
        keywords: Vec<SharedString>,
        control: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        let (title, description) = (title.into(), description.into());
        let field = SettingField::render(move |_, window, cx| control(window, cx));
        let item = SettingItem::new(title.clone(), field)
            .description(description.clone())
            .keywords(keywords.clone());
        let mut words = vec![title, description];
        words.extend(keywords);
        Self {
            words,
            item,
            is_ruled: true,
        }
    }

    /// A part of a section that is not one setting, found by `keywords`.
    pub fn custom<E: IntoElement + 'static>(
        keywords: Vec<SharedString>,
        render: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        let item =
            SettingItem::render(move |_, window, cx| render(window, cx)).keywords(keywords.clone());
        Self {
            words: keywords,
            item,
            is_ruled: false,
        }
    }

    /// The setting's control under its title and sentence, the width of the page, instead of at
    /// their right: for a field that takes a paragraph.
    pub fn stacked(mut self) -> Self {
        self.item = self.item.layout(Axis::Vertical);
        self
    }

    /// Whether the row is a setting, which the kit draws with its title and sentence.
    #[cfg(test)]
    pub fn is_setting(&self) -> bool {
        matches!(self.item, SettingItem::Item { .. })
    }

    /// The sentence the kit draws under the setting's title.
    #[cfg(test)]
    pub fn description(&self) -> Option<&SharedString> {
        use gpui_component::text::Text;
        match &self.item {
            SettingItem::Item {
                description: Some(Text::String(description)),
                ..
            } => Some(description),
            _ => None,
        }
    }

    /// Whether a search for `query` finds the row: every row, for no query; otherwise a row with a
    /// word that holds the query, whatever the case of the letters.
    pub fn matches(&self, query: &str) -> bool {
        self.words.iter().any(|word| holds(word, query)) || query.trim().is_empty()
    }
}

/// Whether `word` holds `query`, whatever the case of the letters and the spaces around the query.
pub fn holds(word: &str, query: &str) -> bool {
    word.to_lowercase().contains(&query.trim().to_lowercase())
}

/// Rows that belong together, under a title when they have one.
pub struct Group {
    title: Option<SharedString>,
    pub rows: Vec<Row>,
    footer: Option<Footer>,
}

impl Group {
    pub fn new(title: Option<SharedString>, rows: Vec<Row>) -> Self {
        Self {
            title,
            rows,
            footer: None,
        }
    }

    /// Words under the group's last row, shown and found with it.
    pub fn footer<E: IntoElement>(
        mut self,
        footer: impl Fn(&mut Window, &mut App) -> E + 'static,
    ) -> Self {
        self.footer = Some(Rc::new(move |window, cx| {
            footer(window, cx).into_any_element()
        }));
        self
    }
}

/// The groups of a section with only the rows a search for `query` finds; a group whose title holds
/// the query keeps every row, and a group none of whose rows it finds is left out.
pub fn matching(groups: Vec<Group>, query: &str) -> Vec<Group> {
    groups
        .into_iter()
        .filter_map(|mut group| {
            let is_title_found = group
                .title
                .as_ref()
                .is_some_and(|title| holds(title, query));
            if !is_title_found {
                group.rows.retain(|row| row.matches(query));
            }
            (!group.rows.is_empty()).then_some(group)
        })
        .collect()
}

/// The kit's groups for `groups`: one for each row, a setting's ended by a hairline, the title over
/// the first row of its group and the footer under the last.
pub fn kit_groups(groups: Vec<Group>, cx: &App) -> Vec<SettingGroup> {
    let hairline = Theme::palette(cx).border;
    let mut kit = Vec::new();
    for group in groups {
        let last = group.rows.len().saturating_sub(1);
        for (nth, row) in group.rows.into_iter().enumerate() {
            let mut one = SettingGroup::new();
            if row.is_ruled {
                one = one.border_b_1().border_color(hairline);
            }
            one = one.item(row.item);
            if let Some(title) = group.title.clone().filter(|_| nth == 0) {
                one = one.title(title);
            }
            if let Some(footer) = group.footer.clone().filter(|_| nth == last) {
                one = one.footer(move |window, cx| footer(window, cx));
            }
            kit.push(one);
        }
    }
    kit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(title: &str, description: &str, keywords: &[&str]) -> Row {
        let keywords = keywords.iter().map(|word| (*word).into()).collect();
        Row::setting(
            title.to_string(),
            description.to_string(),
            keywords,
            |_, _| gpui::div(),
        )
    }

    #[test]
    fn a_row_is_found_by_its_title_its_sentence_or_a_keyword_whatever_the_case() {
        let theme = setting(
            "Theme",
            "Light or dark colours.",
            &["appearance", "dark mode"],
        );
        for query in ["", "theme", "THE", "colours", "dark mode", " Appear "] {
            assert!(theme.matches(query), "{query:?}");
        }
        assert!(!theme.matches("language"));
        let list = Row::custom(vec!["projects".into()], |_, _| gpui::div());
        assert!(list.matches("project") && !list.matches("theme"));
        assert!(!list.is_setting(), "a part that is not one setting");
        assert_eq!(
            theme.description().map(|text| text.as_ref()),
            Some("Light or dark colours.")
        );
    }

    #[test]
    fn a_search_keeps_the_rows_it_finds_and_leaves_out_a_group_with_none() {
        let groups = vec![
            Group::new(
                None,
                vec![
                    setting("Theme", "Colours.", &[]),
                    setting("Language", "Words.", &[]),
                ],
            ),
            Group::new(
                Some("Terminal".into()),
                vec![setting("Font", "Letters.", &[])],
            ),
        ];
        let found = matching(groups, "theme");
        assert_eq!(found.len(), 1, "the terminal's group has no match");
        assert_eq!(found[0].rows.len(), 1, "the language's row is left out");
        assert!(matching(vec![Group::new(None, vec![])], "").is_empty());
    }

    #[test]
    fn a_group_found_by_its_title_keeps_every_row() {
        let terminal = Group::new(
            Some("Terminal".into()),
            vec![
                setting("Font", "Letters.", &[]),
                setting("Size", "Points.", &[]),
            ],
        );
        let found = matching(vec![terminal], "termin");
        assert_eq!(found[0].rows.len(), 2);
    }
}

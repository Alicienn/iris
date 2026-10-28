//! The rules window.
//!
//! This module owns the translation between what the engine knows and what the screen
//! shows: a rule becomes a sentence. The engine should not have to care about wording,
//! and the screen should not have to understand a rule.

use iris_rules::Rule;
use iris_store::{Store, StoredRule};
use iris_types::Result;

/// A rule as the screen shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleView {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    /// "when …, then …" in plain words.
    pub summary: String,
    pub applied: u64,
}

/// Reads the rules and describes them.
///
/// A rule whose definition no longer parses is still listed, with the parse error as
/// its summary. Hiding it would leave the user with a rule they cannot see, cannot
/// disable, and cannot delete.
pub fn rule_views(store: &Store) -> Result<Vec<RuleView>> {
    let mut views = Vec::new();

    for row in store.rules()? {
        let summary = match serde_json::from_str::<Rule>(&row.definition) {
            Ok(rule) => describe(&rule),
            Err(e) => format!("Unreadable rule: {e}"),
        };

        views.push(RuleView {
            applied: store.rule_application_count(&row.id).unwrap_or(0),
            id: row.id,
            name: row.name,
            enabled: row.enabled,
            summary,
        });
    }

    Ok(views)
}

/// Turns a rule into one readable sentence.
///
/// This is the whole reason the screen is usable: a rule you cannot read at a glance
/// is a rule you stop trusting, and a rule you stop trusting you switch off.
pub fn describe(rule: &Rule) -> String {
    let conditions: Vec<String> = rule.conditions.iter().map(|c| c.describe()).collect();
    let actions: Vec<String> = rule.actions.iter().map(|a| a.describe()).collect();

    let joiner = match rule.match_mode {
        iris_rules::Match::All => " and ",
        iris_rules::Match::Any => " or ",
    };

    match (conditions.is_empty(), actions.is_empty()) {
        (true, true) => "Does nothing".into(),
        // A rule with no condition matches everything. Saying so plainly is the only
        // honest wording: "always" is what it will do.
        (true, false) => format!("Always: {}", actions.join(", ")),
        (false, true) => format!("When {}, but does nothing", conditions.join(joiner)),
        (false, false) => format!(
            "When {}, {}{}",
            conditions.join(joiner),
            actions.join(" and "),
            if rule.stop_on_match {
                ", then stop"
            } else {
                ""
            }
        ),
    }
}

/// Builds a rule from the two fields the screen offers.
///
/// Deliberately one shape — "mail from here is already dealt with" — because it is the
/// rule people actually want first, and an editor nobody opens is worse than a single
/// field they use. Richer rules are written in the file and read back here.
pub fn quick_rule(
    name: &str,
    sender: &str,
    position: u32,
) -> std::result::Result<StoredRule, String> {
    let name = name.trim();
    let sender = sender.trim().to_lowercase();

    if sender.is_empty() {
        return Err("Enter a sender or a domain.".into());
    }
    let name = if name.is_empty() {
        format!("Mail from {sender}")
    } else {
        name.to_string()
    };

    // A value with an @ is an address; anything else is treated as a domain, which is
    // what someone typing "newsletters.example" means.
    let condition = if sender.contains('@') {
        iris_rules::Condition::FromContains(sender.clone())
    } else {
        iris_rules::Condition::FromDomain(sender.clone())
    };

    let rule =
        Rule::new(slug(&sender), name.clone())
            .when(condition)
            .then(iris_rules::Action::SetState(
                iris_types::WorkflowState::Done,
            ));

    Ok(StoredRule {
        id: rule.id.clone(),
        name,
        enabled: true,
        position,
        definition: serde_json::to_string(&rule).map_err(|e| e.to_string())?,
    })
}

/// A stable identifier derived from the sender.
///
/// Same sender, same identifier: adding the same rule twice replaces it rather than
/// stacking two copies that both fire.
fn slug(value: &str) -> String {
    let cleaned: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    let trimmed = cleaned.trim_matches('-').to_string();
    if trimmed.is_empty() {
        "rule".into()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_rules::{Action, Condition};
    use iris_types::{Timestamp, WorkflowState};

    #[test]
    fn a_rule_reads_as_a_sentence() {
        // A rule you cannot read at a glance is a rule you switch off.
        let rule = Rule::new("r", "Newsletters")
            .when(Condition::FromDomain("news.example".into()))
            .then(Action::SetState(WorkflowState::Done));

        let text = describe(&rule);
        assert!(text.starts_with("When "), "got: {text}");
        assert!(text.contains("news.example"));
    }

    #[test]
    fn several_conditions_are_joined_by_the_match_mode() {
        let all = Rule::new("r", "R")
            .when(Condition::FromDomain("a.example".into()))
            .when(Condition::SubjectContains("invoice".into()))
            .then(Action::Flag);
        assert!(describe(&all).contains(" and "));

        let any = Rule::new("r", "R")
            .when(Condition::FromDomain("a.example".into()))
            .when(Condition::SubjectContains("invoice".into()))
            .then(Action::Flag)
            .matching_any();
        assert!(describe(&any).contains(" or "));
    }

    #[test]
    fn a_rule_with_no_condition_says_it_matches_everything() {
        // "Always" is what it will do; any softer wording would be a lie.
        let rule = Rule::new("r", "R").then(Action::Flag);
        assert!(
            describe(&rule).starts_with("Always:"),
            "{}",
            describe(&rule)
        );
    }

    #[test]
    fn a_rule_that_stops_the_others_says_so() {
        let rule = Rule::new("r", "R")
            .when(Condition::FromDomain("a.example".into()))
            .then(Action::Flag)
            .stopping();
        assert!(describe(&rule).ends_with(", then stop"));
    }

    #[test]
    fn an_empty_rule_is_described_rather_than_hidden() {
        let rule = Rule::new("r", "R");
        assert_eq!(describe(&rule), "Does nothing");
    }

    #[test]
    fn an_unreadable_rule_is_still_listed() {
        // Hiding it would leave a rule the user cannot see, disable or delete.
        let store = Store::in_memory().unwrap();
        store
            .upsert_rule(&StoredRule {
                id: "broken".into(),
                name: "Broken".into(),
                enabled: true,
                position: 0,
                definition: "{ not json".into(),
            })
            .unwrap();

        let views = rule_views(&store).unwrap();
        assert_eq!(views.len(), 1);
        assert!(views[0].summary.starts_with("Unreadable rule"));
    }

    #[test]
    fn the_view_carries_how_often_a_rule_acted() {
        let store = Store::in_memory().unwrap();
        let rule = quick_rule("Newsletters", "news.example", 0).unwrap();
        store.upsert_rule(&rule).unwrap();

        let views = rule_views(&store).unwrap();
        assert_eq!(views[0].applied, 0);
        assert_eq!(views[0].name, "Newsletters");
    }

    #[test]
    fn a_quick_rule_needs_a_sender() {
        assert!(quick_rule("Name", "   ", 0).is_err());
    }

    #[test]
    fn a_quick_rule_names_itself_when_the_name_is_left_blank() {
        let rule = quick_rule("", "news.example", 0).unwrap();
        assert_eq!(rule.name, "Mail from news.example");
    }

    #[test]
    fn an_address_and_a_domain_produce_different_conditions() {
        // Someone typing "newsletters.example" means the domain.
        let address = quick_rule("", "bob@news.example", 0).unwrap();
        let domain = quick_rule("", "news.example", 0).unwrap();

        assert!(address.definition.contains("from_contains"));
        assert!(domain.definition.contains("from_domain"));
    }

    #[test]
    fn the_same_sender_gives_the_same_identifier() {
        // Adding the same rule twice replaces it rather than stacking two copies that
        // both fire.
        let a = quick_rule("One", "news.example", 0).unwrap();
        let b = quick_rule("Another", "News.Example", 1).unwrap();
        assert_eq!(a.id, b.id);
    }

    #[test]
    fn a_quick_rule_round_trips_through_the_engine() {
        let stored = quick_rule("Newsletters", "news.example", 0).unwrap();
        let rule: Rule = serde_json::from_str(&stored.definition).unwrap();

        let facts = iris_rules::Facts {
            from_addr: "weekly@news.example".into(),
            ..Default::default()
        };
        assert!(rule.matches(&facts, Timestamp::EPOCH));
    }
}

//! The video call an event is held on: the link, and which service it is.
//!
//! A link set by hand is kept beside the event (`event_links`). Otherwise one is looked
//! for in the event's place and description, where Google, Microsoft, Zoom and Webex
//! put theirs in the invitations they send.

/// The services people are invited to, by what their links contain.
const SERVICES: [(&str, &str); 7] = [
    ("meet.google.com", "meet"),
    ("teams.microsoft.com", "teams"),
    ("teams.live.com", "teams"),
    ("zoom.us", "zoom"),
    ("webex.com", "webex"),
    ("whereby.com", "call"),
    ("meet.jit.si", "call"),
];

/// "meet", "teams", "zoom", "webex", or "call" for any other link.
pub fn kind(url: &str) -> &'static str {
    let bas = url.to_lowercase();
    let hote = bas
        .split("://")
        .nth(1)
        .unwrap_or(&bas)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("");
    SERVICES
        .iter()
        .find(|(domaine, _)| hote == *domaine || hote.ends_with(&format!(".{domaine}")))
        .map(|(_, genre)| *genre)
        .unwrap_or("call")
}

/// What the join button says: "Join on Meet", "Join on Teams"…
pub fn label(url: &str) -> &'static str {
    match kind(url) {
        "meet" => "Join on Meet",
        "teams" => "Join on Teams",
        "zoom" => "Join on Zoom",
        "webex" => "Join on Webex",
        _ => "Join the call",
    }
}

/// The first link of a known video service in `text`, if any.
pub fn find(text: &str) -> Option<String> {
    let mut reste = text;
    while let Some(i) = reste.find("https://") {
        let depuis = &reste[i..];
        let fin = depuis
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | ')' | ']'))
            .unwrap_or(depuis.len());
        let lien = depuis[..fin].trim_end_matches(['.', ',', ';']);
        if kind(lien) != "call" {
            return Some(lien.to_string());
        }
        reste = &depuis[fin.max(1)..];
    }
    None
}

/// The link an event is held on: the one set by hand, else one found in its place or
/// description.
pub fn of_event(set: Option<&String>, location: &str, description: &str) -> Option<String> {
    set.filter(|l| !l.trim().is_empty())
        .cloned()
        .or_else(|| find(location))
        .or_else(|| find(description))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_are_told_by_their_host() {
        assert_eq!(kind("https://meet.google.com/abc-defg-hij"), "meet");
        assert_eq!(
            kind("https://teams.microsoft.com/l/meetup-join/19%3a"),
            "teams"
        );
        assert_eq!(kind("https://example.zoom.us/j/123?pwd=x"), "zoom");
        assert_eq!(kind("https://company.webex.com/meet/pat"), "webex");
        assert_eq!(kind("https://example.com/room"), "call");
        // A name that only contains one is not it.
        assert_eq!(kind("https://notzoom.us.example.com/j/1"), "call");
    }

    #[test]
    fn a_link_is_found_in_what_an_invitation_says() {
        let description = "Join: <https://meet.google.com/abc-defg-hij>. See you.\n\
                           Agenda: https://example.com/doc";
        assert_eq!(
            find(description).as_deref(),
            Some("https://meet.google.com/abc-defg-hij")
        );
        assert_eq!(find("Room 12, https://example.com/map"), None);
        assert_eq!(
            of_event(
                None,
                "Microsoft Teams Meeting",
                "https://example.zoom.us/j/1,"
            )
            .as_deref(),
            Some("https://example.zoom.us/j/1")
        );
        let manuel = "https://example.com/our-room".to_string();
        assert_eq!(
            of_event(Some(&manuel), "", "https://meet.google.com/x").as_deref(),
            Some("https://example.com/our-room"),
            "the one set by hand first"
        );
    }
}

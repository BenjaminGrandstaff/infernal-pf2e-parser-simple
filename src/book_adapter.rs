//! Goal: convert a raw book-extracted PF2e page chunk (as produced by an
//! external PDF-to-text tool, not part of this service) into the exact
//! `Type:`/`Label:` block grammar `parser.rs` already expects -- never
//! widen `parser.rs`'s own grammar to understand book formatting
//! directly. This module knows about book conventions (bracketed
//! action-cost annotations, label words with no colon); `parser.rs`
//! still knows nothing about the book, only about the labeled-block
//! grammar it already had.
//!
//! ## Known limitations (v1: actions/reactions only)
//!
//! - only an entry heading carrying a bracketed action-cost annotation
//!   (`[one-action]`, `[two-actions]`, `[three-actions]`,
//!   `[free-action]`, `[reaction]`) is recognized as a new entry; a
//!   sidebar or explanatory heading with no such annotation (e.g.
//!   "STRIKE STATISTICS") is not a boundary and is folded into the
//!   preceding entry's `Effect`, verbatim;
//! - only the book's `Requirements` and `Trigger` labels (no colon,
//!   value running on directly after the label word, ending at the
//!   first `. `) are recognized; degree-of-success text ("Critical
//!   Success", "Success") is not modeled and falls into `Effect`
//!   verbatim, same as any other unrecognized label;
//! - `parser.rs`'s `Requirements`/`Trigger` fields are comma-separated
//!   lists (multiple distinct clauses); a book `Requirements` sentence
//!   that merely uses commas as ordinary punctuation (see
//!   `converts_four_real_actions_off_one_page_into_parseable_blocks`'s
//!   "Take Cover" case) gets fragmented into several list items by
//!   `parser.rs`'s own `split_list`, not reassembled here -- this
//!   module hands `parser.rs` prose, not a real list, and does not
//!   paper over the mismatch;
//! - Feat and Condition book layouts (boxed headers, glossary entries)
//!   are out of scope for this pass -- see this repository's
//!   `ORC-NOTICE.md` for the separate, still-open question of
//!   filtering Reserved Material out of whatever this produces.

const ACTION_COST_MARKERS: &[(&str, &str)] = &[
    ("[one-action]", "Action"),
    ("[two-actions]", "Action"),
    ("[three-actions]", "Action"),
    ("[free-action]", "Free Action"),
    ("[reaction]", "Reaction"),
];

struct RawEntry {
    name: String,
    rule_type_label: &'static str,
    body_lines: Vec<String>,
}

/// Converts one page chunk's raw extracted `text` into zero or more
/// `parser::parse_candidates`-ready blocks, joined by `---`.
pub fn convert_page(text: &str) -> String {
    split_entries(text)
        .iter()
        .map(render_block)
        .collect::<Vec<_>>()
        .join("\n---\n")
}

fn split_entries(text: &str) -> Vec<RawEntry> {
    let mut entries = Vec::new();
    let mut current: Option<RawEntry> = None;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some((name, rule_type_label)) = detect_heading(line) {
            entries.extend(current.take());
            current = Some(RawEntry {
                name,
                rule_type_label,
                body_lines: Vec::new(),
            });
            continue;
        }
        if let Some(entry) = current.as_mut() {
            entry.body_lines.push(line.to_owned());
        }
    }
    entries.extend(current.take());
    entries
}

fn detect_heading(line: &str) -> Option<(String, &'static str)> {
    for (marker, rule_type_label) in ACTION_COST_MARKERS {
        if let Some(prefix) = line.strip_suffix(marker) {
            let name = prefix.trim();
            if !name.is_empty() && is_shouty(name) {
                return Some((title_case(name), rule_type_label));
            }
        }
    }
    None
}

fn is_shouty(text: &str) -> bool {
    text.chars().any(char::is_alphabetic)
        && text
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(char::is_uppercase)
}

fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn render_block(entry: &RawEntry) -> String {
    let paragraphs = group_paragraphs(&entry.body_lines);

    let mut traits = Vec::new();
    let mut trigger = None;
    let mut requirements = None;
    let mut effect_parts = Vec::new();

    for (index, paragraph) in paragraphs.iter().enumerate() {
        if index == 0 && is_shouty(paragraph) && paragraph.split_whitespace().count() <= 2 {
            traits.push(paragraph.to_lowercase());
            continue;
        }
        if let Some((value, leftover)) = extract_book_label(paragraph, "Requirements") {
            requirements = Some(value);
            effect_parts.extend(leftover);
            continue;
        }
        if let Some((value, leftover)) = extract_book_label(paragraph, "Trigger") {
            trigger = Some(value);
            effect_parts.extend(leftover);
            continue;
        }
        effect_parts.push(paragraph.clone());
    }

    let mut lines = vec![
        entry.rule_type_label.to_owned(),
        format!("Name: {}", entry.name),
    ];
    if !traits.is_empty() {
        lines.push(format!("Traits: {}", traits.join(", ")));
    }
    if let Some(trigger) = trigger {
        lines.push(format!("Trigger: {trigger}"));
    }
    if let Some(requirements) = requirements {
        lines.push(format!("Requirements: {requirements}"));
    }
    if !effect_parts.is_empty() {
        lines.push(format!("Effect: {}", effect_parts.join(" ")));
    }
    lines.join("\n")
}

/// Groups body lines into paragraphs on blank-line boundaries. The PDF
/// extraction preserves the book's line wrapping, not its sentence or
/// label structure, so a `Requirements`/`Trigger` clause and the effect
/// prose that follows it commonly land in the very same paragraph --
/// see `extract_book_label`, which splits within a paragraph rather
/// than assuming a label ever gets one of its own.
fn group_paragraphs(lines: &[String]) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = Vec::new();
    for line in lines {
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current = Vec::new();
            }
        } else {
            current.push(line.clone());
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs
}

/// If `paragraph` opens with `label` immediately followed by its value
/// (the book's convention -- no colon), returns the value up to and
/// including the first sentence terminator, plus whatever text in the
/// same paragraph follows it (the effect prose that the book runs on
/// without a paragraph break).
fn extract_book_label(paragraph: &str, label: &str) -> Option<(String, Option<String>)> {
    let rest = paragraph.strip_prefix(label)?.strip_prefix(' ')?;
    match rest.find(". ") {
        Some(index) => {
            let (value, remainder) = rest.split_at(index + 1);
            let remainder = remainder.trim();
            Some((
                value.trim().to_owned(),
                (!remainder.is_empty()).then(|| remainder.to_owned()),
            ))
        }
        None => Some((rest.trim().to_owned(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::convert_page;
    use crate::domain::RuleType;
    use crate::parser::parse_candidates;

    /// Verbatim extraction of Player Core page 419 (Pathfinder Second
    /// Edition Core Rulebook), from the chunker tool's own output --
    /// the real driving case for this module, not a synthetic fixture.
    const PLAYER_CORE_PAGE_419: &str = "\
Player Core

STAND [one-action]
MOVE

You stand up from being prone.

STEP [one-action]
MOVE

Requirements Your Speed is at least 10 feet.
You carefully move 5 feet. Unlike most types of movement,
Stepping doesn't trigger reactions, such as Reactive Strike,
that can be triggered by move actions or upon leaving or
entering a square.
You can't Step into difficult terrain (page 423), and you
can't Step using a Speed other than your land Speed.

STRIDE [one-action]
MOVE

You move up to your Speed (page 420).

TAKE COVER [one-action]
Requirements You are benefiting from cover, are near a
feature that allows you to take cover, or are prone.
You press yourself against a wall or duck behind an
obstacle to take better advantage of cover (page 424).
";

    #[test]
    fn converts_four_real_actions_off_one_page_into_parseable_blocks() {
        let converted = convert_page(PLAYER_CORE_PAGE_419);
        let candidates = parse_candidates(&converted);

        assert_eq!(candidates.len(), 4, "converted:\n{converted}");

        assert_eq!(candidates[0].rule_type, RuleType::Action);
        assert_eq!(candidates[0].name.as_deref(), Some("Stand"));
        assert_eq!(candidates[0].traits, vec!["move".to_owned()]);
        assert_eq!(
            candidates[0].effect.as_deref(),
            Some("You stand up from being prone.")
        );

        assert_eq!(candidates[1].name.as_deref(), Some("Step"));
        assert_eq!(
            candidates[1].requirements,
            vec!["Your Speed is at least 10 feet.".to_owned()]
        );
        assert!(
            candidates[1]
                .effect
                .as_deref()
                .unwrap()
                .starts_with("You carefully move 5 feet.")
        );

        assert_eq!(candidates[2].name.as_deref(), Some("Stride"));
        assert_eq!(
            candidates[2].effect.as_deref(),
            Some("You move up to your Speed (page 420).")
        );

        assert_eq!(candidates[3].name.as_deref(), Some("Take Cover"));
        // The book's Requirements sentence uses commas as ordinary
        // punctuation, not list delimiters, but parser.rs's own
        // `split_list` doesn't know that -- see this module's doc.
        assert_eq!(
            candidates[3].requirements,
            vec![
                "You are benefiting from cover".to_owned(),
                "are near a feature that allows you to take cover".to_owned(),
                "or are prone.".to_owned(),
            ]
        );

        for candidate in &candidates {
            assert!(
                candidate.confidence > 0.5,
                "{:?} parsed at low confidence: {converted}",
                candidate.name
            );
        }
    }
}

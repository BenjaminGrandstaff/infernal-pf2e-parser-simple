//! Goal: turn PF2e source text into structured fields through simple,
//! deterministic, line-based rules -- never a language model, never a
//! guess dressed up as a fact. This is the one module in either PF2e
//! service that actually knows what a PF2e feat or reaction looks like.
//!
//! ## Literal text, normalized fields, inferences, and confidence stay separate
//!
//! Every parse keeps four things distinct:
//! - `source_text` -- the block's text exactly as received, untouched;
//! - normalized fields (`name`, `traits`, `trigger`, `requirements`,
//!   `prerequisites`, `effect`) -- populated *only* from an explicitly
//!   labeled line, never inferred from surrounding prose;
//! - `references` -- a cross-reference this parse *noticed* (a `See
//!   also:` line, or `[[Bracketed Name]]` markup), never resolved or
//!   validated;
//! - `inferences` -- anything this parse concluded but the source did
//!   not state as a label, kept out of every normalized field;
//! - `confidence` -- lower whenever the source's structure was
//!   incomplete or unrecognized, never inflated.
//!
//! ## Recognized shape
//!
//! A block optionally opens with a bare type line (`Feat`, `Action`,
//! `Reaction`, `Free Action`, `Condition`) or a `Type: <word>` line, then
//! zero or more `Label: value` lines (matched case-insensitively):
//! `Name`, `Traits` (comma-separated), `Trigger`, `Requirements`
//! (comma-separated -- conditions that must hold to *use* an action or
//! reaction), `Prerequisites` (comma-separated -- what a *feat* requires
//! to be taken; kept in its own field, distinct from `Requirements`),
//! `Effect`, `See also`/`References` (comma-separated). A source with
//! none of this structure still produces exactly one `Unclassified`
//! candidate wrapping the whole block as `source_text`, at low
//! confidence, rather than an error.
//!
//! Multiple candidates in one source fragment are separated by a line
//! containing only `---`.

use crate::domain::{Candidate, RuleType};

const BLOCK_SEPARATOR: &str = "---";
const BASE_CONFIDENCE: f64 = 0.9;
const UNCLASSIFIED_CONFIDENCE: f64 = 0.2;
const MISSING_TYPE_PENALTY: f64 = 0.2;
const LEFTOVER_PENALTY: f64 = 0.15;

/// Parses `source_text` into zero or more candidates. Never fails:
/// content this parser cannot make sense of becomes a low-confidence
/// `Unclassified` candidate, not an error.
pub fn parse_candidates(source_text: &str) -> Vec<Candidate> {
    source_text
        .split(BLOCK_SEPARATOR)
        .map(str::trim)
        .filter(|block| !block.is_empty())
        .map(parse_block)
        .collect()
}

fn parse_block(block: &str) -> Candidate {
    let lines: Vec<&str> = block
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();

    let mut rule_type = None;
    let mut name = None;
    let mut traits = Vec::new();
    let mut trigger = None;
    let mut requirements = Vec::new();
    let mut prerequisites = Vec::new();
    let mut effect = None;
    let mut references = Vec::new();
    let mut leftover_lines = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if index == 0 {
            if let Some(detected) = detect_bare_type(line) {
                rule_type = Some(detected);
                continue;
            }
        }
        match split_label(line) {
            Some((label, value)) if label == "type" => {
                rule_type = Some(RuleType::parse(&normalize_type(value)));
            }
            Some((label, value)) if label == "name" => name = Some(value.to_owned()),
            Some((label, value)) if label == "traits" => traits = split_list(value),
            Some((label, value)) if label == "trigger" => trigger = Some(value.to_owned()),
            Some((label, value)) if label == "requirements" => requirements = split_list(value),
            Some((label, value)) if label == "prerequisites" => prerequisites = split_list(value),
            Some((label, value)) if label == "effect" => effect = Some(value.to_owned()),
            Some((label, value)) if label == "see also" || label == "references" => {
                references.extend(split_list(value));
            }
            _ => leftover_lines.push(*line),
        }
    }

    references.extend(extract_bracketed_references(block));

    let mut inferences = Vec::new();
    let mut confidence = BASE_CONFIDENCE;

    let rule_type = match rule_type {
        Some(detected) => detected,
        None => {
            confidence -= MISSING_TYPE_PENALTY;
            RuleType::Unclassified
        }
    };

    if !leftover_lines.is_empty() {
        inferences.push(format!(
            "{} untagged line(s) present in source, not mapped to a known field",
            leftover_lines.len()
        ));
        confidence -= LEFTOVER_PENALTY;
    }

    if name.is_none()
        && trigger.is_none()
        && effect.is_none()
        && traits.is_empty()
        && requirements.is_empty()
        && prerequisites.is_empty()
        && matches!(rule_type, RuleType::Unclassified)
    {
        return Candidate {
            candidate_id: uuid::Uuid::new_v4(),
            rule_type: RuleType::Unclassified,
            name: None,
            traits: Vec::new(),
            trigger: None,
            requirements: Vec::new(),
            prerequisites: Vec::new(),
            effect: None,
            references,
            source_text: block.to_owned(),
            inferences: Vec::new(),
            confidence: UNCLASSIFIED_CONFIDENCE,
        };
    }

    Candidate {
        candidate_id: uuid::Uuid::new_v4(),
        rule_type,
        name,
        traits,
        trigger,
        requirements,
        prerequisites,
        effect,
        references,
        source_text: block.to_owned(),
        inferences,
        confidence: confidence.clamp(0.0, 1.0),
    }
}

fn detect_bare_type(line: &str) -> Option<RuleType> {
    let normalized = normalize_type(line);
    match RuleType::parse(&normalized) {
        RuleType::Unclassified => None,
        detected => Some(detected),
    }
}

fn normalize_type(value: &str) -> String {
    value.trim().to_lowercase().replace(' ', "-")
}

fn split_label(line: &str) -> Option<(String, &str)> {
    let (label, value) = line.split_once(':')?;
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 32 {
        return None;
    }
    Some((label.to_lowercase(), value.trim()))
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn extract_bracketed_references(block: &str) -> Vec<String> {
    let mut references = Vec::new();
    let mut remainder = block;
    while let Some(start) = remainder.find("[[") {
        let after_open = &remainder[start + 2..];
        let Some(end) = after_open.find("]]") else {
            break;
        };
        let reference = after_open[..end].trim();
        if !reference.is_empty() {
            references.push(reference.to_owned());
        }
        remainder = &after_open[end + 2..];
    }
    references
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_simple_action_is_parsed_from_labeled_lines() {
        let source = "Action\nName: Stride\nEffect: Move up to your Speed.";

        let candidates = parse_candidates(source);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].rule_type, RuleType::Action);
        assert_eq!(candidates[0].name.as_deref(), Some("Stride"));
        assert!(candidates[0].confidence > 0.8);
    }

    #[test]
    fn a_reaction_keeps_trigger_separate_from_effect() {
        let source = "Reaction\nName: Opportunity Strike\nTraits: attack\nTrigger: A foe leaves a square.\nEffect: Make a melee Strike.";

        let candidates = parse_candidates(source);

        let candidate = &candidates[0];
        assert_eq!(candidate.rule_type, RuleType::Reaction);
        assert_eq!(candidate.trigger.as_deref(), Some("A foe leaves a square."));
        assert_eq!(candidate.effect.as_deref(), Some("Make a melee Strike."));
    }

    #[test]
    fn prerequisites_and_requirements_are_kept_as_separate_fields() {
        let source = "Feat\nName: Guarded Stance\nPrerequisites: trained in a shield\nRequirements: shield raised\nEffect: Reduce damage.";

        let candidates = parse_candidates(source);

        assert_eq!(
            candidates[0].prerequisites,
            vec!["trained in a shield".to_owned()]
        );
        assert_eq!(candidates[0].requirements, vec!["shield raised".to_owned()]);
    }

    #[test]
    fn a_condition_needs_no_trigger_or_prerequisites() {
        let source =
            "Condition\nName: Frightened\nEffect: Take a penalty that decreases each round.";

        let candidates = parse_candidates(source);

        assert_eq!(candidates[0].rule_type, RuleType::Condition);
        assert!(candidates[0].trigger.is_none());
        assert!(candidates[0].prerequisites.is_empty());
    }

    #[test]
    fn an_explicit_cross_reference_is_noticed_but_not_resolved() {
        let source = "Feat\nName: Diehard\nEffect: You are not doomed until [[Doomed]] 4.";

        let candidates = parse_candidates(source);

        assert_eq!(candidates[0].references, vec!["Doomed".to_owned()]);
    }

    #[test]
    fn unrecognized_structure_becomes_a_single_low_confidence_unclassified_candidate() {
        let source = "The wind howls through the ancient ruins.";

        let candidates = parse_candidates(source);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].rule_type, RuleType::Unclassified);
        assert!(candidates[0].confidence < 0.5);
        assert_eq!(candidates[0].source_text, source);
    }

    #[test]
    fn untagged_trailing_text_is_preserved_as_an_inference() {
        let source =
            "Feat\nName: Toughness\nEffect: Increase max HP.\nThis line matches no known label.";

        let candidates = parse_candidates(source);

        assert!(
            candidates[0]
                .inferences
                .iter()
                .any(|note| note.contains("untagged"))
        );
        assert!(candidates[0].confidence < BASE_CONFIDENCE);
    }

    #[test]
    fn multiple_blocks_produce_multiple_candidates() {
        let source = "Feat\nName: A\nEffect: one\n---\nFeat\nName: B\nEffect: two";

        let candidates = parse_candidates(source);

        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].name.as_deref(), Some("A"));
        assert_eq!(candidates[1].name.as_deref(), Some("B"));
    }

    #[test]
    fn a_block_that_is_only_whitespace_or_separators_produces_zero_candidates() {
        assert!(parse_candidates("   \n---\n   ").is_empty());
    }
}

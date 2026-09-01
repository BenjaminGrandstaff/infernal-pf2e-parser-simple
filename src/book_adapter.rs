//! Goal: convert a raw book-extracted PF2e page chunk (as produced by an
//! external PDF-to-text tool, not part of this service) into the exact
//! `Type:`/`Label:` block grammar `parser.rs` already expects -- never
//! widen `parser.rs`'s own grammar to understand book formatting
//! directly. This module knows about book conventions (bracketed
//! action-cost annotations, label words with no colon, glossary-style
//! condition entries); `parser.rs` still knows nothing about the book,
//! only about the labeled-block grammar it already had.
//!
//! Three converters, one per book layout this pass covers:
//! `convert_page` (actions/reactions/free actions), `convert_feat_page`
//! (feats), `convert_condition_page` (the Conditions Appendix glossary).
//! Each is driven by its own real excerpt from the chunker tool's
//! output in this module's tests, not a synthetic fixture.
//!
//! ## Known limitations
//!
//! Actions (`convert_page`):
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
//!   paper over the mismatch.
//!
//! Feats (`convert_feat_page`):
//! - a feat that is *also* an action (its name line carries its own
//!   `[two-actions]`-style bracket, e.g. "Boulder Roll", "Combat
//!   Assessment") is not recognized as a feat heading at all --
//!   `RuleType` cannot express "both Feat and Action" for one
//!   candidate, and this module does not pick one on the book's
//!   behalf; it is simply not extracted yet, rather than mis-typed.
//!   Its orphaned `FEAT <level>` marker and trait line are silently
//!   absorbed into whatever entry precedes it in the text, rather than
//!   producing a candidate of their own -- confirmed as the right
//!   failure mode only after the wrong one (the bare marker line
//!   itself misread as a fresh heading, with the trait word after it
//!   as a bogus name) turned up hundreds of times at full-corpus
//!   scale; see `split_feat_entries`'s own guard against a `FEAT
//!   <level>` line ever starting a heading scan on its own;
//! - `Prerequisites` is deliberately **not** split out of the book's
//!   prose the way `Requirements`/`Trigger` are for actions: unlike
//!   those, a feat's `Prerequisites` clause runs directly into the
//!   next sentence with no delimiting punctuation at all (e.g.
//!   "Prerequisites darkvision Using ancient dwarven methods..."), so
//!   there is no reliable boundary to split on. Guessing one would be
//!   exactly the "a guess dressed up as a fact" `parser.rs`'s own
//!   module doc disclaims. It stays folded into `Effect` verbatim,
//!   same as `Special`.
//!
//! Conditions (`convert_condition_page`):
//! - only text after a literal `List of Conditions` anchor line is
//!   scanned; entries are recognized by a one- or two-word strict
//!   Title Case line (matching the glossary's own heading style) with
//!   no punctuation. A capitalized proper noun or game term of the
//!   same shape appearing mid-description would false-positive as a
//!   new entry -- not observed in the driving fixture, but not
//!   structurally ruled out either;
//! - a bare page-number line (a PDF-pagination artifact between
//!   entries) is dropped rather than treated as content.
//!
//! See this repository's `ORC-NOTICE.md` for the separate, still-open
//! question of filtering Reserved Material out of whatever any of
//! these three converters produce.

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

struct RawFeatEntry {
    name: String,
    traits: Vec<String>,
    body_lines: Vec<String>,
}

/// Converts one page chunk's raw extracted `text` containing Feat
/// entries into zero or more `parser::parse_candidates`-ready blocks.
///
/// A feat entry's heading is not always the simple "name, then one
/// trait, then `FEAT <level>`" shape it first appears to be. A
/// dual-trait feat (e.g. Player Core's *Additional Lore*, a General
/// *and* Skill feat) prints its name immediately followed by its
/// *first* trait tag with no blank line between them at all, then
/// interleaves a `FEAT <level>` marker with each trait tag before the
/// real prose begins:
///
/// ```text
/// ADDITIONAL LORE
/// GENERAL
///
/// FEAT 1
///
/// SKILL
///
/// FEAT 1
///
/// GENERAL
///
/// Your knowledge has expanded...
/// ```
///
/// A fixed two-line "name, then FEAT N" lookahead cannot recover this:
/// it never matches "ADDITIONAL LORE" (immediately followed by
/// "GENERAL", not "FEAT N"), silently drops the real name, and then
/// matches "GENERAL" as its own bogus feat instead -- confirmed at
/// scale (`GENERAL`, `ARCHETYPE`, and every class trait word turned
/// up as a repeated "feat name" hundreds of times once this ran
/// against the full corpus, not just a handful of curated fixtures).
///
/// `try_read_heading_block` fixes this by not assuming a fixed shape
/// at all: it scans forward through the *entire* contiguous run of
/// shouty lines and `FEAT <level>` markers (blank lines skipped, any
/// order, any count) until the first line that is neither. The first
/// shouty line in that run is the name; every other distinct shouty
/// line is a trait, in the order encountered. A run with no `FEAT`
/// marker anywhere in it is not a feat heading at all.
pub fn convert_feat_page(text: &str) -> String {
    split_feat_entries(text)
        .iter()
        .map(render_feat_block)
        .collect::<Vec<_>>()
        .join("\n---\n")
}

fn split_feat_entries(text: &str) -> Vec<RawFeatEntry> {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut entries = Vec::new();
    let mut current: Option<RawFeatEntry> = None;
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.is_empty() {
            index += 1;
            continue;
        }
        // A `FEAT <level>` line is itself technically "shouty" (a digit
        // doesn't break the all-uppercase check), but it must never be
        // the *start* of a heading-block scan on its own -- it only
        // ever belongs inside one already in progress. Without this
        // guard, a dual Feat+Action entry (its name line carries its
        // own action-cost bracket, e.g. "Combat Assessment
        // [one-action]" -- not itself shouty, so skipped as ordinary
        // text per the limitation this module's doc already names)
        // leaves its own orphaned `FEAT <level>` and trait lines
        // behind, and the scanner would misread the bare marker line
        // as a fresh heading, with the trait word that follows it
        // becoming a bogus name (confirmed at scale: "Fighter",
        // "Archetype", and other class-trait words each turned up as
        // a repeated "feat name" dozens of times).
        if is_shouty(line) && !is_feat_level_marker(line) {
            if let Some((name, traits, next_index)) = try_read_heading_block(&lines, index) {
                entries.extend(current.take());
                current = Some(RawFeatEntry {
                    name,
                    traits,
                    body_lines: Vec::new(),
                });
                index = next_index;
                continue;
            }
        }
        if let Some(entry) = current.as_mut() {
            entry.body_lines.push(line.to_owned());
        }
        index += 1;
    }
    entries.extend(current.take());
    entries
}

/// A safety bound on how many shouty/level-marker lines one heading
/// block may absorb before giving up -- guards against an unrelated
/// run of all-caps text elsewhere in the book (an index page, a table
/// of contents) being scanned indefinitely looking for a `FEAT`
/// marker that will never come.
const MAX_HEADING_BLOCK_LINES: usize = 8;

/// See `convert_feat_page`'s own documentation for why this exists.
/// Returns `None` (not a heading) if the run starting at `start` never
/// contains a `FEAT <level>` marker before hitting real prose.
fn try_read_heading_block(lines: &[&str], start: usize) -> Option<(String, Vec<String>, usize)> {
    let mut index = start;
    let mut shouty_lines: Vec<&str> = Vec::new();
    let mut saw_feat_marker = false;

    while index < lines.len() && shouty_lines.len() <= MAX_HEADING_BLOCK_LINES {
        let line = lines[index];
        if line.is_empty() {
            index += 1;
            continue;
        }
        if is_feat_level_marker(line) {
            saw_feat_marker = true;
            index += 1;
            continue;
        }
        if is_shouty(line) {
            shouty_lines.push(line);
            index += 1;
            continue;
        }
        break;
    }

    if !saw_feat_marker {
        return None;
    }
    let (name_line, trait_lines) = shouty_lines.split_first()?;

    let mut traits = Vec::new();
    for &line in trait_lines {
        let trait_word = line.to_lowercase();
        if !traits.contains(&trait_word) {
            traits.push(trait_word);
        }
    }

    Some((title_case(name_line), traits, index))
}

fn is_feat_level_marker(line: &str) -> bool {
    line.strip_prefix("FEAT ")
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

fn render_feat_block(entry: &RawFeatEntry) -> String {
    let effect_parts = group_paragraphs(&entry.body_lines);

    let mut lines = vec!["Feat".to_owned(), format!("Name: {}", entry.name)];
    if !entry.traits.is_empty() {
        lines.push(format!("Traits: {}", entry.traits.join(", ")));
    }
    if !effect_parts.is_empty() {
        lines.push(format!("Effect: {}", effect_parts.join(" ")));
    }
    lines.join("\n")
}

struct RawConditionEntry {
    name: String,
    body_lines: Vec<String>,
}

/// Converts a Conditions Appendix page chunk's raw extracted `text`
/// into zero or more `parser::parse_candidates`-ready blocks. Unlike
/// actions and feats, the glossary has no blank lines or bracket
/// markers between entries at all -- only a literal `List of
/// Conditions` anchor line and each entry's own strict Title Case
/// heading (see this module's doc for the false-positive risk that
/// implies).
pub fn convert_condition_page(text: &str) -> String {
    split_condition_entries(text)
        .iter()
        .map(render_condition_block)
        .collect::<Vec<_>>()
        .join("\n---\n")
}

fn split_condition_entries(text: &str) -> Vec<RawConditionEntry> {
    let mut entries = Vec::new();
    let mut current: Option<RawConditionEntry> = None;
    let mut past_anchor = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if !past_anchor {
            past_anchor = line.eq_ignore_ascii_case("List of Conditions");
            continue;
        }
        if line.is_empty() || is_page_number(line) {
            continue;
        }
        if is_condition_heading(line) {
            entries.extend(current.take());
            current = Some(RawConditionEntry {
                name: line.to_owned(),
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

fn is_page_number(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c.is_ascii_digit())
}

fn is_condition_heading(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    !words.is_empty() && words.len() <= 2 && words.iter().all(|word| is_strict_title_case(word))
}

fn is_strict_title_case(word: &str) -> bool {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) if first.is_uppercase() => {
            chars.all(|c| !c.is_alphabetic() || c.is_lowercase())
        }
        _ => false,
    }
}

fn render_condition_block(entry: &RawConditionEntry) -> String {
    let effect = entry.body_lines.join(" ");
    let mut lines = vec!["Condition".to_owned(), format!("Name: {}", entry.name)];
    if !effect.trim().is_empty() {
        lines.push(format!("Effect: {}", effect.trim()));
    }
    lines.join("\n")
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

    use super::{convert_condition_page, convert_feat_page};

    /// Verbatim extraction of Player Core page 43 (dwarf ancestry
    /// feats), from the chunker tool's own output. "Defy the Darkness"
    /// is included specifically to prove its unsplittable
    /// `Prerequisites darkvision` clause stays folded into `Effect`
    /// rather than being guessed at -- see this module's doc.
    const PLAYER_CORE_DWARF_FEATS: &str = "\
CLAN DAGGER

FEAT 1

DWARF

You are naturally calm and collected in the face of imminent danger. At the end of
your turn, reduce your frightened condition by 2 instead of 1.

DWARVEN WEAPON FAMILIARITY

FEAT 1

DWARF

Your kin have instilled in you an affinity for hard-hitting
weapons, and you prefer these to more elegant arms. You gain
access to all uncommon weapons with the dwarf trait.

ROCK RUNNER

FEAT 1

DWARF

Your innate connection to stone makes you adept at moving
across uneven surfaces. You can ignore difficult terrain caused
by stone.

DEFY THE DARKNESS

FEAT 5

DWARF

Prerequisites darkvision
Using ancient dwarven methods developed to fight enemies
wielding magical darkness, you've honed your darkvision
and sworn not to use such magic yourself.
";

    #[test]
    fn converts_four_real_feats_off_one_page_into_parseable_blocks() {
        let converted = convert_feat_page(PLAYER_CORE_DWARF_FEATS);
        let candidates = parse_candidates(&converted);

        assert_eq!(candidates.len(), 4, "converted:\n{converted}");

        for candidate in &candidates {
            assert_eq!(candidate.rule_type, RuleType::Feat);
            assert_eq!(candidate.traits, vec!["dwarf".to_owned()]);
        }

        assert_eq!(candidates[0].name.as_deref(), Some("Clan Dagger"));
        assert_eq!(
            candidates[1].name.as_deref(),
            Some("Dwarven Weapon Familiarity")
        );
        assert_eq!(candidates[2].name.as_deref(), Some("Rock Runner"));

        assert_eq!(candidates[3].name.as_deref(), Some("Defy The Darkness"));
        // Not split out -- see this module's doc on why Prerequisites
        // is deliberately left folded into Effect for feats.
        assert!(
            candidates[3]
                .effect
                .as_deref()
                .unwrap()
                .starts_with("Prerequisites darkvision"),
            "{:?}",
            candidates[3].effect
        );
        assert!(candidates[3].prerequisites.is_empty());
    }

    /// Verbatim extraction of Player Core page 253 -- the exact real
    /// text that first exposed this bug: at full-corpus scale (all
    /// 1,404 page chunks across four books), the old two-line
    /// lookahead misdetected the trait word "General" as a feat name
    /// 124 times, "Archetype" 76 times, and so on for every dual-trait
    /// feat and every class-trait word in the book.
    const PLAYER_CORE_DUAL_TRAIT_FEATS: &str = "\
ADDITIONAL LORE
GENERAL

FEAT 1

SKILL

FEAT 1

GENERAL

Your knowledge has expanded to encompass a new field.
Choose a Lore skill subcategory. You become trained in it.

ADOPTED ANCESTRY

FEAT 1

GENERAL

You're fully immersed in another ancestry's culture and
traditions.
";

    #[test]
    fn a_dual_trait_feats_name_is_recovered_not_swallowed_by_its_own_trait_tag() {
        let converted = convert_feat_page(PLAYER_CORE_DUAL_TRAIT_FEATS);
        let candidates = parse_candidates(&converted);

        assert_eq!(candidates.len(), 2, "converted:\n{converted}");

        assert_eq!(candidates[0].name.as_deref(), Some("Additional Lore"));
        assert_eq!(
            candidates[0].traits,
            vec!["general".to_owned(), "skill".to_owned()],
            "converted:\n{converted}"
        );
        assert!(
            candidates[0]
                .effect
                .as_deref()
                .unwrap()
                .starts_with("Your knowledge has expanded")
        );

        assert_eq!(candidates[1].name.as_deref(), Some("Adopted Ancestry"));
        assert_eq!(candidates[1].traits, vec!["general".to_owned()]);
    }

    /// Verbatim extraction of Player Core page 78 (Fighter feats) --
    /// the second real bug the full-corpus run turned up. "Combat
    /// Assessment" and "Double Slice" are dual Feat+Action entries
    /// (their own `[one-action]`/`[two-actions]` bracket), an already-
    /// documented gap. What the corpus run actually exposed was what
    /// happened to their *orphaned* `FEAT 1`/`FIGHTER` lines once the
    /// entry itself was skipped: the bare `FEAT 1` marker line was
    /// misread as a fresh heading trigger, turning "Fighter" into a
    /// bogus feat name -- 55 times across the corpus.
    const PLAYER_CORE_FIGHTER_FEATS: &str = "\
1ST LEVEL
COMBAT ASSESSMENT [one-action]

FEAT 1

FIGHTER

You make a telegraphed attack to learn about your foe.

DOUBLE SLICE [two-actions]

FEAT 1

FIGHTER

Requirements You are wielding two melee weapons, each in a
different hand.
You lash out at your foe with both weapons.

MOBILE SHOT STANCE

FEAT 1

FIGHTER

You focus on ranged combat, ready to fire at any moment.
";

    #[test]
    fn a_dual_typed_feats_orphaned_feat_marker_never_becomes_a_bogus_heading() {
        let converted = convert_feat_page(PLAYER_CORE_FIGHTER_FEATS);
        let candidates = parse_candidates(&converted);

        // Only the one real, cleanly-typed feat -- neither
        // "Combat Assessment" nor "Double Slice" is extracted (both
        // are dual Feat+Action, a separate documented gap), and
        // critically, no bogus "Fighter" candidate is fabricated from
        // their orphaned FEAT 1 / FIGHTER lines.
        assert_eq!(candidates.len(), 1, "converted:\n{converted}");
        assert_eq!(candidates[0].name.as_deref(), Some("Mobile Shot Stance"));
        assert_eq!(candidates[0].traits, vec!["fighter".to_owned()]);
        assert!(
            !candidates
                .iter()
                .any(|c| c.name.as_deref() == Some("Fighter"))
        );
    }

    /// Verbatim extraction of the Player Core Conditions Appendix
    /// (page 442-443), from the chunker tool's own output.
    const PLAYER_CORE_CONDITIONS_APPENDIX: &str = "\
CONDITIONS APPENDIX

While adventuring, characters are affected by conditions.

Condition Values

Some conditions have a number after the condition, called a condition value.

Overriding Conditions

Some conditions override others.

List of Conditions
Blinded
You can't see. All normal terrain is difficult terrain to you.
You automatically critically fail Perception checks that require
you to be able to see. Blinded overrides dazzled.

Broken
Broken is a condition that affects only objects. An object is
broken when damage has reduced its Hit Points to equal or
less than its Broken Threshold.

442

Clumsy
Your movements become clumsy and inexact. Clumsy
always includes a value. You take a status penalty equal
to the condition value to Dexterity-based checks and DCs.
";

    #[test]
    fn converts_three_real_conditions_off_the_glossary_into_parseable_blocks() {
        let converted = convert_condition_page(PLAYER_CORE_CONDITIONS_APPENDIX);
        let candidates = parse_candidates(&converted);

        // "Condition Values" and "Overriding Conditions" precede the
        // `List of Conditions` anchor and must not be picked up as
        // entries themselves.
        assert_eq!(candidates.len(), 3, "converted:\n{converted}");

        for candidate in &candidates {
            assert_eq!(candidate.rule_type, RuleType::Condition);
        }

        assert_eq!(candidates[0].name.as_deref(), Some("Blinded"));
        assert!(
            candidates[0]
                .effect
                .as_deref()
                .unwrap()
                .starts_with("You can't see.")
        );

        assert_eq!(candidates[1].name.as_deref(), Some("Broken"));

        assert_eq!(candidates[2].name.as_deref(), Some("Clumsy"));
        // The stray "442" page-number line between entries must be
        // dropped, not folded into Broken's or Clumsy's Effect.
        assert!(!candidates[1].effect.as_deref().unwrap().contains("442"));
        assert!(!candidates[2].effect.as_deref().unwrap().contains("442"));
    }
}

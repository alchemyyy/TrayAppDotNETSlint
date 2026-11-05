//! Settings-window search scoring (port of `Utils/SearchMatcher.cs`,
//! `UI/Settings/SettingsSearchScorer.cs`, `UI/Settings/SettingsSearchSynonymMap.cs`,
//! `UI/Settings/SettingsSearchMetadata.cs`). Pure logic; the settings window owns the UI.
//!
//! The C# settings window filters its cards in place: every card is a document (title, keywords,
//! body, subsection, page), [`find_matches`] decides which documents match, and matches keep their
//! page order. [`rank`] returns the matching indices in entry order for the same reason.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

/// `CommonStrings.SettingsWindow_SearchSynonymGroups_Common` (English): one synonym group per line,
/// equivalent terms separated by `|`. Apps add their own groups (`SettingsWindow_SearchSynonymGroups_App`)
/// through [`SynonymMap::parse`].
pub const COMMON_SYNONYM_GROUPS: &str = r"maximum|max|highest|largest|greatest|upper limit|upper bound
minimum|lowest|smallest|lower limit|lower bound
average|mean
enable|activate
disable|deactivate
automatic|automatically|auto
manual|manually
reset|reset to defaults|restore defaults|revert to defaults
opacity|transparency|alpha channel
color|colour
notification|alert|toast|tray notification
hotkey|keyboard shortcut|key binding|shortcut key|global shortcut|system wide shortcut
run on startup|autostart|start with Windows|launch at sign in|launch at logon|run at login|run at logon
folder|directory
icon|glyph|pictogram
installation|install application|set up application|add application
uninstall|uninstall application|remove application
upgrade|software update|app update|upgrade application|new app version
backdate|downgrade|roll back|rollback|revert to older version
current user|local user|per user|user only
GPU rendering|hardware acceleration|graphics acceleration|graphics renderer
software rendering|CPU rendering|software renderer
keep flyout warm|keep tray context menu warm|preload|preload window|pre initialize window|prepare in background
tray flyout|flyout panel
undock|detach|floating mode|make floating
dock to tray|anchor to tray|attach to tray
keep on screen|constrain to work area|clamp to work area
tray icon|system tray icon|notification area icon
context menu|right click menu|tray menu
theme mode|theme style|color scheme
font size|text size|type size|text scale
tooltip|tool tip|hover text|hover tip|help tip
animation|motion effect|transition|fade effect
text color|foreground color|font color|lettering color
background color|window background|surface color
title bar|titlebar|window header
slider thumb|slider handle|slider knob
mouse wheel|scroll wheel|wheel scrolling
touchpad|trackpad
centered|centred|center aligned|middle aligned";

/// One searchable settings card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchEntry {
    /// Card title, e.g. "Wheel volume step (%)".
    pub title: String,
    /// Card description / help text.
    pub description: String,
    /// Extra search keywords (the resx `*_Keywords` values).
    pub keywords: Vec<String>,
    /// Page title, e.g. "Tray icon".
    pub page: String,
    /// Section header inside the page, e.g. "Modified actions"; empty when none.
    pub section: String,
}

/// Returns the indices of `entries` that match `query` using the common synonym groups, in entry
/// order (the C# window filters cards in place, so matches keep their page order). An empty or
/// whitespace query returns nothing.
pub fn rank(query: &str, entries: &[SearchEntry]) -> Vec<usize> {
    static COMMON: LazyLock<SynonymMap> = LazyLock::new(|| SynonymMap::parse(&[COMMON_SYNONYM_GROUPS]));
    find_matches(query, entries, Some(&COMMON))
}

// ---------------------------------------------------------------------------------------------
// Unicode helpers (ICU, as .NET uses ICU data on Windows)
// ---------------------------------------------------------------------------------------------

const U_UPPERCASE_LETTER: i8 = 1;
const U_OTHER_LETTER: i8 = 5;
const U_NON_SPACING_MARK: i8 = 6;
const U_COMBINING_SPACING_MARK: i8 = 8;
const U_DECIMAL_DIGIT_NUMBER: i8 = 9;

fn char_type(c: char) -> i8 {
    // SAFETY: u_charType takes a code point by value.
    unsafe { windows::Win32::Globalization::u_charType(c as i32) }
}

/// `Rune.IsLetterOrDigit` / `char.IsLetterOrDigit`: categories Lu, Ll, Lt, Lm, Lo, Nd.
fn is_letter_or_digit(c: char) -> bool {
    let category = char_type(c);
    (U_UPPERCASE_LETTER..=U_OTHER_LETTER).contains(&category) || category == U_DECIMAL_DIGIT_NUMBER
}

/// Mn, Mc, Me.
fn is_mark(c: char) -> bool {
    (U_NON_SPACING_MARK..=U_COMBINING_SPACING_MARK).contains(&char_type(c))
}

/// `ToLowerInvariant`: simple (one-to-one) lowercase mapping.
fn to_lower_invariant(c: char) -> char {
    if c.is_ascii() {
        return c.to_ascii_lowercase();
    }
    // SAFETY: u_tolower takes a code point by value.
    let lower = unsafe { windows::Win32::Globalization::u_tolower(c as i32) };
    u32::try_from(lower).ok().and_then(char::from_u32).unwrap_or(c)
}

/// `string.Normalize(NormalizationForm.FormKD)`.
fn normalize_kd(text: &str) -> String {
    use windows::Win32::Globalization::{NormalizationKD, NormalizeString};
    if text.is_ascii() {
        return text.to_owned();
    }
    let source: Vec<u16> = text.encode_utf16().collect();
    let mut capacity = source.len() * 3 + 16;
    for _ in 0..4 {
        let mut buffer = vec![0u16; capacity];
        // SAFETY: both slices are live for the call; the API writes at most `buffer.len()` units.
        let written = unsafe { NormalizeString(NormalizationKD, &source, Some(&mut buffer)) };
        if written > 0 {
            buffer.truncate(written as usize);
            return String::from_utf16_lossy(&buffer);
        }
        if written == 0 && source.is_empty() {
            return String::new();
        }
        // ERROR_INSUFFICIENT_BUFFER returns a negative estimate of the needed size.
        capacity = (written.unsigned_abs() as usize).max(capacity * 2);
    }
    text.to_owned()
}

fn is_ascii(text: &str) -> bool {
    text.is_ascii()
}

fn rune_length(text: &str) -> usize {
    text.chars().count()
}

fn tokenize(normalized: &str) -> Vec<&str> {
    normalized.split(' ').map(str::trim).filter(|t| !t.is_empty()).collect()
}

// ---------------------------------------------------------------------------------------------
// SearchMatcher
// ---------------------------------------------------------------------------------------------

const EXACT_SCORE: i32 = 10_000;
const PREFIX_SCORE: i32 = 8_500;
const WORD_PREFIX_SCORE: i32 = 7_600;
const ACRONYM_PREFIX_SCORE: i32 = 7_200;
const CONTAINS_SCORE: i32 = 5_600;
const ACRONYM_SUBSEQUENCE_SCORE: i32 = 4_600;
const SUBSEQUENCE_SCORE: i32 = 3_200;
const GAP_PENALTY: i32 = 12;
const START_PENALTY: i32 = 4;

/// `SearchMatcher.Normalize`: lower-invariant letters and digits, every other run of characters
/// one space, trimmed. Works per UTF-16 unit like C#: supplementary characters are separators.
fn matcher_normalize(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut previous_was_separator = true;
    for c in value.trim().chars() {
        if (c as u32) <= 0xFFFF && is_letter_or_digit(c) {
            out.push(to_lower_invariant(c));
            previous_was_separator = false;
        } else if !previous_was_separator {
            out.push(' ');
            previous_was_separator = true;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }
    out
}

fn build_acronym(candidate: &str) -> String {
    candidate.split(' ').filter_map(|word| word.chars().next()).collect()
}

fn utf16_len(text: &str) -> i32 {
    text.encode_utf16().count() as i32
}

fn score_subsequence(candidate: &str, token: &str) -> Option<i32> {
    let token: Vec<char> = token.chars().collect();
    let mut token_index = 0;
    let mut first_match: i32 = -1;
    let mut previous_match: i32 = -1;
    let mut gap_total = 0;
    for (i, c) in candidate.chars().enumerate() {
        if token_index >= token.len() {
            break;
        }
        if c != token[token_index] {
            continue;
        }
        let i = i as i32;
        if first_match < 0 {
            first_match = i;
        }
        if previous_match >= 0 {
            gap_total += (i - previous_match - 1).max(0);
        }
        previous_match = i;
        token_index += 1;
    }
    (token_index == token.len()).then(|| token.len() as i32 - first_match * START_PENALTY - gap_total * GAP_PENALTY)
}

fn score_token(candidate: &str, acronym: &str, token: &str) -> Option<i32> {
    let length = utf16_len(token);
    if candidate == token {
        return Some(EXACT_SCORE + length);
    }
    if candidate.starts_with(token) {
        return Some(PREFIX_SCORE + length);
    }
    if candidate.contains(&format!(" {token}")) {
        return Some(WORD_PREFIX_SCORE + length);
    }
    if !acronym.is_empty() && acronym.starts_with(token) {
        return Some(ACRONYM_PREFIX_SCORE + length);
    }
    if let Some(index) = candidate.find(token) {
        return Some(CONTAINS_SCORE + length - utf16_len(&candidate[..index]));
    }
    if !acronym.is_empty()
        && let Some(score) = score_subsequence(acronym, token)
    {
        return Some(ACRONYM_SUBSEQUENCE_SCORE + score);
    }
    if length >= 4
        && let Some(score) = score_subsequence(candidate, token)
    {
        return Some(SUBSEQUENCE_SCORE + score);
    }
    None
}

/// `SearchMatcher.Score`: None when `candidate` does not match every query token; an empty query
/// matches with score 0.
pub fn matcher_score(candidate: &str, query: &str) -> Option<i32> {
    let candidate = matcher_normalize(candidate);
    let query = matcher_normalize(query);
    if query.trim().is_empty() {
        return Some(0);
    }
    if candidate.trim().is_empty() {
        return None;
    }
    let acronym = build_acronym(&candidate);
    query.split(' ').filter(|t| !t.is_empty()).try_fold(0i32, |total, token| {
        score_token(&candidate, &acronym, token).map(|score| total.saturating_add(score))
    })
}

/// `SearchMatcher.FilterAndRank`: indices of the items whose text matches, best score first, ties
/// in item order. An empty query returns every index.
pub fn filter_and_rank<T>(items: &[T], query: &str, text: impl Fn(&T) -> &str) -> Vec<usize> {
    let query = matcher_normalize(query);
    if query.trim().is_empty() {
        return (0..items.len()).collect();
    }
    let mut ranked: Vec<(usize, i32)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| matcher_score(text(item), &query).map(|score| (index, score)))
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.into_iter().map(|(index, _)| index).collect()
}

// ---------------------------------------------------------------------------------------------
// SettingsSearchSynonymMap
// ---------------------------------------------------------------------------------------------

const NO_SYNONYM_GROUP: i32 = -1;
const TERM_SEPARATOR: char = '|';

#[derive(Debug)]
struct SynonymTerm {
    text: String,
    tokens: Vec<String>,
}

#[derive(Debug)]
struct SynonymGroup {
    id: i32,
    terms: Vec<SynonymTerm>,
}

#[derive(Clone, Debug)]
struct QueryPart {
    text: String,
    group: i32,
}

fn contains_non_ascii(value: &str) -> bool {
    !value.is_ascii()
}

/// Localized equivalent phrases mapped to search concepts (`SettingsSearchSynonymMap`).
#[derive(Debug, Default)]
pub struct SynonymMap {
    groups: Vec<SynonymGroup>,
    ambiguous_query_terms: HashSet<String>,
}

impl SynonymMap {
    /// `SettingsSearchSynonymMap.Parse(definitions)`: each definition holds one group per line.
    /// Groups sharing two or more terms merge; a term in several groups is ambiguous and never
    /// selects a group from the query.
    pub fn parse(definitions: &[&str]) -> Self {
        let mut parsed_groups: Vec<Vec<String>> = Vec::new();
        let mut seen_groups: HashSet<String> = HashSet::new();
        for definition in definitions {
            parse_definition(definition, &mut parsed_groups, &mut seen_groups);
        }

        let mut consolidated: Vec<HashSet<String>> = Vec::new();
        for parsed in parsed_groups {
            let mut group: HashSet<String> = parsed.into_iter().collect();
            loop {
                let mut did_merge = false;
                let mut index = consolidated.len();
                while index > 0 {
                    index -= 1;
                    if group.intersection(&consolidated[index]).count() < 2 {
                        continue;
                    }
                    let existing = consolidated.remove(index);
                    group.extend(existing);
                    did_merge = true;
                }
                if !did_merge {
                    break;
                }
            }
            consolidated.push(group);
        }

        let mut frequencies: HashMap<&str, usize> = HashMap::new();
        for group in &consolidated {
            for term in group {
                *frequencies.entry(term.as_str()).or_default() += 1;
            }
        }
        let ambiguous_query_terms =
            frequencies.into_iter().filter(|(_, count)| *count > 1).map(|(term, _)| term.to_owned()).collect();

        let mut groups = Vec::new();
        for group in consolidated {
            if group.len() < 2 {
                continue;
            }
            let mut terms: Vec<SynonymTerm> = group
                .into_iter()
                .map(|text| SynonymTerm { tokens: tokenize(&text).into_iter().map(str::to_owned).collect(), text })
                .collect();
            terms.sort_by(|a, b| a.text.cmp(&b.text));
            groups.push(SynonymGroup { id: groups.len() as i32, terms });
        }
        Self { groups, ambiguous_query_terms }
    }

    fn parse_query(&self, normalized_query: &str) -> Vec<QueryPart> {
        let tokens = tokenize(normalized_query);
        let mut parts = Vec::new();
        let mut index = 0;
        while index < tokens.len() {
            if let Some((group, term)) = self.find_longest_term(&tokens, index) {
                parts.push(QueryPart { text: term.text.clone(), group: group.id });
                index += term.tokens.len();
                continue;
            }
            let token = tokens[index];
            if contains_non_ascii(token) {
                self.add_unsegmented_token_parts(token, &mut parts);
            } else {
                parts.push(QueryPart { text: token.to_owned(), group: NO_SYNONYM_GROUP });
            }
            index += 1;
        }
        parts
    }

    fn find_group_ids(&self, normalized_text: &str) -> Vec<i32> {
        self.groups
            .iter()
            .filter(|group| group.terms.iter().any(|term| contains_term(normalized_text, &term.text)))
            .map(|group| group.id)
            .collect()
    }

    fn find_longest_term(&self, query_tokens: &[&str], start: usize) -> Option<(&SynonymGroup, &SynonymTerm)> {
        let mut best: Option<(&SynonymGroup, &SynonymTerm)> = None;
        for group in &self.groups {
            for term in &group.terms {
                if self.ambiguous_query_terms.contains(&term.text) {
                    continue;
                }
                if term.tokens.len() > query_tokens.len() - start {
                    continue;
                }
                if !term.tokens.iter().zip(&query_tokens[start..]).all(|(t, q)| t == q) {
                    continue;
                }
                if best.is_some_and(|(_, b)| term.tokens.len() <= b.tokens.len()) {
                    continue;
                }
                best = Some((group, term));
            }
        }
        best
    }

    fn add_unsegmented_token_parts(&self, token: &str, parts: &mut Vec<QueryPart>) {
        let mut current = 0;
        while current < token.len() {
            let Some((match_index, group, term)) = self.find_next_unsegmented_term(token, current) else {
                parts.push(QueryPart { text: token[current..].to_owned(), group: NO_SYNONYM_GROUP });
                return;
            };
            if match_index > current {
                parts.push(QueryPart { text: token[current..match_index].to_owned(), group: NO_SYNONYM_GROUP });
            }
            parts.push(QueryPart { text: term.text.clone(), group: group.id });
            current = match_index + term.text.len();
        }
    }

    fn find_next_unsegmented_term(&self, token: &str, start: usize) -> Option<(usize, &SynonymGroup, &SynonymTerm)> {
        let mut best: Option<(usize, &SynonymGroup, &SynonymTerm)> = None;
        for group in &self.groups {
            for term in &group.terms {
                if self.ambiguous_query_terms.contains(&term.text) {
                    continue;
                }
                if term.tokens.len() != 1 || !contains_non_ascii(&term.text) {
                    continue;
                }
                let Some(offset) = token[start..].find(term.text.as_str()) else { continue };
                let match_index = start + offset;
                if let Some((best_index, _, best_term)) = best
                    && (match_index > best_index
                        || (match_index == best_index && utf16_len(&term.text) <= utf16_len(&best_term.text)))
                {
                    continue;
                }
                best = Some((match_index, group, term));
            }
        }
        best
    }
}

fn parse_definition(definition: &str, parsed_groups: &mut Vec<Vec<String>>, seen_groups: &mut HashSet<String>) {
    if definition.trim().is_empty() {
        return;
    }
    for line in definition.split(['\r', '\n']).map(str::trim).filter(|l| !l.is_empty()) {
        let mut terms: Vec<String> = Vec::new();
        for raw in line.split(TERM_SEPARATOR).map(str::trim).filter(|t| !t.is_empty()) {
            let normalized = normalize(raw);
            if normalized.is_empty() || terms.contains(&normalized) {
                continue;
            }
            terms.push(normalized);
        }
        if terms.len() < 2 {
            continue;
        }
        let mut identity = terms.clone();
        identity.sort();
        if seen_groups.insert(identity.join("|")) {
            parsed_groups.push(terms);
        }
    }
}

fn contains_term(normalized_text: &str, normalized_term: &str) -> bool {
    if normalized_term.is_empty() {
        return false;
    }
    if contains_non_ascii(normalized_term) && !normalized_term.contains(' ') {
        return normalized_text.contains(normalized_term);
    }
    let bytes = normalized_text.as_bytes();
    let mut search_start = 0;
    while search_start + normalized_term.len() <= normalized_text.len() {
        let Some(offset) = normalized_text[search_start..].find(normalized_term) else { return false };
        let index = search_start + offset;
        let end = index + normalized_term.len();
        let start_boundary = index == 0 || bytes[index - 1] == b' ';
        let end_boundary = end == bytes.len() || bytes[end] == b' ';
        if start_boundary && end_boundary {
            return true;
        }
        search_start = index + normalized_text[index..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

// ---------------------------------------------------------------------------------------------
// SettingsSearchScorer
// ---------------------------------------------------------------------------------------------

const TITLE_FIELD_WEIGHT: f64 = 1.00;
const KEYWORD_FIELD_WEIGHT: f64 = 0.98;
const BODY_FIELD_WEIGHT: f64 = 0.78;
const SUBSECTION_FIELD_WEIGHT: f64 = 0.64;
const PAGE_FIELD_WEIGHT: f64 = 0.56;
const MINIMUM_PHRASE_SCORE: f64 = 0.53;
const MINIMUM_SINGLE_TOKEN_SCORE: f64 = 0.53;
const MINIMUM_MULTIPLE_TOKEN_SCORE: f64 = 0.50;
const MINIMUM_TOKEN_COVERAGE: f64 = 0.64;
const MINIMUM_RECOGNIZED_TOKEN_SIMILARITY: f64 = 0.62;
const MINIMUM_COVERED_TOKEN_SIMILARITY: f64 = 0.64;
const DOCUMENT_FREQUENCY_SIMILARITY: f64 = 0.72;
const LONG_TOKEN_RARITY_WEIGHT_SCALE: f64 = 0.12;
const SHORT_ASCII_QUERY_LENGTH: usize = 2;
const MAXIMUM_ACRONYM_LENGTH: usize = 6;

/// `SettingsSearchScorer.Normalize`: NFKD, marks removed, lower-invariant letters and digits with a
/// few Latin letters folded (ß→ss, æ→ae, ð/đ→d, ø→o, þ→th, ł→l, œ→oe), other runs → one space.
pub fn normalize(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let decomposed = normalize_kd(text);
    let mut out = String::with_capacity(decomposed.len());
    let mut previous_was_separator = true;
    for c in decomposed.chars() {
        if is_mark(c) {
            continue;
        }
        if is_letter_or_digit(c) {
            match to_lower_invariant(c) {
                '\u{DF}' => out.push_str("ss"),
                '\u{E6}' => out.push_str("ae"),
                '\u{F0}' | '\u{111}' => out.push('d'),
                '\u{F8}' => out.push('o'),
                '\u{FE}' => out.push_str("th"),
                '\u{142}' => out.push('l'),
                '\u{153}' => out.push_str("oe"),
                lower => out.push(lower),
            }
            previous_was_separator = false;
            continue;
        }
        if previous_was_separator {
            continue;
        }
        out.push(' ');
        previous_was_separator = true;
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

struct SearchField {
    text: String,
    tokens: Vec<String>,
    synonym_group_ids: Vec<i32>,
    weight: f64,
}

impl SearchField {
    fn new(text: &str, weight: f64, synonyms: Option<&SynonymMap>) -> Self {
        let normalized = normalize(text);
        let tokens = tokenize(&normalized).into_iter().map(str::to_owned).collect();
        let synonym_group_ids = synonyms.map_or_else(Vec::new, |map| map.find_group_ids(&normalized));
        Self { text: normalized, tokens, synonym_group_ids, weight }
    }
}

struct PreparedDocument {
    index: usize,
    fields: Vec<SearchField>,
}

struct QueryToken {
    text: String,
    group: i32,
    weight: f64,
}

fn join_text(first: &str, second: &str) -> String {
    if first.trim().is_empty() {
        return second.to_owned();
    }
    if second.trim().is_empty() {
        return first.to_owned();
    }
    format!("{first}. {second}")
}

/// `SettingsSearchTextExtractor.JoinValues`: non-blank values joined with ". ".
fn join_values(values: &[String]) -> String {
    values.iter().filter(|v| !v.trim().is_empty()).map(String::as_str).collect::<Vec<_>>().join(". ")
}

impl PreparedDocument {
    /// A card document: title, keywords, body (description), subsection, page — in the C# field
    /// order. Returns None for a card without title and description text (not indexed).
    fn new(index: usize, entry: &SearchEntry, synonyms: Option<&SynonymMap>) -> Option<Self> {
        if join_text(&entry.title, &entry.description).trim().is_empty() {
            return None;
        }
        let keywords = join_values(&entry.keywords);
        let mut fields = Vec::with_capacity(5);
        for (text, weight) in [
            (entry.title.as_str(), TITLE_FIELD_WEIGHT),
            (keywords.as_str(), KEYWORD_FIELD_WEIGHT),
            (entry.description.as_str(), BODY_FIELD_WEIGHT),
            (entry.section.as_str(), SUBSECTION_FIELD_WEIGHT),
            (entry.page.as_str(), PAGE_FIELD_WEIGHT),
        ] {
            if !text.trim().is_empty() {
                fields.push(SearchField::new(text, weight, synonyms));
            }
        }
        Some(Self { index, fields })
    }
}

/// `SettingsSearchScorer.FindMatches`: indices of the matching entries, in entry order. Entries
/// with neither title nor description are never matched. `synonyms` is the parsed common + app
/// synonym groups (None disables synonym matching).
pub fn find_matches(query: &str, entries: &[SearchEntry], synonyms: Option<&SynonymMap>) -> Vec<usize> {
    let normalized_query = normalize(query);
    if normalized_query.is_empty() || entries.is_empty() {
        return Vec::new();
    }
    let documents: Vec<PreparedDocument> =
        entries.iter().enumerate().filter_map(|(index, entry)| PreparedDocument::new(index, entry, synonyms)).collect();
    if documents.is_empty() {
        return Vec::new();
    }
    let query_tokens = prepare_query_tokens(&normalized_query, &documents, synonyms);
    if query_tokens.is_empty() {
        return Vec::new();
    }
    documents
        .iter()
        .filter(|document| is_match(&normalized_query, &query_tokens, document))
        .map(|document| document.index)
        .collect()
}

fn prepare_query_tokens(
    normalized_query: &str,
    documents: &[PreparedDocument],
    synonyms: Option<&SynonymMap>,
) -> Vec<QueryToken> {
    let parts = match synonyms {
        Some(map) => map.parse_query(normalized_query),
        None => tokenize(normalized_query)
            .into_iter()
            .map(|token| QueryPart { text: token.to_owned(), group: NO_SYNONYM_GROUP })
            .collect(),
    };
    let mut seen_text: HashSet<String> = HashSet::new();
    let mut seen_groups: HashSet<i32> = HashSet::new();
    let distinct: Vec<QueryPart> = parts
        .into_iter()
        .filter(|part| {
            if part.group == NO_SYNONYM_GROUP {
                seen_text.insert(part.text.clone())
            } else {
                seen_groups.insert(part.group)
            }
        })
        .collect();

    let count = documents.len() as f64;
    let mut tokens = Vec::with_capacity(distinct.len());
    for part in distinct {
        let mut highest = 0.0f64;
        let mut document_frequency = 0usize;
        for document in documents {
            let similarity = best_raw_query_part_similarity(&part, document);
            highest = highest.max(similarity);
            if similarity >= DOCUMENT_FREQUENCY_SIMILARITY {
                document_frequency += 1;
            }
        }
        if highest < MINIMUM_RECOGNIZED_TOKEN_SIMILARITY {
            continue;
        }
        let maximum_rarity = (count + 1.0).ln();
        let normalized_rarity = if maximum_rarity <= 0.0 {
            0.0
        } else {
            ((count + 1.0) / (document_frequency as f64 + 1.0)).ln() / maximum_rarity
        };
        let weight = if part.group == NO_SYNONYM_GROUP {
            query_token_weight(&part.text, normalized_rarity)
        } else {
            1.0 + normalized_rarity * LONG_TOKEN_RARITY_WEIGHT_SCALE
        };
        tokens.push(QueryToken { text: part.text, group: part.group, weight });
    }
    tokens
}

fn is_match(normalized_query: &str, query_tokens: &[QueryToken], document: &PreparedDocument) -> bool {
    let phrase_score = document
        .fields
        .iter()
        .map(|field| phrase_similarity(normalized_query, &field.text) * field.weight)
        .fold(0.0f64, f64::max);
    if phrase_score >= MINIMUM_PHRASE_SCORE {
        return true;
    }

    let mut total_weight = 0.0;
    let mut covered_weight = 0.0;
    let mut weighted_total = 0.0;
    for token in query_tokens {
        let (raw, weighted) = best_document_token_similarity(token, document);
        total_weight += token.weight;
        weighted_total += weighted * token.weight;
        if raw >= MINIMUM_COVERED_TOKEN_SIMILARITY {
            covered_weight += token.weight;
        }
    }
    if total_weight <= 0.0 {
        return false;
    }
    let coverage = covered_weight / total_weight;
    let token_score = weighted_total / total_weight;
    if query_tokens.len() == 1 {
        return coverage >= 1.0 && token_score >= MINIMUM_SINGLE_TOKEN_SCORE;
    }
    coverage >= MINIMUM_TOKEN_COVERAGE && token_score >= MINIMUM_MULTIPLE_TOKEN_SCORE
}

fn best_raw_query_part_similarity(part: &QueryPart, document: &PreparedDocument) -> f64 {
    document.fields.iter().map(|field| search_field_similarity(&part.text, part.group, field)).fold(0.0, f64::max)
}

fn best_document_token_similarity(token: &QueryToken, document: &PreparedDocument) -> (f64, f64) {
    let mut best_raw = 0.0f64;
    let mut best_weighted = 0.0f64;
    for field in &document.fields {
        let raw = search_field_similarity(&token.text, token.group, field);
        best_raw = best_raw.max(raw);
        best_weighted = best_weighted.max(raw * field.weight);
    }
    (best_raw, best_weighted)
}

fn search_field_similarity(query_text: &str, group: i32, field: &SearchField) -> f64 {
    let synonym =
        if group != NO_SYNONYM_GROUP && field.synonym_group_ids.binary_search(&group).is_ok() { 1.0 } else { 0.0 };
    let lexical = if query_text.contains(' ') {
        lexical_similarity(query_text, &field.text)
    } else {
        best_token_similarity(query_text, &field.tokens).max(acronym_similarity(query_text, &field.tokens))
    };
    f64::max(synonym, lexical)
}

/// `SettingsSearchScorer.LexicalSimilarity` of two normalized texts (0..1).
pub fn lexical_similarity(normalized_query: &str, normalized_candidate: &str) -> f64 {
    if normalized_query.is_empty() || normalized_candidate.is_empty() {
        return 0.0;
    }
    if normalized_query == normalized_candidate {
        return 1.0;
    }
    let query_tokens = tokenize(normalized_query);
    let candidate_tokens: Vec<String> = tokenize(normalized_candidate).into_iter().map(str::to_owned).collect();
    if query_tokens.is_empty() || candidate_tokens.is_empty() {
        return 0.0;
    }
    let phrase = phrase_similarity(normalized_query, normalized_candidate);
    let total: f64 = query_tokens.iter().map(|token| best_token_similarity(token, &candidate_tokens)).sum();
    f64::max(phrase, total / query_tokens.len() as f64)
}

fn phrase_similarity(normalized_query: &str, normalized_candidate: &str) -> f64 {
    if normalized_query.is_empty() || normalized_candidate.is_empty() {
        return 0.0;
    }
    if normalized_query == normalized_candidate {
        return 1.0;
    }
    let query_length = normalized_query.chars().filter(|&c| c != ' ').count();
    let is_short_ascii_query = query_length <= SHORT_ASCII_QUERY_LENGTH && is_ascii(normalized_query);
    if normalized_candidate.starts_with(normalized_query) {
        return 0.98;
    }
    if !is_short_ascii_query && normalized_candidate.contains(normalized_query) {
        return 0.94;
    }
    0.0
}

fn best_token_similarity(query_token: &str, candidate_tokens: &[String]) -> f64 {
    candidate_tokens.iter().map(|candidate| token_similarity(query_token, candidate)).fold(0.0, f64::max)
}

fn token_similarity(query_token: &str, candidate_token: &str) -> f64 {
    if query_token == candidate_token {
        return 1.0;
    }
    let query_length = rune_length(query_token);
    let candidate_length = rune_length(candidate_token);
    let is_short_ascii_query = query_length <= SHORT_ASCII_QUERY_LENGTH && is_ascii(query_token);
    if candidate_token.starts_with(query_token) {
        return 0.95;
    }
    if is_short_ascii_query {
        return 0.0;
    }
    if candidate_token.contains(query_token) {
        return 0.88;
    }
    if query_token.starts_with(candidate_token) && candidate_length >= 3 {
        return 0.78;
    }
    let maximum_length = query_length.max(candidate_length);
    let maximum_distance = maximum_edit_distance(maximum_length);
    if maximum_distance > 0 && query_length.abs_diff(candidate_length) <= maximum_distance {
        let distance = damerau_levenshtein_distance(query_token, candidate_token, maximum_distance);
        if distance <= maximum_distance {
            let edit_similarity = 1.0 - distance as f64 / maximum_length as f64;
            if edit_similarity >= 0.64 {
                return edit_similarity * 0.94;
            }
        }
    }
    let n_gram = character_n_gram_dice(query_token, candidate_token);
    if n_gram >= 0.50 { 0.48 + n_gram * 0.45 } else { 0.0 }
}

fn acronym_similarity(query_token: &str, candidate_tokens: &[String]) -> f64 {
    let query: Vec<char> = query_token.chars().collect();
    if query.len() < 2 || query.len() > MAXIMUM_ACRONYM_LENGTH || candidate_tokens.len() < query.len() {
        return 0.0;
    }
    for start in 0..=candidate_tokens.len() - query.len() {
        let matched = query.iter().enumerate().all(|(offset, &c)| candidate_tokens[start + offset].starts_with(c));
        if matched {
            return 0.87;
        }
    }
    0.0
}

fn query_token_weight(token: &str, normalized_rarity: f64) -> f64 {
    if !is_ascii(token) {
        return 1.0 + normalized_rarity * LONG_TOKEN_RARITY_WEIGHT_SCALE;
    }
    match rune_length(token) {
        1 => 0.05 + normalized_rarity * 0.20,
        2 => 0.08 + normalized_rarity * 0.37,
        3 => 0.10 + normalized_rarity * 0.65,
        _ => 1.0 + normalized_rarity * LONG_TOKEN_RARITY_WEIGHT_SCALE,
    }
}

fn maximum_edit_distance(maximum_length: usize) -> usize {
    match maximum_length {
        0..=3 => 0,
        4..=5 => 1,
        6..=9 => 2,
        _ => 3,
    }
}

fn damerau_levenshtein_distance(left: &str, right: &str, maximum_distance: usize) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let mut previous_previous = vec![0usize; right.len() + 1];
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0usize; right.len() + 1];
    for li in 1..=left.len() {
        current[0] = li;
        let mut row_minimum = current[0];
        for ri in 1..=right.len() {
            let substitution = usize::from(left[li - 1] != right[ri - 1]);
            let mut distance = (current[ri - 1] + 1).min(previous[ri] + 1).min(previous[ri - 1] + substitution);
            if li > 1 && ri > 1 && left[li - 1] == right[ri - 2] && left[li - 2] == right[ri - 1] {
                distance = distance.min(previous_previous[ri - 2] + 1);
            }
            current[ri] = distance;
            row_minimum = row_minimum.min(distance);
        }
        if row_minimum > maximum_distance {
            return maximum_distance + 1;
        }
        std::mem::swap(&mut previous_previous, &mut previous);
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn character_n_gram_dice(left: &str, right: &str) -> f64 {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    let size = if left.len().min(right.len()) >= 5 { 3 } else { 2 };
    if left.len() < size || right.len() < size {
        return 0.0;
    }
    let grams = |runes: &[char]| -> HashSet<u64> {
        runes.windows(size).map(|w| w.iter().fold(0u64, |acc, &c| (acc << 21) | u64::from(c as u32))).collect()
    };
    let left = grams(&left);
    let right = grams(&right);
    let intersection = left.iter().filter(|g| right.contains(g)).count();
    2.0 * intersection as f64 / (left.len() + right.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Strings.resx` `SettingsWindow_SearchSynonymGroups_App` of VolumeTrayAppDotNET.
    const VOLUME_APP_SYNONYM_GROUPS: &str = r"maximum|max|highest|largest|greatest|upper limit|upper bound
minimum|lowest|smallest|lower limit|lower bound
threshold|cutoff|trigger level
playback device|playback devices|audio output device|audio output devices|output endpoint|output endpoints|render endpoint|render endpoints
recording device|recording devices|audio input device|audio input devices|capture endpoint|capture endpoints|input endpoint|input endpoints
default device|default audio device|default endpoint|primary audio device|primary audio endpoint
communications device|comms device|communications role device|communications role endpoint
disabled device|disabled devices|deactivated device|deactivated devices
disconnected device|disconnected devices|unplugged device|unplugged devices|disconnected endpoint|disconnected endpoints
not present device|not present devices|ghost device|ghost devices|phantom device|phantom devices|stale endpoint|stale endpoints
exclusive mode|exclusive control|application exclusive audio
equalizer|EQ
listen to this device|input monitoring|microphone monitoring
communications ducking|audio ducking|communications attenuation
volume change sound|volume change beep|volume feedback sound|volume feedback tone|audio feedback tone
logarithmic volume scale|logarithmic volume curve|perceptual volume curve|audio taper
default format|default audio format|audio endpoint format|Windows mix format
Bluetooth codec|A2DP codec|Bluetooth audio codec
peak meter|audio level meter|signal peak meter
unified peak meter|combined channel meter|single bar meter
peak meter refresh rate|meter redraw rate|meter frame rate|meter FPS
peak meter sample rate|meter polling rate|peak sampling frequency
peak meter change ceiling|meter slew limit|maximum change per redraw|single frame jump limit
low channel bias|quieter channel weighting|channel weighting multiplier
flyout|volume flyout|volume control flyout|tray volume flyout
undocked flyout|detached flyout|floating flyout
opens docked|stays anchored to the tray|attached to the tray
restore undock state|restore flyout state|remember flyout state|persist flyout state
keep undocked flyout on screen|clamp flyout to screen|constrain flyout to work area
flyout title bar|flyout header
app drawer|app drawers|application drawer|application drawers|volume mixer drawer|volume mixer drawers
app slider|app sliders|application volume slider|application volume sliders|mixer slider|mixer sliders|audio fader|audio faders
intermix|interleave|mix together
Bluetooth power button|Bluetooth power control|Bluetooth radio control|Bluetooth power toggle
actively capturing apps|capture activity indicator|microphone in use indicator|recording activity indicator
volume step|volume increment|volume change amount
device name max length|device name maximum length|device label character limit|truncate device name
modified actions|tray action mapping|mouse action mapping|gesture action assignment
centered|centred|center aligned|centre aligned|middle aligned
stack direction|flow direction|layout orientation
device sort|sort order|device ordering|endpoint ordering
tray icon|system tray icon|notification area icon
context menu|right click menu|tray menu
mouse wheel|scroll wheel
touchpad|trackpad
tooltip|tool tip|hover tip
slider thumb|slider handle|slider knob";

    fn doc(primary: &str, context: &str) -> SearchEntry {
        SearchEntry { title: primary.to_owned(), section: context.to_owned(), ..SearchEntry::default() }
    }

    fn full(title: &str, body: &str, keywords: &str, section: &str, page: &str) -> SearchEntry {
        SearchEntry {
            title: title.to_owned(),
            description: body.to_owned(),
            keywords: vec![keywords.to_owned()],
            section: section.to_owned(),
            page: page.to_owned(),
        }
    }

    fn matches(query: &str, entries: &[SearchEntry], synonyms: Option<&SynonymMap>) -> Vec<usize> {
        find_matches(query, entries, synonyms)
    }

    #[test]
    fn common_synonym_resource_is_present_and_parseable() {
        for line in COMMON_SYNONYM_GROUPS.lines().map(str::trim).filter(|l| !l.is_empty()) {
            assert!(line.split('|').filter(|t| !t.trim().is_empty()).count() >= 2, "{line}");
        }
        let map = SynonymMap::parse(&[COMMON_SYNONYM_GROUPS]);
        assert_eq!(matches("highest", &[doc("Maximum rows before scrolling", "App drawer")], Some(&map)), [0]);
    }

    #[test]
    fn finds_misspelled_card_title() {
        let entries = [doc("Brightness update rate", "Monitors"), doc("Update check interval", "About")];
        assert_eq!(matches("brighness", &entries, None), [0]);
    }

    #[test]
    fn finds_adjacent_transposition() {
        let entries = [doc("Theme mode", "Appearance"), doc("Update interval", "About")];
        assert_eq!(matches("tehme", &entries, None), [0]);
    }

    #[test]
    fn uses_character_n_grams_for_heavily_misspelled_terms() {
        let entries = [doc("Environmental controls", "Brightness"), doc("Update interval", "About")];
        assert_eq!(matches("environxxxxmental", &entries, None), [0]);
    }

    #[test]
    fn uses_page_and_subsection_context() {
        let entries =
            [doc("Context menu font size", "Theme. Appearance"), doc("Update check interval", "About. Updates")];
        assert_eq!(matches("theme", &entries, None), [0]);
    }

    #[test]
    fn uses_localized_search_keywords_for_conceptual_matches() {
        let entries = [
            full(
                "Run on startup",
                "Start the app automatically.",
                "launch login sign in boot autostart",
                "Startup",
                "General",
            ),
            full("Card spacing", "Vertical distance between cards.", "layout gap margin", "Layout", "Flyout"),
        ];
        assert_eq!(matches("launch when I sign in", &entries, None), [0]);
    }

    #[test]
    fn handles_multiword_synonym_phrases() {
        let entries = [doc("Maximum rows before scrolling", "App drawer"), doc("Scroll speed", "Flyout")];
        let map = SynonymMap::parse(&["maximum|max|highest|upper limit|upper bound"]);
        assert_eq!(matches("upper limit", &entries, Some(&map)), [0]);
    }

    #[test]
    fn does_not_expose_words_inside_qualified_synonyms() {
        let entries = [doc("Network adapter", "Network"), doc("Card spacing", "Layout")];
        let map = SynonymMap::parse(&["network interface|network adapter|NIC|network card|connection adapter"]);
        assert_eq!(matches("card", &entries, Some(&map)), [1]);
    }

    #[test]
    fn preserves_lexical_fallback_for_unanchored_synonym_groups() {
        let entries = [full(
            "Installation",
            "Manage the installed application.",
            "setup install uninstall remove application program files",
            "General",
            "General",
        )];
        let map = SynonymMap::parse(&["install application|set up application|add application"]);
        assert_eq!(matches("set up application", &entries, Some(&map)), [0]);
    }

    #[test]
    fn merges_extensions_of_the_same_synonym_group() {
        let map = SynonymMap::parse(&["maximum|max|highest|upper limit", "maximum|highest|largest|greatest"]);
        assert_eq!(matches("largest", &[doc("Max apps per row", "App drawer")], Some(&map)), [0]);
    }

    #[test]
    fn does_not_bridge_groups_through_one_ambiguous_phrase() {
        let map = SynonymMap::parse(&[
            "autostart|run on startup|launch at login",
            "fan jumpstart|run on startup|spin-up boost",
        ]);
        assert!(matches("autostart", &[doc("Fan jumpstart", "Fan control")], Some(&map)).is_empty());
    }

    #[test]
    fn allows_one_card_to_anchor_multiple_concept_groups() {
        let entries = [doc("Battery below 10%", "Battery trigger")];
        let map =
            SynonymMap::parse(&["battery below|battery under|charge below", "battery below|low battery|low charge"]);
        assert_eq!(matches("battery under", &entries, Some(&map)), [0]);
        assert_eq!(matches("low charge", &entries, Some(&map)), [0]);
    }

    #[test]
    fn does_not_expand_synonyms_inside_ascii_words() {
        let map = SynonymMap::parse(&["max|highest"]);
        assert!(matches("highest", &[doc("Maximize the window", "Window")], Some(&map)).is_empty());
    }

    #[test]
    fn expands_localized_synonyms_inside_unsegmented_text() {
        let entries = [doc("最高音量", "音频"), doc("主题颜色", "外观")];
        let map = SynonymMap::parse(&["最高|最大"]);
        assert_eq!(matches("最大", &entries, Some(&map)), [0]);
    }

    #[test]
    fn restricts_short_queries_to_token_prefixes() {
        let entries = [doc("Volume", "General"), doc("Theme", "Appearance")];
        assert_eq!(matches("v", &entries, None), [0]);
    }

    #[test]
    fn ignores_unrecognized_natural_language_filler() {
        let entries = [doc("Use dark theme", "Appearance"), doc("Check for updates", "About")];
        assert_eq!(matches("please make the interface dark", &entries, None), [0]);
    }

    #[test]
    fn downweights_short_incidental_filler_matches() {
        let entries = [doc("Master volume", "Audio"), doc("Theme mode", "Appearance")];
        assert_eq!(matches("set the volume", &entries, None), [0]);
    }

    #[test]
    fn requires_coverage_for_multiple_meaningful_terms() {
        let entries = [doc("Theme mode", "Appearance"), doc("Update interval", "About")];
        assert!(matches("theme interval", &entries, None).is_empty());
    }

    #[test]
    fn normalizes_diacritics() {
        let entries = [doc("Luminosité de l'écran", "Affichage"), doc("Volume principal", "Audio")];
        assert_eq!(matches("luminosite", &entries, None), [0]);
    }

    #[test]
    fn folds_common_latin_letters() {
        let entries = [doc("Größe der Menüstraße", "Darstellung"), doc("Lautstärke", "Audio")];
        assert_eq!(matches("menustrasse", &entries, None), [0]);
    }

    #[test]
    fn uses_substrings_for_unsegmented_scripts() {
        let entries = [doc("自动调节屏幕亮度", "显示"), doc("主音量", "音频")];
        assert_eq!(matches("亮度", &entries, None), [0]);
    }

    #[test]
    fn recognizes_acronyms() {
        let entries = [doc("Graphics processing unit rendering", "Rendering"), doc("Software rendering", "Rendering")];
        assert_eq!(matches("gpu", &entries, None), [0]);
    }

    #[test]
    fn indexes_explicit_card_keywords_separately() {
        let entries = [SearchEntry {
            title: "Run on startup".into(),
            keywords: vec!["launch login boot".into()],
            page: "General".into(),
            ..SearchEntry::default()
        }];
        assert_eq!(matches("boot", &entries, None), [0]);
    }

    #[test]
    fn uses_volume_synonyms_for_conceptual_matches() {
        let entries = [doc("Max apps per row", "App drawer"), doc("Card spacing", "Layout")];
        let map = SynonymMap::parse(&[COMMON_SYNONYM_GROUPS, VOLUME_APP_SYNONYM_GROUPS]);
        assert_eq!(matches("highest", &entries, Some(&map)), [0]);
    }

    #[test]
    fn rank_keeps_entry_order_and_ignores_blank_queries_and_cards() {
        let entries = [
            doc("Theme mode", "Appearance"),
            SearchEntry { section: "Theme".into(), ..SearchEntry::default() },
            doc("Dark theme", "Appearance"),
        ];
        assert_eq!(rank("theme", &entries), [0, 2]);
        assert!(rank("   ", &entries).is_empty());
        assert!(rank("theme", &[]).is_empty());
    }

    #[test]
    fn normalize_matches_scorer_rules() {
        assert_eq!(normalize("  Hello,  World!! "), "hello world");
        assert_eq!(normalize("Ｆｕｌｌｗｉｄｔｈ ①"), "fullwidth 1");
        assert_eq!(normalize("Æsir Øre Þor Łódź Œuvre"), "aesir ore thor lodz oeuvre");
        assert_eq!(normalize("   "), "");
    }

    #[test]
    fn search_matcher_scores_words_out_of_middle_and_subsequences() {
        assert!(matcher_score("AMD Ryzen 9950X CCD0", "ccd").is_some());
        assert!(matcher_score("CPU Package Temperature", "cpt").is_some());
        assert_eq!(matcher_score("anything", "  "), Some(0));
        assert_eq!(matcher_score("", "x"), None);
    }

    #[test]
    fn filter_and_rank_prefers_prefix_over_contains() {
        let items = ["Package Temperature", "Temperature Package", "Temp"];
        let ranked = filter_and_rank(&items, "temp", |s| s);
        assert_eq!(
            ranked.iter().map(|&i| items[i]).collect::<Vec<_>>(),
            ["Temp", "Temperature Package", "Package Temperature"]
        );
    }

    #[test]
    fn filter_and_rank_requires_every_token() {
        let items = ["CPU Package Temperature", "GPU Package Temperature", "CPU Package Power"];
        assert_eq!(filter_and_rank(&items, "cpu temp", |s| s), [0]);
        assert_eq!(filter_and_rank(&items, "", |s| s), [0, 1, 2]);
    }
}

//! Stages 3 and 4: block roles from document-wide font statistics, the heading hierarchy
//! (`section_path`), repeated page furniture and the reference list.

use std::collections::{HashMap, HashSet};

use crate::chars::join_lines;
use crate::layout::Block;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Body,
    Heading,
    Caption,
    Reference,
    Title,
    FrontMatter,
}

impl Role {
    /// The `kind` stored on `document_blocks`.
    pub fn kind(self) -> &'static str {
        match self {
            Self::Body => "text",
            Self::Heading => "heading",
            Self::Caption => "caption",
            Self::Reference => "reference",
            Self::Title => "title",
            Self::FrontMatter => "front_matter",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PageBlocks {
    pub page_number: u32,
    pub height: f32,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StructuredBlock {
    /// Index into the page list given to [`analyze_structure`].
    pub page_index: usize,
    pub block: Block,
    pub role: Role,
    pub section_path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reference {
    pub page_number: u32,
    pub raw_text: String,
    pub doi: Option<String>,
    pub year: Option<u16>,
}

#[derive(Debug, Clone, Default)]
pub struct Structure {
    pub blocks: Vec<StructuredBlock>,
    pub references: Vec<Reference>,
}

/// Modal font size (rounded to 0.5pt), weighted by glyph count.
pub fn body_size(pages: &[PageBlocks]) -> f32 {
    let mut histogram: HashMap<i32, usize> = HashMap::new();
    for block in pages.iter().flat_map(|p| &p.blocks) {
        for line in &block.lines {
            *histogram
                .entry((line.size * 2.0).round() as i32)
                .or_default() += line.glyphs;
        }
    }
    histogram
        .into_iter()
        .max_by_key(|(size, count)| (*count, *size))
        .map_or(10.0, |(size, _)| size as f32 / 2.0)
        .max(4.0)
}

// ---------------------------------------------------------------------------------------
// Page furniture
// ---------------------------------------------------------------------------------------

fn furniture_key(text: &str) -> String {
    let mut key = String::new();
    let mut previous_space = true;
    for c in text.chars() {
        let mapped = if c.is_ascii_digit() {
            '#'
        } else {
            c.to_ascii_lowercase()
        };
        if mapped.is_whitespace() {
            if !previous_space {
                key.push(' ');
            }
            previous_space = true;
        } else {
            if !(mapped == '#' && key.ends_with('#')) {
                key.push(mapped);
            }
            previous_space = false;
        }
    }
    key.trim().to_owned()
}

fn is_page_number_key(key: &str) -> bool {
    let stripped: String = key.chars().filter(|c| !c.is_whitespace()).collect();
    matches!(
        stripped.as_str(),
        "#" | "-#-" | "#/#" | "(#)" | "[#]" | "page#" | "page#of#" | "p.#" | "#of#" | "–#–"
    ) || (stripped.starts_with("page") && stripped.chars().all(|c| c.is_alphabetic() || c == '#'))
}

/// Returns `(page_index, block_index)` pairs that are running headers/footers or page numbers.
pub fn detect_furniture(pages: &[PageBlocks]) -> HashSet<(usize, usize)> {
    let mut furniture = HashSet::new();
    let page_total = pages.len();
    // key -> list of (page_index, block_index, normalized y)
    let mut occurrences: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
    for (page_index, page) in pages.iter().enumerate() {
        for (block_index, block) in page.blocks.iter().enumerate() {
            let from_top = (page.height - block.top) / page.height;
            let to_bottom = block.bottom / page.height;
            let in_band = from_top < 0.09 || to_bottom < 0.09;
            if !in_band || block.text.chars().count() > 200 || block.lines.len() > 3 {
                continue;
            }
            let key = furniture_key(&block.text);
            if key.is_empty() {
                continue;
            }
            if is_page_number_key(&key) {
                furniture.insert((page_index, block_index));
                continue;
            }
            occurrences
                .entry(key)
                .or_default()
                .push((page_index, block_index));
        }
    }
    for (_, list) in occurrences {
        let distinct_pages: HashSet<usize> = list.iter().map(|(p, _)| *p).collect();
        let all = distinct_pages.len();
        let odd = distinct_pages.iter().filter(|p| **p % 2 == 0).count();
        let even = all - odd;
        let odd_total = page_total.div_ceil(2);
        let even_total = page_total / 2;
        let repeated = (page_total >= 3 && all * 2 >= page_total)
            || (page_total >= 4 && odd_total >= 2 && odd >= 2 && odd * 2 >= odd_total)
            || (page_total >= 4 && even_total >= 2 && even >= 2 && even * 2 >= even_total);
        if repeated {
            furniture.extend(list);
        }
    }
    furniture
}

// ---------------------------------------------------------------------------------------
// Headings, captions, references
// ---------------------------------------------------------------------------------------

const SECTION_NAMES: &[&str] = &[
    "abstract",
    "summary",
    "introduction",
    "background",
    "methods",
    "method",
    "materials and methods",
    "methodology",
    "patients and methods",
    "results",
    "findings",
    "discussion",
    "results and discussion",
    "conclusion",
    "conclusions",
    "limitations",
    "keywords",
    "key words",
    "acknowledgements",
    "acknowledgments",
    "funding",
    "references",
    "bibliography",
    "appendix",
    "resumo",
    "introdução",
    "métodos",
    "resultados",
    "discussão",
    "conclusão",
    "referências",
];

const REFERENCE_HEADINGS: &[&str] = &[
    "references",
    "reference list",
    "bibliography",
    "literature cited",
    "works cited",
    "referências",
    "referencias",
    "referências bibliográficas",
    "bibliografia",
];

const POST_REFERENCE_HEADINGS: &[&str] = &[
    "appendix",
    "appendices",
    "supplement",
    "supplementary",
    "acknowledg",
    "author contributions",
    "funding",
    "conflict",
    "declaration",
    "data availability",
    "ethics",
    "apêndice",
    "anexo",
];

/// Splits a leading section number ("2.1", "3.", "IV.") from the title.
pub fn split_numbering(text: &str) -> Option<(usize, &str)> {
    let trimmed = text.trim_start();
    let number_end = trimmed
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.'))
        .map_or(trimmed.len(), |(index, _)| index);
    let (number, rest) = trimmed.split_at(number_end);
    let number = number.trim_end_matches('.');
    if number.is_empty() || !number.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let segments: Vec<&str> = number.split('.').collect();
    if segments.len() > 4 || segments.iter().any(|s| s.is_empty() || s.len() > 2) {
        return None;
    }
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let title = rest.trim();
    title
        .starts_with(char::is_uppercase)
        .then_some((segments.len(), title))
}

fn normalized_title(text: &str) -> String {
    text.trim()
        .trim_end_matches([':', '.'])
        .trim()
        .to_lowercase()
}

fn is_known_section(text: &str) -> bool {
    let normalized = normalized_title(text);
    if normalized.chars().count() > 40 {
        return false;
    }
    let stripped = split_numbering(text)
        .map(|(_, title)| normalized_title(title))
        .unwrap_or(normalized);
    SECTION_NAMES.contains(&stripped.as_str())
}

/// "Background and Purpose—...", "Objective: ..." openings of a structured abstract.
fn is_abstract_run_in(text: &str) -> bool {
    let lower = text.trim_start().to_lowercase();
    [
        "background",
        "objective",
        "purpose",
        "importance",
        "context",
        "aims",
        "aim",
    ]
    .iter()
    .any(|label| {
        lower.strip_prefix(label).is_some_and(|rest| {
            let head: String = rest.chars().take(30).collect();
            head.contains(['—', '–', ':'])
                && !head
                    .chars()
                    .take_while(|c| !matches!(c, '—' | '–' | ':'))
                    .any(|c| c == '.')
        })
    })
}

pub fn is_caption(text: &str) -> bool {
    let trimmed = text.trim_start();
    let lower = trimmed.to_lowercase();
    let mut rest = lower.as_str();
    let mut original = trimmed;
    for prefix in ["supplementary ", "supplemental ", "extended data "] {
        if let Some(r) = rest.strip_prefix(prefix) {
            let skip = rest.len() - r.len();
            rest = r;
            original = original.get(skip..).unwrap_or(original);
        }
    }
    let labels = [
        "figure", "fig.", "fig ", "table", "chart", "box", "scheme", "plate", "figura", "tabela",
        "quadro", "gráfico",
    ];
    let Some(label) = labels.iter().find(|l| rest.starts_with(**l)) else {
        return false;
    };
    let after = original.get(label.len()..).unwrap_or("").trim_start();
    let digits_end = after
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric()))
        .map_or(after.len(), |(index, _)| index);
    let (number, tail) = after.split_at(digits_end);
    if number.is_empty() || number.chars().count() > 3 {
        return false;
    }
    if !number
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == 'S')
    {
        return false;
    }
    let tail = tail.trim_start();
    if let Some(first) = tail.chars().next() {
        if matches!(first, '.' | ':' | '|' | '-' | '–' | '—') {
            return true;
        }
        // "Figure 3 Overview of ..." (capitalised) but not "Figure 3 shows ...".
        return first.is_uppercase();
    }
    true
}

#[derive(Debug, Clone)]
struct HeadingCandidate {
    depth: Option<usize>,
    title: String,
}

/// A heading that shares its block with the text below it is taken from its first line when that
/// line is a known section ("Results" above "General Demographics" is not one heading).
fn heading_text(block: &Block) -> &str {
    match block.lines.as_slice() {
        [first, _, ..] if is_known_section(first.text.trim()) => first.text.trim(),
        _ => block.text.trim(),
    }
}

fn heading_candidate(block: &Block, body: f32) -> Option<HeadingCandidate> {
    let text = heading_text(block);
    let chars = text.chars().count();
    if !(3..=160).contains(&chars) || block.lines.len() > 3 {
        return None;
    }
    let alphabetic = text.chars().filter(|c| c.is_alphabetic()).count();
    if alphabetic * 2 < chars {
        return None;
    }
    if text.ends_with([',', ';']) || is_caption(text) {
        return None;
    }
    let numbering = split_numbering(text);
    let larger = block.size >= body * 1.12;
    let bold_short =
        block.bold && block.size >= body * 0.95 && block.lines.len() == 1 && chars <= 100;
    let known = block.lines.len() == 1 && is_known_section(text);
    let numbered_short = numbering.is_some()
        && block.size >= body * 0.95
        && chars <= 110
        && text.split_whitespace().count() <= 14
        && !text.ends_with('.');
    if !(larger || bold_short || known || numbered_short) {
        return None;
    }
    if !numbered_short && !known && text.ends_with('.') && chars > 60 {
        return None;
    }
    if !text.starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit()) {
        return None;
    }
    let (depth, title) = match numbering {
        Some((depth, title)) if numbered_short || known || larger || bold_short => {
            (Some(depth), title)
        }
        _ => (None, text),
    };
    let title = title.trim().trim_end_matches([':', '.']).trim().to_owned();
    Some(HeadingCandidate { depth, title })
}

fn matches_any(title: &str, list: &[&str]) -> bool {
    let normalized = normalized_title(title);
    list.iter().any(|entry| normalized.starts_with(entry))
}

// ---------------------------------------------------------------------------------------
// Orchestration
// ---------------------------------------------------------------------------------------

pub fn analyze_structure(pages: Vec<PageBlocks>) -> Structure {
    let body = body_size(&pages);
    let furniture = detect_furniture(&pages);

    // Flatten, dropping furniture.
    struct Item {
        page_index: usize,
        block: Block,
    }
    let mut items: Vec<Item> = Vec::new();
    let mut page_numbers = Vec::new();
    for (page_index, page) in pages.into_iter().enumerate() {
        page_numbers.push(page.page_number);
        for (block_index, block) in page.blocks.into_iter().enumerate() {
            if !furniture.contains(&(page_index, block_index)) {
                items.push(Item { page_index, block });
            }
        }
    }

    // A caption label on its own ("TABLE 1.") belongs to the block right below it.
    let mut merged: Vec<Item> = Vec::with_capacity(items.len());
    let mut pending: Option<Item> = None;
    for item in items {
        match pending.take() {
            Some(mut label) if label.page_index == item.page_index => {
                label.block.text =
                    format!("{} {}", label.block.text.trim(), item.block.text.trim());
                label.block.bottom = label.block.bottom.min(item.block.bottom);
                label.block.left = label.block.left.min(item.block.left);
                label.block.right = label.block.right.max(item.block.right);
                label.block.top = label.block.top.max(item.block.top);
                label.block.lines.extend(item.block.lines);
                merged.push(label);
            }
            other => {
                merged.extend(other);
                if item.block.text.chars().count() <= 12 && is_caption(&item.block.text) {
                    pending = Some(item);
                } else {
                    merged.push(item);
                }
            }
        }
    }
    merged.extend(pending);
    let items = merged;

    // Title: the largest block on page 1 clearly bigger than the body.
    let title_index = items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.page_index == 0 && item.block.size >= body * 1.25)
        .filter(|(_, item)| !is_caption(&item.block.text))
        .max_by(|(ia, a), (ib, b)| a.block.size.total_cmp(&b.block.size).then(ib.cmp(ia)))
        .map(|(index, _)| index);

    // Page-1 front matter (authors, affiliations) can be bold or large without being a
    // heading: nothing before the first anchor ("Abstract", "1 Introduction", a structured
    // abstract run-in) is a section heading.
    let anchor = items.iter().position(|item| {
        item.page_index == 0
            && (is_known_section(&item.block.text)
                || normalized_title(&item.block.text).starts_with("abstract")
                || is_abstract_run_in(&item.block.text)
                || (item.block.lines.len() <= 2 && split_numbering(&item.block.text).is_some()))
    });
    let candidates: Vec<Option<HeadingCandidate>> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let in_front = anchor.is_some_and(|anchor| index < anchor && item.page_index == 0);
            if Some(index) == title_index || in_front {
                None
            } else {
                heading_candidate(&item.block, body)
            }
        })
        .collect();

    // Rank heading sizes for unnumbered headings (larger = higher level).
    let mut sizes: Vec<i32> = candidates
        .iter()
        .zip(&items)
        .filter(|(c, _)| c.as_ref().is_some_and(|c| c.depth.is_none()))
        .map(|(_, item)| (item.block.size * 2.0).round() as i32)
        .collect();
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes.dedup();
    // A bold, body-sized heading and a body-sized plain known section share the lowest rank.
    let numbered_exists = candidates
        .iter()
        .any(|c| c.as_ref().is_some_and(|c| c.depth.is_some()));

    // First heading (or "Abstract" run-in) ends the front matter on page 1.
    let front_end = anchor.unwrap_or_else(|| {
        items
            .iter()
            .enumerate()
            .position(|(index, _)| candidates[index].is_some())
            .unwrap_or(0)
    });

    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut in_references = false;
    let mut reference_depth = 1usize;
    let mut structured = Vec::with_capacity(items.len());
    let mut reference_lines: Vec<RefLine> = Vec::new();

    for (index, item) in items.into_iter().enumerate() {
        let mut role = Role::Body;
        let mut section_path: Vec<String> = Vec::new();
        if Some(index) == title_index {
            role = Role::Title;
        } else if let Some(candidate) = &candidates[index] {
            let rank = candidate.depth.unwrap_or_else(|| {
                let size = (item.block.size * 2.0).round() as i32;
                let rank = sizes.iter().position(|s| *s == size).unwrap_or(0) + 1;
                if numbered_exists { 1 } else { rank }
            });
            let stops_references = in_references
                && (matches_any(&candidate.title, POST_REFERENCE_HEADINGS)
                    || (rank <= reference_depth
                        && !matches_any(&candidate.title, REFERENCE_HEADINGS)
                        && item.block.size > body * 1.05));
            if !in_references || stops_references {
                in_references = false;
                role = Role::Heading;
                while stack.last().is_some_and(|(level, _)| *level >= rank) {
                    stack.pop();
                }
                stack.push((rank, candidate.title.clone()));
                if matches_any(&candidate.title, REFERENCE_HEADINGS) {
                    in_references = true;
                    reference_depth = rank;
                }
            }
        }
        if role == Role::Body {
            if in_references {
                role = Role::Reference;
            } else if is_caption(&item.block.text) && item.block.size <= body * 1.05 {
                role = Role::Caption;
            } else if item.page_index == 0 && index < front_end && stack.is_empty() {
                role = Role::FrontMatter;
            }
        }
        if !matches!(role, Role::Title | Role::FrontMatter) {
            section_path = stack.iter().map(|(_, title)| title.clone()).collect();
        }
        if role == Role::Reference {
            // Numbering lives in its own column in some layouts: the marker alone is no evidence.
            if item
                .block
                .text
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_punctuation())
            {
                continue;
            }
            collect_reference_lines(
                &item.block,
                page_numbers[item.page_index],
                &mut reference_lines,
            );
        }
        structured.push(StructuredBlock {
            page_index: item.page_index,
            block: item.block,
            role,
            section_path,
        });
    }

    Structure {
        references: split_references(&reference_lines, body),
        blocks: structured,
    }
}

// ---------------------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RefLine {
    text: String,
    page_number: u32,
    first_in_block: bool,
    hanging: bool,
}

fn collect_reference_lines(block: &Block, page_number: u32, out: &mut Vec<RefLine>) {
    for (index, line) in block.lines.iter().enumerate() {
        out.push(RefLine {
            text: line.text.clone(),
            page_number,
            first_in_block: index == 0,
            hanging: line.left - block.left > 0.3 * line.size,
        });
    }
}

/// "[12] ", "12. ", "12) ", "(12) " at the start of a reference line.
fn numeric_marker(text: &str) -> bool {
    let t = text.trim_start();
    let t = t.strip_prefix(['[', '(']).unwrap_or(t);
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 || digits > 3 {
        return false;
    }
    let after: String = t.chars().skip(digits).take(2).collect();
    after.starts_with([']', ')', '.']) && !after.starts_with("..")
}

/// "Smith J", "Smith, J.", "van der Berg A" style opening of an entry.
fn author_start(text: &str) -> bool {
    let mut words = text.split_whitespace();
    let (Some(first), Some(second)) = (words.next(), words.next()) else {
        return false;
    };
    let surname = first.trim_end_matches(',');
    surname.starts_with(char::is_uppercase)
        && surname.chars().count() >= 2
        && (second.trim_end_matches([',', '.']).chars().count() <= 3
            && second.starts_with(char::is_uppercase)
            || first.ends_with(','))
}

fn ends_entry(text: &str) -> bool {
    text.trim_end()
        .chars()
        .last()
        .is_some_and(|c| matches!(c, '.' | ')' | ']') || c.is_ascii_digit())
}

fn split_references(lines: &[RefLine], _body: f32) -> Vec<Reference> {
    let mut entries: Vec<(u32, Vec<String>)> = Vec::new();
    for line in lines {
        let text = line.text.trim();
        if text.is_empty() {
            continue;
        }
        let previous_complete = entries
            .last()
            .and_then(|(_, parts)| parts.last())
            .is_some_and(|last| ends_entry(last));
        let starts_new = match entries.last() {
            None => true,
            Some(_) => {
                numeric_marker(text)
                    || (!line.hanging
                        && previous_complete
                        && (author_start(text) || line.first_in_block))
            }
        };
        if starts_new {
            entries.push((line.page_number, vec![text.to_owned()]));
        } else if let Some((_, parts)) = entries.last_mut() {
            parts.push(text.to_owned());
        }
    }
    entries
        .into_iter()
        .filter_map(|(page_number, parts)| {
            let raw = join_lines(&parts);
            let raw = strip_numeric_marker(&raw);
            if raw.chars().count() < 15 {
                return None;
            }
            Some(Reference {
                page_number,
                doi: extract_doi(&raw),
                year: extract_year(&raw),
                raw_text: raw,
            })
        })
        .collect()
}

fn strip_numeric_marker(text: &str) -> String {
    if numeric_marker(text) {
        let t = text.trim_start();
        let t = t.strip_prefix(['[', '(']).unwrap_or(t);
        let rest = t.trim_start_matches(|c: char| c.is_ascii_digit());
        return rest
            .trim_start_matches([']', ')', '.'])
            .trim_start()
            .to_owned();
    }
    text.to_owned()
}

/// Finds a DOI ("10.xxxx/suffix") and trims trailing punctuation.
pub fn extract_doi(text: &str) -> Option<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index + 7 < bytes.len() {
        if bytes[index] == '1'
            && bytes[index + 1] == '0'
            && bytes[index + 2] == '.'
            && (index == 0 || !bytes[index - 1].is_alphanumeric())
        {
            let digits = bytes[index + 3..]
                .iter()
                .take_while(|c| c.is_ascii_digit())
                .count();
            let slash = index + 3 + digits;
            if (4..=9).contains(&digits) && bytes.get(slash) == Some(&'/') {
                let suffix: String = bytes[slash + 1..]
                    .iter()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '<' | '>' | '’'))
                    .collect();
                let suffix = suffix.trim_end_matches(['.', ',', ';', ')', ']', ':']);
                if !suffix.is_empty() {
                    let prefix: String = bytes[index..=slash].iter().collect();
                    return Some(format!("{prefix}{suffix}").to_lowercase());
                }
            }
        }
        index += 1;
    }
    None
}

fn extract_year(text: &str) -> Option<u16> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = None;
    for window in chars.windows(4) {
        if window.iter().all(char::is_ascii_digit) {
            let year: String = window.iter().collect();
            if let Ok(value) = year.parse::<u16>()
                && (1900..=2100).contains(&value)
            {
                found = found.or(Some(value));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chars::{build_words, testing::run};
    use crate::layout::build_blocks;

    #[allow(clippy::too_many_arguments)]
    fn page(
        number: u32,
        lines: &[(&str, f32, f32, bool)], // text, baseline, size, bold
        header: Option<&str>,
        footer: Option<&str>,
    ) -> PageBlocks {
        let mut chars = Vec::new();
        for (text, baseline, size, bold) in lines {
            chars.extend(run(text, 72.0, *baseline, *size, *bold, true));
        }
        if let Some(header) = header {
            chars.extend(run(header, 72.0, 770.0, 8.0, false, true));
        }
        if let Some(footer) = footer {
            chars.extend(run(footer, 300.0, 30.0, 9.0, false, true));
        }
        PageBlocks {
            page_number: number,
            height: 792.0,
            blocks: build_blocks(&build_words(&chars)),
        }
    }

    fn body_lines(
        start: f32,
        count: usize,
        text: &'static str,
    ) -> Vec<(&'static str, f32, f32, bool)> {
        (0..count)
            .map(|i| (text, start - 12.0 * i as f32, 10.0, false))
            .collect()
    }

    #[test]
    fn numbering_is_split() {
        assert_eq!(
            split_numbering("2.1 Study Design"),
            Some((2, "Study Design"))
        );
        assert_eq!(split_numbering("3. Results"), Some((1, "Results")));
        assert_eq!(split_numbering("2020 was a year"), None);
        assert_eq!(split_numbering("12 patients were"), None);
    }

    #[test]
    fn captions_recognised() {
        assert!(is_caption("Figure 3. Flow of participants"));
        assert!(is_caption("Table 2: Baseline characteristics"));
        assert!(is_caption("Fig. 1 Overview of the method"));
        assert!(!is_caption("Figure 3 shows that the results hold"));
        assert!(!is_caption("Tables are useful"));
    }

    #[test]
    fn doi_extraction() {
        assert_eq!(
            extract_doi("Smith J. Title. Lancet. 2020;1:2. doi:10.1016/S0140-6736(20)30183-5."),
            Some("10.1016/s0140-6736(20)30183-5".to_owned())
        );
        assert_eq!(
            extract_doi("https://doi.org/10.1000/xyz123, next"),
            Some("10.1000/xyz123".to_owned())
        );
        assert_eq!(extract_doi("no identifier 10.5 here"), None);
    }

    #[test]
    fn repeated_header_and_page_numbers_are_removed() {
        let pages: Vec<PageBlocks> = (1..=4)
            .map(|n| {
                page(
                    n,
                    &body_lines(700.0, 3, "ordinary body text goes here for the page"),
                    Some("Journal of Testing 2020"),
                    Some(&format!("{n}")),
                )
            })
            .collect();
        let structure = analyze_structure(pages);
        assert!(
            structure
                .blocks
                .iter()
                .all(|b| !b.block.text.contains("Journal"))
        );
        assert!(
            structure
                .blocks
                .iter()
                .all(|b| b.block.text != "1" && b.block.text != "3")
        );
        assert_eq!(structure.blocks.len(), 4);
    }

    #[test]
    fn numbered_headings_build_section_path() {
        let mut lines: Vec<(&str, f32, f32, bool)> = vec![
            ("1 Introduction", 700.0, 12.0, true),
            (
                "intro paragraph text goes on and on here",
                680.0,
                10.0,
                false,
            ),
            (
                "intro paragraph text goes on and on here",
                668.0,
                10.0,
                false,
            ),
            ("2 Methods", 640.0, 12.0, true),
            (
                "method paragraph text goes on and on here",
                620.0,
                10.0,
                false,
            ),
            ("2.1 Participants", 590.0, 10.0, true),
            (
                "participants paragraph text goes on here",
                570.0,
                10.0,
                false,
            ),
            (
                "participants paragraph text goes on here",
                558.0,
                10.0,
                false,
            ),
            ("3 Results", 530.0, 12.0, true),
            (
                "results paragraph text goes on and on here",
                510.0,
                10.0,
                false,
            ),
        ];
        lines.extend(body_lines(
            490.0,
            8,
            "more filler body text so that the body size dominates",
        ));
        let structure = analyze_structure(vec![page(2, &lines, None, None)]);
        let find = |needle: &str| {
            structure
                .blocks
                .iter()
                .find(|b| b.block.text.starts_with(needle))
                .map(|b| (b.role, b.section_path.clone()))
        };
        assert_eq!(find("1 Introduction").map(|x| x.0), Some(Role::Heading));
        assert_eq!(
            find("intro paragraph").map(|x| x.1),
            Some(vec!["Introduction".to_owned()])
        );
        assert_eq!(
            find("participants paragraph").map(|x| x.1),
            Some(vec!["Methods".to_owned(), "Participants".to_owned()])
        );
        assert_eq!(
            find("results paragraph").map(|x| x.1),
            Some(vec!["Results".to_owned()])
        );
    }

    #[test]
    fn front_matter_references_and_captions() {
        let mut first: Vec<(&str, f32, f32, bool)> = vec![
            ("A Study Of Important Things", 740.0, 20.0, true),
            ("Jane Smith, John Doe", 700.0, 10.0, false),
            ("University of Somewhere", 688.0, 9.0, false),
            ("Abstract", 650.0, 12.0, true),
            (
                "this is the abstract text which runs on",
                630.0,
                10.0,
                false,
            ),
            (
                "this is the abstract text which runs on",
                618.0,
                10.0,
                false,
            ),
            ("Figure 1. A caption for something", 560.0, 9.0, false),
        ];
        first.extend(body_lines(
            520.0,
            6,
            "filler body text so that body size is modal",
        ));
        let refs: Vec<(&str, f32, f32, bool)> = vec![
            ("References", 700.0, 12.0, true),
            (
                "1. Smith J, Doe J. A trial of things. Lancet. 2020;1:2-3. doi:10.1000/abc.1",
                670.0,
                10.0,
                false,
            ),
            (
                "2. Brown A, Green B. Another trial of other things. BMJ. 2019;4:5-6.",
                640.0,
                10.0,
                false,
            ),
        ];
        let structure = analyze_structure(vec![
            page(1, &first, None, None),
            page(2, &refs, None, None),
        ]);
        let role_of = |needle: &str| {
            structure
                .blocks
                .iter()
                .find(|b| b.block.text.starts_with(needle))
                .map(|b| b.role)
        };
        assert_eq!(role_of("A Study"), Some(Role::Title));
        assert_eq!(role_of("Jane Smith"), Some(Role::FrontMatter));
        assert_eq!(role_of("Abstract"), Some(Role::Heading));
        assert_eq!(role_of("this is the abstract"), Some(Role::Body));
        assert_eq!(role_of("Figure 1."), Some(Role::Caption));
        assert_eq!(role_of("1. Smith"), Some(Role::Reference));
        assert_eq!(structure.references.len(), 2, "{:?}", structure.references);
        assert_eq!(
            structure.references[0].doi.as_deref(),
            Some("10.1000/abc.1")
        );
        assert_eq!(structure.references[1].year, Some(2019));
    }

    #[test]
    fn reference_splitting_by_author_start() {
        let lines: Vec<RefLine> = [
            (
                "Smith J, Doe J. A trial of things. Lancet 2020;1:2.",
                true,
                false,
            ),
            ("Brown A, Green B. Another trial of", true, false),
            ("other things. BMJ 2019;4:5.", false, false),
        ]
        .iter()
        .map(|(text, first, hanging)| RefLine {
            text: (*text).to_owned(),
            page_number: 1,
            first_in_block: *first,
            hanging: *hanging,
        })
        .collect();
        let references = split_references(&lines, 10.0);
        assert_eq!(references.len(), 2);
        assert!(references[1].raw_text.ends_with("BMJ 2019;4:5."));
    }
}

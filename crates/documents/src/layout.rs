//! Stage 2: words -> reading-ordered regions (recursive XY-cut) -> lines -> blocks.

use crate::chars::{Word, join_lines};

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
    pub baseline: f32,
    pub size: f32,
    pub bold: bool,
    pub glyphs: usize,
}

/// A paragraph-like unit of text on one page, in reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub text: String,
    pub lines: Vec<Line>,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
    /// Glyph-weighted mean font size.
    pub size: f32,
    /// Most of the glyphs are bold.
    pub bold: bool,
    pub glyphs: usize,
}

const MAX_DEPTH: usize = 48;

/// Returns the midpoint of the widest gap of at least `min_gap` between projected intervals.
fn widest_gap(mut intervals: Vec<(f32, f32)>, min_gap: f32) -> Option<f32> {
    intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut reach = f32::MIN;
    let mut best: Option<(f32, f32)> = None;
    for (index, (start, end)) in intervals.iter().enumerate() {
        if index > 0 {
            let gap = start - reach;
            if gap >= min_gap && best.is_none_or(|(width, _)| gap > width) {
                best = Some((gap, (start + reach) / 2.0));
            }
        }
        reach = reach.max(*end);
    }
    best.map(|(_, middle)| middle)
}

fn center_x(word: &Word) -> f32 {
    (word.left + word.right) / 2.0
}

fn center_y(word: &Word) -> f32 {
    (word.top + word.bottom) / 2.0
}

/// Median of the word heights, used as the unit for gap thresholds.
fn unit_size(words: &[&Word]) -> f32 {
    let mut sizes: Vec<f32> = words.iter().map(|w| w.size).collect();
    sizes.sort_by(f32::total_cmp);
    sizes.get(sizes.len() / 2).copied().unwrap_or(10.0).max(4.0)
}

fn split_by<'a>(
    words: &[&'a Word],
    key: impl Fn(&Word) -> f32,
    middle: f32,
) -> (Vec<&'a Word>, Vec<&'a Word>) {
    words.iter().copied().partition(|w| key(w) < middle)
}

fn has_wide_line_gap(words: &[&Word], unit: f32) -> bool {
    let lines = cluster_lines(words);
    lines.iter().any(|line| {
        let mut sorted: Vec<&&Word> = line.iter().collect();
        sorted.sort_by(|a, b| a.left.total_cmp(&b.left));
        sorted.windows(2).any(|pair| {
            let (a, b) = (pair[0], pair[1]);
            b.left - a.right >= 1.2 * unit
        })
    })
}

fn xy_cut_into<'a>(words: Vec<&'a Word>, depth: usize, out: &mut Vec<Vec<&'a Word>>) {
    if words.len() < 2 || depth >= MAX_DEPTH {
        if !words.is_empty() {
            out.push(words);
        }
        return;
    }
    let unit = unit_size(&words);
    let side_ok = |a: &Vec<&Word>, b: &Vec<&Word>| a.len() >= 2 && b.len() >= 2;

    // 1. Column gutters spanning the whole region.
    let columns = widest_gap(
        words.iter().map(|w| (w.left, w.right)).collect(),
        1.0 * unit,
    );
    if let Some(middle) = columns {
        let (left, right) = split_by(&words, center_x, middle);
        if side_ok(&left, &right) {
            xy_cut_into(left, depth + 1, out);
            xy_cut_into(right, depth + 1, out);
            return;
        }
    }
    // 2. Clear horizontal whitespace bands. Y grows upwards: the top part reads first.
    let rows =
        |min_gap: f32| widest_gap(words.iter().map(|w| (-w.top, -w.bottom)).collect(), min_gap);
    let mut middle = rows(0.9 * unit);
    if middle.is_none() && has_wide_line_gap(&words, unit) {
        // Columns exist but are bridged by a full-width element: accept finer row gaps.
        middle = rows(0.3 * unit);
    }
    if let Some(middle) = middle {
        let (top, bottom) = split_by(&words, |w| -center_y(w), middle);
        if !top.is_empty() && !bottom.is_empty() {
            xy_cut_into(top, depth + 1, out);
            xy_cut_into(bottom, depth + 1, out);
            return;
        }
    }
    out.push(words);
}

/// Recursive XY-cut: returns word groups in reading order.
pub fn xy_cut(words: &[Word]) -> Vec<Vec<&Word>> {
    let mut out = Vec::new();
    xy_cut_into(words.iter().collect(), 0, &mut out);
    out
}

fn vertical_overlap(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.1.min(b.1) - a.0.max(b.0)).max(0.0)
}

/// Clusters words into lines by vertical overlap with the line's anchor (largest) words.
fn cluster_lines<'a>(words: &[&'a Word]) -> Vec<Vec<&'a Word>> {
    let mut order: Vec<&Word> = words.to_vec();
    order.sort_by(|a, b| b.size.total_cmp(&a.size).then(b.top.total_cmp(&a.top)));
    struct Cluster<'a> {
        bottom: f32,
        top: f32,
        members: Vec<&'a Word>,
    }
    let mut clusters: Vec<Cluster> = Vec::new();
    for word in order {
        let band = (
            word.baseline - 0.2 * word.size,
            word.baseline + 0.8 * word.size,
        );
        let mut best: Option<(usize, f32)> = None;
        for (index, cluster) in clusters.iter().enumerate() {
            let overlap = vertical_overlap(band, (cluster.bottom, cluster.top));
            let reference = (band.1 - band.0).min(cluster.top - cluster.bottom).max(0.1);
            if overlap >= 0.5 * reference && best.is_none_or(|(_, o)| overlap > o) {
                best = Some((index, overlap));
            }
        }
        match best {
            Some((index, _)) => clusters[index].members.push(word),
            None => clusters.push(Cluster {
                bottom: band.0,
                top: band.1,
                members: vec![word],
            }),
        }
    }
    clusters.into_iter().map(|c| c.members).collect()
}

fn join_line_words(words: &[&Word]) -> String {
    let mut text = String::new();
    let mut previous: Option<&Word> = None;
    for word in words {
        if let Some(previous) = previous {
            let gap = word.left - previous.right;
            let tight = gap < 0.12 * word.size.min(previous.size);
            let closing =
                word.text.chars().next().is_some_and(|c| {
                    matches!(c, ',' | '.' | ';' | ':' | ')' | ']' | '%' | '!' | '?')
                });
            let opening = previous
                .text
                .chars()
                .last()
                .is_some_and(|c| matches!(c, '(' | '['));
            if !(tight && (closing || opening)) {
                text.push(' ');
            }
        }
        text.push_str(&word.text);
        previous = Some(word);
    }
    text
}

fn build_line(mut words: Vec<&Word>) -> Line {
    words.sort_by(|a, b| a.left.total_cmp(&b.left));
    let glyphs: usize = words.iter().map(|w| w.glyphs).sum::<usize>().max(1);
    let size = words.iter().map(|w| w.size * w.glyphs as f32).sum::<f32>() / glyphs as f32;
    let bold_glyphs: usize = words.iter().filter(|w| w.bold).map(|w| w.glyphs).sum();
    let baseline = words
        .iter()
        .max_by_key(|w| w.glyphs)
        .map_or(0.0, |w| w.baseline);
    Line {
        text: join_line_words(&words),
        left: words.iter().map(|w| w.left).fold(f32::MAX, f32::min),
        right: words.iter().map(|w| w.right).fold(f32::MIN, f32::max),
        bottom: words.iter().map(|w| w.bottom).fold(f32::MAX, f32::min),
        top: words.iter().map(|w| w.top).fold(f32::MIN, f32::max),
        baseline,
        size,
        bold: bold_glyphs * 5 >= glyphs * 3,
        glyphs,
    }
}

fn ends_sentence(text: &str) -> bool {
    text.trim_end()
        .chars()
        .last()
        .is_some_and(|c| matches!(c, '.' | '!' | '?' | ':'))
}

/// Splits the ordered lines of one region into paragraphs.
fn split_paragraphs(lines: Vec<Line>) -> Vec<Vec<Line>> {
    if lines.is_empty() {
        return Vec::new();
    }
    let region_left = lines.iter().map(|l| l.left).fold(f32::MAX, f32::min);
    let region_right = lines.iter().map(|l| l.right).fold(f32::MIN, f32::max);
    let width = (region_right - region_left).max(1.0);
    let mut pitches: Vec<f32> = lines
        .windows(2)
        .map(|pair| pair[0].baseline - pair[1].baseline)
        .filter(|pitch| *pitch > 0.0)
        .collect();
    pitches.sort_by(f32::total_cmp);
    let median_pitch = pitches.get(pitches.len() / 2).copied();
    // The dominant left edge decides whether indentation marks a paragraph start: a first line
    // pushed right of it (conventional) or a first line pulled out left of it (hanging).
    let mut buckets: std::collections::HashMap<i32, usize> = std::collections::HashMap::new();
    for line in &lines {
        *buckets.entry((line.left / 2.0).round() as i32).or_default() += 1;
    }
    let mode_left = buckets
        .into_iter()
        .max_by_key(|(bucket, count)| (*count, -*bucket))
        .map_or(region_left, |(bucket, _)| bucket as f32 * 2.0);
    let mut paragraphs: Vec<Vec<Line>> = Vec::new();
    for line in lines {
        let split = paragraphs
            .last()
            .and_then(|paragraph| paragraph.last())
            .is_some_and(|previous| {
                let pitch = previous.baseline - line.baseline;
                let big_gap = match median_pitch {
                    Some(median) if pitches.len() >= 3 => {
                        pitch > 1.35 * median.max(0.8 * line.size)
                    }
                    _ => pitch > 1.7 * line.size.max(previous.size),
                };
                let size_change = (line.size / previous.size.max(0.1) - 1.0).abs() > 0.12;
                let style_change = line.bold != previous.bold;
                let indent = line.left - mode_left > 1.2 * line.size
                    && line.right >= previous.right - 0.5 * line.size
                    && previous.left - mode_left < 0.6 * line.size;
                let hanging = mode_left - line.left > 0.8 * line.size
                    && previous.left - mode_left > -0.4 * line.size;
                let short_end = (previous.right - region_left) < 0.7 * width
                    && ends_sentence(&previous.text)
                    && line.left - mode_left < 0.6 * line.size;
                big_gap || size_change || style_change || indent || hanging || short_end
            });
        if split || paragraphs.is_empty() {
            paragraphs.push(vec![line]);
        } else if let Some(last) = paragraphs.last_mut() {
            last.push(line);
        }
    }
    paragraphs
}

fn build_block(lines: Vec<Line>) -> Block {
    let glyphs: usize = lines.iter().map(|l| l.glyphs).sum::<usize>().max(1);
    let size = lines.iter().map(|l| l.size * l.glyphs as f32).sum::<f32>() / glyphs as f32;
    let bold_glyphs: usize = lines.iter().filter(|l| l.bold).map(|l| l.glyphs).sum();
    let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
    Block {
        text: join_lines(&texts),
        left: lines.iter().map(|l| l.left).fold(f32::MAX, f32::min),
        right: lines.iter().map(|l| l.right).fold(f32::MIN, f32::max),
        bottom: lines.iter().map(|l| l.bottom).fold(f32::MAX, f32::min),
        top: lines.iter().map(|l| l.top).fold(f32::MIN, f32::max),
        size,
        bold: bold_glyphs * 5 >= glyphs * 3,
        glyphs,
        lines,
    }
}

/// Symbol soup from undecodable fonts and stray punctuation: not worth storing as evidence.
fn is_noise(text: &str) -> bool {
    let total = text.chars().filter(|c| !c.is_whitespace()).count();
    if total == 0 {
        return true;
    }
    let alphanumeric = text.chars().filter(|c| c.is_alphanumeric()).count();
    alphanumeric == 0 || (total < 30 && alphanumeric * 10 < total * 4)
}

/// Words of one page -> blocks in reading order.
pub fn build_blocks(words: &[Word]) -> Vec<Block> {
    let mut blocks = Vec::new();
    for region in xy_cut(words) {
        let mut lines: Vec<Line> = cluster_lines(&region).into_iter().map(build_line).collect();
        lines.sort_by(|a, b| b.baseline.total_cmp(&a.baseline));
        for paragraph in split_paragraphs(lines) {
            let block = build_block(paragraph);
            if !is_noise(&block.text) {
                blocks.push(block);
            }
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chars::{build_words, testing::run};

    fn para(
        lines: &[&str],
        x: f32,
        top_baseline: f32,
        size: f32,
        pitch: f32,
        bold: bool,
    ) -> Vec<crate::chars::RawChar> {
        let mut chars = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            chars.extend(run(
                line,
                x,
                top_baseline - pitch * index as f32,
                size,
                bold,
                true,
            ));
        }
        chars
    }

    #[test]
    fn two_columns_read_left_then_right() {
        // 612pt page; columns at x=50 and x=330; interleaved stream order (as many PDFs do).
        let left = [
            "left column first line here",
            "left column second line here",
            "left column third line here",
        ];
        let right = [
            "right column first line here",
            "right column second line here",
            "right column third line here",
        ];
        let mut chars = Vec::new();
        for i in 0..3 {
            chars.extend(run(
                left[i],
                50.0,
                700.0 - 12.0 * i as f32,
                10.0,
                false,
                true,
            ));
            chars.extend(run(
                right[i],
                330.0,
                700.0 - 12.0 * i as f32,
                10.0,
                false,
                true,
            ));
        }
        let blocks = build_blocks(&build_words(&chars));
        let texts: Vec<&str> = blocks.iter().map(|b| b.text.as_str()).collect();
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].starts_with("left column first"));
        assert!(texts[0].ends_with("third line here"));
        assert!(texts[1].starts_with("right column first"));
    }

    #[test]
    fn full_width_title_precedes_columns() {
        let mut chars = run(
            "A Wide Title Over Both Columns Of The Page",
            50.0,
            760.0,
            18.0,
            true,
            true,
        );
        for i in 0..4 {
            chars.extend(run(
                "left text left text left",
                50.0,
                700.0 - 12.0 * i as f32,
                10.0,
                false,
                true,
            ));
            chars.extend(run(
                "right text right text right",
                330.0,
                700.0 - 12.0 * i as f32,
                10.0,
                false,
                true,
            ));
        }
        let blocks = build_blocks(&build_words(&chars));
        assert!(blocks[0].text.starts_with("A Wide Title"));
        assert!(blocks[1].text.starts_with("left text"));
        assert!(blocks[2].text.starts_with("right text"));
    }

    #[test]
    fn paragraph_gap_splits_and_hyphen_joins() {
        let mut chars = para(
            &["The study collected infor-", "mation from patients."],
            50.0,
            700.0,
            10.0,
            12.0,
            false,
        );
        chars.extend(para(
            &["A second paragraph starts here."],
            50.0,
            670.0,
            10.0,
            12.0,
            false,
        ));
        let blocks = build_blocks(&build_words(&chars));
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(
            blocks[0].text,
            "The study collected information from patients."
        );
    }
}

//! Stage 1: PDFium's character stream and its conversion into words.
//!
//! Everything here is a pure function over [`RawChar`], so it can be unit-tested without
//! PDFium. Coordinates are PDF user space (origin bottom-left, y grows upwards).

/// One character from the page text stream.
#[derive(Debug, Clone, PartialEq)]
pub struct RawChar {
    pub ch: char,
    /// Inserted by PDFium (spaces, line breaks, justification). Authoritative word breaks.
    pub generated: bool,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
    pub baseline: f32,
    pub size: f32,
    pub bold: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub left: f32,
    pub right: f32,
    pub bottom: f32,
    pub top: f32,
    pub baseline: f32,
    pub size: f32,
    pub bold: bool,
    /// Number of glyph characters (for weighting font statistics).
    pub glyphs: usize,
}

/// Soft hyphen marker. PDFium reports a hyphen at a line end as U+0002.
pub const SOFT_HYPHEN: char = '\u{ad}';

fn is_word_break(character: char) -> bool {
    character.is_whitespace() || character == '\u{a0}'
}

fn is_dropped(character: char) -> bool {
    (character.is_control() && !is_word_break(character))
        || matches!(
            character,
            '\u{200b}'
                | '\u{200c}'
                | '\u{200d}'
                | '\u{2060}'
                | '\u{feff}'
                | '\u{fffe}'
                | '\u{ffff}'
        )
}

fn expand_ligature(character: char) -> Option<&'static str> {
    Some(match character {
        '\u{fb00}' => "ff",
        '\u{fb01}' => "fi",
        '\u{fb02}' => "fl",
        '\u{fb03}' => "ffi",
        '\u{fb04}' => "ffl",
        '\u{fb05}' | '\u{fb06}' => "st",
        '\u{2002}' | '\u{2003}' | '\u{2009}' => " ",
        _ => return None,
    })
}

const COMPOSED: &[(char, char, char)] = &[
    ('A', '\u{300}', 'À'),
    ('A', '\u{301}', 'Á'),
    ('A', '\u{302}', 'Â'),
    ('A', '\u{303}', 'Ã'),
    ('A', '\u{308}', 'Ä'),
    ('A', '\u{30a}', 'Å'),
    ('A', '\u{30c}', 'Ǎ'),
    ('A', '\u{328}', 'Ą'),
    ('A', '\u{304}', 'Ā'),
    ('E', '\u{300}', 'È'),
    ('E', '\u{301}', 'É'),
    ('E', '\u{302}', 'Ê'),
    ('E', '\u{303}', 'Ẽ'),
    ('E', '\u{308}', 'Ë'),
    ('E', '\u{30c}', 'Ě'),
    ('E', '\u{327}', 'Ȩ'),
    ('E', '\u{328}', 'Ę'),
    ('E', '\u{304}', 'Ē'),
    ('I', '\u{300}', 'Ì'),
    ('I', '\u{301}', 'Í'),
    ('I', '\u{302}', 'Î'),
    ('I', '\u{303}', 'Ĩ'),
    ('I', '\u{308}', 'Ï'),
    ('I', '\u{30c}', 'Ǐ'),
    ('I', '\u{328}', 'Į'),
    ('I', '\u{304}', 'Ī'),
    ('O', '\u{300}', 'Ò'),
    ('O', '\u{301}', 'Ó'),
    ('O', '\u{302}', 'Ô'),
    ('O', '\u{303}', 'Õ'),
    ('O', '\u{308}', 'Ö'),
    ('O', '\u{30c}', 'Ǒ'),
    ('O', '\u{328}', 'Ǫ'),
    ('O', '\u{304}', 'Ō'),
    ('U', '\u{300}', 'Ù'),
    ('U', '\u{301}', 'Ú'),
    ('U', '\u{302}', 'Û'),
    ('U', '\u{303}', 'Ũ'),
    ('U', '\u{308}', 'Ü'),
    ('U', '\u{30a}', 'Ů'),
    ('U', '\u{30c}', 'Ǔ'),
    ('U', '\u{328}', 'Ų'),
    ('U', '\u{304}', 'Ū'),
    ('Y', '\u{300}', 'Ỳ'),
    ('Y', '\u{301}', 'Ý'),
    ('Y', '\u{302}', 'Ŷ'),
    ('Y', '\u{303}', 'Ỹ'),
    ('Y', '\u{308}', 'Ÿ'),
    ('Y', '\u{304}', 'Ȳ'),
    ('C', '\u{301}', 'Ć'),
    ('C', '\u{302}', 'Ĉ'),
    ('C', '\u{30c}', 'Č'),
    ('C', '\u{327}', 'Ç'),
    ('N', '\u{300}', 'Ǹ'),
    ('N', '\u{301}', 'Ń'),
    ('N', '\u{303}', 'Ñ'),
    ('N', '\u{30c}', 'Ň'),
    ('N', '\u{327}', 'Ņ'),
    ('a', '\u{300}', 'à'),
    ('a', '\u{301}', 'á'),
    ('a', '\u{302}', 'â'),
    ('a', '\u{303}', 'ã'),
    ('a', '\u{308}', 'ä'),
    ('a', '\u{30a}', 'å'),
    ('a', '\u{30c}', 'ǎ'),
    ('a', '\u{328}', 'ą'),
    ('a', '\u{304}', 'ā'),
    ('e', '\u{300}', 'è'),
    ('e', '\u{301}', 'é'),
    ('e', '\u{302}', 'ê'),
    ('e', '\u{303}', 'ẽ'),
    ('e', '\u{308}', 'ë'),
    ('e', '\u{30c}', 'ě'),
    ('e', '\u{327}', 'ȩ'),
    ('e', '\u{328}', 'ę'),
    ('e', '\u{304}', 'ē'),
    ('i', '\u{300}', 'ì'),
    ('i', '\u{301}', 'í'),
    ('i', '\u{302}', 'î'),
    ('i', '\u{303}', 'ĩ'),
    ('i', '\u{308}', 'ï'),
    ('i', '\u{30c}', 'ǐ'),
    ('i', '\u{328}', 'į'),
    ('i', '\u{304}', 'ī'),
    ('o', '\u{300}', 'ò'),
    ('o', '\u{301}', 'ó'),
    ('o', '\u{302}', 'ô'),
    ('o', '\u{303}', 'õ'),
    ('o', '\u{308}', 'ö'),
    ('o', '\u{30c}', 'ǒ'),
    ('o', '\u{328}', 'ǫ'),
    ('o', '\u{304}', 'ō'),
    ('u', '\u{300}', 'ù'),
    ('u', '\u{301}', 'ú'),
    ('u', '\u{302}', 'û'),
    ('u', '\u{303}', 'ũ'),
    ('u', '\u{308}', 'ü'),
    ('u', '\u{30a}', 'ů'),
    ('u', '\u{30c}', 'ǔ'),
    ('u', '\u{328}', 'ų'),
    ('u', '\u{304}', 'ū'),
    ('y', '\u{300}', 'ỳ'),
    ('y', '\u{301}', 'ý'),
    ('y', '\u{302}', 'ŷ'),
    ('y', '\u{303}', 'ỹ'),
    ('y', '\u{308}', 'ÿ'),
    ('y', '\u{30a}', 'ẙ'),
    ('y', '\u{304}', 'ȳ'),
    ('c', '\u{301}', 'ć'),
    ('c', '\u{302}', 'ĉ'),
    ('c', '\u{30c}', 'č'),
    ('c', '\u{327}', 'ç'),
    ('n', '\u{300}', 'ǹ'),
    ('n', '\u{301}', 'ń'),
    ('n', '\u{303}', 'ñ'),
    ('n', '\u{30c}', 'ň'),
    ('n', '\u{327}', 'ņ'),
    ('s', '\u{301}', 'ś'),
    ('s', '\u{302}', 'ŝ'),
    ('s', '\u{30c}', 'š'),
    ('s', '\u{327}', 'ş'),
    ('z', '\u{301}', 'ź'),
    ('z', '\u{302}', 'ẑ'),
    ('z', '\u{30c}', 'ž'),
    ('S', '\u{301}', 'Ś'),
    ('S', '\u{302}', 'Ŝ'),
    ('S', '\u{30c}', 'Š'),
    ('S', '\u{327}', 'Ş'),
    ('Z', '\u{301}', 'Ź'),
    ('Z', '\u{302}', 'Ẑ'),
    ('Z', '\u{30c}', 'Ž'),
];

fn is_combining_mark(character: char) -> bool {
    ('\u{300}'..='\u{36f}').contains(&character)
}

/// Composes a base letter followed by a combining accent ("c" + U+0327 -> "ç").
fn compose(base: char, mark: char) -> Option<char> {
    COMPOSED
        .iter()
        .find(|(b, m, _)| *b == base && *m == mark)
        .map(|(_, _, composed)| *composed)
}

struct Builder {
    text: String,
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    baseline: f32,
    size_sum: f32,
    bold_glyphs: usize,
    glyphs: usize,
    max_size: f32,
}

impl Builder {
    fn new(c: &RawChar) -> Self {
        Self {
            text: String::new(),
            left: c.left,
            right: c.right,
            bottom: c.bottom,
            top: c.top,
            baseline: c.baseline,
            size_sum: 0.0,
            bold_glyphs: 0,
            glyphs: 0,
            max_size: c.size,
        }
    }

    fn push(&mut self, c: &RawChar, text: &str) {
        self.text.push_str(text);
        self.left = self.left.min(c.left);
        self.right = self.right.max(c.right);
        self.bottom = self.bottom.min(c.bottom);
        self.top = self.top.max(c.top);
        self.size_sum += c.size;
        self.max_size = self.max_size.max(c.size);
        self.glyphs += 1;
        if c.bold {
            self.bold_glyphs += 1;
        }
    }

    fn finish(self) -> Option<Word> {
        if self.text.trim().is_empty() || self.glyphs == 0 {
            return None;
        }
        let size = self.size_sum / self.glyphs as f32;
        Some(Word {
            text: self.text,
            left: self.left,
            right: self.right,
            bottom: self.bottom,
            top: self.top,
            baseline: self.baseline,
            size,
            bold: self.bold_glyphs * 5 >= self.glyphs * 3,
            glyphs: self.glyphs,
        })
    }
}

/// Splits the char stream into words.
///
/// Word boundaries are, in order of authority: PDFium-generated or real whitespace, a change
/// of line, a jump backwards in the stream, and finally a horizontal gap that is large
/// relative to the font size (fallback for PDFs where PDFium did not generate a space).
pub fn build_words(chars: &[RawChar]) -> Vec<Word> {
    let mut words = Vec::new();
    let mut current: Option<Builder> = None;
    for c in chars {
        if is_word_break(c.ch) {
            if let Some(builder) = current.take() {
                words.extend(builder.finish());
            }
            continue;
        }
        if is_dropped(c.ch) && c.ch != '\u{2}' {
            continue;
        }
        let character = if c.ch == '\u{2}' { SOFT_HYPHEN } else { c.ch };
        if let Some(builder) = &current {
            let reference = builder.max_size.min(c.size).max(1.0);
            let line_shift = (c.baseline - builder.baseline).abs();
            let gap = c.left - builder.right;
            let breaks = line_shift > 0.7 * reference.max(builder.max_size.min(c.size * 1.5))
                || gap > 0.2 * reference
                || gap < -reference;
            if breaks && let Some(builder) = current.take() {
                words.extend(builder.finish());
            }
        }
        if is_combining_mark(character)
            && let Some(builder) = current.as_mut()
        {
            // Accents arrive as their own zero-width glyph after the base letter.
            let composed = builder
                .text
                .chars()
                .last()
                .and_then(|base| compose(base, character));
            if let Some(composed) = composed {
                builder.text.pop();
                builder.text.push(composed);
            }
            builder.right = builder.right.max(c.right);
            continue;
        }
        let builder = current.get_or_insert_with(|| Builder::new(c));
        match expand_ligature(character) {
            Some(" ") => {
                if let Some(builder) = current.take() {
                    words.extend(builder.finish());
                }
            }
            Some(expansion) => builder.push(c, expansion),
            None => {
                let mut buffer = [0u8; 4];
                builder.push(c, character.encode_utf8(&mut buffer));
            }
        }
    }
    if let Some(builder) = current.take() {
        words.extend(builder.finish());
    }
    words
}

/// True for characters that end a word with a (possibly soft) hyphen.
pub fn is_hyphen_char(character: char) -> bool {
    matches!(character, '-' | '\u{2010}' | '\u{2011}' | SOFT_HYPHEN)
}

/// Joins consecutive lines of one paragraph. A hyphen at a line end is removed when the next
/// line starts with a lowercase letter ("infor-" + "mation"); URLs/DOIs keep their hyphen.
pub fn join_lines<S: AsRef<str>>(lines: &[S]) -> String {
    let mut out = String::new();
    for line in lines {
        let line = line.as_ref().trim();
        if line.is_empty() {
            continue;
        }
        if out.is_empty() {
            out.push_str(line);
            continue;
        }
        let mut chars = out.chars().rev();
        let last = chars.next();
        let before = chars.next();
        let next_lower = line.chars().next().is_some_and(char::is_lowercase);
        let last_token = out.split_whitespace().next_back().unwrap_or("");
        let looks_like_link = last_token.contains('/') || last_token.contains("10.");
        match last {
            Some(h) if is_hyphen_char(h) => {
                let alphabetic_before = before.is_some_and(char::is_alphabetic);
                let join_word = h == SOFT_HYPHEN || (alphabetic_before && next_lower);
                if join_word && !looks_like_link {
                    out.pop();
                }
                // otherwise keep the hyphen and glue ("COVID-" + "19", URLs, DOIs)
            }
            _ => {
                if !(looks_like_link && last_token.ends_with('/')) {
                    out.push(' ');
                }
            }
        }
        out.push_str(line);
    }
    out.retain(|c| c != SOFT_HYPHEN);
    out
}

#[cfg(test)]
pub(crate) mod testing {
    use super::RawChar;

    /// Lays out `text` starting at `x` on `baseline`; returns chars plus the end x. Spaces are
    /// emitted as generated chars when `generated_spaces` is true, else omitted (a gap remains).
    pub fn run(
        text: &str,
        x: f32,
        baseline: f32,
        size: f32,
        bold: bool,
        generated_spaces: bool,
    ) -> Vec<RawChar> {
        let mut out = Vec::new();
        let mut cursor = x;
        for ch in text.chars() {
            let advance = if ch == ' ' { 0.28 * size } else { 0.5 * size };
            if ch == ' ' {
                if generated_spaces {
                    out.push(RawChar {
                        ch,
                        generated: true,
                        left: cursor,
                        right: cursor + advance,
                        bottom: baseline - 0.25 * size,
                        top: baseline + 0.95 * size,
                        baseline,
                        size,
                        bold,
                    });
                }
            } else {
                out.push(RawChar {
                    ch,
                    generated: false,
                    left: cursor,
                    right: cursor + advance,
                    bottom: baseline - 0.25 * size,
                    top: baseline + 0.95 * size,
                    baseline,
                    size,
                    bold,
                });
            }
            cursor += advance;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testing::run;
    use super::*;

    #[test]
    fn generated_spaces_split_words() {
        let chars = run("and Brittany went", 10.0, 100.0, 10.0, false, true);
        let words: Vec<_> = build_words(&chars).into_iter().map(|w| w.text).collect();
        assert_eq!(words, ["and", "Brittany", "went"]);
    }

    #[test]
    fn gap_fallback_splits_words_without_generated_space() {
        let chars = run("and Brittany went", 10.0, 100.0, 10.0, false, false);
        let words: Vec<_> = build_words(&chars).into_iter().map(|w| w.text).collect();
        assert_eq!(words, ["and", "Brittany", "went"]);
    }

    #[test]
    fn combining_accents_compose() {
        let mut chars = run("Servic", 0.0, 50.0, 10.0, false, true);
        chars.extend(run("\u{327}", 30.0, 50.0, 10.0, false, true));
        chars.extend(run("o", 30.0, 50.0, 10.0, false, true));
        let words = build_words(&chars);
        assert_eq!(words[0].text, "Servi\u{e7}o");
    }

    #[test]
    fn tight_kerning_does_not_split() {
        let chars = run("information", 10.0, 100.0, 10.0, false, false);
        assert_eq!(build_words(&chars).len(), 1);
    }

    #[test]
    fn ligatures_expand_and_controls_drop() {
        let mut chars = run("e\u{fb03}cient\u{1}", 0.0, 50.0, 10.0, false, true);
        chars.push(run("x", 80.0, 50.0, 10.0, false, true).remove(0));
        let words = build_words(&chars);
        assert_eq!(words[0].text, "efficient");
    }

    #[test]
    fn new_line_without_break_char_splits() {
        let mut chars = run("first", 10.0, 100.0, 10.0, false, true);
        chars.extend(run("second", 10.0, 88.0, 10.0, false, true));
        assert_eq!(build_words(&chars).len(), 2);
    }

    #[test]
    fn dehyphenates_at_line_end() {
        assert_eq!(
            join_lines(&["the infor-", "mation here"]),
            "the information here"
        );
        assert_eq!(join_lines(&["a well-", "Known case"]), "a well-Known case");
        assert_eq!(
            join_lines(&["see doi 10.1000/abc-", "def"]),
            "see doi 10.1000/abc-def"
        );
        assert_eq!(join_lines(&["tre\u{ad}", "mendous"]), "tremendous");
        assert_eq!(join_lines(&["one", "two"]), "one two");
    }
}

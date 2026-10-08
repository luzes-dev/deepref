use roxmltree::{Document, Node};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TeiError {
    #[error("TEI XML is malformed: {0}")]
    Xml(#[from] roxmltree::Error),
    #[error("response is not a TEI document")]
    NotTei,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TeiDocument {
    pub grobid_version: Option<String>,
    pub title: Option<String>,
    /// The article's DOI from the header (`idno type="DOI"`), lower-cased.
    pub doi: Option<String>,
    pub abstract_text: Option<String>,
    pub sections: Vec<TeiSection>,
    pub references: Vec<TeiReference>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TeiSection {
    /// Numbering as printed (`3.2.1`) when GROBID recovered it.
    pub number: Option<String>,
    pub title: String,
    /// 1-based nesting depth.
    pub depth: usize,
    /// Titles of the ancestors followed by this section's own title.
    pub path: Vec<String>,
    /// Paragraph text with whitespace normalised, paragraphs separated by a blank line.
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TeiReference {
    pub ordinal: usize,
    pub raw: String,
    pub title: Option<String>,
    pub authors: Vec<String>,
    pub year: Option<i32>,
    pub venue: Option<String>,
    pub doi: Option<String>,
}

pub fn parse_tei(xml: &str) -> Result<TeiDocument, TeiError> {
    let document = Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )?;
    let root = document.root_element();
    if root.tag_name().name() != "TEI" {
        return Err(TeiError::NotTei);
    }
    let mut result = TeiDocument {
        grobid_version: root
            .descendants()
            .find(|node| is(node, "application"))
            .and_then(|node| node.attribute("version"))
            .map(str::to_owned),
        ..Default::default()
    };
    if let Some(header) = child(root, "teiHeader") {
        result.title = path(header, &["fileDesc", "titleStmt"]).and_then(|stmt| {
            stmt.children()
                .filter(|node| is(node, "title"))
                .map(text_of)
                .find(|title| !title.is_empty())
        });
        result.doi = header
            .descendants()
            .find(|node| {
                is(node, "idno")
                    && node
                        .attribute("type")
                        .is_some_and(|kind| kind.eq_ignore_ascii_case("doi"))
            })
            .map(text_of)
            .map(|doi| doi.to_lowercase())
            .filter(|doi| !doi.is_empty());
        result.abstract_text = path(header, &["profileDesc", "abstract"])
            .map(paragraph_text)
            .filter(|text| !text.is_empty());
    }
    if let Some(text) = child(root, "text") {
        if let Some(body) = child(text, "body") {
            let mut stack: Vec<(usize, String)> = Vec::new();
            for div in body.children().filter(|node| is(node, "div")) {
                collect_sections(div, 1, &mut stack, &mut result.sections);
            }
        }
        let mut ordinal = 0;
        for bibl in text
            .descendants()
            .filter(|node| is(node, "listBibl"))
            .flat_map(|list| list.children().filter(|node| is(node, "biblStruct")))
        {
            ordinal += 1;
            result.references.push(parse_reference(bibl, ordinal));
        }
    }
    Ok(result)
}

fn collect_sections(
    div: Node<'_, '_>,
    nesting: usize,
    stack: &mut Vec<(usize, String)>,
    out: &mut Vec<TeiSection>,
) {
    let head = child(div, "head");
    let title = head.map(text_of).unwrap_or_default();
    let number = head
        .and_then(|head| head.attribute("n"))
        .map(|n| n.trim().trim_end_matches('.').to_owned())
        .filter(|n| !n.is_empty());
    // GROBID emits a flat list of divs and encodes depth in `n`; nested divs
    // (plain TEI) encode it structurally. Use whichever says deeper.
    let numbered_depth = number
        .as_deref()
        .map(|n| n.split('.').filter(|part| !part.is_empty()).count())
        .unwrap_or(0);
    let depth = nesting.max(numbered_depth).max(1);
    let own_text = div
        .children()
        .filter(|node| is(node, "p") || is(node, "list") || is(node, "formula"))
        .map(text_of)
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut next_nesting = nesting;
    if !title.is_empty() || !own_text.is_empty() {
        while stack.last().is_some_and(|(d, _)| *d >= depth) {
            stack.pop();
        }
        if !title.is_empty() {
            stack.push((depth, title.clone()));
        }
        out.push(TeiSection {
            number,
            title,
            depth,
            path: stack.iter().map(|(_, title)| title.clone()).collect(),
            text: own_text,
        });
        next_nesting = depth + 1;
    }
    for nested in div.children().filter(|node| is(node, "div")) {
        collect_sections(nested, next_nesting, stack, out);
    }
}

fn parse_reference(bibl: Node<'_, '_>, ordinal: usize) -> TeiReference {
    let analytic = child(bibl, "analytic");
    let monogr = child(bibl, "monogr");
    let monogr_title = monogr.and_then(|m| first_title(m));
    let (title, venue) = match (analytic.and_then(|a| first_title(a)), monogr_title) {
        (Some(article), venue) => (Some(article), venue),
        (None, monogr_title) => (monogr_title, None),
    };
    let author_source = analytic
        .filter(|a| child(*a, "author").is_some())
        .or(monogr);
    let authors: Vec<String> = author_source
        .map(|source| {
            source
                .children()
                .filter(|node| is(node, "author"))
                .filter_map(|author| child(author, "persName"))
                .map(person_name)
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let year = monogr
        .and_then(|m| child(m, "imprint"))
        .and_then(|imprint| imprint.children().find(|node| is(node, "date")))
        .and_then(|date| {
            let value = date
                .attribute("when")
                .map(str::to_owned)
                .unwrap_or_else(|| text_of(date));
            year_from(&value)
        });
    let doi = bibl
        .descendants()
        .filter(|node| is(node, "idno"))
        .find(|node| {
            node.attribute("type")
                .is_some_and(|kind| kind.eq_ignore_ascii_case("doi"))
        })
        .map(text_of)
        .filter(|doi| !doi.is_empty());
    let raw = bibl
        .descendants()
        .find(|node| is(node, "note") && node.attribute("type") == Some("raw_reference"))
        .map(text_of)
        .filter(|raw| !raw.is_empty())
        .unwrap_or_else(|| synthesize_raw(&authors, year, title.as_deref(), venue.as_deref()));
    TeiReference {
        ordinal,
        raw,
        title,
        authors,
        year,
        venue,
        doi,
    }
}

fn synthesize_raw(
    authors: &[String],
    year: Option<i32>,
    title: Option<&str>,
    venue: Option<&str>,
) -> String {
    let mut parts = Vec::new();
    if !authors.is_empty() {
        parts.push(authors.join(", "));
    }
    if let Some(year) = year {
        parts.push(format!("({year})"));
    }
    if let Some(title) = title {
        parts.push(title.to_owned());
    }
    if let Some(venue) = venue {
        parts.push(venue.to_owned());
    }
    parts.join(". ")
}

fn first_title(node: Node<'_, '_>) -> Option<String> {
    node.children()
        .filter(|n| is(n, "title"))
        .map(text_of)
        .find(|title| !title.is_empty())
}

fn person_name(person: Node<'_, '_>) -> String {
    let mut parts: Vec<String> = person
        .children()
        .filter(|node| is(node, "forename"))
        .map(text_of)
        .filter(|part| !part.is_empty())
        .collect();
    if let Some(surname) = person
        .children()
        .find(|node| is(node, "surname"))
        .map(text_of)
        .filter(|s| !s.is_empty())
    {
        parts.push(surname);
    }
    parts.join(" ")
}

fn year_from(value: &str) -> Option<i32> {
    let digits: String = value
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take(4)
        .collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

fn is(node: &Node<'_, '_>, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|candidate| is(candidate, name))
}

fn path<'a, 'input>(node: Node<'a, 'input>, names: &[&str]) -> Option<Node<'a, 'input>> {
    names
        .iter()
        .try_fold(node, |current, name| child(current, name))
}

fn text_of(node: Node<'_, '_>) -> String {
    let mut out = String::new();
    for text in node
        .descendants()
        .filter(|n| n.is_text())
        .filter_map(|n| n.text())
    {
        out.push_str(text);
        out.push(' ');
    }
    normalize_space(&out)
}

fn paragraph_text(node: Node<'_, '_>) -> String {
    let paragraphs: Vec<String> = node
        .descendants()
        .filter(|n| is(n, "p"))
        .map(text_of)
        .filter(|text| !text.is_empty())
        .collect();
    if paragraphs.is_empty() {
        text_of(node)
    } else {
        paragraphs.join("\n\n")
    }
}

fn normalize_space(value: &str) -> String {
    let joined = value.split_whitespace().collect::<Vec<_>>().join(" ");
    // Text nodes are joined with spaces, which detaches punctuation; undo that.
    joined
        .replace(" ,", ",")
        .replace(" .", ".")
        .replace(" ;", ";")
        .replace(" :", ":")
        .replace(" )", ")")
        .replace("( ", "(")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<TEI xml:space="preserve" xmlns="http://www.tei-c.org/ns/1.0" xmlns:xlink="http://www.w3.org/1999/xlink">
  <teiHeader xml:lang="en">
    <fileDesc>
      <titleStmt><title level="a" type="main">Effect of Aspirin on Stroke Recurrence</title></titleStmt>
      <sourceDesc><biblStruct><analytic><idno type="DOI">10.1000/ABC.1</idno></analytic></biblStruct></sourceDesc>
      <encodingDesc><appInfo><application version="0.8.2" ident="GROBID" when="2026-10-07T00:00:00+0000"><ref target="https://github.com/kermitt2/grobid">GROBID</ref></application></appInfo></encodingDesc>
    </fileDesc>
    <profileDesc>
      <abstract><div><p>We tested aspirin in 500 patients. Recurrence fell by 20%.</p></div></abstract>
    </profileDesc>
  </teiHeader>
  <text xml:lang="en">
    <body>
      <div><head n="1">Introduction</head><p>Stroke is common <ref type="bibr" target="#b0">[1]</ref>.</p></div>
      <div><head n="2">Methods</head><p>We ran a trial.</p></div>
      <div><head n="2.1">Participants</head><p>Adults aged 40 to 80.</p><p>Written consent was given.</p></div>
      <div><head n="2.1.1">Exclusions</head><p>Prior bleeding.</p></div>
      <div><head n="3">Results</head><p>Recurrence fell.</p></div>
      <div><head>Limitations</head><p>Single centre.</p></div>
    </body>
    <back>
      <div type="references"><listBibl>
        <biblStruct xml:id="b0">
          <analytic>
            <title level="a" type="main">Aspirin after minor stroke</title>
            <author><persName><forename type="first">Jane</forename><forename type="middle">Q</forename><surname>Doe</surname></persName></author>
            <author><persName><forename type="first">John</forename><surname>Roe</surname></persName></author>
            <idno type="DOI">10.1000/xyz123</idno>
          </analytic>
          <monogr>
            <title level="j">Stroke</title>
            <imprint><date type="published" when="2019-05-01">2019</date></imprint>
          </monogr>
          <note type="raw_reference">Doe JQ, Roe J. Aspirin after minor stroke. Stroke. 2019.</note>
        </biblStruct>
        <biblStruct xml:id="b1">
          <monogr>
            <title level="m">Clinical Trials Handbook</title>
            <author><persName><surname>Smith</surname></persName></author>
            <imprint><date>c. 2004</date></imprint>
          </monogr>
        </biblStruct>
      </listBibl></div>
    </back>
  </text>
</TEI>"##;

    #[test]
    fn parses_header_and_abstract() {
        let tei = parse_tei(FIXTURE).unwrap();
        assert_eq!(tei.grobid_version.as_deref(), Some("0.8.2"));
        assert_eq!(
            tei.title.as_deref(),
            Some("Effect of Aspirin on Stroke Recurrence")
        );
        assert_eq!(
            tei.abstract_text.as_deref(),
            Some("We tested aspirin in 500 patients. Recurrence fell by 20%.")
        );
        assert_eq!(tei.doi.as_deref(), Some("10.1000/abc.1"));
    }

    #[test]
    fn builds_section_tree_from_numbering() {
        let tei = parse_tei(FIXTURE).unwrap();
        let paths: Vec<Vec<&str>> = tei
            .sections
            .iter()
            .map(|s| s.path.iter().map(String::as_str).collect())
            .collect();
        assert_eq!(paths[0], ["Introduction"]);
        assert_eq!(paths[2], ["Methods", "Participants"]);
        assert_eq!(paths[3], ["Methods", "Participants", "Exclusions"]);
        assert_eq!(paths[4], ["Results"]);
        assert_eq!(paths[5], ["Limitations"]);
        assert_eq!(tei.sections[3].depth, 3);
        assert_eq!(tei.sections[3].number.as_deref(), Some("2.1.1"));
        assert_eq!(
            tei.sections[2].text,
            "Adults aged 40 to 80.\n\nWritten consent was given."
        );
        assert_eq!(tei.sections[0].text, "Stroke is common [1].");
    }

    #[test]
    fn parses_references() {
        let tei = parse_tei(FIXTURE).unwrap();
        assert_eq!(tei.references.len(), 2);
        let first = &tei.references[0];
        assert_eq!(first.ordinal, 1);
        assert_eq!(first.title.as_deref(), Some("Aspirin after minor stroke"));
        assert_eq!(first.authors, ["Jane Q Doe", "John Roe"]);
        assert_eq!(first.year, Some(2019));
        assert_eq!(first.venue.as_deref(), Some("Stroke"));
        assert_eq!(first.doi.as_deref(), Some("10.1000/xyz123"));
        assert!(first.raw.starts_with("Doe JQ"));
        let second = &tei.references[1];
        assert_eq!(second.title.as_deref(), Some("Clinical Trials Handbook"));
        assert_eq!(second.venue, None);
        assert_eq!(second.authors, ["Smith"]);
        assert_eq!(second.year, Some(2004));
        assert!(second.raw.contains("Clinical Trials Handbook"));
    }

    #[test]
    fn nested_divs_nest_structurally() {
        let xml = r#"<TEI xmlns="http://www.tei-c.org/ns/1.0"><text><body>
            <div><head>A</head><p>a</p><div><head>B</head><p>b</p></div></div>
            <div><head>C</head><p>c</p></div></body></text></TEI>"#;
        let tei = parse_tei(xml).unwrap();
        assert_eq!(tei.sections[1].path, ["A", "B"]);
        assert_eq!(tei.sections[2].path, ["C"]);
    }

    #[test]
    fn rejects_non_tei() {
        assert!(matches!(parse_tei("<html/>"), Err(TeiError::NotTei)));
        assert!(matches!(parse_tei("<TEI"), Err(TeiError::Xml(_))));
    }
}

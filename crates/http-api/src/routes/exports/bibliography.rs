//! RIS and BibTeX renderings of the project's reports.
//!
//! Both formats describe one publication type per record. A record that has a journal is an
//! article even when its work type is missing, because that is what reviewers import it as.

use super::{ReportExport, present};

/// Publication type shared by the RIS and BibTeX renderings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationKind {
    Article,
    Book,
    Chapter,
    Conference,
    Dataset,
    Report,
    Thesis,
    Other,
}

impl PublicationKind {
    fn of(report: &ReportExport) -> Self {
        match report
            .work_type
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("article" | "journal-article" | "journal_article") => Self::Article,
            Some("book") => Self::Book,
            Some("book-chapter" | "book_chapter") => Self::Chapter,
            Some("conference-paper" | "conference_paper" | "proceedings-article") => {
                Self::Conference
            }
            Some("dataset") => Self::Dataset,
            Some("report") => Self::Report,
            Some("dissertation" | "thesis") => Self::Thesis,
            _ if present(report.journal.as_deref()).is_some() => Self::Article,
            _ => Self::Other,
        }
    }

    const fn ris_type(self) -> &'static str {
        match self {
            Self::Article => "JOUR",
            Self::Book => "BOOK",
            Self::Chapter => "CHAP",
            Self::Conference => "CONF",
            Self::Dataset => "DATA",
            Self::Report => "RPRT",
            Self::Thesis => "THES",
            Self::Other => "GEN",
        }
    }

    const fn bibtex_entry(self) -> &'static str {
        match self {
            Self::Article => "article",
            Self::Book => "book",
            Self::Chapter => "incollection",
            Self::Conference => "inproceedings",
            Self::Dataset => "dataset",
            Self::Report => "techreport",
            Self::Thesis => "thesis",
            Self::Other => "misc",
        }
    }
}

pub(super) fn render_reports_ris(reports: &[ReportExport]) -> String {
    let mut output = String::new();
    for report in reports {
        let kind = PublicationKind::of(report);
        ris_field(&mut output, "TY", kind.ris_type());
        ris_field(&mut output, "ID", &report.report_id.to_string());
        for author in &report.authors {
            if let Some(name) = author.display_name() {
                ris_field(&mut output, "AU", &name);
            }
        }
        optional_ris(&mut output, "TI", report.title.as_deref());
        if let Some(year) = report.publication_year {
            ris_field(&mut output, "PY", &year.to_string());
        }
        optional_ris(&mut output, "DO", report.doi.as_deref());
        if kind == PublicationKind::Article {
            optional_ris(&mut output, "JO", report.journal.as_deref());
        }
        optional_ris(&mut output, "T2", report.container_title.as_deref());
        optional_ris(&mut output, "VL", report.volume.as_deref());
        optional_ris(&mut output, "IS", report.issue.as_deref());
        if let Some((start, end)) = report.pages.as_deref().and_then(page_range) {
            ris_field(&mut output, "SP", start);
            if let Some(end) = end {
                ris_field(&mut output, "EP", end);
            }
        }
        optional_ris(&mut output, "PB", report.publisher.as_deref());
        optional_ris(&mut output, "UR", report.url.as_deref());
        optional_ris(&mut output, "AB", report.abstract_text.as_deref());
        output.push_str("ER  -\n\n");
    }
    output
}

pub(super) fn render_reports_bib(reports: &[ReportExport]) -> String {
    let mut output = String::new();
    for report in reports {
        let kind = PublicationKind::of(report);
        output.push_str(&format!(
            "@{}{{report-{},\n",
            kind.bibtex_entry(),
            report.report_id
        ));
        bib_text_field(&mut output, "title", report.title.as_deref());
        let authors = report
            .authors
            .iter()
            .filter_map(|author| author.display_name())
            .map(|name| bib_text(&name))
            .collect::<Vec<_>>()
            .join(" and ");
        if !authors.is_empty() {
            bib_field(&mut output, "author", &authors);
        }
        if let Some(year) = report.publication_year {
            bib_field(&mut output, "year", &year.to_string());
        }
        match kind {
            PublicationKind::Article => {
                bib_text_field(&mut output, "journal", report.journal.as_deref());
            }
            PublicationKind::Chapter | PublicationKind::Conference => {
                bib_text_field(&mut output, "booktitle", report.container_title.as_deref());
            }
            PublicationKind::Other => {
                bib_text_field(&mut output, "container", report.container_title.as_deref());
            }
            PublicationKind::Book
            | PublicationKind::Dataset
            | PublicationKind::Report
            | PublicationKind::Thesis => {}
        }
        bib_text_field(&mut output, "volume", report.volume.as_deref());
        bib_text_field(&mut output, "number", report.issue.as_deref());
        if let Some(pages) = report.pages.as_deref().and_then(bib_pages) {
            bib_field(&mut output, "pages", &pages);
        }
        bib_text_field(&mut output, "publisher", report.publisher.as_deref());
        bib_text_field(&mut output, "abstract", report.abstract_text.as_deref());
        bib_verbatim_field(&mut output, "doi", report.doi.as_deref());
        bib_verbatim_field(&mut output, "url", report.url.as_deref());
        output.push_str("}\n\n");
    }
    output
}

/// Replaces control characters (including line breaks) with spaces. RIS and BibTeX values must
/// stay on one line.
fn single_line(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Splits `start-end` (hyphen or en dash) into its parts. A single page stays whole.
fn page_range(pages: &str) -> Option<(&str, Option<&str>)> {
    let pages = present(Some(pages))?;
    match pages.split_once('-').or_else(|| pages.split_once('–')) {
        Some((start, end)) if !start.trim().is_empty() && !end.trim().is_empty() => {
            Some((start.trim(), Some(end.trim())))
        }
        _ => Some((pages, None)),
    }
}

fn ris_field(output: &mut String, tag: &str, value: &str) {
    output.push_str(tag);
    output.push_str("  - ");
    output.push_str(&single_line(value));
    output.push('\n');
}

fn optional_ris(output: &mut String, tag: &str, value: Option<&str>) {
    if let Some(value) = present(value) {
        ris_field(output, tag, value);
    }
}

fn bib_field(output: &mut String, name: &str, escaped_value: &str) {
    output.push_str(&format!("  {name} = {{{escaped_value}}},\n"));
}

fn bib_text_field(output: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = present(value) {
        bib_field(output, name, &bib_text(value));
    }
}

fn bib_verbatim_field(output: &mut String, name: &str, value: Option<&str>) {
    if let Some(value) = present(value) {
        bib_field(output, name, &bib_verbatim(value));
    }
}

/// Page ranges use BibTeX's double hyphen, so `414-418` becomes `414--418`.
fn bib_pages(pages: &str) -> Option<String> {
    let (start, end) = page_range(pages)?;
    Some(match end {
        Some(end) => format!("{}--{}", bib_text(start), bib_text(end)),
        None => bib_text(start),
    })
}

/// Escapes LaTeX-special characters. Braces become `\textbraceleft{}` and `\textbraceright{}`
/// so that BibTeX never sees an unbalanced brace.
fn bib_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in single_line(value).chars() {
        match character {
            '\\' => escaped.push_str("\\textbackslash{}"),
            '{' => escaped.push_str("\\textbraceleft{}"),
            '}' => escaped.push_str("\\textbraceright{}"),
            '&' => escaped.push_str("\\&"),
            '%' => escaped.push_str("\\%"),
            '#' => escaped.push_str("\\#"),
            '_' => escaped.push_str("\\_"),
            '$' => escaped.push_str("\\$"),
            '^' => escaped.push_str("\\textasciicircum{}"),
            '~' => escaped.push_str("\\textasciitilde{}"),
            character => escaped.push(character),
        }
    }
    escaped
}

/// DOIs and URLs are read verbatim by BibTeX tools, so they keep their characters. Only the
/// characters that would break the braced value are percent-encoded.
fn bib_verbatim(value: &str) -> String {
    single_line(value)
        .chars()
        .map(|character| match character {
            '{' => "%7B".to_owned(),
            '}' => "%7D".to_owned(),
            character if character.is_whitespace() => "%20".to_owned(),
            character => character.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use deepref_domain::ImportFormat;
    use deepref_providers::parse_import;

    use super::super::sample_report;
    use super::{PublicationKind, render_reports_bib, render_reports_ris};

    /// Every RIS line is either blank or `TAG  - value`, with a two-character tag.
    fn assert_ris_lines_are_well_formed(ris: &str) {
        for line in ris.lines().filter(|line| !line.is_empty()) {
            let tag = line.get(..2).unwrap_or_default();
            assert!(
                tag.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                    && line.get(2..5) == Some("  -"),
                "malformed RIS line: {line:?}"
            );
        }
    }

    #[test]
    fn journal_records_are_articles_and_other_records_stay_generic() {
        assert_eq!(
            PublicationKind::of(&sample_report(None)),
            PublicationKind::Article,
            "a journal makes a record an article when its type is missing"
        );
        assert_eq!(
            PublicationKind::of(&sample_report(Some("something-new"))),
            PublicationKind::Article
        );
        let mut bare = sample_report(None);
        bare.journal = None;
        assert_eq!(PublicationKind::of(&bare), PublicationKind::Other);
        assert_eq!(PublicationKind::Other.ris_type(), "GEN");
        assert_eq!(PublicationKind::Other.bibtex_entry(), "misc");
        assert_eq!(
            PublicationKind::of(&sample_report(Some("book"))),
            PublicationKind::Book
        );
        assert_eq!(PublicationKind::Book.ris_type(), "BOOK");
        assert_eq!(PublicationKind::Chapter.bibtex_entry(), "incollection");
        assert_eq!(PublicationKind::Conference.bibtex_entry(), "inproceedings");
    }

    #[test]
    fn ris_writes_article_fields_abstract_and_page_range() {
        let ris = render_reports_ris(&[sample_report(Some("article"))]);
        assert!(ris.starts_with("TY  - JOUR\n"), "{ris}");
        assert!(ris.contains("\nTI  - A {multiline} title & more: Müller's café\n"));
        assert!(ris.contains("\nAU  - O'Neil & Co., Ana Maria\n"));
        assert!(ris.contains("\nAU  - Smith, J\n"));
        assert!(ris.contains("\nPY  - 2026\n"));
        assert!(ris.contains("\nDO  - 10.1000/deepref.export-7\n"));
        assert!(ris.contains("\nJO  - Journal & Name\n"));
        assert!(ris.contains("\nT2  - Proceedings {Container}\n"));
        assert!(ris.contains("\nVL  - 49\n"));
        assert!(ris.contains("\nIS  - 3\n"));
        assert!(ris.contains("\nSP  - 414\n"));
        assert!(ris.contains("\nEP  - 418\n"));
        assert!(ris.contains("\nPB  - Publisher & Sons\n"));
        assert!(ris.contains("\nUR  - https://example.test/a?x=1&y=2\n"));
        assert!(
            ris.contains(
                "\nAB  - Background {x} & y. Results: 50% of 2213 participants; _p_ = .01 #1 ~ $5 ^2 \\ end.\n"
            ),
            "abstract must be one RIS line: {ris}"
        );
        assert!(ris.ends_with("ER  -\n\n"));
        assert_ris_lines_are_well_formed(&ris);
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn ris_round_trips_through_the_repo_importer() -> anyhow::Result<()> {
        let mut report = sample_report(Some("article"));
        report.abstract_text = Some("Line one.\nLine two with a comma, and \"quotes\".".to_owned());
        let ris = render_reports_ris(&[report]);
        let records = parse_import(ris.as_bytes(), ImportFormat::Ris, None)?;
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(
            record.title.as_deref(),
            Some("A {multiline} title & more: Müller's café")
        );
        assert_eq!(
            record.abstract_text.as_deref(),
            Some("Line one. Line two with a comma, and \"quotes\".")
        );
        assert_eq!(record.publication_year, Some(2026));
        assert_eq!(record.journal.as_deref(), Some("Journal & Name"));
        let authors = record
            .authors
            .iter()
            .map(|author| {
                (
                    author.family.clone().unwrap_or_default(),
                    author.given.clone().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            authors,
            vec![
                ("O'Neil & Co.".to_owned(), "Ana Maria".to_owned()),
                ("Smith".to_owned(), "J".to_owned()),
            ]
        );
        assert!(
            record
                .source_identifiers
                .iter()
                .any(|identifier| identifier.normalized_value == "10.1000/deepref.export-7"),
            "the DOI must survive the round trip"
        );
        assert_eq!(record.raw["fields"]["VL"][0], "49");
        assert_eq!(record.raw["fields"]["IS"][0], "3");
        assert_eq!(record.raw["fields"]["SP"][0], "414");
        assert_eq!(record.raw["fields"]["EP"][0], "418");
        Ok(())
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn bibtex_escapes_specials_keeps_identifiers_verbatim_and_stays_parseable() -> anyhow::Result<()>
    {
        let bib = render_reports_bib(&[sample_report(Some("article"))]);
        assert!(bib.starts_with("@article{report-"), "{bib}");
        assert!(bib.contains("  title = {A \\textbraceleft{}multiline\\textbraceright{} title \\& more: Müller's café},\n"));
        assert!(bib.contains("  author = {O'Neil \\& Co., Ana Maria and Smith, J},\n"));
        assert!(bib.contains("  journal = {Journal \\& Name},\n"));
        assert!(bib.contains("  volume = {49},\n"));
        assert!(bib.contains("  number = {3},\n"));
        assert!(bib.contains("  pages = {414--418},\n"));
        assert!(bib.contains("  abstract = {Background \\textbraceleft{}x\\textbraceright{} \\& y. Results: 50\\% of 2213 participants; \\_p\\_ = .01 \\#1 \\textasciitilde{} \\$5 \\textasciicircum{}2 \\textbackslash{} end.},\n"));
        assert!(bib.contains("  doi = {10.1000/deepref.export-7},\n"));
        assert!(bib.contains("  url = {https://example.test/a?x=1&y=2},\n"));
        let records = parse_import(bib.as_bytes(), ImportFormat::Bibtex, None)?;
        assert_eq!(records.len(), 1, "BibTeX must parse back to one record");
        assert_eq!(records[0].publication_year, Some(2026));
        Ok(())
    }

    #[test]
    #[allow(clippy::panic_in_result_fn)]
    fn bibtex_braces_stay_balanced_for_unbalanced_source_text() -> anyhow::Result<()> {
        let mut report = sample_report(Some("book-chapter"));
        report.title = Some("Unbalanced { brace in title".to_owned());
        report.abstract_text = Some("Closing } only".to_owned());
        let bib = render_reports_bib(&[report]);
        assert!(bib.starts_with("@incollection{report-"));
        let opening = bib.matches('{').count();
        let closing = bib.matches('}').count();
        assert_eq!(
            opening, closing,
            "every braced BibTeX value must balance: {bib}"
        );
        let records = parse_import(bib.as_bytes(), ImportFormat::Bibtex, None)?;
        assert_eq!(records.len(), 1);
        Ok(())
    }
}

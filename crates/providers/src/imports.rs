use biblatex::{Bibliography, ChunksExt};
use biblio::Record;
use deepref_application::{
    CsvColumnMapping, ImportError, ImportParser, RawAuthor, RawIdentifier, RawRecord,
};
use deepref_domain::{IdentifierScheme, ImportFormat, normalize_doi};
use serde_json::{Map, Value, json};
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone)]
pub struct ImportParserAdapter {
    format: ImportFormat,
    csv_mapping: Option<CsvColumnMapping>,
}

impl ImportParserAdapter {
    pub fn new(format: ImportFormat, csv_mapping: Option<CsvColumnMapping>) -> Self {
        Self {
            format,
            csv_mapping,
        }
    }
}

impl ImportParser for ImportParserAdapter {
    fn format(&self) -> ImportFormat {
        self.format
    }

    fn parse(&self, bytes: &[u8]) -> Result<Vec<RawRecord>, ImportError> {
        parse_import(bytes, self.format, self.csv_mapping.clone())
    }
}

pub fn parse_import(
    bytes: &[u8],
    format: ImportFormat,
    csv_mapping: Option<CsvColumnMapping>,
) -> Result<Vec<RawRecord>, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|error| ImportError::Invalid {
        format: format.as_str().to_owned(),
        message: format!("input is not UTF-8: {error}"),
    })?;
    match format {
        ImportFormat::Doi => parse_doi_list(text),
        ImportFormat::Ris => parse_biblio(text, format, biblio::ris::parse),
        ImportFormat::Nbib => parse_biblio(text, format, biblio::nbib::parse),
        ImportFormat::Bibtex => parse_bibtex(text),
        ImportFormat::Csv => parse_csv(text, csv_mapping),
    }
}

fn parse_doi_list(input: &str) -> Result<Vec<RawRecord>, ImportError> {
    let mut records = Vec::new();
    for line in input.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') || line.eq_ignore_ascii_case("doi") {
            continue;
        }
        for candidate in line.split([',', ';', '\t']).map(str::trim) {
            if candidate.is_empty() || candidate.eq_ignore_ascii_case("doi") {
                continue;
            }
            let doi = normalize_doi(candidate).map_err(|error| ImportError::Invalid {
                format: ImportFormat::Doi.as_str().to_owned(),
                message: error.to_string(),
            })?;
            records.push(RawRecord {
                source_identifiers: vec![RawIdentifier {
                    scheme: IdentifierScheme::Doi,
                    value: candidate.to_owned(),
                    normalized_value: doi,
                }],
                title: None,
                abstract_text: None,
                authors: Vec::new(),
                publication_year: None,
                journal: None,
                raw: json!({ "format": "doi", "value": candidate }),
            });
        }
    }
    if records.is_empty() {
        return Err(ImportError::Invalid {
            format: ImportFormat::Doi.as_str().to_owned(),
            message: "no DOI records found".to_owned(),
        });
    }
    Ok(records)
}

fn parse_biblio(
    input: &str,
    format: ImportFormat,
    parser: impl Fn(&str) -> Result<Vec<Record>, biblio::Error>,
) -> Result<Vec<RawRecord>, ImportError> {
    let records = parser(input).map_err(|error| ImportError::Invalid {
        format: format.as_str().to_owned(),
        message: error.to_string(),
    })?;
    if records.is_empty() {
        return Err(ImportError::Invalid {
            format: format.as_str().to_owned(),
            message: "no records found".to_owned(),
        });
    }
    Ok(records
        .into_iter()
        .map(|record| raw_record_from_biblio(format, record))
        .collect())
}

fn parse_bibtex(input: &str) -> Result<Vec<RawRecord>, ImportError> {
    match biblio::bibtex::parse(input) {
        Ok(records) if !records.is_empty() => Ok(records
            .into_iter()
            .map(|record| raw_record_from_biblio(ImportFormat::Bibtex, record))
            .collect()),
        Ok(_) => Err(invalid(ImportFormat::Bibtex, "no records found")),
        Err(primary_error) => parse_biblatex_fallback(input).map_err(|fallback_error| {
            invalid(
                ImportFormat::Bibtex,
                &format!("{primary_error}; fallback: {fallback_error}"),
            )
        }),
    }
}

fn parse_biblatex_fallback(input: &str) -> Result<Vec<RawRecord>, String> {
    let bibliography = Bibliography::parse(input).map_err(|error| error.to_string())?;
    let mut records = Vec::new();
    for entry in bibliography.iter() {
        let title = entry
            .get("title")
            .or_else(|| entry.get("booktitle"))
            .or_else(|| entry.get("shorttitle"))
            .map(ChunksExt::format_verbatim);
        let authors = entry
            .get("author")
            .map(ChunksExt::format_verbatim)
            .map(|value| split_authors(&value))
            .unwrap_or_default();
        let doi = entry.get("doi").map(ChunksExt::format_verbatim);
        let source_identifiers = doi
            .into_iter()
            .filter_map(|value| normalized_identifier(IdentifierScheme::Doi, &value))
            .collect();
        let year = entry
            .get("year")
            .map(ChunksExt::format_verbatim)
            .and_then(|value| value.trim().parse().ok());
        let journal = entry
            .get("journaltitle")
            .or_else(|| entry.get("journal"))
            .map(ChunksExt::format_verbatim);
        let mut fields = Map::new();
        for (key, value) in &entry.fields {
            fields.insert(key.clone(), Value::String(value.format_verbatim()));
        }
        records.push(RawRecord {
            source_identifiers,
            title: title.map(|value| normalize_text(&value)),
            abstract_text: entry
                .get("abstract")
                .map(ChunksExt::format_verbatim)
                .map(|value| normalize_text(&value)),
            authors,
            publication_year: year,
            journal: journal.map(|value| normalize_text(&value)),
            raw: json!({ "format": "bibtex", "key": entry.key, "fields": fields, "parser": "biblatex-fallback" }),
        });
    }
    if records.is_empty() {
        return Err("no records found".to_owned());
    }
    Ok(records)
}

fn parse_csv(
    input: &str,
    mapping: Option<CsvColumnMapping>,
) -> Result<Vec<RawRecord>, ImportError> {
    let mapping = mapping.ok_or(ImportError::MissingCsvMapping)?;
    if mapping.is_empty() {
        return Err(ImportError::MissingCsvMapping);
    }
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(input.as_bytes());
    let headers = reader
        .headers()
        .map_err(|error| invalid(ImportFormat::Csv, &error.to_string()))?
        .clone();
    for column in mapped_columns(&mapping) {
        if !headers.iter().any(|header| header == column) {
            return Err(ImportError::MissingCsvColumn(column.to_owned()));
        }
    }

    let mut records = Vec::new();
    for row in reader.records() {
        let row = row.map_err(|error| invalid(ImportFormat::Csv, &error.to_string()))?;
        let field = |column: &Option<String>| {
            column
                .as_ref()
                .and_then(|name| headers.iter().position(|header| header == name))
                .and_then(|index| row.get(index))
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        };
        let mut identifiers = Vec::new();
        if let Some(value) = field(&mapping.doi)
            && let Some(identifier) = normalized_identifier(IdentifierScheme::Doi, &value)
        {
            identifiers.push(identifier);
        }
        if let Some(value) = field(&mapping.pmid)
            && let Some(identifier) = normalized_identifier(IdentifierScheme::Pmid, &value)
        {
            identifiers.push(identifier);
        }
        if let Some(value) = field(&mapping.pmcid)
            && let Some(identifier) = normalized_identifier(IdentifierScheme::Pmcid, &value)
        {
            identifiers.push(identifier);
        }
        let authors = field(&mapping.authors)
            .map(|value| {
                value
                    .split([';', '|'])
                    .map(str::trim)
                    .filter(|author| !author.is_empty())
                    .map(parse_author)
                    .collect()
            })
            .unwrap_or_default();
        let year = field(&mapping.publication_year).and_then(|value| value.parse().ok());
        let mut raw = Map::new();
        for (header, value) in headers.iter().zip(row.iter()) {
            raw.insert(header.to_owned(), Value::String(value.to_owned()));
        }
        records.push(RawRecord {
            source_identifiers: identifiers,
            title: field(&mapping.title).map(|value| normalize_text(&value)),
            abstract_text: field(&mapping.abstract_text).map(|value| normalize_text(&value)),
            authors,
            publication_year: year,
            journal: field(&mapping.journal).map(|value| normalize_text(&value)),
            raw: Value::Object(raw),
        });
    }
    if records.is_empty() {
        return Err(invalid(ImportFormat::Csv, "no records found"));
    }
    Ok(records)
}

fn raw_record_from_biblio(format: ImportFormat, record: Record) -> RawRecord {
    let title = record.title.clone();
    let abstract_text = record.abstract_text.clone();
    let authors = record.authors.clone();
    let journal = record.journal.clone();
    let doi = record.doi.clone();
    let year = record.date.map(|date| date.year);
    let mut fields = Map::new();
    for (key, value) in &record.extras {
        fields.insert(key.clone(), Value::String(value.clone()));
    }
    let mut identifiers = Vec::new();
    if let Some(value) = &doi
        && let Some(identifier) = normalized_identifier(IdentifierScheme::Doi, value)
    {
        identifiers.push(identifier);
    }
    for (key, scheme) in [
        ("PMID", IdentifierScheme::Pmid),
        ("PMC", IdentifierScheme::Pmcid),
    ] {
        if let Some(value) = record.extras.get(key)
            && let Some(identifier) = normalized_identifier(scheme, value)
        {
            identifiers.push(identifier);
        }
    }
    RawRecord {
        source_identifiers: identifiers,
        title: nonempty(title.clone()).map(|value| normalize_text(&value)),
        abstract_text: abstract_text.clone().map(|value| normalize_text(&value)),
        authors: authors.iter().map(|value| parse_author(value)).collect(),
        publication_year: year,
        journal: journal.clone().map(|value| normalize_text(&value)),
        raw: json!({
            "format": format.as_str(),
            "fields": fields,
            "doi": doi,
            "title": title,
            "abstract": abstract_text,
            "authors": authors,
            "year": year,
            "journal": journal,
        }),
    }
}

fn normalized_identifier(scheme: IdentifierScheme, value: &str) -> Option<RawIdentifier> {
    let normalized = match scheme {
        IdentifierScheme::Doi => normalize_doi(value).ok()?,
        _ => value
            .trim()
            .trim_start_matches("PMID:")
            .trim_start_matches("PMC")
            .to_lowercase(),
    };
    if normalized.is_empty() {
        None
    } else {
        Some(RawIdentifier {
            scheme,
            value: value.to_owned(),
            normalized_value: normalized,
        })
    }
}

fn parse_author(value: &str) -> RawAuthor {
    let value = normalize_text(value);
    if let Some((family, given)) = value.split_once(',') {
        RawAuthor::named(
            Some(given.trim().to_owned()).filter(|value| !value.is_empty()),
            Some(family.trim().to_owned()).filter(|value| !value.is_empty()),
        )
    } else {
        let mut words = value.split_whitespace().collect::<Vec<_>>();
        match words.pop() {
            Some(family) if !words.is_empty() => {
                RawAuthor::named(Some(words.join(" ")), Some(family.to_owned()))
            }
            _ => RawAuthor::literal(value),
        }
    }
}

fn split_authors(value: &str) -> Vec<RawAuthor> {
    value
        .split(" and ")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_author)
        .collect()
}

fn normalize_text(value: &str) -> String {
    value
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn mapped_columns(mapping: &CsvColumnMapping) -> impl Iterator<Item = &str> {
    [
        mapping.doi.as_deref(),
        mapping.pmid.as_deref(),
        mapping.pmcid.as_deref(),
        mapping.title.as_deref(),
        mapping.abstract_text.as_deref(),
        mapping.authors.as_deref(),
        mapping.publication_year.as_deref(),
        mapping.journal.as_deref(),
    ]
    .into_iter()
    .flatten()
}

fn invalid(format: ImportFormat, message: &str) -> ImportError {
    ImportError::Invalid {
        format: format.as_str().to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping() -> CsvColumnMapping {
        CsvColumnMapping {
            doi: Some("DOI".to_owned()),
            pmid: Some("PMID".to_owned()),
            pmcid: None,
            title: Some("Title".to_owned()),
            abstract_text: Some("Abstract".to_owned()),
            authors: Some("Authors".to_owned()),
            publication_year: Some("Year".to_owned()),
            journal: Some("Journal".to_owned()),
        }
    }

    #[test]
    fn golden_fixtures_use_one_raw_record_shape() {
        let cases = [
            (
                ImportFormat::Doi,
                include_bytes!("../tests/fixtures/doi.txt").as_slice(),
                None,
            ),
            (
                ImportFormat::Ris,
                include_bytes!("../tests/fixtures/sample.ris").as_slice(),
                None,
            ),
            (
                ImportFormat::Bibtex,
                include_bytes!("../tests/fixtures/sample.bib").as_slice(),
                None,
            ),
            (
                ImportFormat::Bibtex,
                include_bytes!("../tests/fixtures/malformed.bib").as_slice(),
                None,
            ),
            (
                ImportFormat::Nbib,
                include_bytes!("../tests/fixtures/sample.nbib").as_slice(),
                None,
            ),
            (
                ImportFormat::Csv,
                include_bytes!("../tests/fixtures/sample.csv").as_slice(),
                Some(mapping()),
            ),
        ];
        for (format, input, csv_mapping) in cases {
            let records = parse_import(input, format, csv_mapping).expect("fixture parses");
            assert!(
                !records.is_empty(),
                "{format:?} fixture should emit records"
            );
            assert!(records.iter().all(|record| record.raw.is_object()));
        }
    }

    #[test]
    fn csv_requires_explicit_mapping_and_preserves_duplicate_rows() {
        let input = include_bytes!("../tests/fixtures/sample.csv");
        assert!(matches!(
            parse_import(input, ImportFormat::Csv, None),
            Err(ImportError::MissingCsvMapping)
        ));
        let records = parse_import(input, ImportFormat::Csv, Some(mapping())).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].authors[0].family.as_deref(), Some("Müller"));
    }

    #[test]
    fn each_golden_fixture_has_expected_raw_record_values() {
        let dois = parse_import(
            include_bytes!("../tests/fixtures/doi.txt"),
            ImportFormat::Doi,
            None,
        )
        .unwrap();
        assert_eq!(dois.len(), 2);
        assert_eq!(
            dois[1].source_identifiers[0].normalized_value,
            "10.5555/example-second"
        );

        let ris = parse_import(
            include_bytes!("../tests/fixtures/sample.ris"),
            ImportFormat::Ris,
            None,
        )
        .unwrap()
        .remove(0);
        assert_eq!(ris.title.as_deref(), Some("Über die Wirkung von Kaffee"));
        assert_eq!(ris.publication_year, Some(2024));
        assert_eq!(ris.authors[0].family.as_deref(), Some("Müller"));

        let bib = parse_import(
            include_bytes!("../tests/fixtures/sample.bib"),
            ImportFormat::Bibtex,
            None,
        )
        .unwrap()
        .remove(0);
        assert_eq!(bib.journal.as_deref(), Some("Journal of Café Studies"));
        assert_eq!(
            bib.source_identifiers[0].normalized_value,
            "10.5555/example-unicode"
        );

        let fallback = parse_import(
            include_bytes!("../tests/fixtures/malformed.bib"),
            ImportFormat::Bibtex,
            None,
        )
        .unwrap()
        .remove(0);
        assert_eq!(
            fallback.title.as_deref(),
            Some("Über die Wirkung von Kaffee")
        );
        assert_eq!(fallback.raw["parser"], "biblatex-fallback");

        let nbib = parse_import(
            include_bytes!("../tests/fixtures/sample.nbib"),
            ImportFormat::Nbib,
            None,
        )
        .unwrap()
        .remove(0);
        assert_eq!(nbib.title.as_deref(), Some("Über die Wirkung von Kaffee"));
        assert_eq!(nbib.publication_year, Some(2024));
        assert_eq!(nbib.journal.as_deref(), Some("Journal of Café Studies"));
        assert!(nbib.source_identifiers.iter().any(|identifier| {
            identifier.scheme == IdentifierScheme::Pmid && identifier.normalized_value == "12345678"
        }));

        let csv = parse_import(
            include_bytes!("../tests/fixtures/sample.csv"),
            ImportFormat::Csv,
            Some(mapping()),
        )
        .unwrap()
        .remove(0);
        assert_eq!(csv.title.as_deref(), Some("Über die Wirkung von Kaffee"));
        assert_eq!(csv.publication_year, Some(2024));
        assert_eq!(csv.authors[0].family.as_deref(), Some("Müller"));
    }

    #[test]
    fn equivalent_imports_normalize_to_the_same_semantic_fields() {
        let ris = parse_import(
            include_bytes!("../tests/fixtures/sample.ris"),
            ImportFormat::Ris,
            None,
        )
        .unwrap()
        .remove(0);
        let bib = parse_import(
            include_bytes!("../tests/fixtures/sample.bib"),
            ImportFormat::Bibtex,
            None,
        )
        .unwrap()
        .remove(0);
        assert_eq!(ris.source_identifiers, bib.source_identifiers);
        assert_eq!(ris.title, bib.title);
        assert_eq!(ris.abstract_text, bib.abstract_text);
        assert_eq!(ris.authors, bib.authors);
        assert_eq!(ris.publication_year, bib.publication_year);
        assert_eq!(ris.journal, bib.journal);
    }
}

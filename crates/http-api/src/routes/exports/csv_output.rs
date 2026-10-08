//! RFC 4180 record writing shared by the CSV exports.

/// Renders the header row. Column names are plain identifiers, so they are written unquoted.
pub(super) fn header(columns: &[&str]) -> String {
    let mut line = columns.join(",");
    line.push_str("\r\n");
    line
}

/// Renders one record: every field is quoted, embedded quotes are doubled, and the record ends
/// with CRLF. Embedded commas, quotes and line breaks therefore survive a round trip.
pub(super) fn record<S: AsRef<str>>(fields: &[S]) -> String {
    let mut line = String::new();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            line.push(',');
        }
        line.push('"');
        line.push_str(&neutralise_formula(field.as_ref()).replace('"', "\"\""));
        line.push('"');
    }
    line.push_str("\r\n");
    line
}

/// Titles, abstracts and notes come from outside sources. A spreadsheet runs a cell that
/// starts with `=`, `+`, `-` or `@` as a formula, so such text gets a leading apostrophe.
/// Plain numbers (for example `-1.5`) are left as they are.
fn neutralise_formula(field: &str) -> std::borrow::Cow<'_, str> {
    let risky = field.starts_with(['=', '+', '-', '@', '\t', '\r']);
    if risky && field.trim().parse::<f64>().is_err() {
        std::borrow::Cow::Owned(format!("'{field}"))
    } else {
        std::borrow::Cow::Borrowed(field)
    }
}

#[cfg(test)]
mod tests {
    use ::csv::ReaderBuilder;

    use super::record;

    fn parse(text: &str) -> Vec<Vec<String>> {
        ReaderBuilder::new()
            .has_headers(false)
            .from_reader(text.as_bytes())
            .records()
            .map(|row| row.map(|row| row.iter().map(str::to_owned).collect::<Vec<String>>()))
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default()
    }

    #[test]
    fn header_is_unquoted_and_crlf_terminated() {
        assert_eq!(super::header(&["id", "created_at"]), "id,created_at\r\n");
    }

    #[test]
    fn records_quote_every_field_and_end_with_crlf() {
        assert_eq!(record(&["a", "b"]), "\"a\",\"b\"\r\n");
        assert_eq!(record::<&str>(&[]), "\r\n");
    }

    #[test]
    fn embedded_commas_quotes_newlines_and_accents_round_trip() {
        let fields = [
            "Smith, J. \"quoted\" title",
            "line one\nline two\r\nthree",
            "Müller & Ösund — café",
            "",
        ];
        let text = record(&fields);
        assert_eq!(
            text,
            "\"Smith, J. \"\"quoted\"\" title\",\"line one\nline two\r\nthree\",\"Müller & Ösund — café\",\"\"\r\n"
        );
        let parsed = parse(&text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0], fields);
    }

    #[test]
    fn text_that_a_spreadsheet_would_run_as_a_formula_is_neutralised() {
        let text = record(&[
            "=HYPERLINK(\"x\")",
            "+cmd",
            "@SUM(A1)",
            "-1.5",
            "+2",
            "plain",
        ]);
        let parsed = parse(&text);
        assert_eq!(
            parsed[0],
            [
                "'=HYPERLINK(\"x\")",
                "'+cmd",
                "'@SUM(A1)",
                "-1.5",
                "+2",
                "plain"
            ]
        );
    }

    #[test]
    fn header_and_rows_form_a_parseable_table() {
        let mut csv = record(&["id", "note"]);
        csv.push_str(&record(&["1", "first, with comma"]));
        csv.push_str(&record(&["2", "second \"with quotes\""]));
        let parsed = parse(&csv);
        assert_eq!(
            parsed,
            vec![
                vec!["id".to_owned(), "note".to_owned()],
                vec!["1".to_owned(), "first, with comma".to_owned()],
                vec!["2".to_owned(), "second \"with quotes\"".to_owned()],
            ]
        );
    }
}

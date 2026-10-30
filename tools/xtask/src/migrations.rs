use std::{fmt, path::Path, process::Command};

use anyhow::{Context, Result, bail};

use crate::workspace::find_root;

const DEFAULT_BASE: &str = "origin/main";
const MIGRATIONS_DIR: &str = "crates/app/migrations";
const MAX_SNIPPET_CHARS: usize = 120;

const DROP_OBJECTS: &[&str] = &[
    "access",
    "aggregate",
    "cast",
    "collation",
    "column",
    "constraint",
    "database",
    "domain",
    "extension",
    "foreign",
    "function",
    "index",
    "language",
    "materialized",
    "operator",
    "owned",
    "policy",
    "procedure",
    "publication",
    "role",
    "rule",
    "schema",
    "sequence",
    "server",
    "statistics",
    "subscription",
    "table",
    "tablespace",
    "transform",
    "trigger",
    "type",
    "view",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckOptions {
    base: String,
    report_only: bool,
}

impl CheckOptions {
    pub fn new(base: impl Into<String>, report_only: bool) -> Self {
        Self {
            base: base.into(),
            report_only,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub file: String,
    pub rule: &'static str,
    pub line: usize,
    pub snippet: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}: {}: line {}: {}",
            self.file, self.rule, self.line, self.snippet
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    pub status: char,
    pub path: String,
    pub old_path: Option<String>,
}

pub fn run(options: CheckOptions) -> Result<()> {
    let root = find_root()?;
    let violations = collect_violations(&root, &options.base)?;

    for violation in &violations {
        println!("{violation}");
    }

    if violations.is_empty() {
        println!("migrations check: clean against {}", options.base);
        return Ok(());
    }

    if options.report_only {
        println!(
            "migrations check: {} violation(s) against {} (report-only; not failing)",
            violations.len(),
            options.base
        );
        return Ok(());
    }

    bail!(
        "migrations check: {} violation(s) against {}",
        violations.len(),
        options.base
    )
}

pub fn parse_check_args(args: &[String]) -> Result<CheckOptions> {
    let mut base = DEFAULT_BASE.to_owned();
    let mut report_only = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--base" => {
                index += 1;
                let value = args
                    .get(index)
                    .context("'migrations check --base' requires a git ref")?;
                base = value.clone();
            }
            "--report-only" => report_only = true,
            other => bail!("unknown option for 'migrations check': {other}"),
        }
        index += 1;
    }

    Ok(CheckOptions::new(base, report_only))
}

fn collect_violations(root: &Path, base: &str) -> Result<Vec<Violation>> {
    let mut violations = append_only_violations(&git_diff(root, base, "MDR")?);

    for entry in git_diff(root, base, "A")? {
        if !is_migration_path(&entry.path) {
            continue;
        }
        let path = root.join(&entry.path);
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read added migration {}", path.display()))?;
        violations.extend(scan_migration(&entry.path, &contents));
    }

    Ok(violations)
}

fn git_diff(root: &Path, base: &str, filter: &str) -> Result<Vec<DiffEntry>> {
    let diff_filter = format!("--diff-filter={filter}");
    let output = Command::new("git")
        .args([
            "diff",
            "--name-status",
            &diff_filter,
            base,
            "--",
            MIGRATIONS_DIR,
        ])
        .current_dir(root)
        .output()
        .context("failed to run 'git diff' for migrations")?;

    if !output.status.success() {
        bail!(
            "'git diff' against base '{base}' failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    Ok(parse_diff_name_status(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

pub fn parse_diff_name_status(output: &str) -> Vec<DiffEntry> {
    output
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let status = fields.next()?.chars().next()?;
            let first_path = fields.next()?;

            if status == 'R' || status == 'C' {
                let new_path = fields.next()?;
                Some(DiffEntry {
                    status,
                    path: new_path.to_owned(),
                    old_path: Some(first_path.to_owned()),
                })
            } else {
                Some(DiffEntry {
                    status,
                    path: first_path.to_owned(),
                    old_path: None,
                })
            }
        })
        .collect()
}

pub fn append_only_violations(entries: &[DiffEntry]) -> Vec<Violation> {
    entries
        .iter()
        .filter(|entry| is_migration_path(&entry.path))
        .filter_map(|entry| {
            let snippet = match entry.status {
                'M' => "modified migration file; add a new migration instead".to_owned(),
                'D' => "deleted migration file; applied migrations are immutable".to_owned(),
                'R' => format!(
                    "renamed from {}; add a new migration instead",
                    entry.old_path.as_deref().unwrap_or("unknown path")
                ),
                _ => return None,
            };
            Some(Violation {
                file: entry.path.clone(),
                rule: "append-only",
                line: 1,
                snippet,
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Expand,
    Contract,
}

pub fn scan_migration(file: &str, contents: &str) -> Vec<Violation> {
    let mut violations = Vec::new();
    let header = leading_header_lines(contents);

    let mut phase = None;
    let mut invalid_phase = None;
    let mut has_release_precondition = false;

    for (line_number, line) in &header {
        if phase.is_none()
            && invalid_phase.is_none()
            && let Some(value) = header_directive_value(line, "phase")
        {
            if value.eq_ignore_ascii_case("expand") {
                phase = Some(Phase::Expand);
            } else if value.eq_ignore_ascii_case("contract") {
                phase = Some(Phase::Contract);
            } else {
                invalid_phase = Some((*line_number, *line));
            }
        }

        if header_directive_value(line, "requires-release-out-of-service")
            .is_some_and(|value| !value.is_empty())
        {
            has_release_precondition = true;
        }
    }

    if phase.is_none() {
        match invalid_phase {
            Some((line_number, line)) => violations.push(Violation {
                file: file.to_owned(),
                rule: "phase-header",
                line: line_number,
                snippet: format!(
                    "{} (expected '-- phase: expand' or '-- phase: contract')",
                    line.trim()
                ),
            }),
            None => violations.push(Violation {
                file: file.to_owned(),
                rule: "phase-header",
                line: 1,
                snippet: "missing '-- phase: expand' or '-- phase: contract' header".to_owned(),
            }),
        }
    }

    let contract_exempt = phase == Some(Phase::Contract) && has_release_precondition;
    if !contract_exempt {
        for (line_number, kind) in destructive_findings(contents) {
            violations.push(Violation {
                file: file.to_owned(),
                rule: "destructive-sql",
                line: line_number,
                snippet: format!("{kind}: {}", line_snippet(contents, line_number)),
            });
        }
    }

    violations
}

fn leading_header_lines(contents: &str) -> Vec<(usize, &str)> {
    let mut header = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            header.push((index + 1, line));
        } else {
            break;
        }
    }
    header
}

fn header_directive_value<'a>(line: &'a str, directive: &str) -> Option<&'a str> {
    let comment = line.trim().strip_prefix("--")?.trim();
    let (name, value) = comment.split_once(':')?;
    if name.trim().eq_ignore_ascii_case(directive) {
        Some(value.trim())
    } else {
        None
    }
}

fn is_migration_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|extension| extension == "sql")
}

fn destructive_findings(contents: &str) -> Vec<(usize, &'static str)> {
    let tokens = tokenize(&strip_comments_and_strings(contents));
    let widened_checks = check_widening_drops(&tokens);
    let mut findings = Vec::new();

    for (index, token) in tokens.iter().enumerate() {
        match token.word.as_str() {
            "drop"
                if !widened_checks.contains(&index)
                    && tokens
                        .get(index + 1)
                        .is_some_and(|next| DROP_OBJECTS.contains(&next.word.as_str())) =>
            {
                findings.push((token.line, "DROP"));
            }
            "rename" => findings.push((token.line, "RENAME")),
            "set"
                if token_at(&tokens, index + 1, "not") && token_at(&tokens, index + 2, "null") =>
            {
                findings.push((token.line, "SET NOT NULL"));
            }
            "alter" => {
                if is_alter_column_type(&tokens, index) {
                    findings.push((token.line, "ALTER COLUMN ... TYPE"));
                }
                if is_enum_value_removal(&tokens, index) {
                    findings.push((token.line, "enum value removal"));
                }
            }
            "delete" | "update"
                if tokens
                    .iter()
                    .skip(index + 1)
                    .take(3)
                    .any(|candidate| candidate.word == "pg_enum") =>
            {
                findings.push((token.line, "enum value removal (pg_enum)"));
            }
            _ => {}
        }
    }

    findings
}

/// Words that end the table name of `ALTER TABLE` and start its first action.
const ALTER_TABLE_ACTIONS: &[&str] =
    &["add", "alter", "drop", "owner", "rename", "set", "validate"];

/// Token indexes of `DROP CONSTRAINT x` that are the expand half of a CHECK
/// widening: the same file later runs `ALTER TABLE` on the same table with
/// `ADD CONSTRAINT x CHECK (...) NOT VALID`. The new constraint only skips
/// existing rows until a separate migration validates it, so old releases keep
/// writing rows the wider rule accepts. Any other constraint drop, a re-add
/// without `NOT VALID` or a non-CHECK re-add stays a finding. Quoted names are
/// stripped before tokenizing, so they never match and stay findings.
fn check_widening_drops(tokens: &[Token]) -> Vec<usize> {
    let mut drops = Vec::new();
    let mut adds = Vec::new();
    let mut start = 0;
    while start < tokens.len() {
        let end = tokens[start..]
            .iter()
            .position(|token| token.word == ";")
            .map_or(tokens.len(), |offset| start + offset);
        collect_constraint_clauses(tokens, start, end, &mut drops, &mut adds);
        start = end + 1;
    }
    drops
        .into_iter()
        .filter(|(index, table, name)| {
            adds.iter().any(|(add_index, add_table, add_name)| {
                add_index > index && add_table == table && add_name == name
            })
        })
        .map(|(index, _, _)| index)
        .collect()
}

type ConstraintClause = (usize, String, String);

fn collect_constraint_clauses(
    tokens: &[Token],
    start: usize,
    end: usize,
    drops: &mut Vec<ConstraintClause>,
    adds: &mut Vec<ConstraintClause>,
) {
    if !(token_at(tokens, start, "alter") && token_at(tokens, start + 1, "table")) {
        return;
    }
    let mut index = start + 2;
    if token_at(tokens, index, "if") && token_at(tokens, index + 1, "exists") {
        index += 2;
    }
    if token_at(tokens, index, "only") {
        index += 1;
    }
    let name_start = index;
    while index < end && !ALTER_TABLE_ACTIONS.contains(&tokens[index].word.as_str()) {
        index += 1;
    }
    if index == name_start {
        return;
    }
    let table = tokens[name_start..index]
        .iter()
        .map(|token| token.word.as_str())
        .collect::<Vec<_>>()
        .join(".");

    let clause_starts: Vec<usize> = (index..end)
        .filter(|&position| {
            matches!(
                tokens[position].word.as_str(),
                "add" | "drop" | "alter" | "validate"
            ) && token_at(tokens, position + 1, "constraint")
        })
        .collect();
    for (offset, &position) in clause_starts.iter().enumerate() {
        let clause_end = clause_starts.get(offset + 1).copied().unwrap_or(end);
        let mut name_index = position + 2;
        let verb = tokens[position].word.as_str();
        if verb == "drop" && token_at(tokens, name_index, "if") {
            name_index += 2;
        }
        let Some(name) = tokens.get(name_index).filter(|_| name_index < clause_end) else {
            continue;
        };
        let clause = (position, table.clone(), name.word.clone());
        match verb {
            "drop" => drops.push(clause),
            "add" => {
                let is_check = token_at(tokens, name_index + 1, "check");
                let not_valid = (name_index + 1..clause_end.saturating_sub(1)).any(|candidate| {
                    token_at(tokens, candidate, "not") && token_at(tokens, candidate + 1, "valid")
                });
                if is_check && not_valid {
                    adds.push(clause);
                }
            }
            _ => {}
        }
    }
}

fn is_alter_column_type(tokens: &[Token], index: usize) -> bool {
    if token_at(tokens, index + 1, "column") {
        return tokens
            .iter()
            .skip(index + 2)
            .take(5)
            .any(|token| token.word == "type");
    }

    // PostgreSQL accepts `ALTER TABLE t ALTER c TYPE ...` without the COLUMN keyword.
    tokens.get(index + 1).is_some() && token_at(tokens, index + 2, "type")
}

fn is_enum_value_removal(tokens: &[Token], index: usize) -> bool {
    if !token_at(tokens, index + 1, "type") {
        return false;
    }

    let end = (index + 8).min(tokens.len());
    (index + 2..end).any(|position| {
        token_at(tokens, position, "drop") && token_at(tokens, position + 1, "value")
    })
}

fn token_at(tokens: &[Token], index: usize, word: &str) -> bool {
    tokens.get(index).is_some_and(|token| token.word == word)
}

fn line_snippet(contents: &str, line: usize) -> String {
    let text = contents
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or("")
        .trim();
    let mut snippet: String = text.chars().take(MAX_SNIPPET_CHARS).collect();
    if text.chars().count() > MAX_SNIPPET_CHARS {
        snippet.push_str("...");
    }
    snippet
}

#[derive(Debug)]
struct Token {
    word: String,
    line: usize,
}

fn tokenize(sql: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut current_line = 1;
    let mut line = 1;

    for character in sql.chars() {
        if character == '\n' {
            flush_token(&mut tokens, &mut current, current_line);
            line += 1;
        } else if character.is_alphanumeric() || character == '_' {
            if current.is_empty() {
                current_line = line;
            }
            current.push(character.to_ascii_lowercase());
        } else if character == ';' {
            // Statement boundaries let the CHECK-widening rule stay inside
            // one `ALTER TABLE`.
            flush_token(&mut tokens, &mut current, current_line);
            tokens.push(Token {
                word: ";".to_owned(),
                line,
            });
        } else {
            flush_token(&mut tokens, &mut current, current_line);
        }
    }
    flush_token(&mut tokens, &mut current, current_line);

    tokens
}

fn flush_token(tokens: &mut Vec<Token>, current: &mut String, line: usize) {
    if !current.is_empty() {
        tokens.push(Token {
            word: std::mem::take(current),
            line,
        });
    }
}

#[derive(Debug)]
enum ScanState {
    Normal,
    LineComment,
    BlockComment(usize),
    SingleQuote,
    DoubleQuote,
    DollarQuote(String),
}

/// Replaces SQL comments and string literals with spaces while preserving line
/// breaks, so token-based checks keep accurate line numbers.
fn strip_comments_and_strings(contents: &str) -> String {
    let characters: Vec<char> = contents.chars().collect();
    let mut sanitized = String::with_capacity(contents.len());
    let mut state = ScanState::Normal;
    let mut index = 0;

    while index < characters.len() {
        let current = characters[index];
        let next = characters.get(index + 1).copied();
        let scan_state = std::mem::replace(&mut state, ScanState::Normal);

        match scan_state {
            ScanState::Normal => {
                if current == '-' && next == Some('-') {
                    state = ScanState::LineComment;
                    sanitized.push_str("  ");
                    index += 2;
                } else if current == '/' && next == Some('*') {
                    state = ScanState::BlockComment(1);
                    sanitized.push_str("  ");
                    index += 2;
                } else if current == '\'' {
                    state = ScanState::SingleQuote;
                    sanitized.push(' ');
                    index += 1;
                } else if current == '"' {
                    state = ScanState::DoubleQuote;
                    sanitized.push(' ');
                    index += 1;
                } else if current == '$'
                    && let Some(end) = dollar_quote_open(&characters, index)
                {
                    state = ScanState::DollarQuote(
                        characters[index + 1..end - 1].iter().copied().collect(),
                    );
                    sanitized.push(' ');
                    index = end;
                } else {
                    sanitized.push(current);
                    index += 1;
                }
            }
            ScanState::LineComment => {
                if current == '\n' {
                    sanitized.push('\n');
                } else {
                    sanitized.push(' ');
                    state = ScanState::LineComment;
                }
                index += 1;
            }
            ScanState::BlockComment(depth) => {
                if current == '/' && next == Some('*') {
                    state = ScanState::BlockComment(depth + 1);
                    sanitized.push_str("  ");
                    index += 2;
                } else if current == '*' && next == Some('/') {
                    if depth > 1 {
                        state = ScanState::BlockComment(depth - 1);
                    }
                    sanitized.push_str("  ");
                    index += 2;
                } else {
                    sanitized.push(if current == '\n' { '\n' } else { ' ' });
                    state = ScanState::BlockComment(depth);
                    index += 1;
                }
            }
            ScanState::SingleQuote => {
                if current == '\'' {
                    if next == Some('\'') {
                        sanitized.push_str("  ");
                        index += 2;
                    } else {
                        sanitized.push(' ');
                        index += 1;
                    }
                    state = if next == Some('\'') {
                        ScanState::SingleQuote
                    } else {
                        ScanState::Normal
                    };
                } else {
                    sanitized.push(if current == '\n' { '\n' } else { ' ' });
                    state = ScanState::SingleQuote;
                    index += 1;
                }
            }
            ScanState::DoubleQuote => {
                if current == '"' {
                    if next == Some('"') {
                        sanitized.push_str("  ");
                        index += 2;
                    } else {
                        sanitized.push(' ');
                        index += 1;
                    }
                    state = if next == Some('"') {
                        ScanState::DoubleQuote
                    } else {
                        ScanState::Normal
                    };
                } else {
                    sanitized.push(if current == '\n' { '\n' } else { ' ' });
                    state = ScanState::DoubleQuote;
                    index += 1;
                }
            }
            ScanState::DollarQuote(tag) => {
                if current == '$' && dollar_quote_closes(&characters, index, &tag) {
                    sanitized.push(' ');
                    index += tag.chars().count() + 2;
                } else {
                    sanitized.push(if current == '\n' { '\n' } else { ' ' });
                    state = ScanState::DollarQuote(tag);
                    index += 1;
                }
            }
        }
    }

    sanitized
}

fn dollar_quote_open(characters: &[char], start: usize) -> Option<usize> {
    let tag_start = start + 1;
    if characters.get(tag_start) == Some(&'$') {
        return Some(tag_start + 1);
    }

    let first = *characters.get(tag_start)?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }

    let mut index = tag_start + 1;
    while let Some(&character) = characters.get(index) {
        if character == '$' {
            return Some(index + 1);
        }
        if !(character.is_ascii_alphanumeric() || character == '_') {
            return None;
        }
        index += 1;
    }

    None
}

fn dollar_quote_closes(characters: &[char], start: usize, tag: &str) -> bool {
    let mut index = start + 1;
    for tag_character in tag.chars() {
        if characters.get(index) != Some(&tag_character) {
            return false;
        }
        index += 1;
    }
    characters.get(index) == Some(&'$')
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPAND_ADD_COLUMN: &str = "\
-- phase: expand
-- Add a nullable column without rewriting existing rows.
ALTER TABLE widgets ADD COLUMN note text;
";

    const CONTRACT_DROP: &str = "\
-- phase: contract
-- requires-release-out-of-service: 2026.10
DROP TABLE old_widgets;
";

    #[test]
    fn modified_deleted_and_renamed_migrations_violate_append_only() {
        let entries = vec![
            DiffEntry {
                status: 'M',
                path: "crates/app/migrations/0001_initial.sql".to_owned(),
                old_path: None,
            },
            DiffEntry {
                status: 'D',
                path: "crates/app/migrations/0002_metrics.sql".to_owned(),
                old_path: None,
            },
            DiffEntry {
                status: 'R',
                path: "crates/app/migrations/0003_renamed.sql".to_owned(),
                old_path: Some("crates/app/migrations/0003_original.sql".to_owned()),
            },
        ];

        let violations = append_only_violations(&entries);

        assert_eq!(violations.len(), 3);
        assert!(
            violations
                .iter()
                .all(|violation| violation.rule == "append-only")
        );
        assert!(violations[2].snippet.contains("0003_original.sql"));
    }

    #[test]
    fn added_non_sql_files_are_not_append_only_violations() {
        let entries = vec![DiffEntry {
            status: 'M',
            path: "crates/app/migrations/README.md".to_owned(),
            old_path: None,
        }];

        assert!(append_only_violations(&entries).is_empty());
    }

    #[test]
    fn expand_migration_with_addition_passes() {
        assert!(scan_migration("0001_expand.sql", EXPAND_ADD_COLUMN).is_empty());
    }

    #[test]
    fn contract_migration_with_precondition_passes() {
        assert!(scan_migration("0002_contract.sql", CONTRACT_DROP).is_empty());
    }

    #[test]
    fn destructive_sql_in_expand_migration_fails() {
        let contents = "\
-- phase: expand
-- Remove the legacy table in a later release.
CREATE TABLE widgets (id bigint);

DROP TABLE old_widgets;
";

        let violations = scan_migration("0003_expand_drop.sql", contents);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "destructive-sql");
        assert_eq!(violations[0].line, 5);
        assert!(violations[0].snippet.contains("DROP TABLE old_widgets"));
    }

    #[test]
    fn missing_phase_header_fails() {
        let contents = "CREATE TABLE widgets (id bigint);\n";

        let violations = scan_migration("0004_no_header.sql", contents);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "phase-header");
        assert_eq!(violations[0].line, 1);
        assert!(violations[0].snippet.contains("missing"));
    }

    #[test]
    fn invalid_phase_header_fails() {
        let contents = "\
-- phase: backfill
ALTER TABLE widgets ADD COLUMN note text;
";

        let violations = scan_migration("0005_bad_phase.sql", contents);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "phase-header");
        assert!(violations[0].snippet.contains("backfill"));
    }

    #[test]
    fn contract_without_release_precondition_still_fails_destructive_sql() {
        let contents = "\
-- phase: contract
DROP TABLE old_widgets;
";

        let violations = scan_migration("0006_contract_missing_release.sql", contents);

        assert_eq!(violations.len(), 1);
        assert_eq!(violations[0].rule, "destructive-sql");
    }

    #[test]
    fn destructive_keywords_in_comments_are_ignored() {
        let contents = "\
-- phase: expand
-- DROP TABLE old_widgets;
ALTER TABLE widgets ADD COLUMN note text;
";

        assert!(scan_migration("0007_comment.sql", contents).is_empty());
    }

    #[test]
    fn destructive_keywords_in_string_literals_are_ignored() {
        let contents = "\
-- phase: expand
COMMENT ON TABLE widgets IS 'DROP TABLE old_widgets';
";

        assert!(scan_migration("0008_string.sql", contents).is_empty());
    }

    #[test]
    fn destructive_statement_families_are_detected() {
        let contents = "\
-- phase: expand
ALTER TABLE widgets RENAME COLUMN old_name TO new_name;
ALTER TABLE widgets ALTER COLUMN note SET NOT NULL;
ALTER TABLE widgets ALTER COLUMN note TYPE varchar(20);
ALTER TYPE widget_status ADD VALUE 'new_status';
ALTER TYPE widget_status DROP VALUE 'old_status';
DROP INDEX widgets_note_idx;
";

        let violations = scan_migration("0009_destructive.sql", contents);

        assert_eq!(violations.len(), 5);
        assert!(
            violations
                .iter()
                .all(|violation| violation.rule == "destructive-sql")
        );
        assert_eq!(
            violations
                .iter()
                .map(|violation| violation.line)
                .collect::<Vec<_>>(),
            vec![2, 3, 4, 6, 7]
        );
    }

    const CHECK_WIDENING: &str = "\
-- phase: expand
ALTER TABLE ONLY public.widgets DROP CONSTRAINT widgets_kind_check;
ALTER TABLE ONLY public.widgets ADD CONSTRAINT widgets_kind_check
    CHECK (kind IN ('a', 'b', 'c')) NOT VALID;
";

    #[test]
    fn check_widening_with_not_valid_re_add_passes() {
        assert!(scan_migration("0030_widen.sql", CHECK_WIDENING).is_empty());
    }

    #[test]
    fn check_widening_in_one_statement_passes() {
        let contents = "\
-- phase: expand
ALTER TABLE widgets
    DROP CONSTRAINT IF EXISTS widgets_kind_check,
    ADD CONSTRAINT widgets_kind_check CHECK (kind <> '') NOT VALID;
";
        assert!(scan_migration("0030_widen.sql", contents).is_empty());
    }

    #[test]
    fn constraint_drops_outside_the_widening_pattern_stay_findings() {
        let cases = [
            (
                "re-added without NOT VALID",
                "ALTER TABLE widgets DROP CONSTRAINT c;\nALTER TABLE widgets ADD CONSTRAINT c CHECK (kind <> '');\n",
            ),
            ("never re-added", "ALTER TABLE widgets DROP CONSTRAINT c;\n"),
            (
                "re-added under another name",
                "ALTER TABLE widgets DROP CONSTRAINT c;\nALTER TABLE widgets ADD CONSTRAINT d CHECK (kind <> '') NOT VALID;\n",
            ),
            (
                "re-added on another table",
                "ALTER TABLE widgets DROP CONSTRAINT c;\nALTER TABLE gadgets ADD CONSTRAINT c CHECK (kind <> '') NOT VALID;\n",
            ),
            (
                "re-added as a foreign key",
                "ALTER TABLE widgets DROP CONSTRAINT c;\nALTER TABLE widgets ADD CONSTRAINT c FOREIGN KEY (g) REFERENCES gadgets (id) NOT VALID;\n",
            ),
            (
                "re-added before the drop",
                "ALTER TABLE widgets ADD CONSTRAINT c CHECK (kind <> '') NOT VALID;\nALTER TABLE widgets DROP CONSTRAINT c;\n",
            ),
            (
                "quoted name",
                "ALTER TABLE widgets DROP CONSTRAINT \"c\";\nALTER TABLE widgets ADD CONSTRAINT \"c\" CHECK (kind <> '') NOT VALID;\n",
            ),
        ];
        for (case, body) in cases {
            let violations = scan_migration("0030_drop.sql", &format!("-- phase: expand\n{body}"));
            assert_eq!(violations.len(), 1, "{case}: {violations:?}");
            assert_eq!(violations[0].rule, "destructive-sql", "{case}");
            assert!(violations[0].snippet.starts_with("DROP:"), "{case}");
        }
    }

    #[test]
    fn widening_one_check_does_not_hide_other_drops() {
        let contents = format!(
            "{CHECK_WIDENING}DROP INDEX widgets_note_idx;\nALTER TABLE widgets DROP COLUMN note;\n"
        );
        let violations = scan_migration("0030_widen.sql", &contents);
        assert_eq!(
            violations
                .iter()
                .map(|violation| violation.line)
                .collect::<Vec<_>>(),
            vec![5, 6]
        );
    }

    #[test]
    fn parses_git_name_status_output() {
        let entries = parse_diff_name_status(
            "M\tcrates/app/migrations/0001_initial.sql\n\
             R100\tcrates/app/migrations/old.sql\tcrates/app/migrations/new.sql\n",
        );

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].status, 'M');
        assert_eq!(entries[1].status, 'R');
        assert_eq!(
            entries[1].old_path.as_deref(),
            Some("crates/app/migrations/old.sql")
        );
        assert_eq!(entries[1].path, "crates/app/migrations/new.sql");
    }
}

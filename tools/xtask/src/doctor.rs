use anyhow::Result;

use crate::workspace::find_root;

pub fn run() -> Result<()> {
    let root = find_root()?;
    println!("Deepref Repository Doctor");
    println!("Root: {}", root.display());
    println!();

    let mut checks_passed = 0;
    let mut warnings = 0;
    let mut failures = 0;

    // 1. Cargo workspace metadata and architecture boundaries
    println!("Checking Cargo workspace metadata and architecture boundaries...");
    match crate::boundaries::run() {
        Ok(()) => {
            checks_passed += 1;
        }
        Err(e) => {
            eprintln!("[error] Architecture validation failed: {e}");
            failures += 1;
        }
    }

    // 2. Database migrations directory
    print!("Checking PostgreSQL migrations directory... ");
    let migrations_dir = root.join("crates/postgres/migrations");
    if migrations_dir.is_dir() {
        let count = std::fs::read_dir(&migrations_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "sql"))
                    .count()
            })
            .unwrap_or(0);
        println!("[ok] ({count} migration files found)");
        checks_passed += 1;
    } else {
        println!("[error] (not found at {})", migrations_dir.display());
        failures += 1;
    }

    // 3. SQLx offline metadata directory
    print!("Checking SQLx offline query metadata (.sqlx)... ");
    let sqlx_dir = root.join(".sqlx");
    if sqlx_dir.is_dir() {
        let query_count = std::fs::read_dir(&sqlx_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                    .count()
            })
            .unwrap_or(0);
        println!("[ok] ({query_count} cached query files found)");
        checks_passed += 1;
    } else {
        println!("[warn] (.sqlx directory missing; required for SQLX_OFFLINE=true builds)");
        warnings += 1;
    }

    // 4. Generated API contract paths
    print!("Checking generated API contract paths... ");
    let openapi_doc = root.join("docs/openapi.json");
    let generated_client_dir = root.join("apps/web/src/lib/api/generated");
    if openapi_doc.is_file() && generated_client_dir.is_dir() {
        println!("[ok] (docs/openapi.json and generated client exist)");
        checks_passed += 1;
    } else {
        println!(
            "[warn] (docs/openapi.json or generated client missing; run 'cargo xtask generate')"
        );
        warnings += 1;
    }

    // 5. Core configuration coherence
    print!("Checking core configuration files... ");
    let config_files = [
        ".mise.toml",
        "justfile",
        "process-compose.yaml",
        "Cargo.toml",
        "package.json",
        "pnpm-workspace.yaml",
    ];
    let missing_configs: Vec<_> = config_files
        .iter()
        .filter(|f| !root.join(f).is_file())
        .copied()
        .collect();
    if missing_configs.is_empty() {
        println!(
            "[ok] (all {} configuration files present)",
            config_files.len()
        );
        checks_passed += 1;
    } else {
        println!(
            "[error] (missing configuration files: {:?})",
            missing_configs
        );
        failures += 1;
    }

    // 6. Environment file diagnostic (.env)
    print!("Checking local development environment (.env)... ");
    let env_file = root.join(".env");
    if env_file.is_file() {
        println!("[ok] (.env present)");
        checks_passed += 1;
    } else {
        println!("[info] (.env not found; using system environment variables)");
    }

    println!();
    println!(
        "Diagnostic summary: {checks_passed} passed, {warnings} warnings, {failures} failures."
    );

    if failures > 0 {
        anyhow::bail!("{failures} repository health checks failed.");
    }

    Ok(())
}

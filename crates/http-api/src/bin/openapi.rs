fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "{}",
        serde_json::to_string_pretty(&deepref_http_api::routes::openapi_document())?
    );
    Ok(())
}

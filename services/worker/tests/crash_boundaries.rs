#[test]
fn final_transaction_inserts_children_before_completion() {
    let source = include_str!("../src/store.rs");
    let success = &source[source.find("pub async fn finalize_success").unwrap()..];
    let child = success.find("enqueue_child(&mut tx").unwrap();
    let complete = success.find("complete_item_and_claim(\n        &mut tx,\n        event,\n        doi,\n        owner,\n        IngestionItemStatus::Fetched").unwrap();
    assert!(child < complete);
    assert!(success.contains("tx.commit().await?"));
}

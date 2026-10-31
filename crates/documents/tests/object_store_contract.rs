#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Same byte/integrity/lifecycle contract on filesystem and a real TLS S3
//! fixture. The fixture path is explicit and mandatory in its CI lane.
use bytes::Bytes;
use deepref_documents::{DocumentStore, ObjectKey, StoreConfig, content_sha256};
use futures::stream;
use std::sync::Arc;
use uuid::Uuid;

async fn contract(store: &DocumentStore) {
    store
        .check_read_health()
        .await
        .expect("actual backend read health probe");
    let workspace = Uuid::new_v4();
    let project = Uuid::new_v4();
    let key = ObjectKey::tenant_document(workspace, project, Uuid::new_v4());
    let body = Bytes::from(vec![b'x'; 5 * 1024 * 1024 + 17]);
    let stored = store
        .for_upload(key.clone())
        .put_stream(stream::iter([
            Ok::<_, String>(body.slice(..3)),
            Ok(body.slice(3..)),
        ]))
        .await
        .expect("multipart upload");
    assert_eq!(stored.sha256, content_sha256(&body));
    assert_eq!(stored.byte_size, body.len());
    assert!(store.exists(key.as_str()).await.unwrap());
    assert_eq!(store.get_object(&key, body.len()).await.unwrap(), body);
    assert!(store.get_object(&key, 3).await.is_err());
    let export = ObjectKey::account_export(Uuid::new_v4(), Uuid::new_v4());
    let written = store
        .put_object(&export, Bytes::from_static(b"ZIP"), 1024)
        .await
        .unwrap();
    assert_eq!(written.sha256, content_sha256(b"ZIP"));
    store.delete_object(&export).await.unwrap();
    let failed = ObjectKey::tenant_document(workspace, project, Uuid::new_v4());
    assert!(
        store
            .for_upload(failed.clone())
            .put_stream(stream::iter([
                Ok(Bytes::from_static(b"%PDF-")),
                Err("injected stream failure"),
            ]))
            .await
            .is_err()
    );
    assert!(!store.exists(failed.as_str()).await.unwrap());
    store.delete_object(&key).await.unwrap();
    assert!(!store.exists(key.as_str()).await.unwrap());
    assert!(
        store
            .get_object(&key, 1024)
            .await
            .unwrap_err()
            .is_not_found()
    );
    // Backend NotFound semantics differ; application cleanup normalizes it.
    if let Err(error) = store.delete_object(&key).await {
        assert!(error.is_not_found());
    }
    assert!(ObjectKey::parse("workspaces/../../private").is_err());
}

#[tokio::test]
async fn filesystem_contract() {
    let root = std::env::temp_dir().join(format!("deepref-storage-contract-{}", Uuid::new_v4()));
    let store = DocumentStore::new(StoreConfig::local(&root).unwrap(), 16 * 1024 * 1024);
    contract(&store).await;
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn tls_s3_compatible_contract() {
    let endpoint = match std::env::var("DEEPREF_S3_CONTRACT_ENDPOINT") {
        Ok(value) => value,
        Err(_) if std::env::var("DEEPREF_REQUIRE_S3_CONTRACT").as_deref() == Ok("1") => {
            panic!("S3 contract fixture is required")
        }
        Err(_) => return,
    };
    assert!(
        endpoint.starts_with("https://localhost:"),
        "contract fixture must be loopback TLS"
    );
    let certificate =
        std::fs::read(std::env::var("DEEPREF_S3_CONTRACT_CA").expect("fixture CA path")).unwrap();
    let client = object_store::ClientOptions::new()
        .with_root_certificate(object_store::Certificate::from_pem(&certificate).unwrap());
    let backend = object_store::aws::AmazonS3Builder::new()
        .with_endpoint(endpoint)
        .with_bucket_name("deepref-contract")
        .with_region("us-east-1")
        .with_access_key_id("deepref-fixture")
        .with_secret_access_key("deepref-local-fixture-only")
        .with_client_options(client)
        .build()
        .unwrap();
    contract(&DocumentStore::from_object_store(Arc::new(backend))).await;
    let certificate =
        std::fs::read(std::env::var("DEEPREF_S3_CONTRACT_CA").expect("fixture CA path")).unwrap();
    let client = object_store::ClientOptions::new()
        .with_root_certificate(object_store::Certificate::from_pem(&certificate).unwrap());
    let missing_bucket = object_store::aws::AmazonS3Builder::new()
        .with_endpoint(std::env::var("DEEPREF_S3_CONTRACT_ENDPOINT").unwrap())
        .with_bucket_name(format!("absent-health-{}", Uuid::new_v4()))
        .with_region("us-east-1")
        .with_access_key_id("deepref-fixture")
        .with_secret_access_key("deepref-local-fixture-only")
        .with_client_options(client)
        .build()
        .unwrap();
    assert!(
        DocumentStore::from_object_store(Arc::new(missing_bucket))
            .check_read_health()
            .await
            .is_err(),
        "an absent bucket must never look healthy because HEAD returns 404"
    );
}

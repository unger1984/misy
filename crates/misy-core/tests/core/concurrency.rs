//! Additional headless-core integration coverage.

use super::*;

#[tokio::test]
async fn core_serializes_concurrent_submissions_into_one_canonical_history() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join("target.txt");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
    write_fixture_manifest(&bundled, "fixture-two", &fixture, &target);
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    core.select_model(fixture_model())
        .await
        .expect("select first model");
    let mut events = core.subscribe_lossless();
    let first = core
        .submit(Message::user("session-one"))
        .await
        .expect("first submit");
    core.select_model(ModelRef::new(
        ProviderId::new("fixture-two"),
        ModelId::new("fixture-model"),
    ))
    .await
    .expect("select second model");
    let second = core
        .submit(Message::user("session-two"))
        .await
        .expect("second submit");

    receive_until(
        &mut events,
        first,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == first),
    )
    .await;
    receive_until(
        &mut events,
        second,
        |event| matches!(event, CoreEvent::Completed { submission } if *submission == second),
    )
    .await;
    let history = core.history().await;
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].message.content, "session-one");
    assert_eq!(history[2].message.content, "session-two");
    assert!(history.chunks_exact(2).all(|pair| {
        pair[0].message.role == misy_core::MessageRole::User
            && pair[1].message.role == misy_core::MessageRole::Assistant
    }));
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn concurrent_model_selection_keeps_memory_and_disk_in_sync() {
    let (temporary, core, _) = test_core("model-race");
    let first = fixture_model();
    let second = ModelRef::new(ProviderId::new("fixture"), ModelId::new("fixture-model-b"));
    let core_a = core.clone();
    let core_b = core.clone();
    let first_for_thread = first;
    let second_for_thread = second;

    let select_a = tokio::spawn(async move { core_a.select_model(first_for_thread).await });
    let select_b = tokio::spawn(async move { core_b.select_model(second_for_thread).await });
    select_a
        .await
        .expect("first selector")
        .expect("first selection");
    select_b
        .await
        .expect("second selector")
        .expect("second selection");

    let persisted =
        misy_core::ConfigStore::new(MisyPaths::from_root(temporary.path().join("misy")))
            .load()
            .expect("saved config")
            .default_model;
    assert_eq!(core.selected_model().await, persisted);
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn concurrent_auth_mutations_do_not_lose_or_resurrect_credentials() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let target = temporary.path().join("target.txt");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(&bundled, "fixture", &fixture, &target);
    write_fixture_manifest(&bundled, "fixture-two", &fixture, &target);
    let paths = MisyPaths::from_root(temporary.path().join("misy"));
    let core = MisyCore::discover(paths.clone(), &bundled).expect("core discovery");
    let first = ProviderId::new("fixture");
    let second = ProviderId::new("fixture-two");

    let complete_a = {
        let core = core.clone();
        let provider = first.clone();
        tokio::spawn(async move {
            core.complete_auth(
                &provider,
                json!({"id":"fixture-session"}),
                json!({"code":"a"}),
            )
            .await
        })
    };
    let complete_b = {
        let core = core.clone();
        let provider = second.clone();
        tokio::spawn(async move {
            core.complete_auth(
                &provider,
                json!({"id":"fixture-session"}),
                json!({"code":"b"}),
            )
            .await
        })
    };
    complete_a
        .await
        .expect("first completion")
        .expect("first auth");
    complete_b
        .await
        .expect("second completion")
        .expect("second auth");
    let store = misy_core::CredentialStore::new(paths);
    assert!(store.load(&first).expect("first credential").is_some());
    assert!(store.load(&second).expect("second credential").is_some());

    let refreshing = {
        let core = core.clone();
        let provider = first.clone();
        tokio::spawn(async move { core.refresh_auth(&provider).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    let logging_out = {
        let core = core.clone();
        let provider = first.clone();
        tokio::spawn(async move { core.logout(&provider).await })
    };
    refreshing.await.expect("refresh thread").expect("refresh");
    logging_out.await.expect("logout thread").expect("logout");
    assert!(store.load(&first).expect("logged out credential").is_none());
    core.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn pending_auth_for_one_provider_does_not_block_logout_for_another() {
    let temporary = tempfile::tempdir().expect("temporary root");
    let bundled = temporary.path().join("bundled");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/core_provider_fixture.sh");
    write_fixture_manifest(
        &bundled,
        "fixture",
        &fixture,
        &temporary.path().join("first"),
    );
    write_fixture_manifest(
        &bundled,
        "fixture-two",
        &fixture,
        &temporary.path().join("second"),
    );
    let core = MisyCore::discover(
        MisyPaths::from_root(temporary.path().join("misy")),
        &bundled,
    )
    .expect("core discovery");
    let first = ProviderId::new("fixture");
    let second = ProviderId::new("fixture-two");
    core.complete_auth(
        &second,
        json!({"id":"fixture-session"}),
        json!({"code":"b"}),
    )
    .await
    .expect("authenticate second provider");

    let pending = {
        let core = core.clone();
        let provider = first;
        tokio::spawn(async move {
            core.complete_auth(&provider, json!({"id":"pending-a"}), json!({}))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let started = Instant::now();
    core.logout(&second).await.expect("logout second provider");
    assert!(started.elapsed() < Duration::from_millis(500));

    pending.await.expect("pending auth thread").expect("auth");
    core.shutdown().await.expect("shutdown");
}

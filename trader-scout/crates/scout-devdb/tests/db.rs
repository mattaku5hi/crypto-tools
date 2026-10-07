//! Fact-store behaviour against a real PostgreSQL. Runs only when
//! `SCOUT_TEST_DATABASE_URL` is set (e.g. a local container); otherwise every
//! test returns early and says so (CI stays offline).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use scout_devdb::{AthObservation, DevDb, Launch, Migration};

async fn db() -> Option<DevDb> {
    let Ok(url) = std::env::var("SCOUT_TEST_DATABASE_URL") else {
        eprintln!("SCOUT_TEST_DATABASE_URL not set: database test skipped");
        return None;
    };
    let db = DevDb::connect(&url, 2).await.unwrap();
    db.migrate().await.unwrap();
    Some(db)
}

fn launch(chain: &str, token: &str, creator: &str, at: i64) -> Launch {
    Launch {
        chain: chain.into(),
        token: token.into(),
        launchpad: "flap".into(),
        creator: creator.into(),
        created_block: at,
        created_at: at,
        tx_hash: format!("0x{token}"),
        source: "test".into(),
    }
}

#[tokio::test]
async fn facts_are_idempotent_and_joined_for_the_derivation() {
    let Some(db) = db().await else { return };
    // unique namespace per run (tests share one database)
    let chain = format!("test-{}", std::process::id());
    let rows = vec![
        launch(&chain, "t1", "dev", 100),
        launch(&chain, "t2", "dev", 200),
        launch(&chain, "t3", "other", 300),
    ];
    assert_eq!(db.insert_launches(&rows).await.unwrap(), 3);
    assert_eq!(
        db.insert_launches(&rows).await.unwrap(),
        0,
        "replay inserts nothing"
    );
    let mig = Migration {
        chain: chain.clone(),
        token: "t1".into(),
        launchpad: "flap".into(),
        migrated_block: 150,
        migrated_at: 150,
        tx_hash: "0xm".into(),
        pool: Some("0xpool".into()),
        source: "test".into(),
    };
    assert_eq!(
        db.insert_migrations(std::slice::from_ref(&mig))
            .await
            .unwrap(),
        1
    );
    assert_eq!(db.insert_migrations(&[mig]).await.unwrap(), 0);
    let ath = |cents: i64, observed: i64| AthObservation {
        chain: chain.clone(),
        token: "t1".into(),
        ath_fdv_cents: cents,
        ath_at: Some(160),
        source: "codex".into(),
        observed_at: observed,
    };
    db.upsert_ath(&[ath(10_000_000, 10)]).await.unwrap();
    // an older observation never overwrites a newer one
    assert_eq!(db.upsert_ath(&[ath(1, 5)]).await.unwrap(), 0);
    db.upsert_ath(&[ath(20_000_000, 20)]).await.unwrap();

    let got = db.dev_launches(Some(&chain), 0).await.unwrap();
    assert_eq!(got.len(), 3);
    let t1 = got.iter().find(|r| r.token == "t1").unwrap();
    assert_eq!(
        (t1.creator.as_str(), t1.migrated_at, t1.ath_fdv_cents),
        ("dev", Some(150), Some(20_000_000))
    );
    let t2 = got.iter().find(|r| r.token == "t2").unwrap();
    assert_eq!((t2.migrated_at, t2.ath_fdv_cents), (None, None));
    assert_eq!(
        db.dev_launches(Some(&chain), 250).await.unwrap().len(),
        1,
        "since filter"
    );

    let src = format!("{chain}:flap:launches");
    assert_eq!(db.cursor(&src).await.unwrap(), None);
    db.set_cursor(&src, "123", 1).await.unwrap();
    db.set_cursor(&src, "456", 2).await.unwrap();
    assert_eq!(db.cursor(&src).await.unwrap().as_deref(), Some("456"));

    let cat = format!("{chain}:top-migr");
    assert_eq!(db.last_delivery(&cat, "gmgn").await.unwrap(), None);
    db.record_delivery(&cat, "gmgn", "h1", &["a".into()], 1)
        .await
        .unwrap();
    db.record_delivery(&cat, "gmgn", "h2", &["a".into(), "b".into()], 2)
        .await
        .unwrap();
    assert_eq!(
        db.last_delivery(&cat, "gmgn").await.unwrap().as_deref(),
        Some("h2")
    );
    let d = db
        .deliveries()
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.category == cat)
        .unwrap();
    assert_eq!(d.members, ["a", "b"]);
    assert_eq!(d.delivered_at, 2);
}

#[tokio::test]
async fn dev_identity_follows_the_creator_kind() {
    let Some(db) = db().await else { return };
    let chain = format!("id-{}", std::process::id());
    let rows = vec![
        launch(&chain, "a1", "eoa", 100),
        launch(&chain, "b1", "bot", 110),
        launch(&chain, "b2", "bot", 120),
        launch(&chain, "s1", "shared", 130),
        launch(&chain, "s2", "shared", 140),
        launch(&chain, "u1", "unknown", 150),
    ];
    db.insert_launches(&rows).await.unwrap();
    let kind = |address: &str, is_contract: bool, owner: Option<&str>| scout_devdb::AddressKind {
        chain: chain.clone(),
        address: address.into(),
        is_contract,
        owner: owner.map(Into::into),
        sampled: 8,
        checked_at: 1,
    };
    assert_eq!(db.creators_without_kind(&chain, 10).await.unwrap().len(), 4);
    db.set_address_kind(&kind("eoa", false, None))
        .await
        .unwrap();
    db.set_address_kind(&kind("bot", true, Some("operator")))
        .await
        .unwrap();
    db.set_address_kind(&kind("shared", true, None))
        .await
        .unwrap();
    assert_eq!(
        db.creators_without_kind(&chain, 10).await.unwrap(),
        vec!["unknown".to_string()]
    );
    let queue: Vec<String> = db
        .shared_launches_without_signer(&chain, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(queue, ["s2", "s1"], "newest first");
    // a migrated launch jumps the queue (it decides categories)
    db.insert_migrations(&[scout_devdb::Migration {
        chain: chain.clone(),
        token: "s1".into(),
        launchpad: "flap".into(),
        migrated_block: 1,
        migrated_at: 200,
        tx_hash: "0xm".into(),
        pool: None,
        source: "test".into(),
    }])
    .await
    .unwrap();
    let queue: Vec<String> = db
        .shared_launches_without_signer(&chain, 10)
        .await
        .unwrap()
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(queue, ["s1", "s2"]);
    db.set_launch_signer(&chain, "s1", "alice").await.unwrap();
    assert_eq!(
        db.shared_launches_without_signer(&chain, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let devs: std::collections::BTreeMap<String, String> = db
        .dev_launches(Some(&chain), 0)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.token, r.creator))
        .collect();
    assert_eq!(devs["a1"], "eoa");
    // a single-owner contract stays its own dev (never merged into a signer)
    assert_eq!(devs["b1"], "bot");
    assert_eq!(devs["b2"], "bot");
    assert_eq!(devs["s1"], "alice");
    assert_eq!(
        devs["s2"], "contract:shared",
        "unresolved signer is never merged"
    );
    assert_eq!(devs["u1"], "unknown", "kind not checked yet");
}

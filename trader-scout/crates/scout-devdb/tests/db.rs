//! Fact-store behaviour against a real PostgreSQL. Runs only when
//! `SCOUT_TEST_DATABASE_URL` is set (e.g. a local container); otherwise every
//! test returns early and says so (CI stays offline).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
    assert_eq!(db.insert_migrations(&[mig.clone()]).await.unwrap(), 1);
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
    db.record_delivery(&cat, "gmgn", "h1", 1).await.unwrap();
    db.record_delivery(&cat, "gmgn", "h2", 2).await.unwrap();
    assert_eq!(
        db.last_delivery(&cat, "gmgn").await.unwrap().as_deref(),
        Some("h2")
    );
}

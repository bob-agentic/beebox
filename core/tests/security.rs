//! The vulnerability this file exists for: the daemon binds every interface by
//! default, so reaching the port must not be the same as being authorised.

use beebox_core::app::App;
use beebox_core::store::Store;

#[tokio::test]
async fn owner_access_requires_the_key() {
    let app = App::new(Store::in_memory().unwrap(), 100);

    assert!(!app.is_owner_key(""), "empty key must not authorise");
    assert!(!app.is_owner_key("guess"), "a wrong key must not authorise");
    assert!(app.is_owner_key(app.owner_key()), "the real key must work");
}

#[tokio::test]
async fn the_key_is_long_enough_to_resist_guessing() {
    let app = App::new(Store::in_memory().unwrap(), 100);
    assert_eq!(app.owner_key().len(), 32, "128 bits of hex");
}

#[tokio::test]
async fn keys_differ_between_runs() {
    // Regenerated per process and never written to disk, so a leaked key dies
    // with the daemon.
    let a = App::new(Store::in_memory().unwrap(), 100);
    let b = App::new(Store::in_memory().unwrap(), 100);
    assert_ne!(a.owner_key(), b.owner_key());
}

#[tokio::test]
async fn switching_sharing_off_disconnects_remote_clients() {
    use beebox_core::share::{Grant, Scope};
    let app = App::new(Store::in_memory().unwrap(), 100);
    app.set_exposed(true).await;

    let g = Grant { token: "t".into(), scope: Scope::All, writable: false, host: false };
    // One remote viewer, one local (owner's own window).
    let (_, mut remote) = app.add_conn(&g, "192.168.1.50".into()).await;
    let (_, mut local) = app.add_conn(&g, "127.0.0.1".into()).await;

    app.set_exposed(false).await;

    // The remote socket is closed with no reason, so it comes back once
    // sharing reopens; the local one is left alone.
    assert_eq!(remote.try_recv().ok(), Some(None));
    assert!(local.try_recv().is_err(), "loopback connection must survive");
}

#[tokio::test]
async fn revoking_a_link_closes_it_for_good() {
    use beebox_core::proto::CloseReason;
    use beebox_core::share::{Grant, Scope};
    let app = App::new(Store::in_memory().unwrap(), 100);
    let g = Grant { token: "t".into(), scope: Scope::All, writable: false, host: false };
    app.store.lock().await.put_grant(&g).unwrap();
    let (_, mut phone) = app.add_conn(&g, "192.168.1.50".into()).await;

    app.revoke("t").await;

    assert_eq!(phone.try_recv().ok(), Some(Some(CloseReason::Revoked)));
    assert!(app.store.lock().await.grant("t").unwrap().is_none());
}

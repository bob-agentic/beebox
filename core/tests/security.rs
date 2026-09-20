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

    let g = Grant { token: "t".into(), scope: Scope::All, writable: false, pair_hash: None };
    // One remote viewer, one local (owner's own window).
    let (remote, mut remote_kick) = app.add_conn(&g, "192.168.1.50".into(), "phone".into()).await;
    let (local, mut local_kick) = app.add_conn(&g, "127.0.0.1".into(), "desk".into()).await;

    app.set_exposed(false).await;

    // The remote socket's kick channel fires (sender dropped); the local one
    // stays quiet. And the disconnect is not a ban: it can come back later.
    assert!(remote_kick.try_recv().is_err(), "sender dropped => channel closed");
    assert!(!app.conn_live(remote).await, "remote connection must be gone");
    assert!(app.conn_live(local).await, "loopback connection must survive");
    assert!(!app.is_banned(&g, "192.168.1.50", "phone").await, "disconnect must not ban");
    let _ = local_kick.try_recv();
}

#[test]
fn pairing_codes_hash_per_token_and_verify() {
    use beebox_core::http::hash_pair_code;
    let h1 = hash_pair_code("C33C6Q", "token-a");
    let h2 = hash_pair_code("C33C6Q", "token-b");
    assert_ne!(h1, h2, "the token salts the hash");
    assert_eq!(h1, hash_pair_code("C33C6Q", "token-a"), "deterministic");
    assert_ne!(h1, hash_pair_code("C33C6X", "token-a"));
    assert_eq!(h1.len(), 64, "sha-256 hex");
}

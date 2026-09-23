//! Anything that is not a live share link or the owner's page must look like
//! an ordinary 404, with nothing naming what runs behind the port.

use std::net::SocketAddr;

use beebox_core::app::App;
use beebox_core::share::{Grant, Scope};
use beebox_core::store::Store;

async fn get(addr: SocketAddr, path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut resp = String::new();
    stream.read_to_string(&mut resp).await.unwrap();
    let code = resp.split_whitespace().nth(1).unwrap().parse().unwrap();
    (code, resp)
}

#[tokio::test]
async fn unknown_and_forged_addresses_are_a_plain_404() {
    let app = App::new(Store::in_memory().unwrap(), 100);
    app.store
        .lock()
        .await
        .put_grant(&Grant { token: "real".into(), scope: Scope::Tab(1), writable: false, host: false })
        .unwrap();
    let key = app.owner_key().to_string();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = beebox_core::http::router(app, None);
    tokio::spawn(async move {
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())
            .await
            .unwrap();
    });

    for path in ["/", "/?key=wrong", "/t/forged", "/p/real", "/x/real", "/admin", "/ws"] {
        let (code, body) = get(addr, path).await;
        assert_eq!(code, 404, "{path}");
        assert!(!body.to_lowercase().contains("beebox"), "{path} names the app");
    }
    assert_eq!(get(addr, "/t/real").await.0, 200);
    assert_eq!(get(addr, &format!("/?key={key}")).await.0, 200);
}

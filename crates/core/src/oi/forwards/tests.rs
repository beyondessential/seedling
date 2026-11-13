use std::{net::Ipv6Addr, sync::Arc, time::Duration};

use jiff::Timestamp;
use seedling_protocol::names::{AppName, ForwardId};
use tokio::sync::{mpsc, watch};

use super::{ForwardEntry, ForwardProto, end_forwards_of_app, end_undeclared_forwards};
use crate::oi::test_support::{TestOi, loopback};

fn tcp_forward(
    oi: &TestOi,
    app: &str,
    service: &str,
    port: u16,
    target_port: u16,
) -> (ForwardId, watch::Receiver<bool>) {
    let (stop_tx, stop_rx) = watch::channel(false);
    let forward_id = ForwardId::generate();
    let (app, service) = (AppName::new(app).unwrap(), service.to_owned());
    let _ = target_port;
    oi.state
        .forwards
        .lock()
        .register(1, |forward_key| ForwardEntry {
            forward_id,
            forward_key,
            conn_id: 1,
            app,
            service,
            port,
            proto: ForwardProto::Tcp,
            target_addr: Ipv6Addr::LOCALHOST,
            opened_at: Timestamp::now(),
            actor: None,
            stop_tx,
            udp_tx: None,
            status_tx: mpsc::channel(4).0,
            udp_dropped: 0,
        })
        .unwrap();
    (forward_id, stop_rx)
}

// spec: FWD#forward-lifetime
#[test]
fn every_forward_of_an_app_that_leaves_installed_ends_and_other_apps_keep_theirs() {
    let oi = TestOi::new();
    let (_, gone) = tcp_forward(&oi, "demo", "db", 5432, 0);
    let (_, kept) = tcp_forward(&oi, "other", "db", 5432, 0);

    end_forwards_of_app(&oi.state, "demo");
    assert!(*gone.borrow(), "the forward did not end");
    assert!(!*kept.borrow(), "another app's forward ended");
    assert_eq!(oi.state.forwards.lock().count(), 1);
}

// spec: FWD#forward-lifetime
#[test]
fn a_new_resource_graph_ends_only_the_forwards_it_no_longer_declares() {
    let oi = TestOi::new();
    oi.call(
        "/apps/create",
        serde_json::json!({
            "app": "demo",
            "script": r#"
                let db = app.service("database");
                app.deployment("pgdb").image("docker.io/library/postgres:16").tcp(5432, db.port(5432));
            "#,
        }),
    )
    .unwrap();
    let (_, declared) = tcp_forward(&oi, "demo", "database", 5432, 0);
    let (_, undeclared) = tcp_forward(&oi, "demo", "database", 9999, 0);

    end_undeclared_forwards(&oi.state, "demo");
    assert!(!*declared.borrow());
    assert!(*undeclared.borrow());
}

// spec: FWD#tcp-tunnelling
#[test]
fn a_stream_naming_no_live_forward_is_reset_not_finished() {
    let oi = TestOi::new();
    oi.block_on(async {
        let link = loopback().await;
        let (mut client_send, mut client_recv) = link.client.open_bi().await.unwrap();
        client_send.write_all(b"x").await.unwrap();
        let (send, recv) = link.server.accept_bi().await.unwrap();

        super::session::handle_forward_stream(
            (send, recv),
            ForwardId::generate().to_string(),
            Vec::new(),
            Arc::clone(&oi.state),
        )
        .await;

        let read = tokio::time::timeout(Duration::from_secs(5), client_recv.read_to_end(1024))
            .await
            .expect("the stream was neither reset nor finished");
        assert!(
            matches!(
                read,
                Err(quinn::ReadToEndError::Read(quinn::ReadError::Reset(_)))
            ),
            "a finished stream reads as a complete transfer: {read:?}"
        );
    });
}

// spec: FWD#forward-lifetime
#[test]
fn a_tunnelled_connection_ends_with_its_forward() {
    let oi = TestOi::new();
    oi.block_on(async {
        let service = tokio::net::TcpListener::bind("[::1]:0").await.unwrap();
        let port = service.local_addr().unwrap().port();
        // The service accepts and then holds the connection open.
        let held = tokio::spawn(async move {
            let (conn, _) = service.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(conn);
        });

        let (forward_id, _) = tcp_forward(&oi, "demo", "db", port, port);
        // Aim the forward at the listener's port.
        let link = loopback().await;
        let (mut client_send, mut client_recv) = link.client.open_bi().await.unwrap();
        client_send.write_all(b"x").await.unwrap();
        let (send, recv) = link.server.accept_bi().await.unwrap();

        let state = Arc::clone(&oi.state);
        let relay = tokio::spawn(async move {
            super::session::handle_forward_stream(
                (send, recv),
                forward_id.to_string(),
                Vec::new(),
                state,
            )
            .await;
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !relay.is_finished(),
            "the relay ended before its forward did"
        );

        end_forwards_of_app(&oi.state, "demo");
        tokio::time::timeout(Duration::from_secs(5), relay)
            .await
            .expect("the relay outlived its forward")
            .unwrap();
        let read = client_recv.read_to_end(1024).await;
        assert!(
            read.is_err(),
            "the tunnelled connection was finished, not cut"
        );
        held.abort();
    });
}

//! The bad-network proxy between a real bridge connection and a real server.

use b3313_bridge::client::Connection;
use b3313_nettools::netsim::{Impairment, Proxy};
use b3313_protocol::metrics::{LinkReport, REPORT_INTERVAL};
use b3313_server::{Config, StopOnDrop, serve};
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

/// Runs `test` with a server behind a proxy, and returns the proxy's address to it.
fn through_proxy(
    impairment: Impairment,
    test: impl FnOnce(SocketAddr) -> io::Result<()>,
) -> io::Result<()> {
    let server_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let server = server_socket.local_addr()?;
    let proxy_socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let proxy_address = proxy_socket.local_addr()?;
    let mut proxy = Proxy::new(proxy_socket, server, impairment, 42)?;
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let serving = scope.spawn(|| serve(&server_socket, Config::default(), &stop, |_| {}));
        let proxying = scope.spawn(|| proxy.run(&stop));
        let stopping = StopOnDrop(&stop);
        let outcome = test(proxy_address);
        drop(stopping);
        for handle in [serving, proxying] {
            handle
                .join()
                .map_err(|_| io::Error::other("a thread panicked"))??;
        }
        outcome
    })
}

/// Pings and polls for `period`, then returns the link's report.
fn measure(connection: &mut Connection, period: Duration) -> io::Result<LinkReport> {
    let start = Instant::now();
    while start.elapsed() < period {
        connection.ping_if_due(Instant::now())?;
        connection.poll(Instant::now() + Duration::from_millis(33))?;
    }
    connection
        .report_if_due(start + REPORT_INTERVAL)
        .ok_or_else(|| io::Error::other("no report after a full interval"))
}

#[test]
fn delay_shows_in_the_round_trip() -> io::Result<()> {
    let impairment = Impairment {
        delay: Duration::from_millis(50),
        ..Impairment::default()
    };
    through_proxy(impairment, |proxy| {
        let mut connection = Connection::join(proxy, "")?;
        let report = measure(&mut connection, Duration::from_millis(1_500))?;

        let trips = report.round_trips;
        assert!(trips.count >= 1, "{report}");
        // 50 ms each way, plus scheduling slack.
        assert!(trips.min >= Duration::from_millis(100), "{report}");
        assert!(trips.max < Duration::from_millis(200), "{report}");
        Ok(())
    })
}

#[test]
fn loss_shows_in_the_report() -> io::Result<()> {
    let impairment = Impairment {
        loss: 0.5,
        ..Impairment::default()
    };
    through_proxy(impairment, |proxy| {
        let mut connection = Connection::join(proxy, "")?;
        let report = measure(&mut connection, Duration::from_secs(2))?;

        // About 60 snapshots; the seed is fixed, but timing still varies a little.
        let rate = report.losses.rate();
        assert!((0.25..0.75).contains(&rate), "{report}");
        Ok(())
    })
}

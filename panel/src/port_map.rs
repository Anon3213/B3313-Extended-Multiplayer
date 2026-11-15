//! Asks the user's router to send the server's port to this PC (`UPnP`), so players over the
//! internet can join without the user forwarding a port by hand, and learns the address they
//! join at: the router's public one.
//!
//! Everything here waits on the network, so it runs off the panel's own thread.

use igd_next::aio::Gateway;
use igd_next::aio::tokio::{Tokio, search_gateway};
use igd_next::{AddPortError, PortMappingProtocol, SearchOptions};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long to wait for a router to answer; one that supports this answers within a second.
const SEARCH: Duration = Duration::from_secs(3);
/// How long the router keeps the port open unless asked again, so one left behind by a panel
/// that crashed closes by itself.
const LEASE: Duration = Duration::from_hours(2);
/// How often the thread keeping the port open looks whether it is to close it.
const CLOSING_CHECK: Duration = Duration::from_millis(100);
/// How long a closing panel waits for the router to be told; one that answers does so sooner.
const CLOSE_WAIT: Duration = Duration::from_secs(2);
/// What the router lists the port under.
const DESCRIPTION: &str = "B3313 Extended Multiplayer";

/// The server's port, open on the router until this is dropped.
#[derive(Debug)]
pub struct PortMap {
    /// The address players anywhere join at.
    pub public: SocketAddr,
    closing: Arc<AtomicBool>,
    /// Hangs up once the thread keeping the port open has closed it and ended.
    closed: mpsc::Receiver<()>,
}

impl PortMap {
    /// Closes the port and waits for the router to have been told, `CLOSE_WAIT` at most: for
    /// a panel about to exit, whose process would otherwise end before the router heard, and
    /// leave the port open for the rest of its lease.
    pub fn close(self) {
        self.closing.store(true, Ordering::Relaxed);
        // An error is the thread hanging up, which is what is waited for, or the wait's end.
        let _ = self.closed.recv_timeout(CLOSE_WAIT);
    }
}

impl Drop for PortMap {
    fn drop(&mut self) {
        // The thread keeping the port open closes it and ends; nothing waits on the router.
        self.closing.store(true, Ordering::Relaxed);
    }
}

/// Whether `ip` is an address the internet can reach, not one a home or a provider's network
/// keeps to itself.
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [first, second, ..] = ip.octets();
            // 100.64.0.0/10 is what providers give customers who share one public address.
            let shared = first == 100 && second & 0xC0 == 64;
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || shared)
        }
        IpAddr::V6(ip) => !(ip.is_loopback() || ip.is_unspecified()),
    }
}

/// Asks for the port, for `LEASE` or, from a router that only opens ports for good, for good.
async fn add_port(gateway: &Gateway<Tokio>, local: SocketAddr) -> Result<(), AddPortError> {
    let udp = PortMappingProtocol::UDP;
    let lease = u32::try_from(LEASE.as_secs()).unwrap_or(u32::MAX);
    let asked = gateway
        .add_port(udp, local.port(), local, lease, DESCRIPTION)
        .await;
    if matches!(asked, Err(AddPortError::OnlyPermanentLeasesSupported)) {
        return gateway
            .add_port(udp, local.port(), local, 0, DESCRIPTION)
            .await;
    }
    asked
}

/// Finds the router and has it open the port, returning it and its public address.
async fn ask_router(local: SocketAddr) -> Result<(Gateway<Tokio>, IpAddr), String> {
    let mut options = SearchOptions::default();
    options.timeout = Some(SEARCH);
    let gateway = search_gateway(options).await.map_err(|_| {
        "Your router did not offer to open the port (UPnP is off or unsupported).".to_owned()
    })?;
    let public = gateway
        .get_external_ip()
        .await
        .map_err(|error| format!("Your router would not say its public address: {error}."))?;
    if !is_public(public) {
        return Err(
            "Your router is itself behind another network, so opening its port would not let \
             players in. Your internet provider may share one address among customers."
                .to_owned(),
        );
    }
    add_port(&gateway, local)
        .await
        .map_err(|error| format!("Your router refused to open the port: {error}"))?;
    Ok((gateway, public))
}

/// Keeps the port open, asking again at half the lease, and closes it once `closing` is set.
async fn keep_open(gateway: &Gateway<Tokio>, local: SocketAddr, closing: &AtomicBool) {
    let mut asked = Instant::now();
    while !closing.load(Ordering::Relaxed) {
        tokio::time::sleep(CLOSING_CHECK).await;
        if asked.elapsed() >= LEASE / 2 {
            asked = Instant::now();
            // A router that refuses now still holds the port for the lease's other half.
            let _ = add_port(gateway, local).await;
        }
    }
    // Nobody is left to tell if this fails; the lease then closes the port.
    let _ = gateway
        .remove_port(PortMappingProtocol::UDP, local.port())
        .await;
}

/// Talks to the router for as long as the port is open: sends the first answer to `answer`,
/// then keeps the port until `closing` is set.
fn serve_port(
    local: SocketAddr,
    closing: &AtomicBool,
    answer: &mpsc::Sender<Result<IpAddr, String>>,
) {
    // A small runtime of this thread's own: the library that speaks UPnP is written for tokio,
    // and the panel is not.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let _ = answer.send(Err(format!("Could not ask the router: {error}.")));
            return;
        }
    };
    runtime.block_on(async {
        match ask_router(local).await {
            Ok((gateway, public)) => {
                // The panel may have stopped waiting; the port is then closed straight away.
                if answer.send(Ok(public)).is_err() {
                    closing.store(true, Ordering::Relaxed);
                }
                keep_open(&gateway, local, closing).await;
            }
            Err(reason) => {
                let _ = answer.send(Err(reason));
            }
        }
    });
}

/// Opens the UDP port of `local`, this PC's address on its network, on the router. Waits for
/// the router's answer, a few seconds at most.
///
/// # Errors
/// Says, in a sentence for the user, why players over the internet cannot be let in this way.
pub fn open(local: SocketAddr) -> Result<PortMap, String> {
    let closing = Arc::new(AtomicBool::new(false));
    let keeper = Arc::clone(&closing);
    let (answer, answered) = mpsc::channel();
    let (ending, closed) = mpsc::channel();
    thread::Builder::new()
        .name("b3313-port-map".to_owned())
        .spawn(move || {
            serve_port(local, &keeper, &answer);
            drop(ending);
        })
        .map_err(|error| format!("Could not ask the router: {error}."))?;
    let public = answered
        .recv()
        .map_err(|_| "Asking the router failed.".to_owned())??;
    Ok(PortMap {
        public: SocketAddr::new(public, local.port()),
        closing,
        closed,
    })
}

#[cfg(test)]
mod tests {
    use super::is_public;
    use std::net::IpAddr;

    #[test]
    fn only_addresses_the_internet_reaches_are_public() {
        let public = ["203.0.113.7", "8.8.8.8", "100.128.0.1", "2001:db8::1"];
        let kept = [
            "192.168.1.1",
            "10.0.0.1",
            "172.16.4.4",
            "100.64.0.1",
            "100.127.255.254",
            "169.254.1.1",
            "127.0.0.1",
            "0.0.0.0",
            "::1",
        ];
        let parse = |text: &str| text.parse::<IpAddr>().ok();
        for address in public {
            assert_eq!(parse(address).map(is_public), Some(true), "{address}");
        }
        for address in kept {
            assert_eq!(parse(address).map(is_public), Some(false), "{address}");
        }
    }

    /// Asks this PC's own router, opening a port on it for a moment. Run by hand.
    #[test]
    #[ignore = "changes the router's settings for a moment, and needs one that supports UPnP"]
    fn this_pcs_router_opens_a_port_and_closes_it() {
        let Some(ip) = crate::network::local_network_ip() else {
            unreachable!("this PC must be on a network to run this test");
        };
        match super::open((ip, 41399).into()) {
            Ok(map) => {
                println!("opened: players would join at {}", map.public);
                assert!(is_public(map.public.ip()));
                map.close();
            }
            Err(reason) => unreachable!("not opened: {reason}"),
        }
    }
}

//! Establishing which address a request actually came from.
//!
//! Every rate limit and every session fingerprint is keyed on a client
//! address, and `X-Forwarded-For` is a header like any other: whoever connects
//! writes it. Reading its leftmost value — which is what all four of this
//! crate's extractors used to do — means an attacker rotating one header gets a
//! fresh token bucket per request, and a session's binding to an address is
//! whatever the holder of the token says it is.
//!
//! A forwarding header is evidence only about the hop that wrote it, so it is
//! worth something exactly when the connection came from a proxy the operator
//! named. `server.trusted_proxies` is that list, and it is empty by default:
//! trusting nothing means an engine reachable directly, or misconfigured, keys
//! everyone by the address they actually connected from rather than one they
//! chose.
//!
//! The decision is made once, at the edge ([`normalize_client_ip`]), which
//! rewrites the forwarding headers to hold the single address that survived it.
//! Everything downstream reads that with [`from_headers`], so there is no path
//! left where a handler sees the raw claim.

use axum::{
    extract::{ConnectInfo, Request},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::Response,
};
use ipnet::IpNet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

/// What this crate calls an address it could not establish.
pub const UNKNOWN: &str = "unknown";

const FORWARDED_FOR: &str = "x-forwarded-for";
const REAL_IP: &str = "x-real-ip";

/// The proxies whose forwarding headers are worth reading.
#[derive(Debug, Default, Clone)]
pub struct TrustedProxies {
    networks: Vec<IpNet>,
}

impl TrustedProxies {
    /// Build from configuration. Entries are addresses (`127.0.0.1`) or
    /// networks (`172.16.0.0/12`); a bare address is the /32 or /128 holding
    /// only itself.
    pub fn parse(entries: &[String]) -> Result<Self, String> {
        let mut networks = Vec::new();

        for entry in entries {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }

            let network = entry
                .parse::<IpNet>()
                .or_else(|_| entry.parse::<IpAddr>().map(IpNet::from))
                .map_err(|_| format!("'{}' is not an IP address or CIDR network", entry))?;

            networks.push(network);
        }

        Ok(Self { networks })
    }

    /// Whether anything is trusted at all. Nothing is, by default.
    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }

    fn contains(&self, addr: IpAddr) -> bool {
        self.networks.iter().any(|network| network.contains(&addr))
    }
}

/// The address a request came from, given who connected and what they claimed.
///
/// Walks `X-Forwarded-For` from the right and takes the first address that is
/// not itself a trusted proxy — the rightmost entry is written by the hop
/// closest to this engine and each one to its left is only as trustworthy as
/// the hop that recorded it. Taking the leftmost instead is the bug: that entry
/// is whatever the original client sent, including a client that invented the
/// whole chain.
fn resolve(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &TrustedProxies) -> Option<IpAddr> {
    let peer = peer?;

    // Nothing is trusted, or this connection did not come through anything
    // that is: the header is a claim by whoever is talking to us, and the
    // socket is the only thing that is not.
    if trusted.is_empty() || !trusted.contains(peer) {
        return Some(peer);
    }

    let forwarded = headers
        .get(FORWARDED_FOR)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();

    let chain: Vec<IpAddr> = forwarded
        .split(',')
        .filter_map(|entry| parse_forwarded_entry(entry.trim()))
        .collect();

    if let Some(client) = chain.iter().rev().find(|addr| !trusted.contains(**addr)) {
        return Some(*client);
    }

    // Either there was no chain, or every hop in it is a proxy we trust — in
    // which case the leftmost of them is as far back as this engine can see.
    if let Some(first) = chain.first() {
        return Some(*first);
    }

    // Nginx and friends write this one instead, and a trusted proxy saying it
    // is worth the same as its `X-Forwarded-For`.
    headers
        .get(REAL_IP)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| parse_forwarded_entry(value.trim()))
        .or(Some(peer))
}

/// One entry of a forwarding chain. Some proxies write a port, and IPv6
/// addresses arrive bracketed when they do.
fn parse_forwarded_entry(entry: &str) -> Option<IpAddr> {
    if let Ok(addr) = entry.parse::<IpAddr>() {
        return Some(addr);
    }

    if let Ok(addr) = entry.parse::<SocketAddr>() {
        return Some(addr.ip());
    }

    entry
        .strip_prefix('[')
        .and_then(|rest| rest.split(']').next())
        .and_then(|inner| inner.parse::<IpAddr>().ok())
}

/// Resolve the client's address once, and leave the request carrying that
/// answer instead of the claim it arrived with.
///
/// Outermost in the stack, so nothing downstream — middleware, handler, or
/// script — can read a forwarding header this has not already judged.
pub async fn normalize_client_ip(
    axum::extract::State(trusted): axum::extract::State<Arc<TrustedProxies>>,
    mut request: Request,
    next: Next,
) -> Response {
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(addr)| addr.ip());

    let resolved = resolve(peer, request.headers(), &trusted);

    let headers = request.headers_mut();
    headers.remove(REAL_IP);
    match resolved.and_then(|addr| HeaderValue::from_str(&addr.to_string()).ok()) {
        Some(value) => {
            headers.insert(FORWARDED_FOR, value);
        }
        None => {
            headers.remove(FORWARDED_FOR);
        }
    }

    next.run(request).await
}

/// The client's address, as established at the edge.
///
/// One reader for what used to be four near-identical ones. By the time this
/// runs, [`normalize_client_ip`] has replaced the forwarding headers with the
/// single address that survived the trust check, so there is nothing to walk
/// and nothing to weigh.
pub fn from_headers(headers: &HeaderMap) -> String {
    headers
        .get(FORWARDED_FOR)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(UNKNOWN)
        .to_string()
}

/// The user agent a request carries, beside the address it came from because
/// the two are read together everywhere: they are the session fingerprint.
pub fn user_agent_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(UNKNOWN)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(entries: &[&str]) -> TrustedProxies {
        TrustedProxies::parse(&entries.iter().map(|e| e.to_string()).collect::<Vec<_>>())
            .expect("test entries should parse")
    }

    fn headers(forwarded: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in forwarded {
            headers.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).expect("valid header name"),
                HeaderValue::from_str(value).expect("valid header value"),
            );
        }
        headers
    }

    fn ip(value: &str) -> IpAddr {
        value.parse().expect("valid address")
    }

    /// The finding this closes. With nothing configured as a proxy, a
    /// forwarding header is a claim by a stranger and is worth nothing.
    #[test]
    fn a_header_from_an_untrusted_peer_is_ignored() {
        let resolved = resolve(
            Some(ip("203.0.113.9")),
            &headers(&[("x-forwarded-for", "1.2.3.4")]),
            &trusted(&[]),
        );

        assert_eq!(resolved, Some(ip("203.0.113.9")));
    }

    /// Even with proxies configured: this connection did not come through one.
    #[test]
    fn a_header_from_someone_who_is_not_the_proxy_is_ignored() {
        let resolved = resolve(
            Some(ip("203.0.113.9")),
            &headers(&[("x-forwarded-for", "1.2.3.4")]),
            &trusted(&["127.0.0.1", "172.16.0.0/12"]),
        );

        assert_eq!(resolved, Some(ip("203.0.113.9")));
    }

    #[test]
    fn a_header_from_the_proxy_names_the_client() {
        let resolved = resolve(
            Some(ip("172.18.0.2")),
            &headers(&[("x-forwarded-for", "203.0.113.9")]),
            &trusted(&["172.16.0.0/12"]),
        );

        assert_eq!(resolved, Some(ip("203.0.113.9")));
    }

    /// What an attacker sends through a real proxy: their own invented chain,
    /// with the true address appended to the right of it by the proxy.
    #[test]
    fn an_invented_chain_does_not_move_the_answer() {
        let resolved = resolve(
            Some(ip("172.18.0.2")),
            &headers(&[("x-forwarded-for", "1.2.3.4, 203.0.113.9")]),
            &trusted(&["172.16.0.0/12"]),
        );

        assert_eq!(
            resolved,
            Some(ip("203.0.113.9")),
            "the rightmost entry a trusted hop wrote is the one worth reading"
        );
    }

    #[test]
    fn a_chain_of_trusted_hops_reads_through_to_the_client() {
        let resolved = resolve(
            Some(ip("172.18.0.2")),
            &headers(&[("x-forwarded-for", "203.0.113.9, 172.18.0.5")]),
            &trusted(&["172.16.0.0/12"]),
        );

        assert_eq!(resolved, Some(ip("203.0.113.9")));
    }

    #[test]
    fn a_proxy_may_write_x_real_ip_instead() {
        let resolved = resolve(
            Some(ip("127.0.0.1")),
            &headers(&[("x-real-ip", "203.0.113.9")]),
            &trusted(&["127.0.0.1"]),
        );

        assert_eq!(resolved, Some(ip("203.0.113.9")));
    }

    /// A personal install with nothing in front of it: the socket is all there
    /// is, and it is enough. Before this, every caller shared one bucket keyed
    /// on the string "unknown".
    #[test]
    fn a_direct_connection_is_named_by_its_socket() {
        let resolved = resolve(Some(ip("127.0.0.1")), &HeaderMap::new(), &trusted(&[]));

        assert_eq!(resolved, Some(ip("127.0.0.1")));
    }

    #[test]
    fn without_a_peer_there_is_nothing_to_establish() {
        assert_eq!(
            resolve(
                None,
                &headers(&[("x-forwarded-for", "1.2.3.4")]),
                &trusted(&["127.0.0.1"])
            ),
            None
        );
    }

    #[test]
    fn entries_may_carry_a_port_or_brackets() {
        assert_eq!(
            parse_forwarded_entry("203.0.113.9:4711"),
            Some(ip("203.0.113.9"))
        );
        assert_eq!(
            parse_forwarded_entry("[2001:db8::1]:4711"),
            Some(ip("2001:db8::1"))
        );
        assert_eq!(
            parse_forwarded_entry("2001:db8::1"),
            Some(ip("2001:db8::1"))
        );
        assert_eq!(parse_forwarded_entry("not-an-address"), None);
    }

    #[test]
    fn configuration_is_rejected_rather_than_ignored() {
        assert!(TrustedProxies::parse(&["nonsense".to_string()]).is_err());
        assert!(TrustedProxies::parse(&["10.0.0.0/8".to_string()]).is_ok());
        assert!(TrustedProxies::parse(&["::1".to_string()]).is_ok());
    }
}

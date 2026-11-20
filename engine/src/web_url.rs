//! Public URL validation for provider-side fetching, including local Fake-IP DNS.
use std::{
    io::Read,
    net::{IpAddr, ToSocketAddrs},
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};

use serde_json::Value as Json;

const DNS_TIMEOUT: Duration = Duration::from_secs(10);
const DOH_ENDPOINTS: [&str; 2] = ["https://cloudflare-dns.com/dns-query", "https://dns.google/resolve"];
const MAX_DOH_BYTES: u64 = 64 * 1024;
static DNS_WORKERS: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn guard_url(raw: &str) -> Result<String, String> {
    let deadline = Instant::now() + DNS_TIMEOUT;
    guard_with(raw, |host| local_lookup(host, remaining(deadline)?), |host| doh_lookup(host, deadline))
}

fn remaining(deadline: Instant) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or_else(|| "URL DNS verification timed out".into())
}

fn fake_ip(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(addr) if addr.octets()[0] == 198 && matches!(addr.octets()[1], 18 | 19))
}

fn internal_host(host: &str) -> bool {
    let host = host.trim_end_matches('.');
    !host.contains('.')
        || matches!(
            host,
            "localhost.localdomain" | "metadata.google.internal" | "metadata.goog" | "instance-data.ec2.internal"
        )
        || [".localhost", ".local", ".internal", ".home.arpa"].iter().any(|suffix| host.ends_with(suffix))
}

fn guard_with(
    raw: &str,
    mut local: impl FnMut(&str) -> Result<Vec<IpAddr>, String>,
    mut doh: impl FnMut(&str) -> Result<Vec<IpAddr>, String>,
) -> Result<String, String> {
    if raw.len() > 8192 {
        return Err("URL exceeds 8192 bytes".into());
    }
    let mut url = url::Url::parse(raw).map_err(|_| "unsupported URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err("only HTTP(S) URLs without embedded credentials are allowed".into());
    }
    url.set_fragment(None);
    let host = url.host().ok_or("URL has no host")?;
    let hostname = match host {
        url::Host::Ipv4(addr) => {
            if crate::tools::is_private_addr(addr.into()) {
                return Err(format!("refusing private address for {addr}"));
            }
            return Ok(url.into());
        }
        url::Host::Ipv6(addr) => {
            if crate::tools::is_private_addr(addr.into()) {
                return Err(format!("refusing private address for {addr}"));
            }
            return Ok(url.into());
        }
        url::Host::Domain(host) => host.trim_end_matches('.'),
    };
    if internal_host(hostname) {
        return Err(format!("refusing internal host {hostname}"));
    }
    let addresses = local(hostname)?;
    if addresses.is_empty() {
        return Err(format!("cannot resolve {hostname}"));
    }
    if addresses.iter().any(|ip| crate::tools::is_private_addr(*ip) && !fake_ip(*ip)) {
        return Err(format!("refusing private address for {hostname}"));
    }
    if addresses.iter().any(|ip| fake_ip(*ip)) {
        // A synthetic local routing address is never itself evidence of a public destination.
        let verified = doh(hostname).map_err(|error| format!("cannot verify Fake-IP host {hostname}: {error}"))?;
        if verified.is_empty() || verified.iter().any(|ip| crate::tools::is_private_addr(*ip)) {
            return Err(format!("refusing non-public DNS verification for Fake-IP host {hostname}"));
        }
    }
    Ok(url.into())
}

struct DnsSlot;
impl Drop for DnsSlot {
    fn drop(&mut self) {
        DNS_WORKERS.fetch_sub(1, Ordering::Relaxed);
    }
}

fn local_lookup(host: &str, timeout: Duration) -> Result<Vec<IpAddr>, String> {
    bounded_lookup(host.to_string(), timeout, |host| {
        (host, 443)
            .to_socket_addrs()
            .map(|rows| rows.map(|row| row.ip()).collect())
            .map_err(|error| format!("cannot resolve {host}: {error}"))
    })
}

fn bounded_lookup<T: Send + 'static>(
    host: String,
    timeout: Duration,
    resolve: impl FnOnce(&str) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    DNS_WORKERS
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| (count < 8).then_some(count + 1))
        .map_err(|_| "URL DNS resolver is busy; retry after outstanding lookups finish".to_string())?;
    let slot = DnsSlot;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    // ponytail: OS name resolution cannot be cancelled. The caller is bounded and at most eight late
    // workers may linger; use a cancellable resolver if these slots become a measured availability limit.
    std::thread::Builder::new()
        .name("web-url-dns".into())
        .spawn(move || {
            let result = resolve(&host);
            drop(slot);
            let _ = send.send(result);
        })
        .map_err(|error| format!("cannot start URL DNS lookup: {error}"))?;
    receive.recv_timeout(timeout).map_err(|error| format!("URL DNS lookup did not finish: {error}"))?
}

fn doh_lookup(host: &str, deadline: Instant) -> Result<Vec<IpAddr>, String> {
    doh_lookup_at(host, deadline, &DOH_ENDPOINTS)
}

fn doh_lookup_at(host: &str, deadline: Instant, endpoints: &[&str]) -> Result<Vec<IpAddr>, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .resolver(move |address: &str| {
            bounded_lookup(address.to_string(), remaining(deadline).map_err(std::io::Error::other)?, |address| {
                address.to_socket_addrs().map(|rows| rows.collect::<Vec<_>>()).map_err(|error| error.to_string())
            })
            .map_err(std::io::Error::other)
        })
        .build();
    let mut last = "no addresses returned".to_string();
    for endpoint in endpoints {
        match doh_endpoint(&agent, endpoint, host, deadline) {
            Ok(addresses) if !addresses.is_empty() => return Ok(addresses),
            Ok(_) => last = "no addresses returned".into(),
            Err(error) => last = error,
        }
        remaining(deadline)?;
    }
    Err(format!("trusted HTTPS DNS verification failed: {last}"))
}

fn doh_endpoint(agent: &ureq::Agent, endpoint: &str, host: &str, deadline: Instant) -> Result<Vec<IpAddr>, String> {
    let mut addresses = vec![];
    for record_type in [1, 28] {
        let response = agent
            .get(endpoint)
            .query("name", host)
            .query("type", &record_type.to_string())
            .set("accept", "application/dns-json")
            .timeout(remaining(deadline)?)
            .call()
            .map_err(|error| format!("HTTPS DNS request failed: {error}"))?;
        if response.status() != 200 {
            return Err(format!("HTTPS DNS returned HTTP {}", response.status()));
        }
        let mut bytes = vec![];
        response
            .into_reader()
            .take(MAX_DOH_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("HTTPS DNS body failed: {error}"))?;
        if bytes.len() as u64 > MAX_DOH_BYTES {
            return Err("HTTPS DNS response exceeds 64 KiB".into());
        }
        let payload: Json =
            serde_json::from_slice(&bytes).map_err(|error| format!("invalid HTTPS DNS JSON: {error}"))?;
        let answer = doh_addresses(&payload, record_type)?;
        // A known non-public answer cannot be overridden by another family failing or a fallback resolver.
        if answer.iter().any(|ip| crate::tools::is_private_addr(*ip)) {
            return Ok(answer);
        }
        addresses.extend(answer);
    }
    Ok(addresses)
}

fn doh_addresses(payload: &Json, record_type: u64) -> Result<Vec<IpAddr>, String> {
    if !matches!(payload["Status"].as_u64(), Some(0 | 3)) || payload["TC"] == true {
        return Err("HTTPS DNS returned an unsuccessful or truncated answer".into());
    }
    let Some(answers) = payload.get("Answer") else { return Ok(vec![]) };
    let answers = answers.as_array().ok_or("invalid HTTPS DNS answer array")?;
    if payload["Status"] == 3 && !answers.is_empty() {
        return Err("HTTPS DNS NXDOMAIN has answers".into());
    }
    let mut addresses = vec![];
    for answer in answers {
        if answer["type"].as_u64() != Some(record_type) {
            continue;
        }
        let address = answer["data"]
            .as_str()
            .ok_or("HTTPS DNS address is missing")?
            .parse::<IpAddr>()
            .map_err(|_| "HTTPS DNS address is invalid")?;
        if (record_type == 1) != address.is_ipv4() {
            return Err("HTTPS DNS address family mismatch".into());
        }
        addresses.push(address);
    }
    Ok(addresses)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use serde_json::json;
    static DNS_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn ips(values: &[&str]) -> Vec<IpAddr> {
        values.iter().map(|value| value.parse().unwrap()).collect()
    }

    #[test]
    fn fake_ip_requires_public_verification_and_never_relaxes_literals_or_mixed_answers() {
        let public = ips(&["93.184.216.34", "2606:4700:4700::1111"]);
        for local in [ips(&["198.18.0.61"]), ips(&["198.19.255.255", "93.184.216.34"])] {
            let mut calls = 0;
            let accepted = guard_with(
                "https://github.com/herdrdev/herdr",
                |_| Ok(local.clone()),
                |host| {
                    assert_eq!(host, "github.com");
                    calls += 1;
                    Ok(public.clone())
                },
            );
            assert!(accepted.is_ok());
            assert_eq!(calls, 1);
        }
        for local in [ips(&["93.184.216.34"]), public.clone()] {
            assert!(guard_with("https://example.org/", |_| Ok(local.clone()), |_| panic!("unneeded DoH")).is_ok());
        }
        for local in [vec![], ips(&["198.18.0.61", "127.0.0.1"]), ips(&["198.18.0.61", "10.0.0.1"])] {
            assert!(
                guard_with("https://example.org/", |_| Ok(local.clone()), |_| panic!("private DNS must stop")).is_err()
            );
        }
        for verified in
            [vec![], ips(&["198.18.0.61"]), ips(&["93.184.216.34", "127.0.0.1"]), ips(&["::ffff:169.254.169.254"])]
        {
            assert!(
                guard_with("https://example.org/", |_| Ok(ips(&["198.18.0.61"])), |_| Ok(verified.clone())).is_err()
            );
        }
        assert!(guard_with("https://example.org/", |_| Ok(ips(&["198.18.0.61"])), |_| Err("timeout".into())).is_err());
    }

    #[test]
    fn canonical_url_parsing_refuses_internal_aliases_and_embedded_credentials_before_dns() {
        for raw in [
            "http://198.18.0.61/",
            "http://198.19.1.1/",
            "http://LOCALHOST./",
            "http://metadata.google.internal./",
            "http://metadata.goog/",
            "http://router.local/",
            "http://foo.home.arpa/",
            "http://2130706433/",
            "http://0x7f.1/",
            "http://0177.0.0.1/",
            "http://[::ffff:127.0.0.1]/",
            "http://user:password@example.org/",
            "file:///tmp/file",
        ] {
            assert!(
                guard_with(raw, |_| panic!("unexpected local lookup: {raw}"), |_| panic!("unexpected DoH")).is_err(),
                "{raw}"
            );
        }
        assert_eq!(
            guard_with(
                "HTTPS://Example.org/path#fragment",
                |_| Ok(ips(&["93.184.216.34"])),
                |_| panic!("unneeded DoH")
            )
            .unwrap(),
            "https://example.org/path"
        );
    }

    #[test]
    fn doh_parsing_rejects_failed_partial_malformed_and_wrong_family_answers() {
        for payload in [
            json!({}),
            json!({"Status":2}),
            json!({"Status":0,"TC":true}),
            json!({"Status":0,"Answer":{}}),
            json!({"Status":0,"Answer":[{"type":1,"data":"::1"}]}),
            json!({"Status":0,"Answer":[{"type":1,"data":"bad"}]}),
            json!({"Status":3,"Answer":[{"type":1,"data":"93.184.216.34"}]}),
        ] {
            assert!(doh_addresses(&payload, 1).is_err(), "{payload}");
        }
        assert_eq!(
            doh_addresses(
                &json!({"Status":0,"Answer":[{"type":5,"data":"alias.example.org"},{"type":1,"data":"93.184.216.34"}]}),
                1
            )
            .unwrap(),
            ips(&["93.184.216.34"])
        );
        assert!(doh_addresses(&json!({"Status":0}), 28).unwrap().is_empty());
    }

    pub(crate) fn serve(responses: Vec<(u16, String)>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        use std::io::{BufRead, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            let mut requests = vec![];
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(3);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "fixture did not receive a request");
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut request = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    assert!(reader.read_line(&mut line).unwrap() > 0);
                    request.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(value) = line.to_lowercase().strip_prefix("content-length:") {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut bytes = vec![0; length];
                reader.read_exact(&mut bytes).unwrap();
                request.push_str(&String::from_utf8(bytes).unwrap());
                requests.push(request);
                let response = format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nLocation: /redirected\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = stream.write_all(response.as_bytes());
            }
            requests
        });
        (endpoint, worker)
    }

    #[test]
    fn doh_transport_never_accepts_partial_families_redirects_or_oversized_responses() {
        let v4 = json!({"Status":0,"Answer":[{"type":1,"data":"93.184.216.34"}]}).to_string();
        let v6 = json!({"Status":0,"Answer":[{"type":28,"data":"2606:4700:4700::1111"}]}).to_string();
        let private = json!({"Status":0,"Answer":[{"type":1,"data":"127.0.0.1"}]}).to_string();
        for (responses, expected) in [
            (vec![(200, v4.clone()), (200, v6)], Some(ips(&["93.184.216.34", "2606:4700:4700::1111"]))),
            (vec![(200, private)], Some(ips(&["127.0.0.1"]))),
            (vec![(200, v4), (503, "unavailable".into())], None),
            (vec![(302, "redirect".into())], None),
            (vec![(200, "x".repeat(MAX_DOH_BYTES as usize + 1))], None),
            (vec![(200, "invalid JSON".into())], None),
        ] {
            let count = responses.len();
            let (endpoint, server) = serve(responses);
            let agent = ureq::AgentBuilder::new().redirects(0).build();
            let result = doh_endpoint(&agent, &endpoint, "example.org", Instant::now() + Duration::from_secs(2));
            match expected {
                Some(addresses) => assert_eq!(result.unwrap(), addresses),
                None => assert!(result.is_err()),
            }
            let requests = server.join().unwrap();
            assert_eq!(requests.len(), count);
            assert!(requests[0].starts_with("GET /?name=example.org&type=1 "));
            if count == 2 {
                assert!(requests[1].starts_with("GET /?name=example.org&type=28 "));
            }
            assert!(requests.iter().all(|request| !request.to_lowercase().contains("authorization:")));
        }
    }

    #[test]
    fn provider_fetch_uses_the_verified_url_and_a_utf8_byte_limit() {
        let _lock = crate::env_lock();
        let name = "TA_TEST_WEB_COMPAT_KEY";
        let previous = std::env::var_os(name);
        std::env::set_var(name, "fixture");
        let content = "\u{00e9}".repeat(10);
        let (endpoint, server) = serve(vec![(200, json!({"title":"UTF-8", "content":content}).to_string())]);
        let result =
            crate::tools::web_fetch("https://93.184.216.34/path#fragment", 3, false, Some(&endpoint), Some(name));
        match previous {
            Some(value) => std::env::set_var(name, value),
            None => std::env::remove_var(name),
        }
        let requests = server.join().unwrap();
        let payload: Json = serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(payload["url"], "https://93.184.216.34/path");
        let result = result.unwrap();
        assert_eq!(result["content"], "\u{00e9}");
        assert_eq!(result["truncated"], true);
    }

    #[test]
    fn resolver_fallback_recovers_transport_failures_but_never_overrides_a_private_answer() {
        let _lock = DNS_TEST_LOCK.lock().unwrap();
        let v4 = json!({"Status":0,"Answer":[{"type":1,"data":"93.184.216.34"}]}).to_string();
        let (first, first_server) = serve(vec![(200, v4.clone()), (503, "unavailable".into())]);
        let (second, second_server) = serve(vec![(200, v4.clone()), (200, json!({"Status":0}).to_string())]);
        assert_eq!(
            doh_lookup_at("example.org", Instant::now() + Duration::from_secs(2), &[&first, &second]).unwrap(),
            ips(&["93.184.216.34"])
        );
        assert_eq!(first_server.join().unwrap().len(), 2);
        assert_eq!(second_server.join().unwrap().len(), 2);
        for (responses, expected) in [
            (
                vec![(200, json!({"Status":0,"Answer":[{"type":1,"data":"127.0.0.1"}]}).to_string())],
                ips(&["127.0.0.1"]),
            ),
            (
                vec![(200, v4), (200, json!({"Status":0,"Answer":[{"type":28,"data":"::1"}]}).to_string())],
                ips(&["::1"]),
            ),
        ] {
            let (endpoint, server) = serve(responses);
            let answer = doh_lookup_at(
                "example.org",
                Instant::now() + Duration::from_secs(2),
                &[&endpoint, "invalid://must-not-be-used"],
            );
            assert_eq!(answer.unwrap(), expected);
            server.join().unwrap();
        }
    }

    #[test]
    fn outstanding_dns_workers_are_bounded_and_release_their_slots() {
        let _lock = DNS_TEST_LOCK.lock().unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(9));
        let (started, ready) = std::sync::mpsc::channel();
        let handles = (0..8)
            .map(|_| {
                let barrier = barrier.clone();
                let started = started.clone();
                std::thread::spawn(move || {
                    bounded_lookup("fixture.example.org".into(), Duration::from_secs(3), move |_| {
                        started.send(()).unwrap();
                        barrier.wait();
                        Ok(())
                    })
                })
            })
            .collect::<Vec<_>>();
        for _ in 0..8 {
            ready.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let ninth: Result<(), String> =
            bounded_lookup("fixture.example.org".into(), Duration::from_secs(1), |_| panic!("ninth resolver ran"));
        barrier.wait();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }
        assert!(ninth.unwrap_err().contains("busy"));
        assert!(bounded_lookup("fixture.example.org".into(), Duration::from_secs(1), |_| Ok(())).is_ok());
    }

    #[test]
    fn a_slow_system_resolver_does_not_hold_the_fetch_caller() {
        let _lock = DNS_TEST_LOCK.lock().unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let (finished, done) = std::sync::mpsc::channel();
        let started = Instant::now();
        let result = bounded_lookup("fixture.example.org".into(), Duration::from_millis(30), move |_| {
            blocked.recv().unwrap();
            finished.send(()).unwrap();
            Ok(Vec::<IpAddr>::new())
        });
        assert!(result.unwrap_err().contains("did not finish"));
        assert!(started.elapsed() < Duration::from_secs(2));
        release.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(2)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while DNS_WORKERS.load(Ordering::Relaxed) != 0 {
            assert!(Instant::now() < deadline, "late resolver retained its slot");
            std::thread::yield_now();
        }
    }
}

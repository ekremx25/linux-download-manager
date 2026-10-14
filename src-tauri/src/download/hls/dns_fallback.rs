//! Retry unreachable HLS origins using an independent DNS answer. The URL,
//! TLS hostname validation and credential scoping remain unchanged.
use super::*;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio::sync::{Mutex, OnceCell};

type Entry = Arc<OnceCell<Option<Client>>>;

#[derive(Default)]
pub(super) struct Fallback {
    // Scoped to one playlist download; stale DNS cannot survive a new download.
    entries: Mutex<HashMap<String, Entry>>,
}

impl Fallback {
    pub async fn send(
        &self,
        client: &Client,
        url: &Url,
        origin: &Url,
        headers: &HashMap<String, String>,
        referer: Option<&str>,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let host = url.host_str().unwrap_or_default();
        let cached = self.entries.lock().await.get(host).cloned();
        if let Some(alternative) = cached
            .as_ref()
            .and_then(|entry| entry.get())
            .and_then(Option::as_ref)
        {
            return request(alternative, url, origin, headers, referer)
                .send()
                .await;
        }
        let result = request(client, url, origin, headers, referer).send().await;
        // An HTTP rejection (including 403/429) is not a DNS problem.
        if !result.as_ref().is_err_and(|error| error.is_connect()) || !eligible(url) {
            return result;
        }
        let entry = self
            .entries
            .lock()
            .await
            .entry(host.to_owned())
            .or_default()
            .clone();
        let alternative = entry.get_or_init(|| resolve_client(host)).await;
        match alternative {
            Some(alternative) => {
                request(alternative, url, origin, headers, referer)
                    .send()
                    .await
            }
            None => result,
        }
    }
}

fn eligible(url: &Url) -> bool {
    url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            host.contains('.')
                && host.parse::<IpAddr>().is_err()
                && ![".localhost", ".local", ".internal", ".lan"]
                    .iter()
                    .any(|suffix| host.ends_with(suffix))
        })
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    !ip.is_private()
        && !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_broadcast()
        && !ip.is_documentation()
        && a != 0
        && a < 224
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 198 && (b == 18 || b == 19))
}

fn answer_addresses(body: &[u8]) -> Option<Vec<SocketAddr>> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    if value.get("Status")?.as_u64()? != 0 {
        return None;
    }
    let addresses: Vec<_> = value
        .get("Answer")?
        .as_array()?
        .iter()
        .filter_map(|answer| {
            if answer.get("type")?.as_u64()? != 1 {
                return None;
            }
            let ip: Ipv4Addr = answer.get("data")?.as_str()?.parse().ok()?;
            public_ipv4(ip).then_some(SocketAddr::new(ip.into(), 443))
        })
        .collect();
    (!addresses.is_empty()).then_some(addresses)
}

async fn resolve_client(host: &str) -> Option<Client> {
    // Send only the hostname, never the media URL, cookies or other headers.
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .ok()?;
    let mut endpoint = Url::parse("https://cloudflare-dns.com/dns-query").ok()?;
    endpoint
        .query_pairs_mut()
        .append_pair("name", host)
        .append_pair("type", "A");
    let response = client
        .get(endpoint)
        .header("accept", "application/dns-json")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.ok()?;
        if body.len() + chunk.len() > 64 * 1024 {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    let addresses = answer_addresses(&body)?;
    Client::builder()
        .resolve_to_addrs(host, &addresses)
        .connect_timeout(Duration::from_secs(5))
        .build()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excludes_private_dns_answers_and_non_address_records() {
        let body = br#"{"Status":0,"Answer":[{"type":5,"data":"alias.example"},{"type":1,"data":"127.0.0.1"},{"type":1,"data":"192.168.1.1"},{"type":1,"data":"100.64.0.1"},{"type":1,"data":"104.21.48.20"}]}"#;
        assert_eq!(
            answer_addresses(body).unwrap(),
            vec!["104.21.48.20:443".parse::<SocketAddr>().unwrap()]
        );
        assert!(answer_addresses(br#"{"Status":3,"Answer":[]}"#).is_none());
        assert!(answer_addresses(b"not json").is_none());
    }

    #[test]
    fn local_names_and_literal_addresses_do_not_use_public_dns() {
        for value in [
            "https://localhost/a",
            "https://nas.local/a",
            "https://media.lan/a",
            "https://127.0.0.1/a",
            "http://cdn.example/a",
        ] {
            assert!(!eligible(&Url::parse(value).unwrap()));
        }
        assert!(eligible(&Url::parse("https://cdn.example/a").unwrap()));
    }
}

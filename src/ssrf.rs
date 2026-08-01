use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};

pub const BLOCKED: &str = "that URL resolves to a private, loopback, or special-use \
address; only public internet hosts are allowed";

fn blocked_v4(a: Ipv4Addr) -> bool {
    let o = a.octets();
    a.is_loopback()
        || a.is_private()
        || a.is_link_local()
        || a.is_broadcast()
        || a.is_unspecified()
        || a.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (64..128).contains(&o[1]))
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || o[0] >= 240
}

fn blocked_v6(a: Ipv6Addr) -> bool {
    if let Some(m) = a.to_ipv4_mapped() {
        return blocked_v4(m);
    }
    let s = a.segments();
    a.is_loopback()
        || a.is_unspecified()
        || a.is_multicast()
        || (s[0] & 0xfe00) == 0xfc00
        || (s[0] & 0xffc0) == 0xfe80
        || (s[0] == 0x2001 && s[1] == 0x0000)
        || (s[0] == 0x2001 && s[1] == 0x0db8)
        || (s[0] == 0x0064 && s[1] == 0xff9b)
}

pub fn blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => blocked_v4(a),
        IpAddr::V6(a) => blocked_v6(a),
    }
}

pub fn host_port(url: &str) -> Option<(String, u16)> {
    let (scheme, rest) = url.split_once("://")?;
    let default = match scheme.to_ascii_lowercase().as_str() {
        "http" => 80,
        "https" => 443,
        _ => return None,
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .filter(|s| !s.is_empty())?;
    let authority = authority.rsplit('@').next()?;
    if let Some(open) = authority.strip_prefix('[') {
        let (host, tail) = open.split_once(']')?;
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default);
        return Some((host.to_string(), port));
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            Some((h.to_string(), p.parse().unwrap_or(default)))
        }
        _ => Some((authority.to_string(), default)),
    }
}

fn domain_matches(host: &str, rule: &str) -> bool {
    let rule = rule.trim().trim_end_matches('.').to_ascii_lowercase();
    if rule.is_empty() {
        return false;
    }
    let rule = rule.strip_prefix("*.").unwrap_or(&rule).to_string();
    host == rule || host.ends_with(&format!(".{rule}"))
}

pub fn domain_policy(url: &str, allow: &[String], deny: &[String]) -> Result<(), String> {
    if allow.is_empty() && deny.is_empty() {
        return Ok(());
    }
    let Some((host, _)) = host_port(url) else {
        return Err("only http(s) URLs are allowed".into());
    };
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if let Some(rule) = deny.iter().find(|r| domain_matches(host, r)) {
        return Err(format!(
            "{host} is refused by security.deny_domains ({})",
            rule.trim()
        ));
    }
    if !allow.is_empty() && !allow.iter().any(|r| domain_matches(host, r)) {
        return Err(format!(
            "{host} is not in security.allow_domains; the allowlist has {} entr{}",
            allow.len(),
            if allow.len() == 1 { "y" } else { "ies" }
        ));
    }
    Ok(())
}

pub fn check_url(url: &str) -> Result<(), String> {
    check_url_with(url, false)
}

pub fn check_url_with(url: &str, allow_private: bool) -> Result<(), String> {
    let Some((host, port)) = host_port(url) else {
        return Err("only http(s) URLs are allowed".into());
    };
    if host.is_empty() {
        return Err("the URL has no host".into());
    }
    if allow_private {
        return Ok(());
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if blocked_ip(ip) {
            Err(BLOCKED.into())
        } else {
            Ok(())
        };
    }
    let lower = host.to_ascii_lowercase();
    let bare = lower.trim_end_matches('.');
    if bare == "localhost" || bare.ends_with(".localhost") || bare.ends_with(".local") {
        return Err(BLOCKED.into());
    }
    let resolved: Vec<IpAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve host: {e}"))?
        .map(|s| s.ip())
        .collect();
    resolution_allowed(&resolved)
}

pub fn resolution_allowed(resolved: &[IpAddr]) -> Result<(), String> {
    if resolved.is_empty() {
        return Err("the host did not resolve".into());
    }
    if resolved.iter().any(|ip| blocked_ip(*ip)) {
        return Err(BLOCKED.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn loopback_and_private_ranges_are_blocked() {
        for s in [
            "127.0.0.1",
            "127.1.2.3",
            "10.0.0.5",
            "172.16.4.1",
            "192.168.1.1",
            "0.0.0.0",
            "255.255.255.255",
            "100.64.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "224.0.0.1",
        ] {
            assert!(blocked_ip(ip(s)), "{s} must be blocked");
        }
    }

    #[test]
    fn domain_policy_is_open_when_unconfigured() {
        assert!(domain_policy("https://example.com/x", &[], &[]).is_ok());
    }

    #[test]
    fn deny_domains_refuse_host_and_subdomains_and_win_over_allow() {
        let deny = vec!["evil.com".to_string()];
        let allow = vec!["evil.com".to_string()];
        for url in [
            "https://evil.com/",
            "https://EVIL.com/x",
            "https://api.evil.com/x",
            "https://deep.api.evil.com./x",
        ] {
            let err = domain_policy(url, &allow, &deny).unwrap_err();
            assert!(err.contains("deny_domains"), "{url}: {err}");
        }
        assert!(domain_policy("https://evil.com.example.org/", &[], &deny).is_ok());
    }

    #[test]
    fn allow_domains_close_everything_else() {
        let allow = vec!["example.com".to_string(), "*.rust-lang.org".to_string()];
        assert!(domain_policy("https://example.com/a", &allow, &[]).is_ok());
        assert!(domain_policy("https://sub.example.com/a", &allow, &[]).is_ok());
        assert!(domain_policy("https://doc.rust-lang.org/std", &allow, &[]).is_ok());
        let err = domain_policy("https://other.net/", &allow, &[]).unwrap_err();
        assert!(err.contains("allow_domains"), "{err}");
        assert!(err.contains("2 entries"), "{err}");
    }

    #[test]
    fn cloud_metadata_address_is_blocked() {
        assert!(
            blocked_ip(ip("169.254.169.254")),
            "the cloud metadata endpoint is the classic SSRF credential target"
        );
        assert!(blocked_ip(ip("169.254.0.1")));
    }

    #[test]
    fn ipv6_special_use_is_blocked_including_mapped_v4() {
        for s in [
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd00::1",
            "ff02::1",
            "2001::1",
            "2001:db8::1",
            "::ffff:127.0.0.1",
            "::ffff:169.254.169.254",
        ] {
            assert!(blocked_ip(ip(s)), "{s} must be blocked");
        }
    }

    #[test]
    fn public_addresses_are_allowed() {
        for s in ["1.1.1.1", "8.8.8.8", "142.251.37.14", "2606:4700::1111"] {
            assert!(!blocked_ip(ip(s)), "{s} must be allowed");
        }
    }

    #[test]
    fn dual_stack_with_one_private_address_is_refused() {
        let mixed = [ip("142.251.37.14"), ip("10.0.0.5")];
        assert!(
            resolution_allowed(&mixed).is_err(),
            "a host resolving to both public and private addresses is a rebinding vector"
        );
        let mixed6 = [ip("2606:4700::1111"), ip("fd00::1")];
        assert!(resolution_allowed(&mixed6).is_err());
        let clean = [ip("1.1.1.1"), ip("2606:4700::1111")];
        assert!(resolution_allowed(&clean).is_ok());
        assert!(resolution_allowed(&[]).is_err(), "empty resolution refused");
    }

    #[test]
    fn refusal_text_does_not_double_the_blocked_prefix() {
        assert!(
            !BLOCKED.starts_with("blocked"),
            "callers add the prefix; BLOCKED must not repeat it"
        );
        let err = check_url("http://127.0.0.1/").unwrap_err();
        assert!(!err.starts_with("blocked"), "{err}");
    }

    #[test]
    fn literal_ip_urls_are_checked_without_dns() {
        assert!(check_url("http://127.0.0.1:8080/x").is_err());
        assert!(check_url("http://169.254.169.254/latest/meta-data/").is_err());
        assert!(check_url("http://[::1]:9000/").is_err());
        assert!(check_url("https://[::ffff:10.0.0.1]/").is_err());
    }

    #[test]
    fn localhost_names_are_blocked_without_resolving() {
        assert!(check_url("http://localhost:8080/").is_err());
        assert!(check_url("http://LOCALHOST/").is_err());
        assert!(check_url("http://localhost./").is_err());
        assert!(check_url("http://printer.local/").is_err());
    }

    #[test]
    fn non_http_schemes_are_refused() {
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("gopher://x/").is_err());
        assert!(check_url("ftp://x/").is_err());
    }

    #[test]
    fn host_port_parsing_handles_real_url_shapes() {
        assert_eq!(
            host_port("https://example.com/a/b?c=1"),
            Some(("example.com".into(), 443))
        );
        assert_eq!(
            host_port("http://example.com:8080/x"),
            Some(("example.com".into(), 8080))
        );
        assert_eq!(host_port("http://[::1]/x"), Some(("::1".into(), 80)));
        assert_eq!(host_port("http://[::1]:7000/x"), Some(("::1".into(), 7000)));
        assert_eq!(
            host_port("https://user:pw@example.com/x"),
            Some(("example.com".into(), 443))
        );
    }

    #[test]
    fn credentials_in_the_authority_cannot_smuggle_a_private_host() {
        assert!(check_url("http://example.com@127.0.0.1/").is_err());
    }
}

use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};

use crate::config::Config;

pub const DEFAULT_SERVICE: &str = "_phoenix._tcp";

pub fn valid_domain(domain: &str) -> bool {
    let d = domain.trim().trim_end_matches('.');
    if d.is_empty() || d.len() > 253 || d.starts_with('.') || d.contains("..") {
        return false;
    }
    d.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    })
}

pub fn is_private(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || v4.octets().first().copied() == Some(100)
                    && (64..128).contains(&v4.octets().get(1).copied().unwrap_or(0))
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.segments().first().map(|s| s & 0xfe00 == 0xfc00) == Some(true)
                || v6.segments().first().map(|s| s & 0xffc0 == 0xfe80) == Some(true)
        }
    }
}

pub fn tailscale_ips() -> Vec<IpAddr> {
    let Ok(out) = std::process::Command::new("tailscale")
        .args(["ip", "-4"])
        .stdin(std::process::Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<Ipv4Addr>().ok())
        .map(IpAddr::V4)
        .collect()
}

pub fn resolve(host: &str) -> Result<Vec<IpAddr>, String> {
    if !valid_domain(host) {
        return Err(format!("'{host}' is not a valid hostname"));
    }
    let addrs = (host, 0u16)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?;
    let mut out: Vec<IpAddr> = Vec::new();
    for a in addrs {
        if !out.contains(&a.ip()) {
            out.push(a.ip());
        }
    }
    if out.is_empty() {
        return Err(format!("{host} resolved to nothing"));
    }
    Ok(out)
}

pub fn service_record(domain: &str, port: u16) -> Result<String, String> {
    if !valid_domain(domain) {
        return Err(format!("'{domain}' is not a valid discovery domain"));
    }
    if port == 0 {
        return Err("port 0 cannot be advertised".into());
    }
    let d = domain.trim().trim_end_matches('.');
    Ok(format!(
        "{DEFAULT_SERVICE}.{d}. IN PTR phoenix.{DEFAULT_SERVICE}.{d}.\n\
phoenix.{DEFAULT_SERVICE}.{d}. IN SRV 0 0 {port} phoenix.{d}.\n\
phoenix.{DEFAULT_SERVICE}.{d}. IN TXT \"v=1\" \"path=/\"\n"
    ))
}

pub fn plan_text(cfg: &Config, domain: &str) -> Result<String, String> {
    let zone = service_record(domain, cfg.http_port)?;
    let ips = tailscale_ips();
    let mut out = format!("discovery domain  {domain}\nservice           {DEFAULT_SERVICE}\n");
    out.push_str(&format!("gateway port      {}\n", cfg.http_port));
    if ips.is_empty() {
        out.push_str("tailnet address   (tailscale not found or not logged in)\n");
    } else {
        for ip in &ips {
            out.push_str(&format!("tailnet address   {ip}\n"));
        }
    }
    out.push_str("\nzone records:\n");
    out.push_str(&zone);
    out.push_str("\nnothing was written; this command only plans\n");
    Ok(out)
}

pub fn check_text(host: &str, allow_private: bool) -> Result<String, String> {
    let ips = resolve(host)?;
    let mut out = format!("{host} resolves to {} address(es)\n", ips.len());
    let mut refused = 0usize;
    for ip in &ips {
        let private = is_private(ip);
        if private && !allow_private {
            refused += 1;
        }
        out.push_str(&format!(
            "  {ip}{}\n",
            if private { "  (private)" } else { "" }
        ));
    }
    if refused > 0 && !allow_private {
        out.push_str(&format!(
            "{refused} private address(es): phoenix refuses these unless \
security.allow_private_network is on\n"
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_are_validated_before_anything_is_printed() {
        assert!(valid_domain("phoenix.internal"));
        assert!(valid_domain("a.b.c.d"));
        assert!(valid_domain("host_name.local"));
        assert!(!valid_domain(""));
        assert!(!valid_domain("."));
        assert!(!valid_domain("a..b"));
        assert!(!valid_domain("-bad.example"));
        assert!(!valid_domain("bad-.example"));
        assert!(!valid_domain("a b.example"));
        assert!(!valid_domain(&"x".repeat(64)));
    }

    #[test]
    fn a_zone_record_names_the_service_and_the_port() {
        let zone = service_record("phoenix.internal", 8787).unwrap();
        assert!(
            zone.contains("_phoenix._tcp.phoenix.internal. IN PTR"),
            "{zone}"
        );
        assert!(zone.contains("IN SRV 0 0 8787"), "{zone}");
        assert!(zone.contains("IN TXT"), "{zone}");
    }

    #[test]
    fn a_bad_domain_or_port_is_refused_not_rendered() {
        assert!(service_record("not a domain", 80).is_err());
        assert!(service_record("ok.internal", 0).is_err());
    }

    #[test]
    fn private_and_tailnet_ranges_are_recognised() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "192.168.1.1",
            "172.16.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fd00::1",
            "fe80::1",
        ] {
            assert!(is_private(&ip.parse().unwrap()), "{ip} should be private");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700:4700::1111"] {
            assert!(!is_private(&ip.parse().unwrap()), "{ip} should be public");
        }
    }

    #[test]
    fn resolving_an_invalid_hostname_fails_before_any_lookup() {
        assert!(resolve("not a host").is_err());
        assert!(resolve("").is_err());
    }

    #[test]
    fn the_plan_never_writes_anything_and_says_so() {
        let cfg = Config::default();
        let text = plan_text(&cfg, "phoenix.internal").unwrap();
        assert!(text.contains("nothing was written"), "{text}");
        assert!(text.contains("_phoenix._tcp"), "{text}");
        assert!(plan_text(&cfg, "..").is_err());
    }

    #[test]
    fn localhost_is_reported_as_private() {
        let text = check_text("localhost", false).unwrap();
        assert!(text.contains("private"), "{text}");
        assert!(text.contains("allow_private_network"), "{text}");
        let allowed = check_text("localhost", true).unwrap();
        assert!(!allowed.contains("allow_private_network"), "{allowed}");
    }
}

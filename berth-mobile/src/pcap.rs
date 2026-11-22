//! Which hosts an app talked to, from a PCAPdroid pcap file: one line per
//! destination, "<proto> <host or ip>:<port> conns=N bytes=N", busiest first.
//! Hosts come from TLS ClientHello SNI, HTTP Host headers and DNS answers.

use std::collections::{BTreeMap, BTreeSet, HashMap};

fn u16be(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

fn packets(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    if data.len() < 24 {
        return out;
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let le = magic == 0xA1B2C3D4 || magic == 0xA1B23C4D;
    let rd = |b: &[u8]| {
        let a = [b[0], b[1], b[2], b[3]];
        if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    };
    let linktype = rd(&data[20..24]);
    let mut i = 24;
    while i + 16 <= data.len() {
        let incl = rd(&data[i + 8..i + 12]) as usize;
        i += 16;
        let end = (i + incl).min(data.len());
        let pkt = &data[i..end];
        i = end;
        // 101/228/229 = raw ip (vpn mode), 1 = ethernet (root mode)
        out.push(if linktype == 1 {
            pkt.get(14..).unwrap_or(&[])
        } else {
            pkt
        });
    }
    out
}

fn ip4(b: &[u8]) -> String {
    format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
}

fn ip6(b: &[u8]) -> String {
    (0..16)
        .step_by(2)
        .map(|i| format!("{:x}", (b[i] as u16) << 8 | b[i + 1] as u16))
        .collect::<Vec<_>>()
        .join(":")
}

/// (protocol, source, destination, layer 4 payload)
fn parse_ip(pkt: &[u8]) -> Option<(u8, String, String, &[u8])> {
    let v = pkt.first()? >> 4;
    if v == 4 && pkt.len() >= 20 {
        let ihl = ((pkt[0] & 0x0F) as usize) * 4;
        return Some((
            pkt[9],
            ip4(&pkt[12..16]),
            ip4(&pkt[16..20]),
            pkt.get(ihl..)?,
        ));
    }
    if v == 6 && pkt.len() >= 40 {
        return Some((pkt[6], ip6(&pkt[8..24]), ip6(&pkt[24..40]), &pkt[40..]));
    }
    None
}

fn tls_sni(p: &[u8]) -> Option<String> {
    // record type 22 (handshake), handshake type 1 (client hello)
    if p.len() < 43 || p[0] != 22 || p[5] != 1 {
        return None;
    }
    let mut i = 43;
    i += 1 + *p.get(i)? as usize;
    i += 2 + u16be(p, i)? as usize;
    i += 1 + *p.get(i)? as usize;
    let end = i + 2 + u16be(p, i)? as usize;
    i += 2;
    while i + 4 <= end {
        let (ty, len) = (u16be(p, i)?, u16be(p, i + 2)? as usize);
        if ty == 0 {
            let n = u16be(p, i + 7)? as usize;
            return Some(String::from_utf8_lossy(p.get(i + 9..i + 9 + n)?).into_owned());
        }
        i += 4 + len;
    }
    None
}

fn http_host(p: &[u8]) -> Option<String> {
    let first = p.get(..8.min(p.len()))?.split(|&c| c == b' ').next()?;
    if !matches!(
        first,
        b"GET" | b"POST" | b"PUT" | b"DELETE" | b"PATCH" | b"HEAD" | b"OPTIONS"
    ) {
        return None;
    }
    let text = String::from_utf8_lossy(p);
    text.split("\r\n").skip(1).find_map(|l| {
        (l.len() >= 5 && l[..5].eq_ignore_ascii_case("host:")).then(|| l[5..].trim().to_string())
    })
}

fn dns_name(b: &[u8], mut i: usize, depth: u32) -> (String, usize) {
    let mut labels = Vec::new();
    while i < b.len() && depth < 10 {
        let n = b[i] as usize;
        if n == 0 {
            return (labels.join("."), i + 1);
        }
        if n & 0xC0 == 0xC0 {
            let ptr = u16be(b, i).unwrap_or(0) as usize & 0x3FFF;
            labels.push(dns_name(b, ptr, depth + 1).0);
            return (labels.join("."), i + 2);
        }
        let end = (i + 1 + n).min(b.len());
        labels.push(String::from_utf8_lossy(&b[i + 1..end]).into_owned());
        i += 1 + n;
    }
    (labels.join("."), i)
}

/// (query name, answer ips) of a DNS response.
fn dns_answer(p: &[u8]) -> Option<(String, Vec<String>)> {
    let flags = u16be(p, 2)?;
    let (qd, an) = (u16be(p, 4)?, u16be(p, 6)?);
    if flags & 0x8000 == 0 || qd != 1 {
        return None;
    }
    let (qname, mut i) = dns_name(p, 12, 0);
    i += 4;
    let mut ips = Vec::new();
    for _ in 0..an {
        i = dns_name(p, i, 0).1;
        let rtype = u16be(p, i)?;
        let rdlen = u16be(p, i + 8)? as usize;
        i += 10;
        if rtype == 1 && rdlen == 4 {
            ips.push(ip4(p.get(i..i + 4)?));
        }
        i += rdlen;
    }
    Some((qname, ips))
}

type Conn = (String, u16, String, u16);

pub fn summarize(data: &[u8]) -> Vec<String> {
    let mut ip_names: HashMap<String, String> = HashMap::new();
    let mut flows: HashMap<Conn, String> = HashMap::new();
    let mut stats: BTreeMap<(&'static str, String, u16), (BTreeSet<Conn>, usize)> = BTreeMap::new();
    for pkt in packets(data) {
        let Some((proto, src, dst, l4)) = parse_ip(pkt) else {
            continue;
        };
        let (sport, dport, payload, name) = match proto {
            17 if l4.len() >= 8 => {
                let (sp, dp) = (u16be(l4, 0).unwrap_or(0), u16be(l4, 2).unwrap_or(0));
                if sp == 53 {
                    if let Some((q, ips)) = dns_answer(&l4[8..]) {
                        for ip in ips {
                            ip_names.entry(ip).or_insert_with(|| q.clone());
                        }
                    }
                    continue;
                }
                (sp, dp, &l4[8..], "udp")
            }
            6 if l4.len() >= 20 => {
                let off = ((l4[12] >> 4) as usize) * 4;
                (
                    u16be(l4, 0).unwrap_or(0),
                    u16be(l4, 2).unwrap_or(0),
                    l4.get(off..).unwrap_or(&[]),
                    "tcp",
                )
            }
            _ => continue,
        };
        // the app is the client: its packets go to a well-known port
        let outbound = dport < sport || matches!(dport, 80 | 443 | 853 | 5222 | 8080 | 8443);
        let (server, port) = if outbound {
            (dst.clone(), dport)
        } else {
            (src.clone(), sport)
        };
        let conn = if outbound {
            (src, sport, dst, dport)
        } else {
            (dst, dport, src, sport)
        };
        if outbound
            && !payload.is_empty()
            && let Some(h) = tls_sni(payload).or_else(|| http_host(payload))
        {
            flows.insert(conn.clone(), h);
        }
        let e = stats.entry((name, server, port)).or_default();
        e.0.insert(conn);
        e.1 += payload.len();
    }
    let mut rows: Vec<(usize, String)> = stats
        .into_iter()
        .map(|((name, server, port), (conns, bytes))| {
            let hosts: BTreeSet<&String> = conns.iter().filter_map(|c| flows.get(c)).collect();
            let host = if hosts.is_empty() {
                ip_names.get(&server).cloned().unwrap_or(server)
            } else {
                hosts.into_iter().cloned().collect::<Vec<_>>().join(",")
            };
            let proto = match port {
                443 | 8443 => "tls",
                80 | 8080 => "http",
                _ => name,
            };
            (
                bytes,
                format!("{proto} {host}:{port} conns={} bytes={bytes}", conns.len()),
            )
        })
        .collect();
    rows.sort_by(|a, b| b.cmp(a));
    rows.into_iter().map(|r| r.1).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcap(packets: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend(0xA1B2C3D4u32.to_le_bytes());
        out.extend([2, 0, 4, 0]);
        out.extend([0u8; 8]);
        out.extend(65535u32.to_le_bytes());
        out.extend(101u32.to_le_bytes());
        for p in packets {
            out.extend([0u8; 8]);
            out.extend((p.len() as u32).to_le_bytes());
            out.extend((p.len() as u32).to_le_bytes());
            out.extend(p);
        }
        out
    }

    fn ipv4(proto: u8, src: [u8; 4], dst: [u8; 4], l4: &[u8]) -> Vec<u8> {
        let mut p = vec![0x45, 0, 0, 0, 0, 0, 0, 0, 64, proto, 0, 0];
        p.extend(src);
        p.extend(dst);
        p.extend(l4);
        p
    }

    fn tcp(sport: u16, dport: u16, payload: &[u8]) -> Vec<u8> {
        let mut t = Vec::new();
        t.extend(sport.to_be_bytes());
        t.extend(dport.to_be_bytes());
        t.extend([0u8; 8]);
        t.push(5 << 4);
        t.extend([0u8; 7]);
        t.extend(payload);
        t
    }

    #[test]
    fn names_http_hosts_and_counts_bytes() {
        let get = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let data = pcap(&[
            ipv4(6, [10, 0, 0, 2], [93, 184, 216, 34], &tcp(40000, 80, get)),
            ipv4(
                6,
                [93, 184, 216, 34],
                [10, 0, 0, 2],
                &tcp(80, 40000, b"HTTP/1.1 200 OK\r\n\r\n"),
            ),
        ]);
        let lines = summarize(&data);
        assert_eq!(
            lines,
            [format!(
                "http example.com:80 conns=1 bytes={}",
                get.len() + 19
            )]
        );
    }

    #[test]
    fn falls_back_to_dns_names() {
        // dns response for a.io -> 1.2.3.4
        let mut dns = vec![
            0, 1, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0, 1, b'a', 2, b'i', b'o', 0, 0, 1, 0, 1,
        ];
        dns.extend([0xC0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 1, 2, 3, 4]);
        let mut udp = vec![0, 53, 0x9c, 0x40, 0, 0, 0, 0];
        udp.extend(&dns);
        let data = pcap(&[
            ipv4(17, [8, 8, 8, 8], [10, 0, 0, 2], &udp),
            ipv4(6, [10, 0, 0, 2], [1, 2, 3, 4], &tcp(40001, 443, b"")),
        ]);
        assert_eq!(summarize(&data), ["tls a.io:443 conns=1 bytes=0"]);
    }
}

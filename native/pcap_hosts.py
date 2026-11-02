"""Summarize which hosts an app talked to, from a PCAPdroid pcap file.

Usage: python3 -I pcap_hosts.py capture.pcap
Prints one line per destination: "<proto> <host or ip>:<port> conns=N bytes=N".
Hosts come from TLS ClientHello SNI, HTTP Host headers and DNS answers.
No third-party modules, so it runs with any python3.
"""
import struct
import sys
from collections import defaultdict


def packets(path):
    with open(path, "rb") as f:
        header = f.read(24)
        magic = struct.unpack("<I", header[:4])[0]
        endian = "<" if magic in (0xA1B2C3D4, 0xA1B23C4D) else ">"
        linktype = struct.unpack(endian + "I", header[20:24])[0]
        while True:
            rec = f.read(16)
            if len(rec) < 16:
                return
            _, _, incl, _ = struct.unpack(endian + "IIII", rec)
            data = f.read(incl)
            # 101/228/229 = raw ip (pcapdroid vpn mode), 1 = ethernet (root mode)
            yield data[14:] if linktype == 1 else data


def parse_ip(pkt):
    if not pkt:
        return None
    version = pkt[0] >> 4
    if version == 4:
        ihl = (pkt[0] & 0x0F) * 4
        return pkt[9], ".".join(map(str, pkt[12:16])), ".".join(map(str, pkt[16:20])), pkt[ihl:]
    if version == 6 and len(pkt) >= 40:
        def v6(b):
            return ":".join(f"{b[i] << 8 | b[i + 1]:x}" for i in range(0, 16, 2))
        return pkt[6], v6(pkt[8:24]), v6(pkt[24:40]), pkt[40:]
    return None


def tls_sni(payload):
    # record type 22 (handshake), handshake type 1 (client hello)
    if len(payload) < 43 or payload[0] != 22 or payload[5] != 1:
        return None
    try:
        i = 43
        i += 1 + payload[i]                                  # session id
        i += 2 + struct.unpack(">H", payload[i:i + 2])[0]    # cipher suites
        i += 1 + payload[i]                                  # compression
        end = i + 2 + struct.unpack(">H", payload[i:i + 2])[0]
        i += 2
        while i + 4 <= end:
            ext_type, ext_len = struct.unpack(">HH", payload[i:i + 4])
            if ext_type == 0:
                name_len = struct.unpack(">H", payload[i + 7:i + 9])[0]
                return payload[i + 9:i + 9 + name_len].decode("ascii", "replace")
            i += 4 + ext_len
    except (IndexError, struct.error):
        return None
    return None


def http_host(payload):
    if not payload[:8].split(b" ")[0] in (b"GET", b"POST", b"PUT", b"DELETE", b"PATCH", b"HEAD", b"OPTIONS"):
        return None
    for line in payload.split(b"\r\n")[1:]:
        if line.lower().startswith(b"host:"):
            return line[5:].strip().decode("ascii", "replace")
    return None


def dns_names(udp_payload):
    # returns (query name, [answer ips]) for a dns response
    def name_at(buf, i, depth=0):
        labels = []
        while i < len(buf) and depth < 10:
            n = buf[i]
            if n == 0:
                return ".".join(labels), i + 1
            if n & 0xC0 == 0xC0:
                ptr = struct.unpack(">H", buf[i:i + 2])[0] & 0x3FFF
                labels.append(name_at(buf, ptr, depth + 1)[0])
                return ".".join(labels), i + 2
            labels.append(buf[i + 1:i + 1 + n].decode("ascii", "replace"))
            i += 1 + n
        return ".".join(labels), i

    try:
        flags, qd, an = struct.unpack(">HHH", udp_payload[2:8])
        if not flags & 0x8000 or qd != 1:
            return None
        qname, i = name_at(udp_payload, 12)
        i += 4
        ips = []
        for _ in range(an):
            _, i = name_at(udp_payload, i)
            rtype, _, _, rdlen = struct.unpack(">HHIH", udp_payload[i:i + 10])
            i += 10
            if rtype == 1 and rdlen == 4:
                ips.append(".".join(map(str, udp_payload[i:i + 4])))
            i += rdlen
        return qname, ips
    except (IndexError, struct.error):
        return None


def main(path):
    ip_names = {}
    flows = {}
    stats = defaultdict(lambda: {"conns": set(), "bytes": 0})
    for pkt in packets(path):
        parsed = parse_ip(pkt)
        if not parsed:
            continue
        proto, src, dst, l4 = parsed
        if proto == 17 and len(l4) >= 8:
            sport, dport = struct.unpack(">HH", l4[:4])
            if sport == 53:
                answer = dns_names(l4[8:])
                if answer:
                    for ip in answer[1]:
                        ip_names.setdefault(ip, answer[0])
                continue
            payload, name = l4[8:], "udp"
        elif proto == 6 and len(l4) >= 20:
            sport, dport = struct.unpack(">HH", l4[:4])
            payload, name = l4[(l4[12] >> 4) * 4:], "tcp"
        else:
            continue

        # the app is the client: its packets go to a well-known port
        outbound = dport < sport or dport in (80, 443, 853, 5222, 8080, 8443)
        server, port = (dst, dport) if outbound else (src, sport)
        conn = (src, sport, dst, dport) if outbound else (dst, dport, src, sport)
        if outbound and payload:
            host = tls_sni(payload) or http_host(payload)
            if host:
                flows[conn] = host
        key = (name, server, port)
        stats[key]["conns"].add(conn)
        stats[key]["bytes"] += len(payload)

    rows = []
    for (name, server, port), s in stats.items():
        hosts = {flows[c] for c in s["conns"] if c in flows}
        host = ",".join(sorted(hosts)) or ip_names.get(server, server)
        proto = "tls" if port in (443, 8443) else "http" if port in (80, 8080) else name
        rows.append((s["bytes"], f"{proto} {host}:{port} conns={len(s['conns'])} bytes={s['bytes']}"))
    for _, line in sorted(rows, reverse=True):
        print(line)


if __name__ == "__main__":
    main(sys.argv[1])

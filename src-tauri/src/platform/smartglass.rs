//! Xbox SmartGlass on the local network: finding consoles and turning one on. These are the same
//! UDP packets the Xbox app sends to port 5050. Turning a console on needs its power mode set to
//! Sleep (Settings > General > Power options), so its network stays up while it's off.

use std::collections::HashSet;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

const PORT: u16 = 5050;
const DISCOVERY_REQUEST: u16 = 0xDD00;
const DISCOVERY_RESPONSE: u16 = 0xDD01;
const POWER_ON_REQUEST: u16 = 0xDD02;
/// The client type the console is told about. 3 is a Windows desktop app.
const CLIENT_TYPE: u16 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Console {
    pub name: String,
    /// e.g. "FD00112233445566". Identifies the console and is what a power-on packet names.
    pub live_id: String,
    pub address: Ipv4Addr,
}

/// Header: type, payload length, version (always 0 for these), then the payload.
fn packet(kind: u16, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(6 + payload.len());
    p.extend(kind.to_be_bytes());
    p.extend((payload.len() as u16).to_be_bytes());
    p.extend(0u16.to_be_bytes());
    p.extend(payload);
    p
}

fn discovery_request() -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend(0u32.to_be_bytes()); // flags
    payload.extend(CLIENT_TYPE.to_be_bytes());
    payload.extend(0u16.to_be_bytes()); // lowest protocol version
    payload.extend(2u16.to_be_bytes()); // highest protocol version
    packet(DISCOVERY_REQUEST, &payload)
}

fn power_on_request(live_id: &str) -> Vec<u8> {
    // A SmartGlass string: length, bytes, then a null that the length doesn't count
    let mut payload = Vec::new();
    payload.extend((live_id.len() as u16).to_be_bytes());
    payload.extend(live_id.as_bytes());
    payload.push(0);
    packet(POWER_ON_REQUEST, &payload)
}

struct Reader<'a> {
    buf: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.buf.len() < n {
            return None;
        }
        let (head, rest) = self.buf.split_at(n);
        self.buf = rest;
        Some(head)
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_be_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn string(&mut self) -> Option<String> {
        let len = self.u16()? as usize;
        let s = String::from_utf8_lossy(self.take(len)?).into_owned();
        self.take(1)?;
        Some(s)
    }
    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.u16()? as usize;
        self.take(len)
    }
}

/// The last common name in a DER certificate, which is the subject's (the issuer comes first).
/// The console's certificate is issued to its live ID, so this is the only certificate parsing
/// needed: find each `commonName` OID and read the string after it.
fn common_name(der: &[u8]) -> Option<String> {
    const CN_OID: [u8; 5] = [0x06, 0x03, 0x55, 0x04, 0x03];
    let mut found = None;
    for i in 0..der.len().saturating_sub(CN_OID.len() + 2) {
        if der[i..i + CN_OID.len()] != CN_OID {
            continue;
        }
        let tag = der[i + CN_OID.len()];
        let len = der[i + CN_OID.len() + 1] as usize;
        let start = i + CN_OID.len() + 2;
        // UTF8String, PrintableString or IA5String, with a short-form length
        if matches!(tag, 0x0C | 0x13 | 0x16)
            && len < 0x80
            && let Some(value) = der.get(start..start + len)
            && let Ok(s) = std::str::from_utf8(value)
            && !s.is_empty()
            && s.chars().all(|c| c.is_ascii_alphanumeric())
        {
            found = Some(s.to_string());
        }
    }
    found
}

fn parse_discovery_response(data: &[u8], address: Ipv4Addr) -> Option<Console> {
    let mut r = Reader { buf: data };
    if r.u16()? != DISCOVERY_RESPONSE {
        return None;
    }
    r.take(4)?; // payload length, version
    let strict = (|| {
        r.u32()?; // flags
        r.u16()?; // client type
        let name = r.string()?;
        r.string()?; // uuid
        r.u32()?; // last error
        Some((name, common_name(r.bytes()?)?))
    })();
    let (name, live_id) = match strict {
        Some(found) => found,
        // A layout this doesn't expect: the certificate is still in there somewhere
        None => ("Xbox".to_string(), common_name(data)?),
    };
    Some(Console { name, live_id, address })
}

/// The address of the adapter the default route uses, which is the one the Xbox is almost always
/// on. A broadcast from an unbound socket can leave through any adapter instead, such as a
/// VirtualBox or Hyper-V one, and never reach the real network.
fn lan_address() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // Connecting a UDP socket sends nothing, it only picks the route
    s.connect((Ipv4Addr::new(1, 1, 1, 1), 53)).ok()?;
    match s.local_addr().ok()? {
        SocketAddr::V4(a) if !a.ip().is_unspecified() => Some(*a.ip()),
        _ => None,
    }
}

fn socket() -> std::io::Result<UdpSocket> {
    let s = UdpSocket::bind((lan_address().unwrap_or(Ipv4Addr::UNSPECIFIED), 0))?;
    s.set_broadcast(true)?;
    Ok(s)
}

/// Where to send: the console's own address if known, and the whole network either way, since
/// a console's address can change.
fn targets(address: Option<Ipv4Addr>) -> Vec<SocketAddr> {
    let mut t = vec![SocketAddr::from((Ipv4Addr::BROADCAST, PORT))];
    if let Some(a) = address {
        t.push(SocketAddr::from((a, PORT)));
    }
    t
}

/// Consoles that answer within `timeout`. Only consoles that are on answer.
pub fn discover(address: Option<Ipv4Addr>, timeout: Duration) -> std::io::Result<Vec<Console>> {
    listen(address, timeout, |_| false)
}

/// The console if it's on, checked by asking it to identify itself. Returns as soon as it answers.
pub fn find(live_id: &str, address: Option<Ipv4Addr>, timeout: Duration) -> Option<Console> {
    let wanted = |c: &Console| c.live_id.eq_ignore_ascii_case(live_id);
    listen(address, timeout, wanted).ok()?.into_iter().find(wanted)
}

/// Asks every console to identify itself and collects the answers until `timeout`, or until
/// `enough` says one is all that's needed.
fn listen(
    address: Option<Ipv4Addr>,
    timeout: Duration,
    enough: impl Fn(&Console) -> bool,
) -> std::io::Result<Vec<Console>> {
    let s = socket()?;
    let request = discovery_request();
    for t in targets(address) {
        // A send to one target failing (no route, say) shouldn't stop the others
        let _ = s.send_to(&request, t);
    }

    let deadline = Instant::now() + timeout;
    let mut buf = [0u8; 2048];
    let mut seen = HashSet::new();
    let mut found = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        s.set_read_timeout(Some(left))?;
        match s.recv_from(&mut buf) {
            Ok((n, SocketAddr::V4(from))) => {
                if let Some(c) = parse_discovery_response(&buf[..n], *from.ip())
                    && seen.insert(c.live_id.clone())
                {
                    let done = enough(&c);
                    found.push(c);
                    if done {
                        break;
                    }
                }
            }
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
            // Windows reports an ICMP "port unreachable" from an earlier send as an error here
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e),
        }
    }
    Ok(found)
}

/// Sends power-on packets once. The console doesn't reply to them, so use `find` to see if it worked.
pub fn power_on(live_id: &str, address: Option<Ipv4Addr>) -> std::io::Result<()> {
    let s = socket()?;
    let request = power_on_request(live_id);
    let mut sent = false;
    for _ in 0..3 {
        for t in targets(address) {
            sent |= s.send_to(&request, t).is_ok();
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if sent { Ok(()) } else { Err(std::io::Error::other("couldn't send anything on the network")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_on_packet_matches_the_protocol() {
        let p = power_on_request("FD00112233445566");
        assert_eq!(&p[..8], &[0xDD, 0x02, 0x00, 0x13, 0x00, 0x00, 0x00, 0x10]);
        assert_eq!(&p[8..24], b"FD00112233445566");
        assert_eq!(p[24], 0);
        assert_eq!(p.len(), 25);
    }

    #[test]
    fn discovery_packet_matches_the_protocol() {
        let p = discovery_request();
        assert_eq!(p, [0xDD, 0x00, 0x00, 0x0A, 0x00, 0x00, 0, 0, 0, 0, 0x00, 0x03, 0x00, 0x00, 0x00, 0x02]);
    }

    /// A pretend certificate: just enough DER-ish bytes for an issuer and a subject common name.
    fn cert() -> Vec<u8> {
        let mut c = vec![0x30, 0x82, 0x01, 0x00];
        c.extend([0x06, 0x03, 0x55, 0x04, 0x03, 0x13, 0x06]);
        c.extend(b"Issuer");
        c.extend([0x31, 0x00, 0x06, 0x03, 0x55, 0x04, 0x03, 0x0C, 0x10]);
        c.extend(b"FD00112233445566");
        c.extend([0x30, 0x00]);
        c
    }

    fn sg_string(s: &str) -> Vec<u8> {
        let mut v = (s.len() as u16).to_be_bytes().to_vec();
        v.extend(s.as_bytes());
        v.push(0);
        v
    }

    fn response() -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend(2u32.to_be_bytes());
        payload.extend(1u16.to_be_bytes());
        payload.extend(sg_string("Living room"));
        payload.extend(sg_string("DE305D54-75B4-431B-ADB2-EB6B9E546014"));
        payload.extend(0u32.to_be_bytes());
        let c = cert();
        payload.extend((c.len() as u16).to_be_bytes());
        payload.extend(c);
        packet(DISCOVERY_RESPONSE, &payload)
    }

    #[test]
    fn reads_the_subject_not_the_issuer() {
        assert_eq!(common_name(&cert()).as_deref(), Some("FD00112233445566"));
    }

    #[test]
    fn parses_a_discovery_response() {
        let ip = Ipv4Addr::new(192, 168, 1, 20);
        let c = parse_discovery_response(&response(), ip).unwrap();
        assert_eq!(c, Console { name: "Living room".into(), live_id: "FD00112233445566".into(), address: ip });
    }

    #[test]
    fn falls_back_to_scanning_for_the_live_id() {
        let mut r = response();
        r.truncate(r.len() - 2); // the certificate's length no longer fits
        let c = parse_discovery_response(&r, Ipv4Addr::LOCALHOST).unwrap();
        assert_eq!(c.live_id, "FD00112233445566");
    }

    /// Lists consoles on this network: `cargo test consoles -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn consoles() {
        println!("{:#?}", discover(None, Duration::from_secs(2)).unwrap());
    }

    #[test]
    fn ignores_other_packets() {
        assert!(parse_discovery_response(&power_on_request("FD00112233445566"), Ipv4Addr::LOCALHOST).is_none());
        assert!(parse_discovery_response(&[0xDD], Ipv4Addr::LOCALHOST).is_none());
    }
}

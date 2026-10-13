//! Wi-Fi Direct link to a Roku, the pure parts: addresses, the allowlist pick, peer parsing and the assoc-request
//! IEs. Talking to the supplicants is link.rs.
//!
//! Safety: a P2P peer is only connectable if a Roku on *our own LAN* (ECP :8060) reports the same name AND a wifi
//! MAC matching the peer's P2P address (first octet | 0x02). Neighbours' TVs merely in radio range never qualify.
use crate::paths::hex;
use crate::pyfmt::{repr, repr_list};
use std::collections::BTreeMap;

pub const CTRL: &str = "/run/wpa_supplicant"; // NetworkManager's wpa_supplicant
pub const OWN: &str = "/run/rokast"; // our patched instance (root-owned)
/// WPS primary device type: smartphone, what the Roku accepts as a source.
pub const DEVICE_TYPE: &str = "10-0050F204-5";
/// WFD dev-info: source, session available, RTSP 7236, 50 Mbps (as AOSP phones send).
pub const WFD_SOURCE_IE: &str = "000601101c440032";
/// Vendor IEs a Samsung phone sends in its assoc request; kept verbatim (the TV accepts the join with them).
pub const EXTRA_IES: &str = "dd0b0000f022000104000000eddd0b506f9a23000104010f010f";

/// A Roku found on the LAN over ECP.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Roku {
    pub ip: String,
    pub name: String,
    pub model: String,
    pub wifi_mac: String,
    pub p2p_mac: String,
}

fn mac_bytes(mac: &str) -> Option<Vec<u8>> {
    let b: Vec<u8> = mac.split(':').map(|x| u8::from_str_radix(x, 16)).collect::<Result<_, _>>().ok()?;
    (b.len() == 6).then_some(b)
}

fn mac_str(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(":")
}

pub fn p2p_mac_of(wifi_mac: &str) -> Option<String> {
    let mut b = mac_bytes(wifi_mac)?;
    b[0] |= 0x02;
    Some(mac_str(&b))
}

/// Stable locally-administered address for our group iface, derived from the P2P device address.
pub fn iface_mac(dev_addr: &str) -> Option<String> {
    let mut b = mac_bytes(dev_addr)?;
    b[0] |= 0x02;
    b[4] ^= 0x80;
    Some(mac_str(&b))
}

/// The one LAN Roku called `name` (case-insensitive); refuses none or several.
pub fn pick<'a>(rokus: &'a [Roku], name: &str) -> Result<&'a Roku, String> {
    let want = name.to_lowercase();
    let hits: Vec<&Roku> = rokus.iter().filter(|r| r.name.to_lowercase() == want).collect();
    if hits.len() != 1 {
        let names: Vec<&str> = rokus.iter().map(|r| r.name.as_str()).collect();
        return Err(format!("target {} matched {} LAN Rokus; have: {}", repr(name), hits.len(), repr_list(&names)));
    }
    Ok(hits[0])
}

/// `key=value` lines of `p2p_peer` output.
pub fn parse_peer(text: &str) -> BTreeMap<String, String> {
    text.lines().filter_map(|ln| ln.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("constant hex")).collect()
}

/// Hex of the vendor IEs for our assoc request: P2P IE (capability + device info), WFD IE, extras.
pub fn assoc_ies(dev_addr: &str, name: &str) -> Option<String> {
    let nm = &name.as_bytes()[..name.len().min(32)];
    let mut info = mac_bytes(dev_addr)?;
    info.extend(unhex("0188")); // config methods
    info.extend(unhex("000a0050f2040005")); // 10-0050F204-5
    info.push(0); // no secondary types
    info.extend(unhex("1011")); // WPS device name attr
    info.extend((nm.len() as u16).to_be_bytes());
    info.extend(nm);
    let mut p2p = unhex("506f9a09");
    p2p.extend(unhex("0202002700")); // P2P capability: device 0x27, group 0x00
    p2p.push(0x0d);
    p2p.extend((info.len() as u16).to_le_bytes());
    p2p.extend(info);
    let mut wfd = unhex("506f9a0a00"); // subelement 0 (dev info); value is len+data
    wfd.extend(unhex(WFD_SOURCE_IE));
    let ies: String = [p2p, wfd].iter().map(|e| format!("dd{:02x}{}", e.len(), hex(e))).collect();
    Some(ies + EXTRA_IES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roku(name: &str) -> Roku {
        Roku {
            ip: String::new(),
            name: name.into(),
            model: String::new(),
            wifi_mac: String::new(),
            p2p_mac: String::new(),
        }
    }

    #[test]
    fn test_p2p_mac_matches_observed_roku() {
        assert_eq!(p2p_mac_of("4c:50:dd:21:8b:c4").unwrap(), "4e:50:dd:21:8b:c4");
        assert_eq!(p2p_mac_of("a8:16:9d:d4:b8:c5").unwrap(), "aa:16:9d:d4:b8:c5");
        assert_eq!(p2p_mac_of("nonsense"), None);
    }

    #[test]
    fn test_pick_refuses_unknown_and_ambiguous() {
        let rokus = [roku("A"), roku("B"), roku("B")];
        assert_eq!(pick(&rokus, "a").unwrap().name, "A");
        let err = pick(&rokus, "JERICAS TV").unwrap_err(); // neighbour's TV: not on our LAN
        assert_eq!(err, "target 'JERICAS TV' matched 0 LAN Rokus; have: ['A', 'B', 'B']");
        assert!(pick(&rokus, "B").is_err());
    }

    #[test]
    fn test_assoc_ies_byte_identical_to_working_phone() {
        // Same layout as the assoc-request vendor IEs of an Android phone the Roku accepts (monitor capture),
        // with the phone's address and name replaced.
        let phone = concat!(
            "dd26",
            "506f9a09",   // P2P IE
            "0202002700", // P2P capability: device 0x27, group 0x00
            "0d1a00",
            "021122334455", // device info (len 26), P2P device address
            "0188",
            "000a0050f2040005",
            "00", // config methods, type 10-0050F204-5, no secondary types
            "10110005",
            "50686f6e65", // WPS device name "Phone"
            "dd0d",
            "506f9a0a",
            "00000601101c440032", // WFD IE: dev info = source, RTSP 7236, 50 Mbps
            "dd0b0000f022000104000000ed",
            "dd0b506f9a23000104010f010f", // extra vendor IEs, verbatim
        );
        assert_eq!(assoc_ies("02:11:22:33:44:55", "Phone").unwrap(), phone);
    }

    #[test]
    fn test_iface_mac_is_local_and_distinct() {
        assert_eq!(iface_mac("00:11:22:33:c4:55").unwrap(), "02:11:22:33:44:55");
    }

    #[test]
    fn test_parse_peer() {
        let info = parse_peer("4e:11:22:33:44:55\ndevice_name=Living Room\ninterface_addr=46:11:22:33:44:55\n");
        assert_eq!(
            (info["device_name"].as_str(), info["interface_addr"].as_str()),
            ("Living Room", "46:11:22:33:44:55")
        );
    }
}

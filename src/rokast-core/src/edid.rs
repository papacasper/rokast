//! An EDID (the description a screen gives the computer) for the TV-sized screen SCREEN=extend makes: EDID 1.3, one
//! CEA-861 60 Hz timing of the TV's size, a range-limits block and the name "rokast TV". `edid-decode --check` clean.
#[cfg(test)]
use crate::proc;
#[cfg(test)]
use std::{fs, time::Duration};

/// One detailed timing descriptor (EDID 1.3 §3.10.2): pixel clock in kHz, then active/blank/sync offset/sync width
/// for each axis, and the screen size in mm. Positive syncs.
#[allow(clippy::too_many_arguments)]
fn dtd(
    khz: u32,
    ha: u32,
    hb: u32,
    hso: u32,
    hsw: u32,
    va: u32,
    vb: u32,
    vso: u32,
    vsw: u32,
    mm: (u32, u32),
) -> [u8; 18] {
    let c = khz / 10;
    let lo = |v: u32| (v & 0xff) as u8;
    let hi = |a: u32, b: u32| (((a >> 8) & 0xf) << 4 | ((b >> 8) & 0xf)) as u8;
    [
        lo(c),
        lo(c >> 8),
        lo(ha),
        lo(hb),
        hi(ha, hb),
        lo(va),
        lo(vb),
        hi(va, vb),
        lo(hso),
        lo(hsw),
        ((vso & 0xf) << 4 | (vsw & 0xf)) as u8,
        (((hso >> 8) & 3) << 6 | ((hsw >> 8) & 3) << 4 | ((vso >> 4) & 3) << 2 | ((vsw >> 4) & 3)) as u8,
        lo(mm.0),
        lo(mm.1),
        hi(mm.0, mm.1),
        0,
        0,
        0x1e,
    ]
}

/// An EDID for a screen of the TV's size, 60 Hz (CEA-861 timings). Only the sizes a WFD sink negotiates here.
pub fn edid(w: u32, h: u32) -> Result<[u8; 128], String> {
    let timing = match (w, h) {
        (1920, 1080) => dtd(148_500, 1920, 280, 88, 44, 1080, 45, 4, 5, (880, 500)),
        (1280, 720) => dtd(74_250, 1280, 370, 110, 40, 720, 30, 5, 5, (880, 500)),
        _ => return Err(format!("no screen description for {w}x{h}")),
    };
    let mut e = [0u8; 128];
    e[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    let id = |c: u8| u16::from(c - b'@');
    e[8..10].copy_from_slice(&(id(b'R') << 10 | id(b'K') << 5 | id(b'T')).to_be_bytes()); // maker "RKT"
    e[10] = 1; // product
    e[16] = 1; // week
    e[17] = 34; // 2024
    e[18..25].copy_from_slice(&[1, 3, 0x80, 88, 50, 120, 0x0e]); // EDID 1.3, digital, 88x50 cm, gamma 2.2, sRGB, preferred timing
    e[25..35].copy_from_slice(&[0xee, 0x91, 0xa3, 0x54, 0x4c, 0x99, 0x26, 0x0f, 0x50, 0x54]); // sRGB-ish
    for b in e[38..54].chunks_mut(2) {
        b.copy_from_slice(&[1, 1]); // no standard timings
    }
    e[54..72].copy_from_slice(&timing);
    e[72..90].copy_from_slice(&[0, 0, 0, 0xfd, 0, 24, 75, 15, 81, 15, 0, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20]);
    e[90..95].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
    e[95..108].copy_from_slice(b"rokast TV\n   ");
    e[108..113].copy_from_slice(&[0, 0, 0, 0x10, 0]); // dummy
    e[127] = 0u8.wrapping_sub(e[..127].iter().fold(0u8, |a, b| a.wrapping_add(*b)));
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edid_1080p() {
        let e = edid(1920, 1080).unwrap();
        assert_eq!(e.iter().fold(0u8, |a, b| a.wrapping_add(*b)), 0, "checksum");
        assert_eq!(&e[..8], &[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        // the CEA 1080p60 timing, as edid-decode reads it: 1920x1080 60 Hz 148.5 MHz
        assert_eq!(
            &e[54..72],
            &[0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c, 0x45, 0x00, 0x70, 0xf4, 0x31, 0, 0, 0x1e]
        );
        assert!(edid(1280, 720).is_ok());
        assert!(edid(1024, 768).is_err());
    }

    #[test]
    fn edid_decode_agrees() {
        let Some(_) = proc::run(&["edid-decode", "--help"], &[], Duration::from_secs(5)) else { return };
        for (w, h, want) in [(1920, 1080, "1920x1080   60.000000 Hz"), (1280, 720, "1280x720    60.000000 Hz")] {
            let p = std::env::temp_dir().join(format!("rokast-edid-{w}-{}", std::process::id()));
            fs::write(&p, edid(w, h).unwrap()).unwrap();
            let out = proc::run(&["edid-decode", "--check", p.to_str().unwrap()], &[], Duration::from_secs(5)).unwrap();
            let _ = fs::remove_file(&p);
            assert!(out.stdout.contains(want), "{}", out.stdout);
            assert!(!out.stdout.contains("Failures:"), "{}", out.stdout);
        }
    }
}

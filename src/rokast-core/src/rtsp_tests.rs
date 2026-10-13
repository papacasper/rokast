//! WFD source vs a fake Roku sink: full M1..M7 handshake, no radio, no TV.
use super::*;
use std::cell::RefCell;

const CAPS: &str = "wfd_video_formats: 00 00 03 10 0001ffff 1fffffff 00000fff 00 0000 0000 11 none none\r\n\
                    wfd_audio_codecs: LPCM 00000003 00, AAC 0000000f 00\r\n\
                    wfd_client_rtp_ports: RTP/AVP/UDP;unicast 19000 0 mode=play\r\n";

#[derive(Default)]
struct Seen {
    set: Vec<String>,
    setup_resp: Msg,
    play_resp: Msg,
}

/// Behaves like a Roku: connects to the source, answers M1, sends M2, answers M3/M4/M5, then M6 SETUP, M7 PLAY.
fn fake_sink(port: u16) -> Seen {
    let s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut r = BufReader::new(s.try_clone().unwrap());
    let mut w = s;
    let mut send = |t: &str| w.write_all(t.as_bytes()).unwrap();
    let mut read = || Msg::read(&mut r).unwrap();
    let mut seen = Seen::default();
    let m1 = read(); // source opens with M1
    assert!(m1.method() == "OPTIONS" && m1.headers["require"].contains("org.wfa.wfd1.0"));
    send(&format!(
        "RTSP/1.0 200 OK\r\nCSeq: {}\r\nPublic: org.wfa.wfd1.0, SETUP, PLAY, TEARDOWN, GET_PARAMETER, SET_PARAMETER\r\n\r\n",
        m1.cseq()
    ));
    send("OPTIONS * RTSP/1.0\r\nCSeq: 1\r\nRequire: org.wfa.wfd1.0\r\n\r\n"); // M2
    let r2 = read();
    assert!(r2.start.starts_with("RTSP/1.0 200") && r2.headers["public"].contains("org.wfa.wfd1.0"));
    loop {
        let m = read();
        match m.method() {
            "GET_PARAMETER" => send(&format!(
                "RTSP/1.0 200 OK\r\nCSeq: {}\r\nContent-Type: text/parameters\r\nContent-Length: {}\r\n\r\n{CAPS}",
                m.cseq(),
                CAPS.len()
            )),
            "SET_PARAMETER" => {
                seen.set.push(m.body.clone());
                send(&format!("RTSP/1.0 200 OK\r\nCSeq: {}\r\n\r\n", m.cseq()));
                if m.body.contains("wfd_trigger_method") {
                    break;
                }
            }
            _ => {}
        }
    }
    send("SETUP rtsp://x/wfd1.0/streamid=0 RTSP/1.0\r\nCSeq: 2\r\nTransport: RTP/AVP/UDP;unicast;client_port=19000\r\n\r\n");
    seen.setup_resp = read();
    let session = seen.setup_resp.headers["session"].split(';').next().unwrap().to_string();
    send(&format!("PLAY rtsp://x/wfd1.0/streamid=0 RTSP/1.0\r\nCSeq: 3\r\nSession: {session}\r\n\r\n"));
    seen.play_resp = read();
    std::thread::sleep(Duration::from_millis(100));
    seen
}

#[test]
fn test_full_handshake_and_play() {
    let l = listen(0, None, "127.0.0.1").unwrap();
    let port = l.local_addr().unwrap().port();
    let sink = std::thread::spawn(move || fake_sink(port));
    let logs = RefCell::new(vec![]);
    let log = |m: &str| logs.borrow_mut().push(m.to_string());
    let played = RefCell::new(None);
    let mut src = Source::new(vec!["1080p30".into()], &log);
    let t0 = Instant::now();
    let mut poll =
        |_: bool| if t0.elapsed() > Duration::from_secs(10) { Err(Fail::Err("timeout".into())) } else { Ok(()) };
    let mut on_play = |n: &Negotiated, ip: &str, sp: u16| {
        *played.borrow_mut() = Some((n.clone(), ip.to_string(), sp));
        Ok(())
    };
    assert_eq!(src.serve(&l, &mut poll, &mut on_play), Ok(())); // ends when the sink closes
    let seen = sink.join().unwrap();
    let (n, ip, sp) = played.into_inner().expect("PLAY reached on_play");
    assert_eq!((n.width, n.height, n.fps), (1920, 1080, 30));
    assert_eq!(n.profile, "baseline"); // sink offers CBP -> most compatible
    assert_eq!(n.audio, "AAC");
    assert_eq!(n.rtp_port, 19000);
    assert_eq!((ip.as_str(), sp), ("127.0.0.1", src.server_port));
    let m4 = &seen.set[0];
    assert!(m4.contains("wfd_presentation_URL: rtsp://127.0.0.1/wfd1.0/streamid=0 none"), "{m4}");
    assert!(m4.contains("wfd_video_formats: 00 00 01 04 00000080"), "{m4}"); // CBP, L4.0, only bit7 (1080p30)
    assert!(seen.setup_resp.headers["transport"].contains(&format!("server_port={}", src.server_port)));
    assert!(seen.play_resp.start.starts_with("RTSP/1.0 200"));
    let logs = logs.into_inner();
    assert_eq!(logs[0], "wfd: sink 127.0.0.1 connected");
    assert!(logs[1].starts_with("wfd: sink caps {'wfd_video_formats': '00 00 03 10"), "{}", logs[1]);
    assert_eq!(logs[2], "wfd: negotiated 1920x1080@30 baseline L4.0 AAC");
    assert_eq!(logs.last().unwrap(), "wfd: sink closed the connection");
}

#[test]
fn the_poll_ends_a_wait_for_the_sink() {
    let l = listen(0, None, "127.0.0.1").unwrap();
    let log = |_: &str| {};
    let mut src = Source::new(vec![], &log);
    let mut n = 0;
    let mut poll = |playing: bool| {
        assert!(!playing);
        n += 1;
        if n > 3 {
            Err(Fail::Err("the TV joined but never opened RTSP to us".into()))
        } else {
            Ok(())
        }
    };
    let res = src.serve(&l, &mut poll, &mut |_, _, _| Ok(()));
    assert_eq!(res, Err(Fail::Err("the TV joined but never opened RTSP to us".into())));
}

#[test]
fn session_ids_and_ports_are_in_range() {
    let log = |_: &str| {};
    let src = Source::new(vec![], &log);
    assert_eq!(src.session.len(), 8);
    assert!((20000..30000).contains(&src.server_port));
}

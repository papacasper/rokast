//! Wi-Fi Display RTSP *source*, M1..M7 + keepalive, on blocking sockets.
//!
//! The SOURCE (us) listens on TCP 7236 (advertised in our WFD IE); the sink (TV) connects in. We send M1, answer the
//! sink's M2, query its capabilities (M3), pick a format (M4), trigger SETUP (M5), answer SETUP/PLAY (M6/M7), then
//! the caller streams RTP/MPEG-TS to the negotiated port. A reader thread turns the socket into messages; this
//! thread does the protocol and answers the sink's requests whenever they come, also while waiting for a reply.
use crate::pyfmt::repr;
use crate::stop::Fail;
use crate::wfd::{choose_video, format_video_param, level_name, parse_params, parse_video_formats, Msg, Negotiated};
use crate::wfd::{CRLF, PUBLIC};
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::FromRawFd;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const TICK: Duration = Duration::from_millis(100);
const REPLY: Duration = Duration::from_secs(10);
const KEEPALIVE: Duration = Duration::from_secs(20);

/// Called every tick with "is the stream playing": an Err ends the session (stop, DHCP failed, no PLAY in time).
pub type Poll<'a> = dyn FnMut(bool) -> Result<(), Fail> + 'a;
/// Called on PLAY with what was negotiated, the sink's address and our RTP port.
pub type OnPlay<'a> = dyn FnMut(&Negotiated, &str, u16) -> Result<(), String> + 'a;

fn random_in(lo: u64, hi: u64) -> u64 {
    lo + RandomState::new().build_hasher().finish() % (hi - lo)
}

/// Listen on :port of `device` (SO_BINDTODEVICE, so the listener can be up before DHCP finishes without being
/// reachable from the LAN; the Roku connects ~0.5 s after its lease and gives up after ~4 s), or of `ip`.
pub fn listen(port: u16, device: Option<&str>, ip: &str) -> std::io::Result<TcpListener> {
    let Some(dev) = device else { return TcpListener::bind((ip, port)) };
    // SAFETY: plain socket calls on a descriptor we create; on any failure it is closed before returning.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let fail = |fd| {
            let e = std::io::Error::last_os_error();
            libc::close(fd);
            Err(e)
        };
        let one: libc::c_int = 1;
        let sz = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        if libc::setsockopt(fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, &one as *const _ as *const libc::c_void, sz) < 0
            || libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                dev.as_ptr() as *const libc::c_void,
                dev.len() as libc::socklen_t,
            ) < 0
        {
            return fail(fd);
        }
        let mut addr: libc::sockaddr_in = std::mem::zeroed();
        addr.sin_family = libc::AF_INET as libc::sa_family_t;
        addr.sin_port = port.to_be();
        let alen = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
        if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, alen) < 0 || libc::listen(fd, 8) < 0 {
            return fail(fd);
        }
        Ok(TcpListener::from_raw_fd(fd))
    }
}

/// The connection to the sink while a session runs.
struct Conn {
    w: TcpStream,
    rx: Receiver<Msg>,
    m2_seen: bool,
    playing: bool,
    teardown: bool,
    closed: bool,  // the sink closed the connection
    aborted: bool, // the poll ended it
}

pub struct Source<'a> {
    prefs: Vec<String>,
    log: &'a dyn Fn(&str),
    cseq: i64,
    session: String,
    pub server_port: u16,
    pub local_ip: String,
    pub negotiated: Option<Negotiated>,
}

impl<'a> Source<'a> {
    pub fn new(prefs: Vec<String>, log: &'a dyn Fn(&str)) -> Self {
        Source {
            prefs,
            log,
            cseq: 0,
            session: random_in(10_000_000, 100_000_000).to_string(),
            server_port: random_in(20000, 30000) as u16,
            local_ip: String::new(),
            negotiated: None,
        }
    }

    /// Accept the sink's one connection and run the session until it ends (Ok) or the poll ends it (its Err).
    pub fn serve(&mut self, l: &TcpListener, poll: &mut Poll, on_play: &mut OnPlay) -> Result<(), Fail> {
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let (s, peer) = loop {
            match l.accept() {
                Ok(c) => break c,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    poll(false)?;
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(format!("wfd: accept: {e}").into()),
            }
        };
        s.set_nonblocking(false).map_err(|e| e.to_string())?;
        self.local_ip = s.local_addr().map(|a| a.ip().to_string()).unwrap_or_default();
        let sink_ip = peer.ip().to_string();
        (self.log)(&format!("wfd: sink {sink_ip} connected"));
        let r = s.try_clone().map_err(|e| e.to_string())?;
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(r);
            while let Ok(m) = Msg::read(&mut r) {
                if tx.send(m).is_err() {
                    break;
                }
            }
        });
        let mut c = Conn { w: s, rx, m2_seen: false, playing: false, teardown: false, closed: false, aborted: false };
        let res = self.session(&mut c, &sink_ip, poll, on_play);
        let _ = c.w.shutdown(std::net::Shutdown::Both);
        res
    }

    fn session(&mut self, c: &mut Conn, sink_ip: &str, poll: &mut Poll, on_play: &mut OnPlay) -> Result<(), Fail> {
        if let Err(e) = self.negotiate(c, sink_ip, poll, on_play) {
            if e == Fail::Stop || c.aborted {
                return Err(e);
            }
            if c.closed {
                (self.log)("wfd: sink closed the connection");
                return Ok(());
            }
            (self.log)(&format!("wfd: negotiation failed: {}", msg(&e))); // the sink may still go on; else poll ends it
        }
        let mut keepalive = Instant::now() + KEEPALIVE;
        while !c.teardown {
            if c.playing && Instant::now() >= keepalive {
                if let Err(e) =
                    self.request(c, "GET_PARAMETER", "rtsp://localhost/wfd1.0", &[], "", sink_ip, poll, on_play)
                {
                    if e == Fail::Stop || c.aborted {
                        return Err(e);
                    }
                    let why = if c.closed {
                        "sink closed the connection".into()
                    } else {
                        format!("keepalive failed: {}", msg(&e))
                    };
                    (self.log)(&format!("wfd: {why}"));
                    return Ok(());
                }
                keepalive = Instant::now() + KEEPALIVE;
            }
            match self.next(c, TICK, poll) {
                Ok(Some(m)) if m.is_request() => {
                    let was_playing = c.playing;
                    self.handle(c, &m, sink_ip, on_play)?;
                    if c.playing && !was_playing {
                        keepalive = Instant::now() + KEEPALIVE;
                    }
                }
                Ok(_) => {} // a late reply, or nothing this tick
                Err(e) if e == Fail::Stop || c.aborted => return Err(e),
                Err(_) => {
                    (self.log)("wfd: sink closed the connection");
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// The next message within `wait` (None if none came); polls every tick.
    fn next(&self, c: &mut Conn, wait: Duration, poll: &mut Poll) -> Result<Option<Msg>, Fail> {
        let end = Instant::now() + wait;
        loop {
            if let Err(e) = poll(c.playing) {
                c.aborted = true;
                return Err(e);
            }
            let left = end.saturating_duration_since(Instant::now());
            match c.rx.recv_timeout(left.min(TICK)) {
                Ok(m) => return Ok(Some(m)),
                Err(RecvTimeoutError::Disconnected) => {
                    c.closed = true;
                    return Err("the sink closed the connection".into());
                }
                Err(RecvTimeoutError::Timeout) if left <= TICK => return Ok(None),
                Err(RecvTimeoutError::Timeout) => {}
            }
        }
    }

    fn send(&self, c: &mut Conn, text: &str) -> Result<(), Fail> {
        c.w.write_all(text.as_bytes()).map_err(|e| {
            c.closed = true;
            Fail::Err(format!("wfd: send: {e}"))
        })
    }

    fn head(first: &str, h: &[(&str, String)], body: &str) -> String {
        let mut h = h.to_vec();
        if !body.is_empty() {
            h.push(("Content-Type", "text/parameters".into()));
            h.push(("Content-Length", body.len().to_string()));
        }
        let lines: String = h.iter().map(|(k, v)| format!("{k}: {v}{CRLF}")).collect();
        format!("{first}{CRLF}{lines}{CRLF}{body}")
    }

    fn respond(&self, c: &mut Conn, req: &Msg, headers: &[(&str, String)]) -> Result<(), Fail> {
        let mut h = vec![("CSeq", req.cseq().to_string())];
        h.extend_from_slice(headers);
        self.send(c, &Self::head("RTSP/1.0 200 OK", &h, ""))
    }

    /// Send a request and wait for its reply, answering the sink's own requests meanwhile.
    #[allow(clippy::too_many_arguments)]
    fn request(
        &mut self,
        c: &mut Conn,
        method: &str,
        uri: &str,
        headers: &[(&str, String)],
        body: &str,
        sink_ip: &str,
        poll: &mut Poll,
        on_play: &mut OnPlay,
    ) -> Result<Msg, Fail> {
        self.cseq += 1;
        let mut h = vec![("CSeq", self.cseq.to_string())];
        h.extend_from_slice(headers);
        self.send(c, &Self::head(&format!("{method} {uri} RTSP/1.0"), &h, body))?;
        let end = Instant::now() + REPLY;
        while Instant::now() < end {
            match self.next(c, TICK, poll)? {
                Some(m) if !m.is_request() && m.cseq() == self.cseq => {
                    if m.start.split_whitespace().nth(1) != Some("200") {
                        return Err(format!("{method} -> {}", m.start).into());
                    }
                    return Ok(m);
                }
                Some(m) if m.is_request() => {
                    self.handle(c, &m, sink_ip, on_play)?;
                    if c.teardown {
                        return Err(format!("{method}: the sink tore the session down").into());
                    }
                }
                _ => {}
            }
        }
        Err(format!("{method}: no reply in {} s", REPLY.as_secs()).into())
    }

    /// Answer one request from the sink.
    fn handle(&mut self, c: &mut Conn, m: &Msg, sink_ip: &str, on_play: &mut OnPlay) -> Result<(), Fail> {
        match m.method() {
            "OPTIONS" => {
                // M2 (sink -> source)
                self.respond(c, m, &[("Public", PUBLIC.into())])?;
                c.m2_seen = true;
            }
            "SETUP" => {
                // M6
                let t = m.headers.get("transport").map(String::as_str).unwrap_or("");
                let port = t
                    .split("client_port=")
                    .nth(1)
                    .and_then(|p| p.split(|ch: char| !ch.is_ascii_digit()).next().and_then(|d| d.parse::<u16>().ok()));
                let rtp = match (&mut self.negotiated, port) {
                    (Some(n), Some(p)) => {
                        n.rtp_port = p;
                        p
                    }
                    (Some(n), None) => n.rtp_port,
                    (None, p) => p.unwrap_or(0),
                };
                let transport = format!("RTP/AVP/UDP;unicast;client_port={rtp};server_port={}", self.server_port);
                self.respond(c, m, &[("Session", format!("{};timeout=30", self.session)), ("Transport", transport)])?;
            }
            "PLAY" => {
                // M7
                self.respond(c, m, &[("Session", self.session.clone())])?;
                c.playing = true;
                let n = self.negotiated.clone().ok_or("PLAY before the format was negotiated")?;
                on_play(&n, sink_ip, self.server_port)?;
            }
            "TEARDOWN" => {
                self.respond(c, m, &[("Session", self.session.clone())])?;
                c.teardown = true;
            }
            _ => self.respond(c, m, &[])?, // GET/SET_PARAMETER (keepalive, IDR request)
        }
        Ok(())
    }

    fn negotiate(&mut self, c: &mut Conn, sink_ip: &str, poll: &mut Poll, on_play: &mut OnPlay) -> Result<(), Fail> {
        let wfd1 = "rtsp://localhost/wfd1.0";
        self.request(c, "OPTIONS", "*", &[("Require", "org.wfa.wfd1.0".into())], "", sink_ip, poll, on_play)?; // M1
        let end = Instant::now() + REPLY; // M2 from the sink
        while !c.m2_seen {
            if Instant::now() >= end {
                return Err("no OPTIONS (M2) from the sink in 10 s".into());
            }
            if let Some(m) = self.next(c, TICK, poll)? {
                if m.is_request() {
                    self.handle(c, &m, sink_ip, on_play)?;
                }
            }
        }
        let ask = "wfd_video_formats\r\nwfd_audio_codecs\r\nwfd_client_rtp_ports\r\n";
        let resp = self.request(c, "GET_PARAMETER", wfd1, &[], ask, sink_ip, poll, on_play)?; // M3
        let caps = parse_params(&resp.body);
        let shown: Vec<String> = (resp.body.lines().filter_map(|l| l.split_once(':')))
            .filter(|(k, _)| caps.contains_key(k.trim()))
            .map(|(k, _)| format!("{}: {}", repr(k.trim()), repr(&caps[k.trim()]))) // in the sink's order, like a dict
            .collect();
        (self.log)(&format!("wfd: sink caps {{{}}}", shown.join(", ")));
        let formats = caps.get("wfd_video_formats").map(String::as_str).unwrap_or("");
        let (bit, (w, h, fps), prof, level) =
            choose_video(&parse_video_formats(formats).map_err(|e| e.0)?, &self.prefs).map_err(|e| e.0)?;
        let codecs = caps.get("wfd_audio_codecs").map(String::as_str).unwrap_or("");
        let audio = if codecs.contains("AAC") { "AAC" } else { "LPCM" };
        let audio_param = if audio == "AAC" { "AAC 00000001 00" } else { "LPCM 00000002 00" };
        let ports = caps.get("wfd_client_rtp_ports").map(String::as_str).unwrap_or("");
        let port = ports
            .split_whitespace()
            .collect::<Vec<_>>()
            .windows(2)
            .find_map(|p| p[0].parse::<u16>().ok().filter(|_| p[1].parse::<u32>().is_ok()))
            .unwrap_or(1028);
        let vp = format_video_param(bit, prof, level);
        let profile = if prof == 1 { "baseline" } else { "high" };
        let mut n = Negotiated::new(w, h, fps, profile, level_name(level).unwrap_or("4.0"), audio, port);
        n.video_param = vp.clone();
        let body = format!(
            "wfd_video_formats: {vp}\r\nwfd_audio_codecs: {audio_param}\r\n\
             wfd_presentation_URL: rtsp://{}/wfd1.0/streamid=0 none\r\n\
             wfd_client_rtp_ports: RTP/AVP/UDP;unicast {port} 0 mode=play\r\n",
            self.local_ip
        );
        let said = format!("wfd: negotiated {w}x{h}@{fps} {} L{} {audio}", n.profile, n.level);
        self.negotiated = Some(n);
        self.request(c, "SET_PARAMETER", wfd1, &[], &body, sink_ip, poll, on_play)?; // M4
        let trigger = "wfd_trigger_method: SETUP\r\n";
        self.request(c, "SET_PARAMETER", wfd1, &[], trigger, sink_ip, poll, on_play)?; // M5
        (self.log)(&said);
        Ok(())
    }
}

fn msg(f: &Fail) -> String {
    match f {
        Fail::Stop => "stopped".into(),
        Fail::Err(e) => e.clone(),
    }
}

#[cfg(test)]
#[path = "rtsp_tests.rs"]
mod tests;

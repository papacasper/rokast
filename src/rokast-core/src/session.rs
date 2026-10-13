//! One `rokast start` session: join the TV's group, then stream (join_and_stream in src/miracast/cli.py).
use crate::stop::Fail;

/// The radio side of a session; start.rs has the real one, tests a fake.
pub trait Link {
    /// Join the TV's Wi-Fi Direct group; the group iface.
    fn connect(&mut self) -> Result<String, Fail>;
    /// Stream on `iface` until the session ends; sets `played` once the stream started.
    fn stream(&mut self, iface: &str, played: &mut bool) -> Result<(), Fail>;
    fn disconnect(&mut self);
}

/// Join and stream; if that fails before the stream ever started, tear the link down and do it all once more
/// (status: retrying). A failure after streaming began is not retried: the TV or the user ended it.
pub fn join_and_stream(link: &mut dyn Link, target: &str, say: &dyn Fn(&str), rounds: u32) -> Result<(), Fail> {
    for r in 1..=rounds {
        say(&format!("status: joining {target}"));
        say("joining the TV's Wi-Fi Direct group...");
        let mut played = false;
        let res = link.connect().and_then(|iface| link.stream(&iface, &mut played));
        match res {
            Ok(()) => return Ok(()),
            Err(Fail::Err(e)) if !played && r < rounds => {
                say(&format!("status: retrying {e}"));
                link.disconnect();
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        connects: Vec<Result<String, Fail>>,
        stream_fails: bool,
        played: bool,
        disc: u32,
    }

    impl Link for Fake {
        fn connect(&mut self) -> Result<String, Fail> {
            self.connects.remove(0)
        }
        fn stream(&mut self, _: &str, played: &mut bool) -> Result<(), Fail> {
            *played = self.played;
            if self.stream_fails {
                return Err("the TV joined but never opened RTSP to us".into());
            }
            Ok(())
        }
        fn disconnect(&mut self) {
            self.disc += 1;
        }
    }

    /// (stdout, error, disconnects)
    fn run(connects: &[Result<&str, &str>], stream_fails: bool, played: bool) -> (String, Option<String>, u32) {
        let connects = connects.iter().map(|r| r.map(String::from).map_err(Fail::from)).collect();
        let mut f = Fake { connects, stream_fails, played, disc: 0 };
        let out = RefCell::new(String::new());
        let say = |m: &str| out.borrow_mut().push_str(&format!("{m}\n"));
        let res = join_and_stream(&mut f, "Living Room", &say, 2);
        let err = match res {
            Ok(()) => None,
            Err(Fail::Err(e)) => Some(e),
            Err(Fail::Stop) => Some("stopped".into()),
        };
        (out.into_inner(), err, f.disc)
    }

    #[test]
    fn test_first_round_works() {
        let (out, err, disc) = run(&[Ok("p2p-wlo1-mir")], false, false);
        assert_eq!(err, None);
        assert!(!out.contains("status: retrying"));
        assert_eq!(disc, 0);
    }

    #[test]
    fn test_failed_join_is_retried_once() {
        let (out, err, disc) = run(&[Err("could not join"), Ok("p2p-wlo1-mir")], false, false);
        assert_eq!(err, None);
        assert!(out.contains("status: retrying could not join\n"));
        assert_eq!(out.matches("status: joining Living Room").count(), 2);
        assert_eq!(disc, 1);
    }

    #[test]
    fn test_gives_up_after_the_second_round() {
        let (out, err, _) = run(&[Err("one"), Err("two")], false, false);
        assert_eq!(err.as_deref(), Some("two"));
        assert_eq!(out.matches("status: retrying").count(), 1);
    }

    #[test]
    fn test_no_rtsp_is_retried() {
        let (out, err, _) = run(&[Ok("p2p-wlo1-mir"), Ok("p2p-wlo1-mir")], true, false);
        assert_eq!(err.as_deref(), Some("the TV joined but never opened RTSP to us"));
        assert!(out.contains("status: retrying the TV joined"));
    }

    #[test]
    fn test_failure_after_streaming_is_not_retried() {
        let (out, err, _) = run(&[Ok("p2p-wlo1-mir")], true, true);
        assert_eq!(err.as_deref(), Some("the TV joined but never opened RTSP to us"));
        assert!(!out.contains("status: retrying"));
    }

    #[test]
    fn a_stop_is_not_retried() {
        let mut f = Fake { connects: vec![Err(Fail::Stop)], stream_fails: false, played: false, disc: 0 };
        assert_eq!(join_and_stream(&mut f, "TV", &|_| {}, 2), Err(Fail::Stop));
        assert_eq!(f.disc, 0);
    }
}

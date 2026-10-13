//! Answer a desktop's screen-sharing dialog over AT-SPI (accessibility), for tests/wayland_desktop.sh. Test-only.
//!
//! Waits for a window from the portal backend (xdg-desktop-portal-gnome / -kde). GNOME: selects the first screen it
//! lists if nothing is selected, and presses Share. KDE (Plasma 6): presses the screen's tile, whose heading is
//! 'Share "<name>"'. Prints the dialog's accessible tree when it cannot, so a CI log shows what to change. Exits 0
//! once it pressed the button, 1 on timeout. Needs a live desktop:
//!   cargo run -p rokast-core --example portal_click -- [timeout seconds, default 90]
use std::time::{Duration, Instant};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

const APPS: [&str; 2] = ["xdg-desktop-portal-gnome", "xdg-desktop-portal-kde"];
const BUTTONS: [&str; 4] = ["share", "ok", "allow", "continue"];
// AT-SPI roles (AtspiRole)
const PUSH_BUTTON: u32 = 43;
const HEADING: u32 = 83;
// a screen in the list, most likely role first (GNOME: a toggle button named after the monitor); never "Remember..."
// TOGGLE_BUTTON, LIST_ITEM, TABLE_CELL, RADIO_BUTTON, CHECK_BOX
const ITEMS: [u32; 5] = [62, 32, 56, 44, 7];
// AT-SPI states (AtspiStateType bit numbers)
const STATES: [(u32, &str); 5] = [(8, "enabled"), (24, "sensitive"), (4, "checked"), (20, "pressed"), (23, "selected")];
const SENSITIVE: u32 = 24;
const ON: [u32; 3] = [4, 20, 23]; // checked, pressed, selected

type R<T> = Result<T, zbus::Error>;

#[derive(Clone)]
struct Node {
    dest: String,
    path: OwnedObjectPath,
}

struct Atspi {
    bus: Connection,
}

impl Atspi {
    /// The accessibility bus, whose address the session bus's org.a11y.Bus hands out.
    fn connect() -> R<Atspi> {
        let session = Connection::session()?;
        let r = session.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &())?;
        let addr: String = r.body().deserialize()?;
        let bus = zbus::blocking::connection::Builder::address(addr.as_str())?.build()?;
        Ok(Atspi { bus })
    }

    fn call<B, T>(&self, n: &Node, iface: &str, method: &str, body: &B) -> R<T>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
    {
        let r = self.bus.call_method(Some(n.dest.as_str()), n.path.as_str(), Some(iface), method, body)?;
        r.body().deserialize()
    }

    fn acc<T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type>(&self, n: &Node, method: &str) -> R<T> {
        self.call(n, "org.a11y.atspi.Accessible", method, &())
    }

    fn name(&self, n: &Node) -> String {
        let v: R<OwnedValue> =
            self.call(n, "org.freedesktop.DBus.Properties", "Get", &("org.a11y.atspi.Accessible", "Name"));
        v.ok().and_then(|v| String::try_from(v).ok()).unwrap_or_default()
    }

    fn children(&self, n: &Node) -> R<Vec<Node>> {
        let c: Vec<(String, OwnedObjectPath)> = self.acc(n, "GetChildren")?;
        Ok(c.into_iter()
            .filter(|(_, p)| p.as_str() != "/org/a11y/atspi/null")
            .map(|(dest, path)| Node { dest, path })
            .collect())
    }

    fn walk(&self, n: &Node, depth: usize, out: &mut Vec<(Node, usize)>) -> R<()> {
        out.push((n.clone(), depth));
        for c in self.children(n)? {
            self.walk(&c, depth + 1, out)?;
        }
        Ok(())
    }

    fn role(&self, n: &Node) -> u32 {
        self.acc(n, "GetRole").unwrap_or(0)
    }

    fn has(&self, n: &Node, state: u32) -> bool {
        let s: Vec<u32> = self.acc(n, "GetState").unwrap_or_default();
        s.get((state / 32) as usize).is_some_and(|w| w & (1 << (state % 32)) != 0)
    }

    fn states(&self, n: &Node) -> Vec<&'static str> {
        STATES.iter().filter(|(s, _)| self.has(n, *s)).map(|(_, name)| *name).collect()
    }

    fn usable(&self, n: &Node) -> bool {
        self.has(n, SENSITIVE) // GTK 4 never reports ENABLED
    }

    fn actions(&self, n: &Node) -> Vec<String> {
        let a: R<Vec<(String, String, String)>> = self.call(n, "org.a11y.atspi.Action", "GetActions", &());
        a.map(|a| a.into_iter().map(|(name, _, _)| name).collect()).unwrap_or_default()
    }

    fn press(&self, n: &Node) -> bool {
        let names: Vec<String> = self.actions(n).iter().map(|s| s.to_lowercase()).collect(); // Qt "Press", GTK "click"
        if names.is_empty() {
            return false;
        }
        let i = ["click", "press", "activate", "toggle", "select"]
            .iter()
            .find_map(|w| names.iter().position(|n| n == w))
            .unwrap_or(0);
        self.call(n, "org.a11y.atspi.Action", "DoAction", &(i as i32)).unwrap_or(false)
    }

    fn dump(&self, app: &Node) {
        let mut nodes = vec![];
        if let Err(e) = self.walk(app, 0, &mut nodes) {
            println!("portal_click: (tree changed: {e})");
        }
        for (n, d) in nodes {
            let role: String = self.acc(&n, "GetRoleName").unwrap_or_default();
            println!(
                "{}{role} {:?} actions={:?} {:?}",
                "  ".repeat(d),
                self.name(&n),
                self.actions(&n),
                self.states(&n)
            );
        }
    }

    fn apps(&self) -> Vec<Node> {
        let root = Node {
            dest: "org.a11y.atspi.Registry".into(),
            path: "/org/a11y/atspi/accessible/root".try_into().unwrap(),
        };
        self.children(&root).unwrap_or_default()
    }

    fn portal_app(&self) -> Option<Node> {
        self.apps().into_iter().find(|a| {
            let name = self.name(a);
            APPS.iter().any(|x| name.contains(x))
        })
    }

    fn answer(&self, app: &Node) -> R<bool> {
        let mut nodes = vec![];
        self.walk(app, 0, &mut nodes)?;
        let nodes: Vec<Node> = nodes.into_iter().map(|(n, _)| n).collect();
        let named = nodes
            .iter()
            .find(|n| self.role(n) == PUSH_BUTTON && BUTTONS.contains(&self.name(n).trim().to_lowercase().as_str()));
        let button = match named {
            Some(b) => {
                if !self.usable(b) {
                    let item = nodes
                        .iter()
                        .filter_map(|n| ITEMS.iter().position(|r| *r == self.role(n)).map(|rank| (rank, n)))
                        .filter(|(_, n)| !self.name(n).to_lowercase().contains("remember"))
                        .min_by_key(|(rank, _)| *rank)
                        .map(|(_, n)| n);
                    let Some(item) = item else { return Ok(false) };
                    if !ON.iter().any(|s| self.has(item, *s)) {
                        // pressing again would toggle it back off
                        let role: String = self.acc(item, "GetRoleName").unwrap_or_default();
                        println!("portal_click: selecting {role} {:?} {:?}", self.name(item), self.states(item));
                        self.press(item);
                    }
                    std::thread::sleep(Duration::from_secs(1));
                    if !self.usable(b) {
                        println!(
                            "portal_click: {:?} still {:?}; screen {:?}",
                            self.name(b),
                            self.states(b),
                            self.states(item)
                        );
                        return Ok(false);
                    }
                }
                b
            }
            None => {
                // KDE: the tile is the button
                let tile = nodes.iter().find(|n| {
                    self.role(n) == PUSH_BUTTON
                        && self
                            .children(n)
                            .unwrap_or_default()
                            .iter()
                            .any(|c| self.role(c) == HEADING && self.name(c).to_lowercase().starts_with("share "))
                });
                let Some(t) = tile else { return Ok(false) };
                t
            }
        };
        println!("portal_click: pressing {:?}", self.name(button));
        Ok(self.press(button))
    }
}

fn main() {
    let timeout: u64 = std::env::args().nth(1).map(|s| s.parse().expect("timeout in seconds")).unwrap_or(90);
    let a = Atspi::connect().unwrap_or_else(|e| {
        println!("portal_click: no accessibility bus: {e}");
        std::process::exit(1)
    });
    let end = Instant::now() + Duration::from_secs(timeout);
    let (mut app, mut dumped) = (None, Instant::now());
    while Instant::now() < end {
        if let Some(app) = app.as_ref().filter(|_| dumped.elapsed() > Duration::from_secs(20)) {
            println!("portal_click: still looking; portal tree:"); // what the dialog looks like so far, for the CI log
            a.dump(app);
            dumped = Instant::now();
        }
        app = a.portal_app().or(app);
        if let Some(app) = &app {
            match a.answer(app) {
                Ok(true) => std::process::exit(0),
                Ok(false) => {}
                Err(e) => println!("portal_click: {e}"), // the dialog changing under us
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    println!("portal_click: gave up; accessible apps:");
    for x in a.apps() {
        println!("- {}", a.name(&x));
    }
    if let Some(app) = &app {
        a.dump(app);
    }
    std::process::exit(1);
}

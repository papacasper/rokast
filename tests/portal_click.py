"""Answer a desktop's screen-sharing dialog over AT-SPI (accessibility), for tests/wayland_desktop.sh.

Waits for a window from the portal backend (xdg-desktop-portal-gnome / -kde). GNOME: selects the first screen it lists
if nothing is selected, and presses Share. KDE (Plasma 6): presses the screen's tile, whose heading is 'Share "<name>"'. Prints the dialog's accessible tree when it cannot, so a CI log
shows what to change. Exits 0 once it pressed the button, 1 on timeout. Not a unit test (needs a live desktop).
"""
import sys
import time

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

APPS = ("xdg-desktop-portal-gnome", "xdg-desktop-portal-kde")
BUTTONS = ("share", "ok", "allow", "continue")
# a screen in the list, most likely role first (GNOME: a toggle button named after the monitor); never "Remember..."
ITEMS = (Atspi.Role.TOGGLE_BUTTON, Atspi.Role.LIST_ITEM, Atspi.Role.TABLE_CELL, Atspi.Role.RADIO_BUTTON,
         Atspi.Role.CHECK_BOX)


def walk(node, depth=0):
    yield node, depth
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            yield from walk(child, depth + 1)


def states(n):
    s = n.get_state_set()
    return [t.value_nick for t in (Atspi.StateType.ENABLED, Atspi.StateType.SENSITIVE, Atspi.StateType.CHECKED,
                                   Atspi.StateType.PRESSED, Atspi.StateType.SELECTED) if s.contains(t)]


def on(n):
    s = n.get_state_set()
    return any(s.contains(t) for t in (Atspi.StateType.CHECKED, Atspi.StateType.PRESSED, Atspi.StateType.SELECTED))


def usable(n):
    return n.get_state_set().contains(Atspi.StateType.SENSITIVE)   # GTK 4 never reports ENABLED


def dump(app):
    try:
        for n, d in walk(app):
            print("  " * d + f"{n.get_role_name()} {n.get_name()!r} actions={actions(n)} {states(n)}", flush=True)
    except gi.repository.GLib.Error as e:
        print(f"portal_click: (tree changed: {e.message})", flush=True)


def actions(n):
    a = n.get_action_iface()
    return [a.get_action_name(i) for i in range(a.get_n_actions())] if a else []


def press(n):
    a = n.get_action_iface()
    if a and a.get_n_actions():
        names = [s.lower() for s in actions(n)]   # Qt says "Press", GTK "click"
        for want in ("click", "press", "activate", "toggle", "select"):
            if want in names:
                return a.do_action(names.index(want))
        return a.do_action(0)
    return False


def portal_app():
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(i)
        if app is not None and any(name in (app.get_name() or "") for name in APPS):
            return app
    return None


def answer(app):
    nodes = [n for n, _ in walk(app)]
    button = next((n for n in nodes if n.get_role() == Atspi.Role.PUSH_BUTTON
                   and (n.get_name() or "").strip().lower() in BUTTONS), None)
    if button is None:   # KDE: the tile is the button
        button = next((n for n in nodes if n.get_role() == Atspi.Role.PUSH_BUTTON and any(
            c.get_role() == Atspi.Role.HEADING and (c.get_name() or "").lower().startswith("share ")
            for c in (n.get_child_at_index(i) for i in range(n.get_child_count())) if c is not None)), None)
        if button is None:
            return False
    elif not usable(button):
        items = [n for n in nodes if n.get_role() in ITEMS and "remember" not in (n.get_name() or "").lower()]
        item = min(items, key=lambda n: ITEMS.index(n.get_role()), default=None)
        if item is None:
            return False
        if not on(item):   # pressing again would toggle it back off
            print(f"portal_click: selecting {item.get_role_name()} {item.get_name()!r} {states(item)}", flush=True)
            press(item)
        time.sleep(1)
        if not usable(button):
            print(f"portal_click: {button.get_name()!r} still {states(button)}; screen {states(item)}", flush=True)
            return False
    print(f"portal_click: pressing {button.get_name()!r}", flush=True)
    return press(button)


def main(timeout=90):
    Atspi.init()
    end, app, dumped = time.monotonic() + timeout, None, time.monotonic()
    while time.monotonic() < end:
        if time.monotonic() - dumped > 20 and app is not None:   # what the dialog looks like so far, for the CI log
            print("portal_click: still looking; portal tree:", flush=True)
            dump(app)
            dumped = time.monotonic()
        app = portal_app() or app
        try:
            if app is not None and answer(app):
                return 0
        except gi.repository.GLib.Error as e:   # the dialog changing under us
            print(f"portal_click: {e.message}", flush=True)
        time.sleep(1)
    print("portal_click: gave up; accessible apps:", flush=True)
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        a = desktop.get_child_at_index(i)
        print(f"- {a.get_name() if a else None}", flush=True)
    if app is not None:
        dump(app)
    return 1


if __name__ == "__main__":
    sys.exit(main(int(sys.argv[1]) if len(sys.argv) > 1 else 90))

"""Disposable freedesktop notification service for the private D-Bus test session."""
import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = dbus.service.BusName("org.freedesktop.Notifications", bus, do_not_queue=True)


class Notifications(dbus.service.Object):
    def __init__(self):
        super().__init__(bus, "/org/freedesktop/Notifications")
        self.rows = {}
        self.next = 1

    @dbus.service.method("org.freedesktop.Notifications", in_signature="", out_signature="ssss")
    def GetServerInformation(self):
        return ("Zephium fixture", "Zephium", "1", "1.2")

    @dbus.service.method("org.freedesktop.Notifications", in_signature="", out_signature="as")
    def GetCapabilities(self):
        return ["actions", "body", "body-markup", "icon-static"]

    @dbus.service.method("org.freedesktop.Notifications", in_signature="susssasa{sv}i", out_signature="u")
    def Notify(self, app, replaces, icon, title, body, actions, hints, expiry):
        if title == "reject":
            raise dbus.DBusException("org.freedesktop.Notifications.Error", "fixture refusal")
        if app == "Zephium":
            assert len(hints["image-data"][6]) == 32 * 32 * 4
        identity = replaces if replaces in self.rows else self.next
        if identity == self.next:
            self.next += 1
        self.rows[identity] = (title, body, actions)
        return identity

    @dbus.service.method("org.freedesktop.Notifications", in_signature="u", out_signature="")
    def CloseNotification(self, identity):
        if self.rows.pop(identity, None) is not None:
            self.NotificationClosed(identity, 3)

    @dbus.service.signal("org.freedesktop.Notifications", signature="us")
    def ActionInvoked(self, identity, action):
        pass

    @dbus.service.signal("org.freedesktop.Notifications", signature="uu")
    def NotificationClosed(self, identity, reason):
        pass

    @dbus.service.method("org.freedesktop.Notifications", in_signature="", out_signature="au")
    def TestIds(self):
        return sorted(self.rows)

    @dbus.service.method("org.freedesktop.Notifications", in_signature="us", out_signature="")
    def TestAction(self, identity, action):
        self.ActionInvoked(identity, action)

    @dbus.service.method("org.freedesktop.Notifications", in_signature="uu", out_signature="")
    def TestClose(self, identity, reason):
        self.rows.pop(identity, None)
        self.NotificationClosed(identity, reason)


service = Notifications()
print("READY", flush=True)
GLib.MainLoop().run()

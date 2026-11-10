import QtQuick
import Quickshell
import Nyx.Shell
import Nyx.Ui

// Pinned mini-app panels (Ctrl+P on an app's panel): each stays on screen after
// Glimpse closes, as a persistent overlay Shell Core pins and places (Nyx.Shell's
// standalone mode where Shell Core isn't installed). It shows on every workspace of its
// monitor, hides over fullscreen windows and while locked, and takes the keyboard only
// when clicked. The same panel QML and host as in Glimpse's preview, so a pinned panel
// can do only what its own app's actions allow. Its topics are followed while it shows.
Scope {
    id: pins

    required property var backend
    // Where a refusal goes while Glimpse is open (it closes the pin).
    signal said(string text)

    readonly property var list: (backend.state.pins && backend.state.pins.pins) || []

    function pinned(app) {
        return list.some(p => p.app === app);
    }

    // By app ID, so a pin republished with fresh state keeps its surface.
    Variants {
        model: pins.list.map(p => p.app)

        delegate: PinnedSurface {
            id: surface

            required property string modelData
            readonly property var entry: pins.list.find(p => p.app === modelData) || ({ app: modelData, id: "", name: "", icon: "", available: false })
            readonly property var minimum: (entry.size && entry.size.min) || [420, 320]
            readonly property int header: 40
            property string status: ""

            function openApp() {
                pins.backend.request("PanelOpen", { app: entry.app, arguments: {} }, (ok, err) => {
                    if (err)
                        voice.say(err.msg);
                });
            }

            overlayId: "glimpse.pin." + entry.id
            pinSize: [Math.min(800, minimum[0]), Math.min(900, minimum[1] + header)]
            pinMonitor: entry.monitor || ""
            // Remember where Shell Core put it.
            readonly property string placedOn: overlayState && overlayState.monitor ? overlayState.monitor : ""
            onPlacedOnChanged: if (placedOn !== "" && placedOn !== entry.monitor)
                pins.backend.request("Pin", { app: entry.app, monitor: placedOn }, null)
            onRefused: message => {
                pins.said(message);
                pins.backend.request("Unpin", { app: entry.app }, null);
            }

            // What the panel says, shown under its header for a few seconds.
            QtObject {
                id: voice
                function say(text) {
                    surface.status = text;
                    quiet.restart();
                }
            }
            Timer {
                id: quiet
                interval: 4000
                onTriggered: surface.status = ""
            }

            Surface {
                anchors.fill: parent
                kind: "popover"
                shadow: false

                Item {
                    id: bar
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: parent.top
                    height: surface.header
                    Icon {
                        id: appIcon
                        x: 12
                        anchors.verticalCenter: parent.verticalCenter
                        name: surface.entry.icon
                        size: 18
                        recolor: false
                    }
                    Label {
                        anchors.left: appIcon.right
                        anchors.leftMargin: 8
                        anchors.right: buttons.left
                        anchors.rightMargin: 8
                        anchors.verticalCenter: parent.verticalCenter
                        text: surface.status !== "" ? surface.status : surface.entry.name
                        dim: surface.status !== ""
                        strong: surface.status === ""
                        elide: Text.ElideRight
                        Accessible.role: Accessible.StaticText
                    }
                    Row {
                        id: buttons
                        anchors.right: parent.right
                        anchors.rightMargin: 6
                        anchors.verticalCenter: parent.verticalCenter
                        spacing: 2
                        IconButton {
                            icon: "window-new-symbolic"
                            tooltip: "Open " + surface.entry.name
                            Accessible.name: tooltip
                            onClicked: surface.openApp()
                        }
                        IconButton {
                            icon: "window-close-symbolic"
                            tooltip: "Unpin"
                            Accessible.name: "Unpin " + surface.entry.name
                            onClicked: pins.backend.request("Unpin", { app: surface.entry.app }, null)
                        }
                    }
                }

                PanelHost {
                    id: host
                    anchors.left: parent.left
                    anchors.right: parent.right
                    anchors.top: bar.bottom
                    anchors.bottom: parent.bottom
                    anchors.leftMargin: 14
                    anchors.rightMargin: 14
                    anchors.bottomMargin: 12
                    visible: surface.entry.available
                    focus: true
                    glimpse: voice
                    backend: pins.backend
                    // Followed only while it shows: hidden over a fullscreen window or
                    // while locked, it lets its topics go.
                    offer: surface.shown && surface.entry.available ? surface.entry : null
                }

                StateView {
                    anchors.centerIn: parent
                    width: parent.width - 28
                    visible: !surface.entry.available
                    icon: "system-run-symbolic"
                    text: surface.entry.reason || ("Open " + surface.entry.name + " to use this")
                    actionText: "Open " + surface.entry.name
                    onAction: surface.openApp()
                }
            }
        }
    }
}
